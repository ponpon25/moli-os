//! One RTSP client: `OPTIONS`, `DESCRIBE`, `SETUP`, `PLAY`, `TEARDOWN`,
//! then the video interleaved on the same TCP connection (RTP over RTSP,
//! RFC 2326 §10.12). No UDP: one connection to watch and to close, and what
//! recorders and players accept.

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
}

impl Shared {
    /// The password, read from the store at each request (one stored by
    /// this process counts at once; one set from the command line, after a
    /// restart). None: nobody gets in.
    fn password(&self) -> Option<String> {
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

fn base64(bytes: &[u8]) -> String {
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

/// The interleaved channels the client chose (`interleaved=0-1`).
fn interleaved(transport: &str) -> Option<u8> {
    let first = transport
        .split(',')
        .next()?
        .split(';')
        .map(str::trim)
        .collect::<Vec<_>>();
    if !first.iter().any(|p| p.eq_ignore_ascii_case("RTP/AVP/TCP")) {
        return None;
    }
    Some(
        first
            .iter()
            .find_map(|p| p.strip_prefix("interleaved="))
            .and_then(|r| r.split('-').next())
            .and_then(|c| c.parse().ok())
            .unwrap_or(0),
    )
}

/// What one connection has set up so far.
struct State {
    nonce: String,
    failures: u32,
    /// The camera described, and its stream, opened for `DESCRIBE`.
    described: Option<(String, DeviceId, Option<Pump>)>,
    session: Option<String>,
    channel: u8,
    playing: Option<(DeviceId, mpsc::Receiver<VideoFrame>, Packetizer)>,
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
        nonce: digest::nonce(),
        failures: 0,
        described: None,
        session: None,
        channel: 0,
        playing: None,
    };
    let mut check = tokio::time::interval(CHECK_EVERY);
    let result: anyhow::Result<()> = async {
        loop {
            let playing = state.playing.is_some();
            tokio::select! {
                () = cancel.cancelled() => return Ok(()),
                // Before playing, a silent client is let go; while playing,
                // the video itself keeps the connection.
                request = tokio::time::timeout(if playing { NEVER } else { IDLE }, incoming.recv()) => {
                    let Ok(Some(request)) = request else { return Ok(()) };
                    if !answer(&mut write_half, &request, &mut state, &shared, &local, peer).await? {
                        return Ok(());
                    }
                }
                frame = next_frame(&mut state.playing) => {
                    let Some(frame) = frame else {
                        anyhow::bail!("the camera's video ended");
                    };
                    let Some((_, _, packetizer)) = state.playing.as_mut() else { continue };
                    let mut bytes = Vec::new();
                    for packet in packetizer.frame(frame.timestamp, &frame.nals) {
                        bytes.push(b'$');
                        bytes.push(state.channel);
                        #[allow(clippy::cast_possible_truncation)] // ≤ MTU
                        bytes.extend_from_slice(&(packet.len() as u16).to_be_bytes());
                        bytes.extend_from_slice(&packet);
                    }
                    write(&mut write_half, &bytes).await?;
                }
                _ = check.tick(), if playing => {
                    let Some((device, _, _)) = &state.playing else { continue };
                    if !shared.cameras.values().any(|d| d == device) {
                        anyhow::bail!("the camera is no longer served");
                    }
                    if shared.hub.protected_room(device).is_some() {
                        anyhow::bail!("the camera is now in a protected room");
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

async fn next_frame(
    playing: &mut Option<(DeviceId, mpsc::Receiver<VideoFrame>, Packetizer)>,
) -> Option<VideoFrame> {
    match playing {
        Some((_, rx, _)) => rx.recv().await,
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
    let proved = request
        .header("authorization")
        .zip(password.as_deref())
        .is_some_and(|(h, password)| {
            digest::verify(h, &request.method, &state.nonce, &shared.user, password)
        });
    if !proved {
        if request.header("authorization").is_some() {
            state.failures += 1;
            tracing::warn!(%peer, "rtsp: wrong password");
            // Slows down guessing.
            tokio::time::sleep(Duration::from_secs(1)).await;
            if state.failures >= MAX_FAILURES {
                return Next::Close(Reply::new(401, "Unauthorized"));
            }
        }
        return Next::Reply(
            Reply::new(401, "Unauthorized")
                .header("WWW-Authenticate", digest::challenge(&state.nonce)),
        );
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
            let body = sdp(pump.info(), local, random_u32());
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
            let Some(channel) = request.header("transport").and_then(interleaved) else {
                return Next::Reply(Reply::new(461, "Unsupported Transport"));
            };
            if channel > 254 {
                return Next::Reply(Reply::new(461, "Unsupported Transport"));
            }
            state.channel = channel;
            if state.session.is_none() {
                state.session = Some(format!("{:08X}", random_u32()));
            }
            Next::Reply(
                Reply::ok()
                    .header(
                        "Transport",
                        format!(
                            "RTP/AVP/TCP;unicast;interleaved={}-{}",
                            channel,
                            channel + 1
                        ),
                    )
                    .header(
                        "Session",
                        session_header(state.session.as_ref()).unwrap_or_default(),
                    ),
            )
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
            let Some(pump) = pump.take() else {
                return Next::Reply(Reply::new(455, "Method Not Valid in This State"));
            };
            let device = device.clone();
            let (seq, start, ssrc) = (random_u32().to_be_bytes(), random_u32(), random_u32());
            let seq = u16::from_be_bytes([seq[0], seq[1]]);
            let frames = feed::spawn(shared.hub.clone(), device.clone(), pump, start);
            let packetizer = Packetizer::new(PAYLOAD_TYPE, ssrc, seq, MTU);
            state.playing = Some((device.clone(), frames, packetizer));
            let base = request.uri.trim_end_matches('/');
            tracing::info!(%peer, device = %device, "rtsp: playing");
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
    fn the_transport_must_be_interleaved() {
        assert_eq!(interleaved("RTP/AVP/TCP;unicast;interleaved=0-1"), Some(0));
        assert_eq!(interleaved("RTP/AVP/TCP;unicast;interleaved=2-3"), Some(2));
        assert_eq!(interleaved("RTP/AVP/TCP;unicast"), Some(0));
        assert_eq!(interleaved("RTP/AVP;unicast;client_port=5000-5001"), None);
        assert_eq!(
            interleaved("RTP/AVP;unicast;client_port=5000-5001,RTP/AVP/TCP;interleaved=0-1"),
            None
        );
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
