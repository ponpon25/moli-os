//! A HomeKit camera seen from the controller's side (a doorbell paired to
//! Moli): its live video and sound, a still image, and the voice of the
//! person who answers, all on the local network.
//!
//! A stream is negotiated as the Home app does: `SetupEndpoints` (where to
//! send, the SRTP keys), then `SelectedRTPStreamConfiguration` (H.264 and
//! Opus at 16 kHz, start). The camera sends SRTP to two UDP sockets of
//! Moli; the H.264 packets are put back into frames, the Opus decoded to
//! samples, and both handed to whoever watches (the HomeKit bridge, which
//! sends them on to the phones). Receiver reports keep the camera sending;
//! the end of the watch stops it.

// RTP clocks, sample counts and ports are far below the limits of the casts.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]

use std::collections::VecDeque;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context as _, bail};
use moli_runtime::BoxFuture;
use moli_runtime::media::{
    AudioChunk, Image, LiveInfo, LiveItem, LiveSource, LiveStream, LiveWant, SnapshotSource,
    TalkSink, TalkSource, VideoFrame,
};
use ring::rand::{SecureRandom as _, SystemRandom};
use tokio::net::UdpSocket;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

use crate::audio::{FRAME, OpusPipe, RATE};
use crate::controller::{Connection, Pairing};
use crate::srtp::{KEY_LEN, Receiver, SALT_LEN, Sender, header_len};
use crate::tlv::Tlv;

const VIDEO_PT: u8 = 99;
const AUDIO_PT: u8 = 110;
const COMFORT_PT: u8 = 13;
/// What the camera is asked for, whoever watches (and the H.264 level).
const SIZE: (u16, u16, u8) = (1280, 720, 0);
/// Frames and sound kept for viewers that read late (a few seconds).
const SHARED_ITEMS: usize = 512;
/// A viewer is let go after this: a TV left on must not hold the camera.
const MAX_WATCH: Duration = Duration::from_secs(10 * 60);
/// The first picture a viewer can start on must come within this.
const FIRST_PICTURE: Duration = Duration::from_secs(10);
const REPORT_EVERY: Duration = Duration::from_secs(1);
/// A camera that sends nothing for this long has stopped.
const SILENT: Duration = Duration::from_secs(10);

/// Where a camera's stream management lives in its database.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ids {
    pub aid: u64,
    pub setup: u64,
    pub selected: u64,
}

/// The way back to the camera's speaker while a stream runs.
struct VoiceOut {
    socket: Arc<UdpSocket>,
    to: SocketAddr,
    sender: Arc<Mutex<Sender>>,
    ssrc: u32,
}

/// A stream management taken for the shared stream, given back when it
/// ends (or never started).
struct Claim {
    busy: Arc<Mutex<Vec<bool>>>,
    index: usize,
}

impl Drop for Claim {
    fn drop(&mut self) {
        if let Some(slot) = self.busy.lock().expect("slots lock").get_mut(self.index) {
            *slot = false;
        }
    }
}

/// The camera's one stream, shared by everyone who watches it: two Apple
/// TVs and a phone cost the camera a single stream.
struct Upstream {
    generation: u64,
    items: broadcast::Sender<LiveItem>,
    info: LiveInfo,
    viewers: usize,
    stop: CancellationToken,
    voice: VoiceOut,
}

type SharedUpstream = Arc<Mutex<Option<Upstream>>>;

/// One camera of an accessory paired to Moli.
pub struct Camera {
    address: String,
    pairing: Pairing,
    ids: Vec<Ids>,
    busy: Arc<Mutex<Vec<bool>>>,
    upstream: SharedUpstream,
    /// One stream started at a time: a second viewer waits and joins it.
    starting: tokio::sync::Mutex<()>,
    generations: AtomicU64,
}

impl std::fmt::Debug for Camera {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Camera")
            .field("address", &self.address)
            .field("ids", &self.ids)
            .finish_non_exhaustive()
    }
}

fn random<const N: usize>() -> [u8; N] {
    let mut out = [0u8; N];
    SystemRandom::new()
        .fill(&mut out)
        .expect("system randomness");
    out
}

fn srtp_params(key: &[u8; KEY_LEN], salt: &[u8; SALT_LEN]) -> Vec<u8> {
    Tlv::new().with(1, [0]).with(2, key).with(3, salt).encode()
}

/// The camera's own keys from its answer (tag 4 video, 5 audio).
fn keys_of(answer: &Tlv, tag: u8) -> Option<([u8; KEY_LEN], [u8; SALT_LEN])> {
    let t = Tlv::parse(answer.get(tag)?)?;
    Some((t.get(2)?.try_into().ok()?, t.get(3)?.try_into().ok()?))
}

/// `SelectedRTPStreamConfiguration`: H.264 main, Opus 16 kHz mono, start
/// (`command` 1) or end (0).
fn selected(session: &[u8; 16], command: u8, size: (u16, u16, u8), ssrcs: (u32, u32)) -> Vec<u8> {
    let control = Tlv::new().with(1, session).with(2, [command]);
    if command == 0 {
        return Tlv::new().with(1, control.encode()).encode();
    }
    let (width, height, level) = size;
    let video = Tlv::new()
        .with(1, [0])
        .with(
            2,
            Tlv::new()
                .with(1, [1])
                .with(2, [level])
                .with(3, [0])
                .encode(),
        )
        .with(
            3,
            Tlv::new()
                .with(1, width.to_le_bytes())
                .with(2, height.to_le_bytes())
                .with(3, [30])
                .encode(),
        )
        .with(
            4,
            Tlv::new()
                .with(1, [VIDEO_PT])
                .with(2, ssrcs.0.to_le_bytes())
                .with(3, (if width > 1280 { 2000u16 } else { 1000 }).to_le_bytes())
                .with(4, 0.5f32.to_le_bytes())
                .with(5, 1378u16.to_le_bytes())
                .encode(),
        );
    let audio = Tlv::new()
        .with(1, [3])
        .with(
            2,
            Tlv::new()
                .with(1, [1])
                .with(2, [0])
                .with(3, [1])
                .with(4, [20])
                .encode(),
        )
        .with(
            3,
            Tlv::new()
                .with(1, [AUDIO_PT])
                .with(2, ssrcs.1.to_le_bytes())
                .with(3, 24u16.to_le_bytes())
                .with(4, 5.0f32.to_le_bytes())
                .with(6, [COMFORT_PT])
                .encode(),
        )
        .with(4, [0]);
    Tlv::new()
        .with(1, control.encode())
        .with(2, video.encode())
        .with(3, audio.encode())
        .encode()
}

/// An empty receiver report (RFC 3550): "still here".
fn receiver_report(ssrc: u32) -> Vec<u8> {
    let mut p = vec![0x80, 201, 0, 1];
    p.extend_from_slice(&ssrc.to_be_bytes());
    p
}

/// H.264 RTP (RFC 6184) back into frames: single NAL units, STAP-A and
/// FU-A; a frame ends with the marker bit or a new timestamp.
#[derive(Default)]
struct Depacketizer {
    nals: Vec<Vec<u8>>,
    timestamp: Option<u32>,
    fragment: Option<Vec<u8>>,
}

impl Depacketizer {
    fn flush(&mut self) -> Option<VideoFrame> {
        let timestamp = self.timestamp.take()?;
        if self.nals.is_empty() {
            return None;
        }
        let nals = std::mem::take(&mut self.nals);
        let keyframe = nals
            .iter()
            .any(|n| n.first().is_some_and(|b| b & 0x1F == 5));
        Some(VideoFrame {
            timestamp,
            nals,
            keyframe,
        })
    }

    fn push(&mut self, payload: &[u8], timestamp: u32, marker: bool) -> Vec<VideoFrame> {
        let mut out = Vec::new();
        if self.timestamp.is_some_and(|t| t != timestamp) {
            self.fragment = None;
            out.extend(self.flush());
        }
        self.timestamp = Some(timestamp);
        let Some(&first) = payload.first() else {
            return out;
        };
        match first & 0x1F {
            1..=23 => self.nals.push(payload.to_vec()),
            24 => {
                let mut rest = &payload[1..];
                while rest.len() > 2 {
                    let len = usize::from(u16::from_be_bytes([rest[0], rest[1]]));
                    let Some(nal) = rest.get(2..2 + len) else {
                        break;
                    };
                    self.nals.push(nal.to_vec());
                    rest = &rest[2 + len..];
                }
            }
            28 if payload.len() > 2 => {
                let header = payload[1];
                if header & 0x80 != 0 {
                    let mut nal = vec![(first & 0xE0) | (header & 0x1F)];
                    nal.extend_from_slice(&payload[2..]);
                    self.fragment = Some(nal);
                } else if let Some(nal) = self.fragment.as_mut() {
                    nal.extend_from_slice(&payload[2..]);
                }
                if header & 0x40 != 0
                    && let Some(nal) = self.fragment.take()
                {
                    self.nals.push(nal);
                }
            }
            _ => {}
        }
        if marker {
            out.extend(self.flush());
        }
        out
    }
}

/// One viewer of the shared stream. It starts on a key frame, stops after
/// [`MAX_WATCH`], and leaving it ends the camera's stream if it was the last.
struct View {
    info: LiveInfo,
    early: VecDeque<LiveItem>,
    items: broadcast::Receiver<LiveItem>,
    waiting_key: bool,
    since: tokio::time::Instant,
    _viewer: Viewer,
}

/// Counts a viewer of one shared stream (its generation).
struct Viewer {
    upstream: SharedUpstream,
    generation: u64,
}

impl Drop for Viewer {
    fn drop(&mut self) {
        let mut upstream = self.upstream.lock().expect("upstream lock");
        let last = upstream.as_mut().is_some_and(|up| {
            if up.generation != self.generation {
                return false;
            }
            up.viewers = up.viewers.saturating_sub(1);
            up.viewers == 0
        });
        if last && let Some(up) = upstream.take() {
            up.stop.cancel();
        }
    }
}

impl LiveStream for View {
    fn info(&self) -> &LiveInfo {
        &self.info
    }

    fn next(&mut self) -> BoxFuture<'_, anyhow::Result<LiveItem>> {
        Box::pin(async move {
            loop {
                // A viewer nobody stopped (a TV left on): let go of it.
                if self.since.elapsed() > MAX_WATCH {
                    bail!("watched for {} minutes: stopped", MAX_WATCH.as_secs() / 60);
                }
                if let Some(item) = self.early.pop_front() {
                    return Ok(item);
                }
                let item = match tokio::time::timeout(SILENT, self.items.recv()).await {
                    Err(_) => bail!("the camera went silent"),
                    Ok(Err(broadcast::error::RecvError::Closed)) => bail!("the camera stopped"),
                    // Frames were missed: start again on the next key frame.
                    Ok(Err(broadcast::error::RecvError::Lagged(_))) => {
                        self.waiting_key = true;
                        continue;
                    }
                    Ok(Ok(item)) => item,
                };
                if self.waiting_key {
                    match &item {
                        LiveItem::Video(frame) if frame.keyframe => self.waiting_key = false,
                        _ => continue,
                    }
                }
                return Ok(item);
            }
        })
    }
}

impl Camera {
    /// `ids`: the stream managements, at least one.
    #[must_use]
    pub fn new(address: &str, pairing: Pairing, ids: Vec<Ids>) -> Self {
        let busy = vec![false; ids.len()];
        Self {
            address: address.to_owned(),
            pairing,
            ids,
            busy: Arc::new(Mutex::new(busy)),
            upstream: Arc::new(Mutex::new(None)),
            starting: tokio::sync::Mutex::new(()),
            generations: AtomicU64::new(0),
        }
    }

    /// A free stream management, taken.
    fn claim(&self) -> anyhow::Result<(Ids, Claim)> {
        let mut busy = self.busy.lock().expect("slots lock");
        let index = busy
            .iter()
            .position(|b| !b)
            .context("every stream of the camera is in use")?;
        busy[index] = true;
        Ok((
            self.ids[index],
            Claim {
                busy: Arc::clone(&self.busy),
                index,
            },
        ))
    }

    async fn session(&self) -> anyhow::Result<Connection> {
        let mut connection = Connection::open(&self.address).await?;
        connection.verify(&self.pairing).await?;
        Ok(connection)
    }

    /// A new viewer of the stream already running, if one is.
    fn join(&self) -> Option<View> {
        let mut upstream = self.upstream.lock().expect("upstream lock");
        let up = upstream.as_mut().filter(|up| !up.stop.is_cancelled())?;
        up.viewers += 1;
        Some(View {
            info: up.info.clone(),
            early: VecDeque::new(),
            items: up.items.subscribe(),
            waiting_key: true,
            since: tokio::time::Instant::now(),
            _viewer: Viewer {
                upstream: Arc::clone(&self.upstream),
                generation: up.generation,
            },
        })
    }

    async fn watch(&self) -> anyhow::Result<Box<dyn LiveStream>> {
        let _one = self.starting.lock().await;
        if let Some(view) = self.join() {
            tracing::info!("homekit camera: a viewer joins the stream");
            return Ok(Box::new(view));
        }
        Ok(Box::new(self.start().await?))
    }

    #[allow(clippy::too_many_lines)] // the negotiation, then the relay, in order
    async fn start(&self) -> anyhow::Result<View> {
        let (ids, claim) = self.claim()?;
        let mut connection = self.session().await?;
        let local = connection.local_ip()?;
        let peer = connection.peer_ip()?;
        let video = UdpSocket::bind((local, 0)).await?;
        let audio = Arc::new(UdpSocket::bind((local, 0)).await?);
        let (video_key, video_salt) = (random::<KEY_LEN>(), random::<SALT_LEN>());
        let (audio_key, audio_salt) = (random::<KEY_LEN>(), random::<SALT_LEN>());
        let session = random::<16>();
        let address = Tlv::new()
            .with(1, [u8::from(local.is_ipv6())])
            .with(2, local.to_string())
            .with(3, video.local_addr()?.port().to_le_bytes())
            .with(4, audio.local_addr()?.port().to_le_bytes());
        let request = Tlv::new()
            .with(1, session)
            .with(3, address.encode())
            .with(4, srtp_params(&video_key, &video_salt))
            .with(5, srtp_params(&audio_key, &audio_salt))
            .encode();
        let answer = connection
            .write_tlv(ids.aid, ids.setup, &request, true)
            .await
            .context("SetupEndpoints")?;
        let answer = Tlv::parse(&answer).context("SetupEndpoints answer")?;
        if answer.byte(2) != Some(0) {
            bail!(
                "the camera refused the stream (status {:?})",
                answer.byte(2)
            );
        }
        let them = Tlv::parse(answer.get(3).context("no address")?).context("address")?;
        let port = |tag| -> anyhow::Result<u16> {
            Ok(u16::from_le_bytes(
                them.get(tag)
                    .context("no port")?
                    .try_into()
                    .context("port")?,
            ))
        };
        let (video_to, audio_to) = (
            SocketAddr::new(peer, port(3)?),
            SocketAddr::new(peer, port(4)?),
        );
        // What the camera protects its stream with (most echo ours).
        let (vk, vs) = keys_of(&answer, 4).unwrap_or((video_key, video_salt));
        let (ak, asalt) = keys_of(&answer, 5).unwrap_or((audio_key, audio_salt));
        let mut video_in = Receiver::new(&vk, &vs);
        let mut audio_in = Receiver::new(&ak, &asalt);
        let mut video_out = Sender::new(&video_key, &video_salt);
        let audio_out = Arc::new(Mutex::new(Sender::new(&audio_key, &audio_salt)));
        let ssrcs = (
            u32::from_be_bytes(random::<4>()),
            u32::from_be_bytes(random::<4>()),
        );
        // One size for everyone: a TV's small picture must not become the
        // phone's.
        let size = SIZE;
        connection
            .write_tlv(
                ids.aid,
                ids.selected,
                &selected(&session, 1, size, ssrcs),
                false,
            )
            .await
            .context("start")?;
        tracing::info!(
            width = size.0,
            height = size.1,
            "homekit camera: stream asked"
        );
        let voice = VoiceOut {
            socket: Arc::clone(&audio),
            to: audio_to,
            sender: Arc::clone(&audio_out),
            ssrc: ssrcs.1,
        };

        let stop = CancellationToken::new();
        let (tx, mut rx) = broadcast::channel::<LiveItem>(SHARED_ITEMS);
        let watch = stop.clone();
        let generation = self.generations.fetch_add(1, Ordering::Relaxed);
        let (upstream, items) = (Arc::clone(&self.upstream), tx.clone());
        tokio::spawn(async move {
            let mut depacketizer = Depacketizer::default();
            let mut decoder = opus_rs::OpusDecoder::new(16_000, 1).ok();
            let mut reports = tokio::time::interval(REPORT_EVERY);
            let (mut vbuf, mut abuf) = (vec![0u8; 2048], vec![0u8; 2048]);
            loop {
                tokio::select! {
                    () = watch.cancelled() => break,
                    _ = reports.tick() => {
                        let report = video_out.protect_rtcp(&receiver_report(ssrcs.0));
                        let _ = video.send_to(&report, video_to).await;
                        let report = audio_out.lock().expect("sender lock").protect_rtcp(&receiver_report(ssrcs.1));
                        let _ = audio.send_to(&report, audio_to).await;
                    }
                    got = video.recv_from(&mut vbuf) => {
                        let Ok((n, _)) = got else { break };
                        let packet = &vbuf[..n];
                        if (200..=204).contains(&packet.get(1).copied().unwrap_or(0)) {
                            continue;
                        }
                        let Some(clear) = video_in.unprotect(packet) else { continue };
                        let Some(header) = header_len(&clear) else { continue };
                        let marker = clear[1] & 0x80 != 0;
                        let timestamp = u32::from_be_bytes([clear[4], clear[5], clear[6], clear[7]]);
                        for frame in depacketizer.push(&clear[header..], timestamp, marker) {
                            let _ = tx.send(LiveItem::Video(frame));
                        }
                    }
                    got = audio.recv_from(&mut abuf) => {
                        let Ok((n, _)) = got else { break };
                        let packet = &abuf[..n];
                        if (200..=204).contains(&packet.get(1).copied().unwrap_or(0)) {
                            continue;
                        }
                        let Some(clear) = audio_in.unprotect(packet) else { continue };
                        let Some(header) = header_len(&clear) else { continue };
                        if clear[1] & 0x7F != AUDIO_PT || clear.len() <= header + 1 {
                            continue;
                        }
                        let Some(decoder) = decoder.as_mut() else { continue };
                        let mut pcm = vec![0f32; 960];
                        let Ok(n) = decoder.decode(&clear[header..], 960, &mut pcm) else { continue };
                        let samples = pcm[..n]
                            .iter()
                            .map(|v| (v * 32768.0).round().clamp(-32768.0, 32767.0) as i16)
                            .collect();
                        let _ = tx.send(LiveItem::Audio(AudioChunk { rate: 16_000, samples }));
                    }
                }
            }
            let end = selected(&session, 0, size, ssrcs);
            let _ = connection
                .write_tlv(ids.aid, ids.selected, &end, false)
                .await;
            drop(claim);
            // Gone: the next viewer starts a new one.
            let mut shared = upstream.lock().expect("upstream lock");
            if shared
                .as_ref()
                .is_some_and(|up| up.generation == generation)
            {
                *shared = None;
            }
            tracing::info!("homekit camera: stream ended");
        });

        // A viewer starts on a picture it can decode alone: wait for one,
        // with its parameter sets.
        let guard = stop.clone().drop_guard();
        let mut early = VecDeque::new();
        let (mut sps, mut pps) = (Vec::new(), Vec::new());
        let deadline = tokio::time::Instant::now() + FIRST_PICTURE;
        loop {
            let item = match tokio::time::timeout_at(deadline, rx.recv())
                .await
                .context("no picture from the camera")?
            {
                Ok(item) => item,
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => bail!("the camera stopped"),
            };
            let ready = match &item {
                LiveItem::Video(frame) => {
                    for nal in &frame.nals {
                        match nal.first().map(|b| b & 0x1F) {
                            Some(7) => sps.clone_from(nal),
                            Some(8) => pps.clone_from(nal),
                            _ => {}
                        }
                    }
                    frame.keyframe && !sps.is_empty() && !pps.is_empty()
                }
                LiveItem::Audio(_) => false,
            };
            if ready {
                early.clear();
                early.push_back(item);
                break;
            }
            // Nothing before the first key frame is of use.
        }
        tracing::info!("homekit camera: first picture");
        let info = LiveInfo {
            width: u32::from(size.0),
            height: u32::from(size.1),
            sps,
            pps,
            audio_rate: Some(16_000),
        };
        guard.disarm();
        *self.upstream.lock().expect("upstream lock") = Some(Upstream {
            generation,
            items,
            info: info.clone(),
            viewers: 1,
            stop,
            voice,
        });
        Ok(View {
            info,
            early,
            items: rx,
            waiting_key: false,
            since: tokio::time::Instant::now(),
            _viewer: Viewer {
                upstream: Arc::clone(&self.upstream),
                generation,
            },
        })
    }
}

impl LiveSource for Camera {
    fn open(&self, _want: LiveWant) -> BoxFuture<'_, anyhow::Result<Box<dyn LiveStream>>> {
        Box::pin(self.watch())
    }
}

impl SnapshotSource for Camera {
    fn snapshot(&self) -> BoxFuture<'_, anyhow::Result<Image>> {
        Box::pin(async move {
            let mut connection = self.session().await?;
            let bytes = connection.snapshot(self.ids[0].aid, 1280, 720).await?;
            Ok(Image {
                content_type: "image/jpeg".into(),
                bytes,
            })
        })
    }
}

/// The person's voice to the camera's speaker, on the stream being watched.
struct Voice {
    socket: Arc<UdpSocket>,
    to: SocketAddr,
    sender: Arc<Mutex<Sender>>,
    ssrc: u32,
    pipe: OpusPipe,
    seq: u16,
    timestamp: u32,
}

impl TalkSink for Voice {
    fn rate(&self) -> u32 {
        RATE
    }

    fn send(&mut self, samples: &[i16]) -> BoxFuture<'_, anyhow::Result<()>> {
        let packets = self.pipe.push(&AudioChunk {
            rate: RATE,
            samples: samples.to_vec(),
        });
        Box::pin(async move {
            for opus in packets {
                let mut rtp = vec![0x80, AUDIO_PT];
                rtp.extend_from_slice(&self.seq.to_be_bytes());
                rtp.extend_from_slice(&self.timestamp.to_be_bytes());
                rtp.extend_from_slice(&self.ssrc.to_be_bytes());
                rtp.extend_from_slice(&opus);
                self.seq = self.seq.wrapping_add(1);
                self.timestamp = self.timestamp.wrapping_add(FRAME as u32);
                let sealed = self.sender.lock().expect("sender lock").protect_rtp(&rtp);
                self.socket.send_to(&sealed, self.to).await?;
            }
            Ok(())
        })
    }
}

impl TalkSource for Camera {
    fn open(&self) -> BoxFuture<'_, anyhow::Result<Box<dyn TalkSink>>> {
        Box::pin(async move {
            let upstream = self.upstream.lock().expect("upstream lock");
            let Some(out) = upstream.as_ref().map(|up| &up.voice) else {
                bail!("nobody is watching the camera: no way to its speaker");
            };
            let pipe = OpusPipe::new(RATE, 24).map_err(|e| anyhow::anyhow!(e))?;
            Ok(Box::new(Voice {
                socket: Arc::clone(&out.socket),
                to: out.to,
                sender: Arc::clone(&out.sender),
                ssrc: out.ssrc,
                pipe,
                seq: u16::from_be_bytes(random::<2>()),
                timestamp: u32::from_be_bytes(random::<4>()),
            }) as Box<dyn TalkSink>)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_come_back_from_single_stap_and_fu_packets() {
        let mut d = Depacketizer::default();
        // STAP-A: SPS and PPS.
        let stap = [24u8, 0, 2, 0x67, 1, 0, 2, 0x68, 2];
        assert!(d.push(&stap, 100, false).is_empty());
        // An IDR in two fragments, the second with the marker.
        assert!(d.push(&[0x7C, 0x85, 9, 9], 100, false).is_empty());
        let frames = d.push(&[0x7C, 0x45, 8], 100, true);
        assert_eq!(frames.len(), 1);
        let f = &frames[0];
        assert!(f.keyframe);
        assert_eq!(
            f.nals,
            vec![vec![0x67, 1], vec![0x68, 2], vec![0x65, 9, 9, 8]]
        );
        // A new timestamp closes a frame that had no marker.
        assert!(d.push(&[0x41, 1], 200, false).is_empty());
        let frames = d.push(&[0x41, 2], 300, true);
        assert_eq!(frames.len(), 2);
        assert!(!frames[0].keyframe);
        assert_eq!(frames[1].timestamp, 300);
    }

    fn frame(timestamp: u32, keyframe: bool) -> LiveItem {
        LiveItem::Video(VideoFrame {
            timestamp,
            nals: vec![vec![if keyframe { 0x65 } else { 0x41 }]],
            keyframe,
        })
    }

    #[tokio::test]
    async fn viewers_share_one_stream_start_on_a_key_frame_and_the_last_stops_it() {
        let camera = Camera::new(
            "127.0.0.1:1",
            Pairing::decode(&format!("c|{}|a|{}", "07".repeat(32), "09".repeat(32))).unwrap(),
            vec![Ids {
                aid: 1,
                setup: 2,
                selected: 3,
            }],
        );
        let socket = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
        let (items, _keep) = broadcast::channel(16);
        let stop = CancellationToken::new();
        *camera.upstream.lock().unwrap() = Some(Upstream {
            generation: 0,
            items: items.clone(),
            info: LiveInfo::default(),
            viewers: 1,
            stop: stop.clone(),
            voice: VoiceOut {
                to: socket.local_addr().unwrap(),
                socket,
                sender: Arc::new(Mutex::new(Sender::new(&[1; KEY_LEN], &[2; SALT_LEN]))),
                ssrc: 1,
            },
        });
        let first = Viewer {
            upstream: Arc::clone(&camera.upstream),
            generation: 0,
        };
        // A second viewer joins: the same stream, from its next key frame.
        let mut second = camera.join().expect("a running stream to join");
        assert_eq!(camera.upstream.lock().unwrap().as_ref().unwrap().viewers, 2);
        items.send(frame(1, false)).unwrap();
        items.send(frame(2, true)).unwrap();
        items.send(frame(3, false)).unwrap();
        let LiveItem::Video(f) = second.next().await.unwrap() else {
            panic!()
        };
        assert_eq!((f.timestamp, f.keyframe), (2, true));
        let LiveItem::Video(f) = second.next().await.unwrap() else {
            panic!()
        };
        assert_eq!(f.timestamp, 3);
        // The talk goes through the shared stream.
        assert!(TalkSource::open(&camera).await.is_ok());
        // One leaves: the stream goes on; the last leaves: it stops.
        drop(first);
        assert!(!stop.is_cancelled());
        drop(second);
        assert!(stop.is_cancelled());
        assert!(camera.upstream.lock().unwrap().is_none());
        assert!(camera.join().is_none());
    }

    #[tokio::test(start_paused = true)]
    async fn a_viewer_is_let_go_after_ten_minutes() {
        let (items, rx) = broadcast::channel(16);
        let mut view = View {
            info: LiveInfo::default(),
            early: VecDeque::new(),
            items: rx,
            waiting_key: false,
            since: tokio::time::Instant::now(),
            _viewer: Viewer {
                upstream: Arc::new(Mutex::new(None)),
                generation: 0,
            },
        };
        items.send(frame(1, true)).unwrap();
        assert!(view.next().await.is_ok());
        tokio::time::advance(MAX_WATCH + Duration::from_secs(1)).await;
        items.send(frame(2, true)).unwrap();
        assert!(view.next().await.is_err());
    }

    #[test]
    fn the_start_names_h264_and_opus_and_the_end_only_the_session() {
        let session = [7u8; 16];
        let start = Tlv::parse(&selected(&session, 1, SIZE, (1, 2))).unwrap();
        let control = Tlv::parse(start.get(1).unwrap()).unwrap();
        assert_eq!(control.byte(2), Some(1));
        let video = Tlv::parse(start.get(2).unwrap()).unwrap();
        assert_eq!(video.byte(1), Some(0));
        let audio = Tlv::parse(start.get(3).unwrap()).unwrap();
        assert_eq!(audio.byte(1), Some(3));
        let end = Tlv::parse(&selected(&session, 0, SIZE, (1, 2))).unwrap();
        assert!(end.get(2).is_none());
    }
}
