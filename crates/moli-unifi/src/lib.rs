//! UniFi Protect cameras, through the console's Integration API
//! (`/proxy/protect/integration/v1`, an API key in `X-API-KEY`): the cameras
//! the console knows, their latest image on demand, and motion from its
//! event websocket, and live video (the console's RTSPS stream, relayed as
//! H.264 without decoding). Nothing here changes a camera's settings; asking
//! for a stream makes the console publish one (`rtsps-stream`).
//!
//! The console's certificate is self-signed: it is pinned at first contact,
//! before the key is sent. The key lives in the encrypted secret store
//! (`moli-os secrets set <instance> api_key`), never in `moli.toml`.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use anyhow::{Context as _, bail};
use moli_audio::aac;
use moli_core::{Access, Device, Kind, PointSpec, Semantic, Value};
use moli_net::h264::{self, Depacketizer};
use moli_net::rtsp::{self, Media};
use moli_net::ws::WebSocket;
use moli_net::{Method, PIN_MISMATCH, PinnedHttps, PinnedTls};
use moli_runtime::media::{
    AudioChunk, Image, LiveInfo, LiveItem, LiveSource, LiveStream, LiveWant, SnapshotSource,
    TalkSink, TalkSource, VideoFrame,
};
use moli_runtime::{BoxFuture, Driver, DriverCtx};
use serde::Deserialize;
use serde_json::Value as Json;

const BASE: &str = "/proxy/protect/integration/v1";
const CERT: &str = "cert_sha256";
const API_KEY: &str = "api_key";
/// The certificate of the console's RTSPS port, pinned on its own: it is not
/// necessarily the one of the API port.
const RTSPS_CERT: &str = "rtsps_cert_sha256";
/// The streams the console published stay valid; asked again after this.
const STREAM_URLS_TTL: Duration = Duration::from_secs(300);
/// The camera list is read again this often (new cameras, connection state).
const POLL: Duration = Duration::from_secs(60);
/// A camera that never reported the end of its motion is still after this.
const MOTION_HOLD: Duration = Duration::from_secs(120);
/// A camera waking up for an image may take a while.
const IMAGE_BUDGET: Duration = Duration::from_secs(15);
/// No message for this long: reopen the websocket.
const STREAM_IDLE: Duration = Duration::from_secs(3600);

/// Driver configuration (`[driver.options]` in `moli.toml`).
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// The console's IP or host name.
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
    /// Optional: take the API key from this environment variable instead of
    /// the secret store.
    #[serde(default)]
    pub api_key_env: Option<String>,
    /// Optional: the console certificate's SHA-256 (hex), when it cannot be
    /// learned and stored at first contact (no secret store).
    #[serde(default)]
    pub cert_sha256: Option<String>,
}

fn default_port() -> u16 {
    443
}

#[derive(Debug)]
pub struct Unifi {
    config: Config,
}

impl Unifi {
    #[must_use]
    pub fn new(config: Config) -> Self {
        Self { config }
    }
}

impl Driver for Unifi {
    fn kind(&self) -> &'static str {
        "unifi"
    }

    fn run<'a>(&'a self, ctx: &'a mut DriverCtx) -> BoxFuture<'a, anyhow::Result<()>> {
        Box::pin(run(&self.config, ctx))
    }
}

/// One console and its API key.
struct Console {
    http: PinnedHttps,
    key: String,
}

impl std::fmt::Debug for Console {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never the key: it reads every camera of the house.
        f.debug_struct("Console")
            .field("http", &self.http)
            .finish_non_exhaustive()
    }
}

/// The console refused the API key.
#[derive(Debug)]
struct KeyRejected;

impl std::fmt::Display for KeyRejected {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("unifi API key rejected")
    }
}

impl std::error::Error for KeyRejected {}

impl Console {
    fn new(host: &str, port: u16, pin: String, key: String) -> anyhow::Result<Self> {
        Ok(Self {
            http: PinnedHttps::new(host, port, Some(pin))?,
            key,
        })
    }

    /// A bare handshake, nothing sent: the fingerprint of the certificate
    /// the console presents, to store before any key leaves.
    async fn first_contact(host: &str, port: u16) -> anyhow::Result<String> {
        let probe = PinnedHttps::new(host, port, None)?;
        probe.handshake().await.context("first contact")?;
        probe.pinned_fingerprint().context("no certificate seen")
    }

    fn request(
        &self,
        path: &str,
    ) -> anyhow::Result<http::Request<http_body_util::Full<moli_net::Body>>> {
        moli_net::empty(
            self.http
                .request(Method::GET, &format!("{BASE}{path}"))
                .header("x-api-key", &self.key),
        )
    }

    /// A `GET` answered with JSON.
    async fn json(&self, path: &str) -> anyhow::Result<Vec<u8>> {
        let (status, body) = self.http.send(self.request(path)?).await?;
        if status == http::StatusCode::UNAUTHORIZED || status == http::StatusCode::FORBIDDEN {
            return Err(anyhow::Error::new(KeyRejected));
        }
        if !status.is_success() {
            bail!("unifi {path}: {status}");
        }
        Ok(body.to_vec())
    }

    /// A `GET` that may take its time (an image), on its own connection.
    async fn slow(&self, path: &str) -> anyhow::Result<(http::StatusCode, moli_net::Body)> {
        self.http.send_slow(self.request(path)?, IMAGE_BUDGET).await
    }
}

/// A camera, as the console lists it.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
struct Camera {
    id: String,
    #[serde(default)]
    name: String,
    /// The model (« UVC G3 Pro », « C1 »…).
    #[serde(default, rename = "type")]
    model: String,
    #[serde(default)]
    state: String,
    #[serde(default, rename = "featureFlags")]
    flags: Flags,
}

/// What the console says a camera can do.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
struct Flags {
    #[serde(default, rename = "hasSpeaker")]
    has_speaker: bool,
}

impl Camera {
    fn online(&self) -> bool {
        self.state == "CONNECTED"
    }

    /// Ids go into URL paths: only what the console really uses.
    fn sane(&self) -> bool {
        !self.id.is_empty()
            && self
                .id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-')
    }
}

fn parse_cameras(body: &[u8]) -> anyhow::Result<Vec<Camera>> {
    let all: Vec<Camera> = serde_json::from_slice(body).context("unifi camera list")?;
    Ok(all.into_iter().filter(Camera::sane).collect())
}

fn camera_device(ctx: &DriverCtx, camera: &Camera) -> Device {
    let name = if camera.name.is_empty() {
        camera.id.clone()
    } else {
        camera.name.clone()
    };
    Device {
        id: ctx.device_id(&camera.id),
        instance: ctx.instance().clone(),
        native_name: name.into(),
        manufacturer: Some("Ubiquiti".into()),
        model: Some(
            if camera.model.is_empty() {
                moli_i18n::tr!("pilotes.unifi.camera")
            } else {
                camera.model.clone()
            }
            .into(),
        ),
        description: None,
        native_room: None,
        members: Vec::new(),
        points: vec![PointSpec {
            key: "motion".into(),
            label: moli_i18n::tr!("pilotes.unifi.mouvement").into(),
            semantic: Semantic::infer("motion", None),
            kind: Kind::Binary,
            access: Access {
                read: true,
                write: false,
            },
            unit: None,
        }],
    }
}

/// The latest image of one camera.
#[derive(Debug)]
struct Snapshots {
    console: Arc<Console>,
    camera: String,
}

impl SnapshotSource for Snapshots {
    fn snapshot(&self) -> BoxFuture<'_, anyhow::Result<Image>> {
        Box::pin(async move {
            let (status, bytes) = self
                .console
                .slow(&format!("/cameras/{}/snapshot", self.camera))
                .await?;
            if !status.is_success() || !bytes.starts_with(&[0xFF, 0xD8]) {
                bail!(moli_i18n::tr!(
                    "pilotes.unifi.aucune_image",
                    status = status
                ));
            }
            Ok(Image {
                content_type: "image/jpeg".into(),
                bytes: bytes.to_vec(),
            })
        })
    }
}

/// What an event message says about a camera's motion.
#[derive(Debug, PartialEq, Eq)]
enum Motion {
    Start(String),
    End(String),
}

/// Reads one websocket message: `{"type":"add|update|remove","item":{…}}`,
/// the item being an event (`modelKey` « event ») with its `type`, the
/// camera in `device`, and an `end` once it is over. Anything else is not
/// about motion.
fn parse_event(text: &str) -> Option<Motion> {
    let message: Json = serde_json::from_str(text).ok()?;
    let item = message.get("item").unwrap_or(&message);
    if item["modelKey"].as_str().is_some_and(|k| k != "event") {
        return None;
    }
    if !matches!(
        item["type"].as_str()?,
        "motion" | "smartDetectZone" | "smartDetectLine"
    ) {
        return None;
    }
    let camera = ["device", "deviceId", "camera"]
        .iter()
        .find_map(|k| item[*k].as_str())?
        .to_owned();
    let over = item["end"].as_u64().is_some_and(|t| t > 0) || message["type"] == "remove";
    Some(if over {
        Motion::End(camera)
    } else {
        Motion::Start(camera)
    })
}

/// Shows `reason` on every surface and idles until shutdown.
async fn park(ctx: &DriverCtx, reason: String) -> anyhow::Result<()> {
    ctx.wait_for(reason);
    ctx.cancelled().await;
    Ok(())
}

fn secrets_problem(ctx: &DriverCtx) -> String {
    ctx.secrets_problem().map_or_else(
        || moli_i18n::tr!("pilotes.unifi.coffre_indisponible"),
        str::to_owned,
    )
}

/// What the driver knows of the cameras it published.
struct Cameras {
    online: HashMap<String, bool>,
    live: Arc<LiveShared>,
}

impl Cameras {
    fn new(host: &str, rtsps_pin: Option<String>) -> Self {
        Self {
            online: HashMap::new(),
            live: Arc::new(LiveShared {
                host: host.to_owned(),
                pin: Mutex::new(rtsps_pin),
                learned: Mutex::new(None),
            }),
        }
    }
}

/// What every camera's live source shares: where the console is, and the
/// certificate pinned for its RTSPS port.
#[derive(Debug)]
struct LiveShared {
    host: String,
    pin: Mutex<Option<String>>,
    /// A pin learned at first contact, not yet in the secret store.
    learned: Mutex<Option<String>>,
}

impl LiveShared {
    /// TLS settings for the RTSPS port: the stored pin, else the certificate
    /// seen by a bare handshake (nothing sent), kept for next time.
    async fn tls(&self, port: u16) -> anyhow::Result<PinnedTls> {
        let known = self
            .pin
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let pin = if let Some(pin) = known {
            pin
        } else {
            let probe = PinnedTls::new(None)?;
            drop(
                probe
                    .connect(&self.host, port)
                    .await
                    .context("rtsps first contact")?,
            );
            let seen = probe.seen_fingerprint().context("no certificate seen")?;
            *self.pin.lock().unwrap_or_else(PoisonError::into_inner) = Some(seen.clone());
            *self.learned.lock().unwrap_or_else(PoisonError::into_inner) = Some(seen.clone());
            seen
        };
        PinnedTls::new(Some(pin))
    }

    fn take_learned(&self) -> Option<String> {
        self.learned
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
    }
}

/// The streams the console publishes for a camera, by quality
/// (`{"high":"rtsps://…","medium":…,"low":…}`).
fn parse_streams(body: &[u8]) -> HashMap<String, String> {
    let Ok(Json::Object(map)) = serde_json::from_slice::<Json>(body) else {
        return HashMap::new();
    };
    map.into_iter()
        .filter_map(|(quality, url)| Some((quality, url.as_str()?.to_owned())))
        .collect()
}

/// The quality to read for a picture of this height: the smallest one at
/// least that tall, else the tallest there is. (Protect's usual sizes.)
fn pick_quality(want_height: u32, available: &HashMap<String, String>) -> Option<&'static str> {
    const QUALITIES: [(&str, u32); 3] = [("low", 360), ("medium", 720), ("high", 1080)];
    let have: Vec<(&'static str, u32)> = QUALITIES
        .into_iter()
        .filter(|(name, _)| available.contains_key(*name))
        .collect();
    have.iter()
        .find(|(_, height)| *height >= want_height)
        .or(have.last())
        .map(|(name, _)| *name)
}

/// The console's own URL for a stream, aimed at the address Moli knows it
/// by, without the SRTP option (RTSPS already protects the stream).
fn stream_url(announced: &str, host: &str) -> Option<rtsp::Url> {
    let mut url = rtsp::Url::parse(announced)?.at(host);
    if let Some((path, query)) = url.path.split_once('?') {
        let query: Vec<&str> = query.split('&').filter(|q| *q != "enableSrtp").collect();
        url.path = if query.is_empty() {
            path.to_owned()
        } else {
            format!("{path}?{}", query.join("&"))
        };
    }
    Some(url)
}

/// Live video of one camera.
#[derive(Debug)]
struct Live {
    console: Arc<Console>,
    shared: Arc<LiveShared>,
    camera: String,
    urls: Mutex<Option<(Instant, HashMap<String, String>)>>,
}

impl Live {
    /// The published streams (asked for, if the console has none yet).
    async fn streams(&self, fresh: bool) -> anyhow::Result<HashMap<String, String>> {
        if !fresh
            && let Some((at, urls)) = self
                .urls
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .as_ref()
            && at.elapsed() < STREAM_URLS_TTL
        {
            return Ok(urls.clone());
        }
        let path = format!("{BASE}/cameras/{}/rtsps-stream", self.camera);
        let request = moli_net::json_body(
            self.console
                .http
                .request(Method::POST, &path)
                .header("x-api-key", &self.console.key),
            &serde_json::json!({ "qualities": ["high", "medium", "low"] }),
        )?;
        let (status, body) = self.console.http.send(request).await?;
        if !status.is_success() {
            bail!("unifi {path}: {status}");
        }
        let urls = parse_streams(&body);
        *self.urls.lock().unwrap_or_else(PoisonError::into_inner) =
            Some((Instant::now(), urls.clone()));
        Ok(urls)
    }

    async fn connect(&self, want: LiveWant, fresh: bool) -> anyhow::Result<UnifiStream> {
        let urls = self.streams(fresh).await?;
        let quality =
            pick_quality(want.height, &urls).context("the console published no stream")?;
        let url =
            stream_url(&urls[quality], &self.shared.host).context("unusable stream address")?;
        let tls = self.shared.tls(url.port).await?;
        let mut client = rtsp::Client::connect(&url, Some(&tls)).await?;
        let (track, audio) = client.play(want.audio).await?;
        let (width, height) = track
            .sps
            .as_deref()
            .and_then(h264::picture_size)
            .unwrap_or_default();
        // Sound is a bonus: a track the decoder cannot read is left unread.
        let audio = audio.and_then(|a| {
            let decoder = aac::Format::parse(&a.fmtp)
                .and_then(|f| f.rate().map(|rate| (f, rate)))
                .and_then(|(f, rate)| aac::Decoder::new(f).ok().map(|d| (d, rate)));
            if decoder.is_none() {
                tracing::debug!(camera = %self.camera, "unifi live stream: audio not readable");
            }
            decoder
        });
        tracing::debug!(camera = %self.camera, quality, width, height, audio = audio.is_some(), "unifi live stream opened");
        Ok(UnifiStream {
            client,
            depacketizer: Depacketizer::new(),
            pending: VecDeque::new(),
            info: LiveInfo {
                width,
                height,
                sps: track.sps.unwrap_or_default(),
                pps: track.pps.unwrap_or_default(),
                audio_rate: audio.as_ref().map(|(_, rate)| *rate),
            },
            audio,
        })
    }
}

impl LiveSource for Live {
    fn open(&self, want: LiveWant) -> BoxFuture<'_, anyhow::Result<Box<dyn LiveStream>>> {
        Box::pin(async move {
            // The published addresses can be stale (the console restarted):
            // one more try with fresh ones.
            let stream = match self.connect(want, false).await {
                Ok(stream) => stream,
                Err(e) if format!("{e:#}").contains(PIN_MISMATCH) => return Err(e),
                Err(e) => {
                    tracing::debug!(error = %format!("{e:#}"), "unifi live stream: trying again");
                    self.connect(want, true).await?
                }
            };
            Ok(Box::new(stream) as Box<dyn LiveStream>)
        })
    }
}

struct UnifiStream {
    client: rtsp::Client,
    depacketizer: Depacketizer,
    pending: VecDeque<LiveItem>,
    info: LiveInfo,
    audio: Option<(aac::Decoder, u32)>,
}

impl LiveStream for UnifiStream {
    fn info(&self) -> &LiveInfo {
        &self.info
    }

    fn next(&mut self) -> BoxFuture<'_, anyhow::Result<LiveItem>> {
        Box::pin(async move {
            loop {
                if let Some(item) = self.pending.pop_front() {
                    return Ok(item);
                }
                match self.client.next_media().await? {
                    Media::Video(packet) => {
                        self.pending
                            .extend(self.depacketizer.push(&packet).into_iter().map(|unit| {
                                LiveItem::Video(VideoFrame {
                                    timestamp: unit.timestamp,
                                    nals: unit.nals,
                                    keyframe: unit.keyframe,
                                })
                            }));
                    }
                    Media::Audio(packet) => {
                        if let (Some((decoder, rate)), Some(payload)) =
                            (self.audio.as_mut(), h264::rtp_payload(&packet))
                        {
                            let samples = decoder.push(payload);
                            if !samples.is_empty() {
                                self.pending.push_back(LiveItem::Audio(AudioChunk {
                                    rate: *rate,
                                    samples,
                                }));
                            }
                        }
                    }
                }
            }
        })
    }
}

/// What the console answers to a talkback request.
#[derive(Debug, Deserialize, PartialEq, Eq)]
struct TalkSession {
    /// Where the camera listens: `rtp://<camera>:<port>` (or `udp://`).
    url: String,
    #[serde(default)]
    codec: String,
    #[serde(default, alias = "sampling_rate", rename = "samplingRate")]
    sampling_rate: u32,
    #[serde(default, alias = "bits_per_sample", rename = "bitsPerSample")]
    bits_per_sample: u32,
}

/// `host:port` out of `rtp://host:port`.
fn talk_target(url: &str) -> Option<String> {
    let rest = url.split_once("://")?.1;
    let authority = rest.split(['/', '?']).next()?;
    let (host, port) = authority.rsplit_once(':')?;
    (!host.is_empty() && port.parse::<u16>().is_ok()).then(|| authority.to_owned())
}

/// Talking to a camera's speaker: a session asked for from the console when
/// somebody first speaks, then the voice as Opus over RTP to the camera.
#[derive(Debug)]
struct Talk {
    console: Arc<Console>,
    camera: String,
}

impl TalkSource for Talk {
    fn open(&self) -> BoxFuture<'_, anyhow::Result<Box<dyn TalkSink>>> {
        Box::pin(async move {
            let path = format!("{BASE}/cameras/{}/talkback-session", self.camera);
            let request = moli_net::empty(
                self.console
                    .http
                    .request(Method::POST, &path)
                    .header("x-api-key", &self.console.key),
            )?;
            // Never repeated: a second request would open a second session.
            let (status, body) = self.console.http.send_once(request).await?;
            if !status.is_success() {
                bail!("unifi {path}: {status}");
            }
            let session: TalkSession =
                serde_json::from_slice(&body).context("unifi talkback session")?;
            let target = talk_target(&session.url).context("unusable talkback address")?;
            tracing::info!(
                camera = %self.camera,
                codec = %session.codec,
                rate = session.sampling_rate,
                bits = session.bits_per_sample,
                to = %target,
                "unifi: talkback session opened"
            );
            if !session.codec.eq_ignore_ascii_case("opus") {
                bail!(
                    "the camera wants {} for talkback, only Opus is made here",
                    session.codec
                );
            }
            let stream = TalkStream::connect(&target, session.sampling_rate).await?;
            Ok(Box::new(stream) as Box<dyn TalkSink>)
        })
    }
}

/// Opus as RTP, to a camera's speaker.
struct TalkStream {
    socket: tokio::net::UdpSocket,
    encoder: opus_rs::OpusEncoder,
    rate: u32,
    pcm: Vec<i16>,
    seq: u16,
    timestamp: u32,
    ssrc: u32,
    packets: u32,
}

/// Dynamic payload type, and Opus's RTP clock (RFC 7587: always 48 kHz).
const TALK_PAYLOAD_TYPE: u8 = 96;
const TALK_TICKS: u32 = 960;

impl TalkStream {
    #[allow(clippy::cast_possible_truncation)] // a seed, cut to the width it feeds
    async fn connect(target: &str, rate: u32) -> anyhow::Result<Self> {
        if !matches!(rate, 8_000 | 12_000 | 16_000 | 24_000 | 48_000) {
            bail!("the camera takes Opus at {rate} Hz, which Opus does not do");
        }
        let socket = tokio::net::UdpSocket::bind("0.0.0.0:0").await?;
        socket.connect(target).await.context("talkback address")?;
        let rate_i32 = i32::try_from(rate)?;
        let mut encoder = opus_rs::OpusEncoder::new(rate_i32, 1, opus_rs::Application::Voip)
            .map_err(|e| anyhow::anyhow!("opus encoder: {e:?}"))?;
        encoder.bitrate_bps = 24_000;
        encoder.use_cbr = true;
        // Not secret, only unlike the next stream's.
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(1, |d| {
                d.subsec_nanos() ^ (d.as_secs() as u32).rotate_left(16)
            });
        Ok(Self {
            socket,
            encoder,
            rate,
            pcm: Vec::new(),
            seq: seed as u16,
            timestamp: seed.wrapping_mul(2_654_435_761),
            ssrc: seed.rotate_left(7) ^ 0x5A5A_1234,
            packets: 0,
        })
    }

    /// The RTP packets the voice so far completes (20 ms each).
    fn packets(&mut self, samples: &[i16]) -> Vec<Vec<u8>> {
        self.pcm.extend_from_slice(samples);
        let frame = (self.rate / 50) as usize;
        let mut out = Vec::new();
        while self.pcm.len() >= frame {
            let pcm: Vec<i16> = self.pcm.drain(..frame).collect();
            let mut opus = vec![0u8; 1276];
            let Ok(n) = self.encoder.encode_i16(&pcm, frame, &mut opus) else {
                continue;
            };
            let mut packet = vec![0x80, TALK_PAYLOAD_TYPE];
            packet.extend_from_slice(&self.seq.to_be_bytes());
            packet.extend_from_slice(&self.timestamp.to_be_bytes());
            packet.extend_from_slice(&self.ssrc.to_be_bytes());
            packet.extend_from_slice(&opus[..n]);
            self.seq = self.seq.wrapping_add(1);
            self.timestamp = self.timestamp.wrapping_add(TALK_TICKS);
            out.push(packet);
        }
        out
    }
}

impl TalkSink for TalkStream {
    fn rate(&self) -> u32 {
        self.rate
    }

    fn send(&mut self, samples: &[i16]) -> BoxFuture<'_, anyhow::Result<()>> {
        let packets = self.packets(samples);
        Box::pin(async move {
            for packet in packets {
                self.socket.send(&packet).await?;
                self.packets = self.packets.wrapping_add(1);
                if self.packets == 1 || self.packets.is_multiple_of(250) {
                    tracing::info!(packets = self.packets, "unifi: voice sent to the camera");
                }
            }
            Ok(())
        })
    }
}

/// Reads the camera list and brings the hub in line with it.
async fn refresh(
    console: &Arc<Console>,
    ctx: &DriverCtx,
    known: &mut Cameras,
) -> anyhow::Result<()> {
    let cameras = parse_cameras(&console.json("/cameras").await?)?;
    for id in ctx.devices() {
        if !cameras.iter().any(|c| ctx.device_id(&c.id) == id) {
            ctx.remove_device(&id);
        }
    }
    known
        .online
        .retain(|id, _| cameras.iter().any(|c| &c.id == id));
    for camera in &cameras {
        let device = ctx.device_id(&camera.id);
        if !known.online.contains_key(&camera.id) {
            ctx.upsert_device(camera_device(ctx, camera));
            ctx.provide_snapshots(
                &device,
                Arc::new(Snapshots {
                    console: Arc::clone(console),
                    camera: camera.id.clone(),
                }),
            );
            ctx.provide_live(
                &device,
                Arc::new(Live {
                    console: Arc::clone(console),
                    shared: Arc::clone(&known.live),
                    camera: camera.id.clone(),
                    urls: Mutex::new(None),
                }),
            );
            if camera.flags.has_speaker {
                ctx.provide_talk(
                    &device,
                    Arc::new(Talk {
                        console: Arc::clone(console),
                        camera: camera.id.clone(),
                    }),
                );
            }
            ctx.set_state(&device, "motion", Value::Bool(false));
        }
        if known.online.get(&camera.id) != Some(&camera.online()) {
            ctx.set_availability(&device, camera.online());
        }
        known.online.insert(camera.id.clone(), camera.online());
    }
    Ok(())
}

#[allow(clippy::too_many_lines)] // one select loop
async fn run(config: &Config, ctx: &mut DriverCtx) -> anyhow::Result<()> {
    let pin = match config.cert_sha256.clone().or_else(|| ctx.secret(CERT)) {
        Some(pin) => pin,
        None if !ctx.can_store_secrets() => {
            let reason = moli_i18n::tr!(
                "pilotes.unifi.epinglage_impossible",
                problem = secrets_problem(ctx)
            );
            return park(ctx, reason).await;
        }
        // Pinned and stored before the key leaves.
        None => {
            let seen = Console::first_contact(&config.host, config.port)
                .await
                .context("unifi")?;
            ctx.store_secret(CERT, &seen).await?;
            tracing::info!(instance = %ctx.instance(), "unifi console certificate pinned");
            seen
        }
    };

    let key = config
        .api_key_env
        .as_ref()
        .and_then(|var| std::env::var(var).ok())
        .or_else(|| ctx.secret(API_KEY));
    let Some(key) = key else {
        let reason = moli_i18n::tr!("pilotes.unifi.cle_manquante", instance = ctx.instance());
        return park(ctx, reason).await;
    };

    let console = Arc::new(Console::new(
        &config.host,
        config.port,
        pin.clone(),
        key.clone(),
    )?);
    let tls = PinnedTls::new(Some(pin))?;
    let mut known = Cameras::new(&config.host, ctx.secret(RTSPS_CERT));
    match refresh(&console, ctx, &mut known).await {
        Ok(()) => {}
        Err(e) if e.is::<KeyRejected>() => {
            let reason = moli_i18n::tr!(
                "pilotes.unifi.cle_refusee",
                host = config.host,
                instance = ctx.instance()
            );
            return park(ctx, reason).await;
        }
        Err(e) if format!("{e:#}").contains(PIN_MISMATCH) => {
            let reason = moli_i18n::tr!(
                "pilotes.unifi.certificat_change",
                host = config.host,
                instance = ctx.instance()
            );
            return park(ctx, reason).await;
        }
        Err(e) => return Err(e),
    }
    tracing::info!(instance = %ctx.instance(), cameras = known.online.len(), "unifi console loaded");
    ctx.ready();

    let events_path = format!("{BASE}/subscribe/events");
    let mut socket: Option<WebSocket> = None;
    // Motion in progress, and when to call it over if nobody says so.
    let mut moving: HashMap<String, tokio::time::Instant> = HashMap::new();
    let mut poll = tokio::time::interval(POLL);
    let mut hold = tokio::time::interval(Duration::from_secs(10));
    loop {
        if socket.is_none() {
            match WebSocket::connect(
                &config.host,
                config.port,
                &events_path,
                &[("X-API-KEY", key.as_str())],
                &tls,
            )
            .await
            {
                Ok(s) => {
                    tracing::info!(instance = %ctx.instance(), "unifi event stream connected");
                    socket = Some(s);
                }
                Err(e) => {
                    tracing::warn!(instance = %ctx.instance(), error = %format!("{e:#}"), "unifi event stream unavailable, retrying");
                }
            }
        }
        tokio::select! {
            command = ctx.next_command() => match command {
                Some(command) => command.reply(Err(moli_i18n::tr!("pilotes.unifi.lecture_seule"))),
                None => return Ok(()),
            },
            message = async {
                match socket.as_mut() {
                    Some(s) => s.recv_text(STREAM_IDLE).await,
                    None => std::future::pending().await,
                }
            } => match message {
                Ok(text) => {
                    tracing::debug!(instance = %ctx.instance(), message = %text.chars().take(300).collect::<String>(), "unifi event");
                    match parse_event(&text) {
                        Some(Motion::Start(camera)) => {
                            moving.insert(camera.clone(), tokio::time::Instant::now() + MOTION_HOLD);
                            ctx.set_state(&ctx.device_id(&camera), "motion", Value::Bool(true));
                        }
                        Some(Motion::End(camera)) => {
                            moving.remove(&camera);
                            ctx.set_state(&ctx.device_id(&camera), "motion", Value::Bool(false));
                        }
                        None => {}
                    }
                }
                Err(e) => {
                    tracing::warn!(instance = %ctx.instance(), error = %format!("{e:#}"), "unifi event stream dropped, reopening");
                    socket = None;
                }
            },
            _ = poll.tick() => {
                if let Some(pin) = known.live.take_learned()
                    && let Err(e) = ctx.store_secret(RTSPS_CERT, &pin).await
                {
                    tracing::warn!(instance = %ctx.instance(), error = %format!("{e:#}"), "unifi: cannot keep the RTSPS certificate");
                }
                if let Err(e) = refresh(&console, ctx, &mut known).await {
                    if e.is::<KeyRejected>() {
                        let reason = moli_i18n::tr!("pilotes.unifi.cle_refusee", host = config.host, instance = ctx.instance());
                        return park(ctx, reason).await;
                    }
                    return Err(e);
                }
                if let Some(s) = socket.as_mut()
                    && s.ping().await.is_err()
                {
                    socket = None;
                }
            }
            _ = hold.tick() => {
                let now = tokio::time::Instant::now();
                moving.retain(|camera, until| {
                    let keep = *until > now;
                    if !keep {
                        ctx.set_state(&ctx.device_id(camera), "motion", Value::Bool(false));
                    }
                    keep
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A real console's camera list (addresses made up).
    const CAMERAS: &str = include_str!("../../../integrations/unifi/fixtures/cameras.json");

    #[test]
    fn the_cameras_of_a_real_console_are_read() {
        let cameras = parse_cameras(CAMERAS.as_bytes()).unwrap();
        assert_eq!(cameras.len(), 3);
        assert_eq!(cameras[0].name, "Caméra extérieure");
        assert_eq!(cameras[0].model, "UVC G3 Pro");
        assert!(cameras.iter().all(Camera::online));
    }

    #[test]
    fn a_disconnected_camera_is_offline() {
        let body = r#"[{"id":"abc123","name":"Porche","type":"G4","state":"DISCONNECTED"}]"#;
        assert!(!parse_cameras(body.as_bytes()).unwrap()[0].online());
    }

    #[test]
    fn an_id_that_is_not_a_path_segment_is_dropped() {
        let body = r#"[{"id":"../x","name":"a","type":"","state":"CONNECTED"},{"id":"ok1","name":"b","type":"","state":"CONNECTED"}]"#;
        let cameras = parse_cameras(body.as_bytes()).unwrap();
        assert_eq!(cameras.len(), 1);
        assert_eq!(cameras[0].id, "ok1");
    }

    #[test]
    fn motion_starts_and_ends() {
        let start = r#"{"type":"add","item":{"id":"e1","modelKey":"event","type":"motion","start":1700000000000,"device":"cam1"}}"#;
        assert_eq!(parse_event(start), Some(Motion::Start("cam1".into())));
        let end = r#"{"type":"update","item":{"id":"e1","modelKey":"event","type":"motion","start":1700000000000,"end":1700000009000,"device":"cam1"}}"#;
        assert_eq!(parse_event(end), Some(Motion::End("cam1".into())));
        let gone = r#"{"type":"remove","item":{"id":"e1","modelKey":"event","type":"motion","device":"cam1"}}"#;
        assert_eq!(parse_event(gone), Some(Motion::End("cam1".into())));
    }

    #[test]
    fn other_messages_are_not_motion() {
        for text in [
            "not json",
            r#"{"type":"update","item":{"modelKey":"camera","id":"cam1","state":"CONNECTED"}}"#,
            r#"{"type":"add","item":{"modelKey":"event","type":"ring","device":"cam1"}}"#,
            r#"{"type":"add","item":{"modelKey":"event","type":"motion"}}"#,
        ] {
            assert_eq!(parse_event(text), None, "{text}");
        }
    }

    #[test]
    fn the_quality_follows_the_height_asked() {
        let all: HashMap<String, String> = ["high", "medium", "low"]
            .into_iter()
            .map(|q| (q.to_owned(), format!("rtsps://h:7441/{q}")))
            .collect();
        assert_eq!(pick_quality(1080, &all), Some("high"));
        assert_eq!(pick_quality(720, &all), Some("medium"));
        assert_eq!(pick_quality(270, &all), Some("low"));
        assert_eq!(pick_quality(2000, &all), Some("high"));
        let some: HashMap<String, String> = [("medium".to_owned(), String::new())].into();
        assert_eq!(pick_quality(1080, &some), Some("medium"));
        assert_eq!(pick_quality(1, &HashMap::new()), None);
    }

    #[test]
    fn the_published_streams_are_read_and_aimed_at_the_console_address() {
        let body = br#"{"high":"rtsps://203.0.113.5:7441/AbC123?enableSrtp","medium":null,"low":"rtsps://203.0.113.5:7441/Def456?enableSrtp"}"#;
        let streams = parse_streams(body);
        assert_eq!(streams.len(), 2);
        let url = stream_url(&streams["high"], "192.168.1.2").unwrap();
        assert_eq!(url.text(), "rtsps://192.168.1.2:7441/AbC123");
        assert!(parse_streams(b"[]").is_empty());
    }

    #[test]
    fn the_talkback_session_and_its_address_are_read() {
        let body = br#"{"url":"rtp://192.168.1.50:7004","codec":"opus","samplingRate":24000,"bitsPerSample":16}"#;
        let session: TalkSession = serde_json::from_slice(body).unwrap();
        assert_eq!(
            (
                session.codec.as_str(),
                session.sampling_rate,
                session.bits_per_sample
            ),
            ("opus", 24000, 16)
        );
        assert_eq!(
            talk_target(&session.url).as_deref(),
            Some("192.168.1.50:7004")
        );
        assert_eq!(
            talk_target("udp://cam:7004/x?y").as_deref(),
            Some("cam:7004")
        );
        assert_eq!(talk_target("rtp://nope"), None);
    }

    #[test]
    fn the_flag_of_a_speaker_comes_from_the_console() {
        let cameras = parse_cameras(CAMERAS.as_bytes()).unwrap();
        let speakers: Vec<&str> = cameras
            .iter()
            .filter(|c| c.flags.has_speaker)
            .map(|c| c.name.as_str())
            .collect();
        assert_eq!(speakers, ["Caméra Salon"]);
    }

    #[tokio::test]
    #[allow(clippy::cast_possible_truncation)]
    async fn the_voice_goes_out_as_opus_rtp_in_steps_of_20_ms() {
        let camera = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let target = camera.local_addr().unwrap().to_string();
        let mut stream = TalkStream::connect(&target, 24_000).await.unwrap();
        // 70 ms of a tone: three whole packets, 10 ms kept for the next.
        let tone: Vec<i16> = (0..1_680)
            .map(|n| (8_000.0 * (f64::from(n) * 0.26).sin()) as i16)
            .collect();
        stream.send(&tone).await.unwrap();
        let mut buf = [0u8; 1500];
        let mut seen = Vec::new();
        for _ in 0..3 {
            let n = camera.recv(&mut buf).await.unwrap();
            seen.push(buf[..n].to_vec());
        }
        assert!(
            seen.iter()
                .all(|p| p[0] == 0x80 && p[1] == 96 && p.len() > 12)
        );
        let seq = |p: &Vec<u8>| u16::from_be_bytes([p[2], p[3]]);
        let ts = |p: &Vec<u8>| u32::from_be_bytes([p[4], p[5], p[6], p[7]]);
        assert_eq!(seq(&seen[1]), seq(&seen[0]).wrapping_add(1));
        assert_eq!(ts(&seen[2]), ts(&seen[0]).wrapping_add(1920));
        assert_eq!(&seen[0][8..12], &seen[2][8..12]);
        // And it is Opus: a decoder takes the payload of 20 ms at 24 kHz.
        let mut decoder = opus_rs::OpusDecoder::new(24_000, 1).unwrap();
        let mut pcm = vec![0f32; 480];
        assert_eq!(decoder.decode(&seen[0][12..], 480, &mut pcm).unwrap(), 480);
        assert!(stream.send(&[0; 100]).await.is_ok());
        assert!(TalkStream::connect(&target, 22_050).await.is_err());
    }

    #[test]
    fn unknown_options_are_refused() {
        let bad: Result<Config, _> = toml_like(r#"{"host":"h","nope":1}"#);
        assert!(bad.is_err());
        let good = toml_like(r#"{"host":"h"}"#).unwrap();
        assert_eq!(good.port, 443);
    }

    fn toml_like(json: &str) -> Result<Config, serde_json::Error> {
        serde_json::from_str(json)
    }
}
