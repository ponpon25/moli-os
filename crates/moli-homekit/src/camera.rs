//! Cameras: live video in the Home app. A camera is an accessory of its own
//! with a `CameraRTPStreamManagement` service. The Home app negotiates a
//! stream with two characteristics (`SetupEndpoints`: addresses and SRTP
//! keys; `SelectedRTPStreamConfiguration`: start, stop), then the bridge
//! sends SRTP to the phone's address.
//!
//! The video is *relayed*, not transcoded: the camera's H.264 frames
//! (`Hub::camera_live`) are packetized again for the phone's packet size,
//! encrypted with the negotiated keys, and sent. Video only: the cameras'
//! audio is AAC-LC, which HomeKit does not take (it wants AAC-ELD or Opus).
//!
//! The guard: HomeKit is a surface like the REST API, never a human session.
//! A camera in a protected room is not shown to HomeKit at all: no
//! accessory, and a stream or an image asked for anyway is refused.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use moli_core::DeviceId;
use moli_runtime::media::LiveWant;
use ring::rand::{SecureRandom as _, SystemRandom};
use serde_json::Value as Json;
use tokio::net::UdpSocket;
use tokio_util::sync::CancellationToken;

use crate::Shared;
use crate::accessories::{OK, ST_COMMUNICATION, ST_INVALID};
use crate::http::Response;
use crate::rtp::{Packetizer, Retimer, ntp_now, sender_report};
use crate::srtp::{KEY_LEN, SALT_LEN, Sender};
use crate::tlv::Tlv;

/// Streams one camera serves at once (waiting for their start included).
const MAX_PER_CAMERA: usize = 2;
/// Streams the bridge serves at once.
const MAX_TOTAL: usize = 6;
/// A negotiated stream the phone never started is forgotten after this.
const PENDING_TTL: Duration = Duration::from_secs(30);
/// A phone that was sending reports and went silent is gone.
const CONTROLLER_SILENT: Duration = Duration::from_secs(30);
const REPORT_EVERY: Duration = Duration::from_secs(2);
/// What the Home app assumes when the phone gives no packet size.
const DEFAULT_MTU: usize = 1378;

/// The characteristics of the stream management service.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CameraChar {
    SupportedVideo,
    SupportedAudio,
    SupportedRtp,
    Selected,
    Setup,
    Status,
}

// ---- base64 (the value of a `tlv8` characteristic) ---------------------------

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn b64(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, b)| n | (u32::from(*b) << (16 - 8 * i)));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(char::from(ALPHABET[((n >> (18 - 6 * i)) & 63) as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

pub fn unb64(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let (mut acc, mut bits) = (0u32, 0);
    for c in text.trim_end_matches('=').bytes() {
        let v = ALPHABET.iter().position(|a| *a == c)? as u32;
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

// ---- what the camera supports ----------------------------------------------

/// H.264 (High profile, level 4), packetization mode 1, and the sizes the
/// cameras' three qualities cover. The profile and level are what the phone
/// is told; the stream keeps whatever the camera encodes.
const RESOLUTIONS: [(u16, u16, u8); 5] = [
    (1920, 1080, 30),
    (1280, 720, 30),
    (640, 360, 30),
    (480, 270, 30),
    (320, 180, 15),
];

/// `SupportedVideoStreamConfiguration`, laid out byte for byte like
/// HAP-python's (an iPhone-tested reference): packetization first, every
/// profile and level as its own item, no separators, frame rate on two bytes.
pub fn supported_video() -> Vec<u8> {
    let params = Tlv::new()
        .with(3, [0]) // packetization: non-interleaved
        .with(1, [0]) // profiles: baseline, main, high
        .with(1, [1])
        .with(1, [2])
        .with(2, [0]) // levels: 3.1, 3.2, 4
        .with(2, [1])
        .with(2, [2]);
    let mut config = Tlv::new().with(1, [0]).with(2, params.encode());
    for (width, height, fps) in RESOLUTIONS {
        let mut size = Vec::new();
        size.extend_from_slice(&Tlv::new().with(1, width.to_le_bytes()).encode());
        size.extend_from_slice(&Tlv::new().with(2, height.to_le_bytes()).encode());
        size.extend_from_slice(&Tlv::new().with(3, u16::from(fps).to_le_bytes()).encode());
        config = config.with(3, size);
    }
    Tlv::new().with(1, config.encode()).encode()
}

/// `SupportedAudioStreamConfiguration`: required by the Home app though no
/// audio is sent. As HAP-python's: Opus 24 kHz and AAC-ELD 16 kHz.
pub fn supported_audio() -> Vec<u8> {
    let codec = |kind: u8, rate: u8| {
        let params = Tlv::new()
            .with(1, [1]) // one channel
            .with(2, [0]) // variable bit rate
            .with(3, [rate]);
        Tlv::new().with(1, [kind]).with(2, params.encode()).encode()
    };
    Tlv::new()
        .with(1, codec(3, 2))
        .with(1, codec(2, 1))
        .with(2, [0])
        .encode()
}

/// `SupportedRTPConfiguration`: SRTP with AES-CM-128 and HMAC-SHA1-80.
pub fn supported_rtp() -> Vec<u8> {
    Tlv::new().with(2, [0]).encode()
}

/// `StreamingStatus`: 0 available, 1 in use, 2 unavailable.
pub fn streaming_status(status: u8) -> Vec<u8> {
    Tlv::new().with(1, [status]).encode()
}

// ---- SetupEndpoints ----------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SrtpParams {
    pub key: [u8; KEY_LEN],
    pub salt: [u8; SALT_LEN],
}

impl SrtpParams {
    fn parse(bytes: &[u8]) -> Option<Self> {
        let t = Tlv::parse(bytes)?;
        // Suite 0: AES_CM_128_HMAC_SHA1_80, the only one offered.
        if t.byte(1) != Some(0) {
            return None;
        }
        Some(Self {
            key: t.get(2)?.try_into().ok()?,
            salt: t.get(3)?.try_into().ok()?,
        })
    }

    fn encode(&self) -> Vec<u8> {
        Tlv::new()
            .with(1, [0])
            .with(2, self.key)
            .with(3, self.salt)
            .encode()
    }
}

/// What the phone asks for in `SetupEndpoints`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SetupRequest {
    pub session: [u8; 16],
    pub address: IpAddr,
    pub video_port: u16,
    pub audio_port: u16,
    pub video: SrtpParams,
    pub audio: SrtpParams,
}

pub fn parse_setup(bytes: &[u8]) -> Option<SetupRequest> {
    let t = Tlv::parse(bytes)?;
    let address = Tlv::parse(t.get(3)?)?;
    let ip = std::str::from_utf8(address.get(2)?).ok()?.parse().ok()?;
    let port = |tag| Some(u16::from_le_bytes(address.get(tag)?.try_into().ok()?));
    Some(SetupRequest {
        session: t.get(1)?.try_into().ok()?,
        address: ip,
        video_port: port(3)?,
        audio_port: port(4)?,
        video: SrtpParams::parse(t.get(4)?)?,
        audio: SrtpParams::parse(t.get(5)?)?,
    })
}

/// The answer the phone reads back. The SRTP parameters are the phone's own,
/// echoed: the video is protected with them (what accessories in the field
/// do, and what the phone expects to open it with).
pub fn setup_response(
    request: &SetupRequest,
    local: IpAddr,
    video_port: u16,
    audio_port: u16,
    video_ssrc: u32,
    audio_ssrc: u32,
) -> Vec<u8> {
    let address = Tlv::new()
        .with(1, [u8::from(local.is_ipv6())])
        .with(2, local.to_string())
        .with(3, video_port.to_le_bytes())
        .with(4, audio_port.to_le_bytes());
    Tlv::new()
        .with(1, request.session)
        .with(2, [0])
        .with(3, address.encode())
        .with(4, request.video.encode())
        .with(5, request.audio.encode())
        .with(6, video_ssrc.to_le_bytes())
        .with(7, audio_ssrc.to_le_bytes())
        .encode()
}

/// Status 2 (error) or 1 (busy), the session echoed.
pub fn setup_refusal(session: [u8; 16], busy: bool) -> Vec<u8> {
    Tlv::new()
        .with(1, session)
        .with(2, [if busy { 1 } else { 2 }])
        .encode()
}

// ---- SelectedRTPStreamConfiguration -------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    End,
    Start,
    Suspend,
    Resume,
    Reconfigure,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VideoChoice {
    pub width: u16,
    pub height: u16,
    pub fps: u8,
    pub payload_type: u8,
    pub ssrc: Option<u32>,
    /// Kilobits per second.
    pub max_bitrate: u16,
    pub mtu: usize,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Selected {
    pub session: [u8; 16],
    pub command: Command,
    pub video: Option<VideoChoice>,
}

pub fn parse_selected(bytes: &[u8]) -> Option<Selected> {
    let t = Tlv::parse(bytes)?;
    let control = Tlv::parse(t.get(1)?)?;
    let command = match control.byte(2)? {
        0 => Command::End,
        1 => Command::Start,
        2 => Command::Suspend,
        3 => Command::Resume,
        4 => Command::Reconfigure,
        _ => return None,
    };
    let video = t.get(2).and_then(|v| {
        let v = Tlv::parse(v)?;
        let size = Tlv::parse(v.get(3)?)?;
        let rtp = Tlv::parse(v.get(4)?)?;
        let le16 = |t: &Tlv, tag| {
            t.get(tag)
                .and_then(|b| Some(u16::from_le_bytes(b.try_into().ok()?)))
        };
        Some(VideoChoice {
            width: le16(&size, 1)?,
            height: le16(&size, 2)?,
            fps: size.byte(3).unwrap_or(30),
            payload_type: rtp.byte(1).unwrap_or(99),
            ssrc: rtp
                .get(2)
                .and_then(|b| Some(u32::from_le_bytes(b.try_into().ok()?))),
            max_bitrate: le16(&rtp, 3).unwrap_or(0),
            mtu: le16(&rtp, 5).map_or(DEFAULT_MTU, usize::from),
        })
    });
    Some(Selected {
        session: control.get(1)?.try_into().ok()?,
        command,
        video,
    })
}

// ---- the streams in progress --------------------------------------------------

struct Pending {
    device: DeviceId,
    owner: u64,
    request: SetupRequest,
    video_socket: UdpSocket,
    _audio_socket: UdpSocket,
    ssrc: u32,
    created: Instant,
}

struct Active {
    device: DeviceId,
    owner: u64,
    cancel: CancellationToken,
}

/// Every stream being negotiated or relayed.
pub struct Streams {
    shutdown: CancellationToken,
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    pending: HashMap<[u8; 16], Pending>,
    active: HashMap<[u8; 16], Active>,
}

impl std::fmt::Debug for Streams {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Streams").finish_non_exhaustive()
    }
}

impl Inner {
    fn forget_stale(&mut self) {
        self.pending
            .retain(|_, p| p.created.elapsed() < PENDING_TTL);
    }

    fn count(&self, device: &DeviceId) -> usize {
        self.pending
            .values()
            .filter(|p| &p.device == device)
            .count()
            + self.active.values().filter(|a| &a.device == device).count()
    }

    fn total(&self) -> usize {
        self.pending.len() + self.active.len()
    }
}

impl Streams {
    pub fn new(shutdown: CancellationToken) -> Self {
        Self {
            shutdown,
            inner: Mutex::new(Inner::default()),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// 0 available, 1 in use: a stream is running (what HAP-python and
    /// HAP-NodeJS report; the phone reads it while it watches).
    pub fn status_of(&self, device: &DeviceId) -> u8 {
        u8::from(self.lock().active.values().any(|a| &a.device == device))
    }

    /// Ends a stream (negotiated or running); the devices it served.
    pub fn stop(&self, session: &[u8; 16]) -> Option<DeviceId> {
        let mut inner = self.lock();
        if let Some(active) = inner.active.remove(session) {
            active.cancel.cancel();
            return Some(active.device);
        }
        inner.pending.remove(session).map(|p| p.device)
    }

    /// Ends what a connection started (it closed).
    pub fn stop_owned_by(&self, owner: u64) -> Vec<DeviceId> {
        let mut inner = self.lock();
        let mut devices = Vec::new();
        inner.pending.retain(|_, p| {
            let keep = p.owner != owner;
            if !keep {
                devices.push(p.device.clone());
            }
            keep
        });
        inner.active.retain(|_, a| {
            let keep = a.owner != owner;
            if !keep {
                a.cancel.cancel();
                devices.push(a.device.clone());
            }
            keep
        });
        devices
    }
}

fn random_u32() -> u32 {
    let mut b = [0u8; 4];
    SystemRandom::new().fill(&mut b).expect("system randomness");
    u32::from_be_bytes(b)
}

impl Shared {
    /// Whether HomeKit may see this camera now (the guard and its state).
    fn camera_allowed(&self, device: &DeviceId) -> bool {
        self.hub.protected_room(device).is_none()
    }

    /// Tells the controllers watching the camera that its status changed.
    pub(crate) fn camera_changed(&self, device: &DeviceId) {
        if let Some(pair) = self.catalog().camera_status(device) {
            let _ = self.updates.send(pair);
        }
    }

    /// A write of `SetupEndpoints`: the answer to keep for the phone to read.
    pub(crate) async fn camera_setup(
        &self,
        owner: u64,
        local: Option<IpAddr>,
        device: &DeviceId,
        value: &Json,
    ) -> Result<Vec<u8>, i32> {
        let request = value
            .as_str()
            .and_then(unb64)
            .and_then(|b| parse_setup(&b))
            .ok_or(ST_INVALID)?;
        let refuse = |busy| Ok(setup_refusal(request.session, busy));
        let Some(local) = local.filter(|l| l.is_ipv4() == request.address.is_ipv4()) else {
            return refuse(false);
        };
        if !self.camera_allowed(device) || self.offline_device(device) {
            tracing::info!(%device, "homekit: a camera stream was refused (guard or offline)");
            return refuse(false);
        }
        {
            let mut inner = self.streams.lock();
            inner.forget_stale();
            if inner.count(device) >= MAX_PER_CAMERA || inner.total() >= MAX_TOTAL {
                return refuse(true);
            }
        }
        let any = if local.is_ipv4() { "0.0.0.0" } else { "::" };
        let (Ok(video_socket), Ok(audio_socket)) = (
            UdpSocket::bind((any, 0)).await,
            UdpSocket::bind((any, 0)).await,
        ) else {
            return refuse(false);
        };
        let (Ok(video_port), Ok(audio_port)) = (
            video_socket.local_addr().map(|a| a.port()),
            audio_socket.local_addr().map(|a| a.port()),
        ) else {
            return refuse(false);
        };
        let ssrc = random_u32();
        let reply = setup_response(&request, local, video_port, audio_port, ssrc, random_u32());
        self.streams.lock().pending.insert(
            request.session,
            Pending {
                device: device.clone(),
                owner,
                request,
                video_socket,
                _audio_socket: audio_socket,
                ssrc,
                created: Instant::now(),
            },
        );
        self.camera_changed(device);
        Ok(reply)
    }

    /// A write of `SelectedRTPStreamConfiguration`.
    pub(crate) fn camera_select(self: &Arc<Self>, device: &DeviceId, value: &Json) -> i32 {
        let Some(selected) = value
            .as_str()
            .and_then(unb64)
            .and_then(|b| parse_selected(&b))
        else {
            return ST_INVALID;
        };
        match selected.command {
            Command::End => {
                if let Some(device) = self.streams.stop(&selected.session) {
                    self.camera_changed(&device);
                }
                OK
            }
            Command::Start => self.camera_start(device, selected),
            // The relay keeps the one quality it opened: there is nothing to
            // change on the way.
            Command::Suspend | Command::Resume | Command::Reconfigure => OK,
        }
    }

    fn camera_start(self: &Arc<Self>, device: &DeviceId, selected: Selected) -> i32 {
        let Some(video) = selected.video else {
            return ST_INVALID;
        };
        let pending = {
            let mut inner = self.streams.lock();
            inner.forget_stale();
            let Some(pending) = inner.pending.remove(&selected.session) else {
                return ST_INVALID;
            };
            pending
        };
        if &pending.device != device || !self.camera_allowed(device) {
            self.camera_changed(device);
            return ST_COMMUNICATION;
        }
        let cancel = self.streams.shutdown.child_token();
        self.streams.lock().active.insert(
            selected.session,
            Active {
                device: device.clone(),
                owner: pending.owner,
                cancel: cancel.clone(),
            },
        );
        tracing::info!(
            %device,
            width = video.width,
            height = video.height,
            max_kbps = video.max_bitrate,
            "homekit: a camera stream starts"
        );
        let shared = Arc::clone(self);
        let watched = device.clone();
        tokio::spawn(async move {
            relay(&shared, &watched, pending, video, &cancel).await;
            shared.streams.lock().active.remove(&selected.session);
            shared.camera_changed(&watched);
        });
        self.camera_changed(device);
        OK
    }

    /// `POST /resource`: a JPEG of the camera (the Home app asks for one for
    /// the tile and the notifications).
    pub(crate) async fn camera_image(&self, aid: u64) -> Response {
        let Some(device) = self.catalog().camera_device(aid).cloned() else {
            return Response::empty(404);
        };
        if !self.camera_allowed(&device) || self.offline_device(&device) {
            return Response::empty(500);
        }
        match self.hub.camera_image(&device).await {
            Ok(image) if image.content_type == "image/jpeg" => {
                Response::bytes(200, "image/jpeg", image.bytes)
            }
            Ok(_) => Response::empty(500),
            Err(e) => {
                tracing::debug!(%device, error = %e, "homekit: no image for the Home app");
                Response::empty(500)
            }
        }
    }
}

/// Sends the camera's frames to the phone until `cancel`, the camera or the
/// phone gives up.
#[allow(clippy::too_many_lines)] // one select loop, counters included
async fn relay(
    shared: &Arc<Shared>,
    device: &DeviceId,
    pending: Pending,
    video: VideoChoice,
    cancel: &CancellationToken,
) {
    let begun = Instant::now();
    let want = LiveWant {
        width: u32::from(video.width),
        height: u32::from(video.height),
    };
    let mut live = match shared.hub.camera_live(device, want).await {
        Ok(live) => live,
        Err(e) => {
            tracing::warn!(%device, error = %e, "homekit: the camera stream could not be opened");
            return;
        }
    };
    let info = live.info().clone();
    let opened = begun;
    tracing::info!(
        %device,
        width = info.width,
        height = info.height,
        wanted_width = video.width,
        wanted_height = video.height,
        open_ms = u64::try_from(begun.elapsed().as_millis()).unwrap_or(u64::MAX),
        "homekit: relaying the camera"
    );
    let destination = SocketAddr::new(pending.request.address, pending.request.video_port);
    let socket = pending.video_socket;
    let mut srtp = Sender::new(&pending.request.video.key, &pending.request.video.salt);
    let ssrc = video.ssrc.unwrap_or(pending.ssrc);
    let mut packetizer = Packetizer::new(video.payload_type, ssrc, random_u32() as u16, video.mtu);
    let (mut sps, mut pps) = (
        (!info.sps.is_empty()).then_some(info.sps),
        (!info.pps.is_empty()).then_some(info.pps),
    );
    let first_out = random_u32();
    let mut retimer = Retimer::new(first_out, Instant::now());
    let mut started = false;
    let (mut packets, mut octets, mut last_ts) = (0u32, 0u32, 0u32);
    let mut last_frame = Instant::now();
    let mut heard: Option<Instant> = None;
    let mut report = tokio::time::interval(REPORT_EVERY);
    report.tick().await;
    let mut inbound = [0u8; 2048];
    // Counters for the journal (never a key).
    let (mut frames_in, mut key_frames, mut skipped, mut send_errors, mut phone_packets) =
        (0u32, 0u32, 0u32, 0u32, 0u32);
    let mut held_ms = 0u64;
    tracing::info!(
        %device,
        to = %destination,
        from_port = socket.local_addr().map_or(0, |a| a.port()),
        ssrc,
        payload_type = video.payload_type,
        mtu = video.mtu,
        max_kbps = video.max_bitrate,
        "homekit: camera stream destination"
    );
    loop {
        tokio::select! {
            () = cancel.cancelled() => break,
            frame = live.next_frame() => {
                let frame = match frame {
                    Ok(frame) => frame,
                    Err(e) => {
                        tracing::info!(%device, error = %format!("{e:#}"), "homekit: the camera stream ended");
                        break;
                    }
                };
                frames_in += 1;
                key_frames += u32::from(frame.keyframe);
                for nal in &frame.nals {
                    match moli_net::h264::nal_type(nal) {
                        moli_net::h264::NAL_SPS => sps = Some(nal.clone()),
                        moli_net::h264::NAL_PPS => pps = Some(nal.clone()),
                        _ => {}
                    }
                }
                // A phone can only begin on a picture it can decode alone.
                if !started && !frame.keyframe {
                    skipped += 1;
                    continue;
                }
                if !started {
                    tracing::info!(
                        %device,
                        waited_ms = u64::try_from(opened.elapsed().as_millis()).unwrap_or(u64::MAX),
                        skipped,
                        "homekit: first key frame, the video starts"
                    );
                    // The report first: it ties the stream's clock to the wall clock.
                    let sr = sender_report(ssrc, ntp_now(), first_out, 0, 0);
                    if socket.send_to(&srtp.protect_rtcp(&sr), destination).await.is_err() {
                        return;
                    }
                }
                started = true;
                let mut nals = frame.nals;
                if frame.keyframe && !nals.iter().any(|n| moli_net::h264::nal_type(n) == moli_net::h264::NAL_SPS) {
                    let sets: Vec<Vec<u8>> = [sps.clone(), pps.clone()].into_iter().flatten().collect();
                    nals.splice(0..0, sets);
                }
                let (hold, timestamp) = retimer.frame(frame.timestamp, Instant::now());
                if !hold.is_zero() {
                    held_ms = held_ms.max(u64::try_from(hold.as_millis()).unwrap_or(0));
                    tokio::time::sleep(hold).await;
                }
                last_ts = timestamp;
                last_frame = Instant::now();
                let burst = packetizer.frame(timestamp, &nals);
                for (n, packet) in burst.iter().enumerate() {
                    // A key frame is dozens of packets: spread them a little
                    // rather than hand the access point a wall of them.
                    if n > 0 && n % 8 == 0 {
                        tokio::time::sleep(Duration::from_millis(1)).await;
                    }
                    let sealed = srtp.protect_rtp(packet);
                    packets = packets.wrapping_add(1);
                    octets = octets.wrapping_add((packet.len() - 12) as u32);
                    if let Err(e) = socket.send_to(&sealed, destination).await {
                        send_errors += 1;
                        if send_errors == 1 {
                            tracing::warn!(%device, error = %e, "homekit: a video packet could not be sent");
                        }
                        if send_errors > 50 {
                            return;
                        }
                    }
                    if packets == 1 {
                        tracing::info!(
                            %device,
                            header = %crate::setup::hex(&packet[..12]),
                            size = sealed.len(),
                            "homekit: first video packet sent (clear header)"
                        );
                    }
                }
            }
            _ = report.tick() => {
                if heard.is_some_and(|at| at.elapsed() > CONTROLLER_SILENT) {
                    tracing::info!(%device, "homekit: the phone stopped answering, closing the stream");
                    break;
                }
                if !shared.camera_allowed(device) {
                    tracing::info!(%device, "homekit: the camera's room became protected, closing the stream");
                    break;
                }
                tracing::info!(
                    %device,
                    frames_in,
                    key_frames,
                    packets,
                    octets,
                    send_errors,
                    phone_packets,
                    reanchors = retimer.reanchors,
                    max_hold_ms = held_ms,
                    "homekit: camera stream counters"
                );
                if started {
                    let ticks = (last_frame.elapsed().as_secs_f64() * 90_000.0) as u32;
                    let sr = sender_report(ssrc, ntp_now(), last_ts.wrapping_add(ticks), packets, octets);
                    if socket.send_to(&srtp.protect_rtcp(&sr), destination).await.is_err() {
                        return;
                    }
                }
            }
            // What the phone sends back (reports): proof it is still there.
            received = socket.recv_from(&mut inbound) => {
                if let Ok((n, from)) = received {
                    heard = Some(Instant::now());
                    phone_packets += 1;
                    if phone_packets <= 3 {
                        tracing::info!(%device, bytes = n, %from, "homekit: the phone sent a packet (RTCP)");
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_round_trips() {
        for bytes in [&b""[..], b"f", b"fo", b"foo", b"foob", b"fooba", b"foobar"] {
            assert_eq!(unb64(&b64(bytes)).unwrap(), bytes);
        }
        assert_eq!(b64(b"foobar"), "Zm9vYmFy");
        assert_eq!(b64(b"fo"), "Zm8=");
        assert!(unb64("a*b").is_none());
    }

    fn srtp(key: u8, salt: u8) -> Tlv {
        Tlv::new()
            .with(1, [0])
            .with(2, [key; 16])
            .with(3, [salt; 14])
    }

    #[test]
    fn setup_endpoints_are_read_and_answered() {
        let request = Tlv::new()
            .with(1, [7u8; 16])
            .with(
                3,
                Tlv::new()
                    .with(1, [0])
                    .with(2, "192.168.1.20")
                    .with(3, 50_000u16.to_le_bytes())
                    .with(4, 50_002u16.to_le_bytes())
                    .encode(),
            )
            .with(4, srtp(1, 2).encode())
            .with(5, srtp(3, 4).encode())
            .encode();
        let parsed = parse_setup(&request).unwrap();
        assert_eq!(parsed.session, [7; 16]);
        assert_eq!(parsed.address, "192.168.1.20".parse::<IpAddr>().unwrap());
        assert_eq!((parsed.video_port, parsed.audio_port), (50_000, 50_002));
        assert_eq!(parsed.video.key, [1; 16]);
        assert_eq!(parsed.audio.salt, [4; 14]);

        let answer = setup_response(
            &parsed,
            "192.168.1.5".parse().unwrap(),
            6000,
            6002,
            0x0A0B_0C0D,
            5,
        );
        let t = Tlv::parse(&answer).unwrap();
        assert_eq!(t.get(1), Some(&[7u8; 16][..]));
        assert_eq!(t.byte(2), Some(0));
        let address = Tlv::parse(t.get(3).unwrap()).unwrap();
        assert_eq!(address.byte(1), Some(0));
        assert_eq!(address.get(2), Some(&b"192.168.1.5"[..]));
        assert_eq!(address.get(3), Some(&6000u16.to_le_bytes()[..]));
        assert_eq!(
            Tlv::parse(t.get(4).unwrap()).unwrap().get(2),
            Some(&[1u8; 16][..])
        );
        assert_eq!(t.get(6), Some(&0x0A0B_0C0Du32.to_le_bytes()[..]));
        // The refusals say busy or error, with the session.
        assert_eq!(
            setup_refusal([7; 16], true),
            Tlv::new().with(1, [7u8; 16]).with(2, [1]).encode()
        );
    }

    #[test]
    fn setup_endpoints_without_the_expected_suite_or_key_are_refused() {
        let wrong_suite = Tlv::new()
            .with(1, [2])
            .with(2, [1u8; 16])
            .with(3, [2u8; 14]);
        assert!(SrtpParams::parse(&wrong_suite.encode()).is_none());
        let short_key = Tlv::new()
            .with(1, [0])
            .with(2, [1u8; 15])
            .with(3, [2u8; 14]);
        assert!(SrtpParams::parse(&short_key.encode()).is_none());
        assert!(parse_setup(&[1, 2, 3]).is_none());
    }

    #[test]
    fn the_selected_configuration_gives_the_command_and_the_stream_wanted() {
        let rtp = Tlv::new()
            .with(1, [99])
            .with(2, 0xDEAD_BEEFu32.to_le_bytes())
            .with(3, 300u16.to_le_bytes())
            .with(4, 0.5f32.to_le_bytes())
            .with(5, 1228u16.to_le_bytes());
        let size = Tlv::new()
            .with(1, 640u16.to_le_bytes())
            .with(2, 360u16.to_le_bytes())
            .with(3, [15]);
        let selected = Tlv::new()
            .with(1, Tlv::new().with(1, [9u8; 16]).with(2, [1]).encode())
            .with(
                2,
                Tlv::new()
                    .with(1, [0])
                    .with(3, size.encode())
                    .with(4, rtp.encode())
                    .encode(),
            )
            .encode();
        let parsed = parse_selected(&selected).unwrap();
        assert_eq!((parsed.session, parsed.command), ([9; 16], Command::Start));
        let video = parsed.video.unwrap();
        assert_eq!((video.width, video.height, video.fps), (640, 360, 15));
        assert_eq!((video.payload_type, video.ssrc), (99, Some(0xDEAD_BEEF)));
        assert_eq!((video.max_bitrate, video.mtu), (300, 1228));
        // Ending carries no video.
        let end = Tlv::new()
            .with(1, Tlv::new().with(1, [9u8; 16]).with(2, [0]).encode())
            .encode();
        let parsed = parse_selected(&end).unwrap();
        assert_eq!((parsed.command, parsed.video), (Command::End, None));
        assert!(
            parse_selected(
                &Tlv::new()
                    .with(1, Tlv::new().with(1, [9u8; 16]).with(2, [9]).encode())
                    .encode()
            )
            .is_none()
        );
    }

    #[test]
    fn what_the_camera_announces_is_h264_srtp_and_a_status() {
        let video = Tlv::parse(&supported_video()).unwrap();
        let config = Tlv::parse(video.get(1).unwrap()).unwrap();
        let params = Tlv::parse(config.get(2).unwrap()).unwrap();
        assert_eq!(
            (params.byte(3), params.all(1).count(), params.all(2).count()),
            (Some(0), 3, 3)
        );
        let sizes: Vec<(u16, u16)> = config
            .all(3)
            .map(|a| {
                let a = Tlv::parse(a).unwrap();
                (
                    u16::from_le_bytes(a.get(1).unwrap().try_into().unwrap()),
                    u16::from_le_bytes(a.get(2).unwrap().try_into().unwrap()),
                )
            })
            .collect();
        assert_eq!(sizes[0], (1920, 1080));
        assert_eq!(sizes.len(), 5);
        assert_eq!(supported_rtp(), [2, 1, 0]);
        assert_eq!(streaming_status(1), [1, 1, 1]);
        assert!(Tlv::parse(&supported_audio()).is_some());
    }

    /// HAP-python's camera, accepted by iPhones, is the reference: its
    /// values for the audio, RTP and status, and the start of its video
    /// configuration (everything before the sizes), byte for byte.
    #[test]
    fn the_values_equal_hap_pythons() {
        assert_eq!(
            b64(&supported_audio()),
            "AQ4BAQMCCQEBAQIBAAMBAgEOAQECAgkBAQECAQADAQECAQA="
        );
        assert_eq!(b64(&supported_rtp()), "AgEA");
        assert_eq!(b64(&streaming_status(0)), "AQEA");
        let reference = unb64("AUQBAQACFQMBAAEBAAEBAQEBAgIBAAIBAQIBAgMMAQJAAQIC8AADAg8A").unwrap();
        // Skip the two length bytes (the sizes differ), compare the rest.
        assert_eq!(supported_video()[2..28], reference[2..28]);
        // A size: width, height, frame rate (two bytes).
        assert_eq!(
            &supported_video()[28..],
            &{
                let mut first = vec![3, 12, 1, 2, 0x80, 0x07, 2, 2, 0x38, 0x04, 3, 2, 30, 0];
                first.extend_from_slice(&supported_video()[42..]);
                first
            }[..]
        );
    }
}
