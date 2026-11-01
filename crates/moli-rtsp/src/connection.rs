//! One RTSP client: `OPTIONS`, `DESCRIBE`, `SETUP`, `PLAY`, `TEARDOWN`,
//! then the video, as the client asks:
//! - interleaved on the same TCP connection (RTP over RTSP, RFC 2326
//!   §10.12), preferred: one connection to watch and to close;
//! - or in UDP to the client's ports (unicast only, to the address of the
//!   RTSP connection, never another one). The client must then show it is
//!   still there (a request, or its receiver reports) within twice the
//!   session timeout.
//!
//! An RTCP sender report goes out every five seconds (the camera's time
//! against the wall clock).

// `rtp` and `rtcp`, as the RFCs name them.
#![allow(clippy::similar_names)]
use std::collections::HashMap;
use std::fmt::Write as _;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use moli_core::{DeviceId, InstanceId};
use moli_net::h264::Packetizer;
use moli_runtime::Hub;
use moli_runtime::media::VideoFrame;
use ring::rand::{SecureRandom as _, SystemRandom};
use tokio::io::{AsyncWrite, AsyncWriteExt as _, BufReader};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::digest;
use crate::feed::{self, Pump};
use crate::request::{self, Incoming, Request, camera_path};

/// A client that says nothing for this long before playing is let go.
const IDLE: Duration = Duration::from_secs(60);
/// No idle limit (a year).
const NEVER: Duration = Duration::from_secs(365 * 24 * 3600);
/// The session timeout announced (keep-alives come before it).
const SESSION_TIMEOUT: u64 = 60;
/// How often a sender report goes out.
const REPORT_EVERY: Duration = Duration::from_secs(5);
/// A UDP client that says nothing for this long is gone.
const UDP_SILENCE: Duration = Duration::from_secs(2 * SESSION_TIMEOUT);
/// A client that cannot take a frame for this long is gone.
const WRITE: Duration = Duration::from_secs(10);
/// Wrong passwords before the connection is closed.
const MAX_FAILURES: u32 = 3;
/// How often a played camera is checked (still served, not in a protected room).
const CHECK_EVERY: Duration = Duration::from_secs(5);
const PAYLOAD_TYPE: u8 = 96;
/// Packets of at most 1400 bytes of payload (what the packetizer leaves
/// from this MTU), as on a network.
const MTU: usize = 1450;

/// What every connection shares.
#[derive(Debug)]
pub struct Shared {
    pub hub: Hub,
    /// Path → camera.
    pub cameras: HashMap<String, DeviceId>,
    pub user: String,
    pub nonces: std::sync::Mutex<digest::Nonces>,
    /// The RTSP port (for the addresses ONVIF gives).
    pub port: u16,
    /// What each camera's stream was like when last opened: width, height,
    /// H.264 profile (`profile_idc`). ONVIF describes it.
    pub seen: std::sync::Mutex<HashMap<DeviceId, (u32, u32, u8)>>,
}

impl Shared {
    /// The password, read from the store at each request (one stored by
    /// this process counts at once; one set from the command line, after a
    /// restart). None: nobody gets in.
    pub(crate) fn password(&self) -> Option<String> {
        self.hub
            .secret(&InstanceId::from(crate::NAMESPACE), crate::SECRET_PASSWORD)
            .filter(|p| !p.is_empty())
    }
}

fn random_u32() -> u32 {
    let mut b = [0u8; 4];
    SystemRandom::new()
        .fill(&mut b)
        .expect("the system's random source");
    u32::from_be_bytes(b)
}

pub(crate) fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
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

/// The session description of a camera's video.
#[must_use]
pub fn sdp(info: &moli_runtime::media::LiveInfo, local: &SocketAddr, session: u32) -> String {
    let (family, address) = match local {
        SocketAddr::V4(a) => ("IP4", a.ip().to_string()),
        SocketAddr::V6(a) => ("IP6", a.ip().to_string()),
    };
    let mut s = format!(
        "v=0\r\no=- {session} 1 IN {family} {address}\r\ns=Moli\r\nc=IN {family} {address}\r\nt=0 0\r\na=control:*\r\na=range:npt=0-\r\nm=video 0 RTP/AVP {PAYLOAD_TYPE}\r\na=rtpmap:{PAYLOAD_TYPE} H264/90000\r\n"
    );
    let _ = write!(s, "a=fmtp:{PAYLOAD_TYPE} packetization-mode=1");
    if info.sps.len() >= 4 && !info.pps.is_empty() {
        let _ = write!(
            s,
            ";profile-level-id={:02X}{:02X}{:02X};sprop-parameter-sets={},{}",
            info.sps[1],
            info.sps[2],
            info.sps[3],
            base64(&info.sps),
            base64(&info.pps)
        );
    }
    s.push_str("\r\n");
    if info.width > 0 && info.height > 0 {
        let _ = write!(s, "a=x-dimensions:{},{}\r\n", info.width, info.height);
    }
    s.push_str("a=control:track1\r\n");
    s
}

struct Reply {
    code: u16,
    reason: &'static str,
    headers: Vec<(&'static str, String)>,
    body: Option<(&'static str, String)>,
}

impl Reply {
    fn new(code: u16, reason: &'static str) -> Self {
        Self {
            code,
            reason,
            headers: Vec::new(),
            body: None,
        }
    }

    fn ok() -> Self {
        Self::new(200, "OK")
    }

    fn header(mut self, name: &'static str, value: impl Into<String>) -> Self {
        self.headers.push((name, value.into()));
        self
    }

    fn render(&self, cseq: &str) -> String {
        let mut s = format!(
            "RTSP/1.0 {} {}\r\nCSeq: {cseq}\r\nServer: Moli\r\n",
            self.code, self.reason
        );
        for (name, value) in &self.headers {
            let _ = write!(s, "{name}: {value}\r\n");
        }
        match &self.body {
            Some((kind, body)) => {
                let _ = write!(
                    s,
                    "Content-Type: {kind}\r\nContent-Length: {}\r\n\r\n{body}",
                    body.len()
                );
            }
            None => s.push_str("\r\n"),
        }
        s
    }
}

async fn write<W: AsyncWrite + Unpin>(w: &mut W, bytes: &[u8]) -> anyhow::Result<()> {
    tokio::time::timeout(WRITE, w.write_all(bytes))
        .await
        .map_err(|_| anyhow::anyhow!("the client does not read"))??;
    Ok(())
}

/// The transport a client asks for in `SETUP`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Asked {
    /// Interleaved, on these channels (RTP, then RTCP).
    Tcp(u8),
    /// UDP to these ports of the client (RTP, RTCP).
    Udp(u16, u16),
}

/// The first transport of the client's list that Moli serves.
pub fn asked(transport: &str) -> Option<Asked> {
    for option in transport.split(',') {
        let parts: Vec<&str> = option.split(';').map(str::trim).collect();
        let Some(profile) = parts.first() else {
            continue;
        };
        if profile.eq_ignore_ascii_case("RTP/AVP/TCP") {
            let channel = parts
                .iter()
                .find_map(|p| p.strip_prefix("interleaved="))
                .and_then(|r| r.split('-').next())
                .and_then(|c| c.parse::<u8>().ok())
                .unwrap_or(0);
            if channel < 255 {
                return Some(Asked::Tcp(channel));
            }
        } else if profile.eq_ignore_ascii_case("RTP/AVP")
            || profile.eq_ignore_ascii_case("RTP/AVP/UDP")
        {
            if parts.iter().any(|p| p.eq_ignore_ascii_case("multicast")) {
                continue;
            }
            let ports = parts.iter().find_map(|p| p.strip_prefix("client_port="));
            let Some((rtp, rtcp)) = ports.and_then(|r| {
                let mut it = r.split('-');
                let rtp: u16 = it.next()?.parse().ok()?;
                let rtcp = it
                    .next()
                    .and_then(|c| c.parse().ok())
                    .unwrap_or(rtp.wrapping_add(1));
                Some((rtp, rtcp))
            }) else {
                continue;
            };
            if rtp >= 1024 && rtcp >= 1024 {
                return Some(Asked::Udp(rtp, rtcp));
            }
        }
    }
    None
}

/// Where the packets of a playing client go.
#[derive(Debug)]
enum Transport {
    Tcp(u8),
    Udp {
        rtp: Arc<tokio::net::UdpSocket>,
        rtcp: Arc<tokio::net::UdpSocket>,
        to_rtp: SocketAddr,
        to_rtcp: SocketAddr,
    },
}

/// A client being sent the video.
struct Playing {
    device: DeviceId,
    frames: mpsc::Receiver<VideoFrame>,
    packetizer: Packetizer,
    transport: Transport,
    ssrc: u32,
    packets: u32,
    octets: u32,
    last_timestamp: u32,
    /// The last sign of life of a UDP client.
    heard: tokio::time::Instant,
}

impl Playing {
    /// The packets of one frame, as they go on the wire.
    async fn send<W: AsyncWrite + Unpin>(
        &mut self,
        w: &mut W,
        frame: &VideoFrame,
    ) -> anyhow::Result<()> {
        self.last_timestamp = frame.timestamp;
        let packets = self.packetizer.frame(frame.timestamp, &frame.nals);
        for p in &packets {
            self.packets = self.packets.wrapping_add(1);
            #[allow(clippy::cast_possible_truncation)] // ≤ MTU
            let len = p.len() as u32;
            self.octets = self.octets.wrapping_add(len.saturating_sub(12));
        }
        match &self.transport {
            Transport::Tcp(channel) => {
                let mut bytes = Vec::new();
                for p in &packets {
                    interleave(&mut bytes, *channel, p);
                }
                write(w, &bytes).await
            }
            Transport::Udp { rtp, to_rtp, .. } => {
                for p in &packets {
                    rtp.send_to(p, *to_rtp).await?;
                }
                Ok(())
            }
        }
    }

    async fn report<W: AsyncWrite + Unpin>(&self, w: &mut W) -> anyhow::Result<()> {
        let sr = sender_report(
            self.ssrc,
            ntp_now(),
            self.last_timestamp,
            self.packets,
            self.octets,
        );
        match &self.transport {
            Transport::Tcp(channel) => {
                let mut bytes = Vec::new();
                interleave(&mut bytes, channel + 1, &sr);
                write(w, &bytes).await
            }
            Transport::Udp { rtcp, to_rtcp, .. } => {
                rtcp.send_to(&sr, *to_rtcp).await?;
                Ok(())
            }
        }
    }
}

fn interleave(out: &mut Vec<u8>, channel: u8, packet: &[u8]) {
    out.push(b'$');
    out.push(channel);
    #[allow(clippy::cast_possible_truncation)] // ≤ MTU
    out.extend_from_slice(&(packet.len() as u16).to_be_bytes());
    out.extend_from_slice(packet);
}

/// An RTCP sender report, no report blocks (RFC 3550 §6.4.1).
fn sender_report(ssrc: u32, ntp: u64, rtp_timestamp: u32, packets: u32, octets: u32) -> Vec<u8> {
    let mut p = vec![0x80, 200, 0, 6];
    p.extend_from_slice(&ssrc.to_be_bytes());
    p.extend_from_slice(&ntp.to_be_bytes());
    p.extend_from_slice(&rtp_timestamp.to_be_bytes());
    p.extend_from_slice(&packets.to_be_bytes());
    p.extend_from_slice(&octets.to_be_bytes());
    p
}

/// The wall clock as NTP time (seconds since 1900 in the high half).
fn ntp_now() -> u64 {
    let since_unix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let seconds = since_unix.as_secs() + 2_208_988_800;
    let fraction = (u64::from(since_unix.subsec_nanos()) << 32) / 1_000_000_000;
    (seconds << 32) | fraction
}

/// The sockets a UDP client may speak on.
fn udp_sockets(
    playing: Option<&Playing>,
) -> Option<(Arc<tokio::net::UdpSocket>, Arc<tokio::net::UdpSocket>)> {
    match &playing?.transport {
        Transport::Udp { rtp, rtcp, .. } => Some((Arc::clone(rtp), Arc::clone(rtcp))),
        Transport::Tcp(_) => None,
    }
}

/// A UDP socket's next datagram (a receiver report), or never.
async fn heard_on(sockets: Option<(Arc<tokio::net::UdpSocket>, Arc<tokio::net::UdpSocket>)>) {
    let (mut a, mut b) = ([0u8; 1500], [0u8; 1500]);
    match sockets {
        Some((rtp, rtcp)) => {
            tokio::select! {
                _ = rtp.recv_from(&mut a) => {}
                _ = rtcp.recv_from(&mut b) => {}
            }
        }
        None => std::future::pending().await,
    }
}

/// What one connection has set up so far.
struct State {
    failures: u32,
    /// The camera described, and its stream, opened for `DESCRIBE`.
    described: Option<(String, DeviceId, Option<Pump>)>,
    session: Option<String>,
    /// The transport set up, before `PLAY`.
    transport: Option<Transport>,
    playing: Option<Playing>,
}

enum Next {
    Reply(Reply),
    /// Reply, then close.
    Close(Reply),
}

/// Serves one client until it leaves, the camera stops, or Moli does.
pub async fn serve(
    stream: TcpStream,
    peer: SocketAddr,
    shared: Arc<Shared>,
    cancel: CancellationToken,
) {
    let Ok(local) = stream.local_addr() else {
        return;
    };
    let _ = stream.set_nodelay(true);
    let (read_half, mut write_half) = stream.into_split();
    let (tx, mut incoming) = mpsc::channel::<Request>(4);
    let reader = tokio::spawn(async move {
        let mut reader = BufReader::new(read_half);
        loop {
            match request::read(&mut reader).await {
                Ok(Incoming::Interleaved) => {}
                Ok(Incoming::Request(r)) => {
                    if tx.send(r).await.is_err() {
                        return;
                    }
                }
                Err(e) => {
                    tracing::debug!(error = %e, "rtsp: the client's side ends");
                    return;
                }
            }
        }
    });
    let mut state = State {
        failures: 0,
        described: None,
        session: None,
        transport: None,
        playing: None,
    };
    let mut check = tokio::time::interval(CHECK_EVERY);
    let mut report = tokio::time::interval(REPORT_EVERY);
    let result: anyhow::Result<()> = async {
        loop {
            let playing = state.playing.is_some();
            let udp = udp_sockets(state.playing.as_ref());
            tokio::select! {
                () = cancel.cancelled() => return Ok(()),
                // Before playing, a silent client is let go; while playing,
                // the video itself keeps the connection.
                request = tokio::time::timeout(if playing { NEVER } else { IDLE }, incoming.recv()) => {
                    let Ok(Some(request)) = request else { return Ok(()) };
                    if let Some(p) = state.playing.as_mut() {
                        p.heard = tokio::time::Instant::now();
                    }
                    if !answer(&mut write_half, &request, &mut state, &shared, &local, peer).await? {
                        return Ok(());
                    }
                }
                frame = next_frame(&mut state.playing) => {
                    let Some(frame) = frame else {
                        anyhow::bail!("the camera's video ended");
                    };
                    let Some(playing) = state.playing.as_mut() else { continue };
                    playing.send(&mut write_half, &frame).await?;
                }
                () = heard_on(udp) => {
                    if let Some(p) = state.playing.as_mut() {
                        p.heard = tokio::time::Instant::now();
                    }
                }
                _ = report.tick(), if playing => {
                    let Some(playing) = state.playing.as_ref() else { continue };
                    playing.report(&mut write_half).await?;
                }
                _ = check.tick(), if playing => {
                    let Some(playing) = &state.playing else { continue };
                    if !shared.cameras.values().any(|d| *d == playing.device) {
                        anyhow::bail!("the camera is no longer served");
                    }
                    if shared.hub.protected_room(&playing.device).is_some() {
                        anyhow::bail!("the camera is now in a protected room");
                    }
                    if matches!(playing.transport, Transport::Udp { .. }) && playing.heard.elapsed() > UDP_SILENCE {
                        anyhow::bail!("the UDP client went silent");
                    }
                }
            }
        }
    }
    .await;
    reader.abort();
    match result {
        Ok(()) => tracing::info!(%peer, "rtsp: client gone"),
        Err(e) => tracing::info!(%peer, reason = %format!("{e:#}"), "rtsp: client let go"),
    }
}

async fn next_frame(playing: &mut Option<Playing>) -> Option<VideoFrame> {
    match playing {
        Some(p) => p.frames.recv().await,
        None => std::future::pending().await,
    }
}

/// Answers one request; `false`: the connection is to be closed.
async fn answer<W: AsyncWrite + Unpin>(
    w: &mut W,
    request: &Request,
    state: &mut State,
    shared: &Shared,
    local: &SocketAddr,
    peer: SocketAddr,
) -> anyhow::Result<bool> {
    let next = handle(request, state, shared, local, peer).await;
    let (reply, keep) = match next {
        Next::Reply(r) => (r, true),
        Next::Close(r) => (r, false),
    };
    write(w, reply.render(request.cseq()).as_bytes()).await?;
    Ok(keep)
}

fn session_header(session: Option<&String>) -> Option<String> {
    session.map(|s| format!("{s};timeout={SESSION_TIMEOUT}"))
}

#[allow(clippy::too_many_lines)] // one arm per method
async fn handle(
    request: &Request,
    state: &mut State,
    shared: &Shared,
    local: &SocketAddr,
    peer: SocketAddr,
) -> Next {
    const PUBLIC: &str = "OPTIONS, DESCRIBE, SETUP, PLAY, TEARDOWN, GET_PARAMETER, SET_PARAMETER";
    if request.method == "OPTIONS" {
        return Next::Reply(Reply::ok().header("Public", PUBLIC));
    }
    // Everything else needs the password.
    let password = shared.password();
    if password.is_none() {
        tracing::warn!(%peer, "rtsp: no password set (moli-os secrets set rtsp password, then restart): nobody is let in");
    }
    let (nonce, accepted) = shared.nonces.lock().expect("nonces lock").now();
    let check = match (request.header("authorization"), password.as_deref()) {
        (Some(h), Some(password)) => {
            digest::check(h, &request.method, &accepted, &shared.user, password)
        }
        (Some(_), None) => digest::Check::Wrong,
        (None, _) => digest::Check::Stale, // not an attempt: just asked
    };
    match check {
        digest::Check::Proved => {}
        digest::Check::Stale => {
            return Next::Reply(Reply::new(401, "Unauthorized").header(
                "WWW-Authenticate",
                digest::challenge(&nonce, request.header("authorization").is_some()),
            ));
        }
        digest::Check::Wrong => {
            state.failures += 1;
            tracing::warn!(%peer, "rtsp: wrong password");
            // Slows down guessing.
            tokio::time::sleep(Duration::from_secs(1)).await;
            if state.failures >= MAX_FAILURES {
                return Next::Close(Reply::new(401, "Unauthorized"));
            }
            return Next::Reply(
                Reply::new(401, "Unauthorized")
                    .header("WWW-Authenticate", digest::challenge(&nonce, false)),
            );
        }
    }
    match request.method.as_str() {
        "DESCRIBE" => {
            if state.playing.is_some() {
                return Next::Reply(Reply::new(455, "Method Not Valid in This State"));
            }
            let Some(path) = camera_path(&request.uri) else {
                return Next::Reply(Reply::new(404, "Not Found"));
            };
            let Some(device) = shared.cameras.get(path) else {
                return Next::Reply(Reply::new(404, "Not Found"));
            };
            if shared.hub.protected_room(device).is_some() {
                tracing::warn!(%peer, device = %device, "rtsp: a camera of a protected room is never served");
                return Next::Reply(Reply::new(403, "Forbidden"));
            }
            // A second DESCRIBE of the same camera keeps the stream already open.
            let reuse =
                matches!(&state.described, Some((p, d, Some(_))) if p == path && d == device);
            if !reuse {
                state.described = None;
                match feed::open(&shared.hub, device).await {
                    Ok(pump) => {
                        state.described = Some((path.to_owned(), device.clone(), Some(pump)));
                    }
                    Err(e) => {
                        tracing::warn!(device = %device, error = %e, "rtsp: the camera did not open");
                        return Next::Reply(Reply::new(503, "Service Unavailable"));
                    }
                }
            }
            let Some((_, _, Some(pump))) = &state.described else {
                return Next::Reply(Reply::new(503, "Service Unavailable"));
            };
            let info = pump.info();
            if info.width > 0 && info.sps.len() > 1 {
                shared
                    .seen
                    .lock()
                    .expect("seen lock")
                    .insert(device.clone(), (info.width, info.height, info.sps[1]));
            }
            let body = sdp(info, local, random_u32());
            let base = if request.uri.ends_with('/') {
                request.uri.clone()
            } else {
                format!("{}/", request.uri)
            };
            tracing::info!(%peer, device = %device, "rtsp: camera described");
            let mut reply = Reply::ok().header("Content-Base", base);
            reply.body = Some(("application/sdp", body));
            Next::Reply(reply)
        }
        "SETUP" => {
            let Some((path, _, Some(_))) = &state.described else {
                return Next::Reply(Reply::new(455, "Method Not Valid in This State"));
            };
            if camera_path(&request.uri) != Some(path.as_str()) || state.playing.is_some() {
                return Next::Reply(Reply::new(455, "Method Not Valid in This State"));
            }
            let Some(asked) = request.header("transport").and_then(asked) else {
                return Next::Reply(Reply::new(461, "Unsupported Transport"));
            };
            if state.session.is_none() {
                state.session = Some(format!("{:08X}", random_u32()));
            }
            let (transport, answer) = match asked {
                Asked::Tcp(channel) => (
                    Transport::Tcp(channel),
                    format!(
                        "RTP/AVP/TCP;unicast;interleaved={}-{}",
                        channel,
                        channel + 1
                    ),
                ),
                Asked::Udp(rtp_port, rtcp_port) => {
                    let bind = SocketAddr::new(local.ip(), 0);
                    let (Ok(rtp), Ok(rtcp)) = (
                        tokio::net::UdpSocket::bind(bind).await,
                        tokio::net::UdpSocket::bind(bind).await,
                    ) else {
                        return Next::Reply(Reply::new(500, "Internal Server Error"));
                    };
                    let (Ok(a), Ok(b)) = (rtp.local_addr(), rtcp.local_addr()) else {
                        return Next::Reply(Reply::new(500, "Internal Server Error"));
                    };
                    let answer = format!(
                        "RTP/AVP;unicast;client_port={rtp_port}-{rtcp_port};server_port={}-{}",
                        a.port(),
                        b.port()
                    );
                    (
                        Transport::Udp {
                            rtp: Arc::new(rtp),
                            rtcp: Arc::new(rtcp),
                            to_rtp: SocketAddr::new(peer.ip(), rtp_port),
                            to_rtcp: SocketAddr::new(peer.ip(), rtcp_port),
                        },
                        answer,
                    )
                }
            };
            state.transport = Some(transport);
            Next::Reply(Reply::ok().header("Transport", answer).header(
                "Session",
                session_header(state.session.as_ref()).unwrap_or_default(),
            ))
        }
        "PLAY" => {
            if state.playing.is_some() {
                return Next::Reply(Reply::ok().header(
                    "Session",
                    session_header(state.session.as_ref()).unwrap_or_default(),
                ));
            }
            let (Some(_), Some((_, device, pump))) = (&state.session, &mut state.described) else {
                return Next::Reply(Reply::new(455, "Method Not Valid in This State"));
            };
            if pump.is_none() || state.transport.is_none() {
                return Next::Reply(Reply::new(455, "Method Not Valid in This State"));
            }
            let (Some(pump), Some(transport)) = (pump.take(), state.transport.take()) else {
                return Next::Reply(Reply::new(455, "Method Not Valid in This State"));
            };
            let device = device.clone();
            let (seq, start, ssrc) = (random_u32().to_be_bytes(), random_u32(), random_u32());
            let seq = u16::from_be_bytes([seq[0], seq[1]]);
            let frames = feed::spawn(shared.hub.clone(), device.clone(), pump, start);
            let packetizer = Packetizer::new(PAYLOAD_TYPE, ssrc, seq, MTU);
            let udp = matches!(transport, Transport::Udp { .. });
            state.playing = Some(Playing {
                device: device.clone(),
                frames,
                packetizer,
                transport,
                ssrc,
                packets: 0,
                octets: 0,
                last_timestamp: start,
                heard: tokio::time::Instant::now(),
            });
            let base = request.uri.trim_end_matches('/');
            tracing::info!(%peer, device = %device, udp, "rtsp: playing");
            Next::Reply(
                Reply::ok()
                    .header(
                        "Session",
                        session_header(state.session.as_ref()).unwrap_or_default(),
                    )
                    .header("Range", "npt=0.000-")
                    .header(
                        "RTP-Info",
                        format!("url={base}/track1;seq={seq};rtptime={start}"),
                    ),
            )
        }
        "GET_PARAMETER" | "SET_PARAMETER" => {
            let mut reply = Reply::ok();
            if let Some(s) = session_header(state.session.as_ref()) {
                reply = reply.header("Session", s);
            }
            Next::Reply(reply)
        }
        "TEARDOWN" => Next::Close(Reply::ok()),
        _ => Next::Reply(Reply::new(501, "Not Implemented").header("Public", PUBLIC)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_the_standard() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn the_first_transport_served_is_taken() {
        assert_eq!(
            asked("RTP/AVP/TCP;unicast;interleaved=0-1"),
            Some(Asked::Tcp(0))
        );
        assert_eq!(
            asked("RTP/AVP/TCP;unicast;interleaved=2-3"),
            Some(Asked::Tcp(2))
        );
        assert_eq!(asked("RTP/AVP/TCP;unicast"), Some(Asked::Tcp(0)));
        assert_eq!(
            asked("RTP/AVP;unicast;client_port=5000-5001"),
            Some(Asked::Udp(5000, 5001))
        );
        assert_eq!(
            asked("RTP/AVP/UDP;unicast;client_port=5000"),
            Some(Asked::Udp(5000, 5001))
        );
        assert_eq!(
            asked("RTP/AVP;multicast;port=5000-5001,RTP/AVP/TCP;interleaved=0-1"),
            Some(Asked::Tcp(0))
        );
        // Privileged ports are not targets.
        assert_eq!(asked("RTP/AVP;unicast;client_port=53-54"), None);
        assert_eq!(asked("RAW/RAW/UDP;unicast"), None);
    }

    #[test]
    fn a_sender_report_is_28_bytes() {
        let sr = sender_report(1, 2, 3, 4, 5);
        assert_eq!(sr.len(), 28);
        assert_eq!(&sr[..4], &[0x80, 200, 0, 6]);
    }

    #[test]
    fn the_description_carries_the_parameter_sets() {
        let info = moli_runtime::media::LiveInfo {
            width: 1280,
            height: 720,
            sps: vec![0x67, 0x64, 0x00, 0x1F, 0xAC],
            pps: vec![0x68, 0xEE, 0x3C, 0x80],
            audio_rate: None,
        };
        let local: SocketAddr = "192.168.1.2:8554".parse().unwrap();
        let s = sdp(&info, &local, 7);
        assert!(s.contains("m=video 0 RTP/AVP 96\r\n"));
        assert!(s.contains("a=rtpmap:96 H264/90000\r\n"));
        assert!(s.contains("profile-level-id=64001F;sprop-parameter-sets=Z2QAH6w=,aO48gA==\r\n"));
        assert!(s.contains("c=IN IP4 192.168.1.2\r\n"));
        // Without parameter sets: the camera sends them in the stream.
        let bare = sdp(&moli_runtime::media::LiveInfo::default(), &local, 7);
        assert!(bare.contains("a=fmtp:96 packetization-mode=1\r\n"));
    }
}
