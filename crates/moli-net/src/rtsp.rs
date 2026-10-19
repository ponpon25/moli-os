//! A small RTSP client: enough to read the H.264 video of a camera, over
//! plain TCP (`rtsp://`) or TLS (`rtsps://`), with the RTP packets
//! interleaved on the control connection (no UDP ports to open).
//! No authentication beyond what the URL carries (cameras that put a token in
//! the path).

use std::fmt::Write as _;
use std::time::Duration;

use anyhow::{Context as _, bail};
use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _};
use tokio::net::TcpStream;

use crate::PinnedTls;

/// A server silent for this long is gone.
const IDLE: Duration = Duration::from_secs(10);
/// A request unanswered for this long failed.
const ANSWER: Duration = Duration::from_secs(8);
/// How often the session is kept alive.
const KEEPALIVE: Duration = Duration::from_secs(20);
/// Largest control message or RTP packet accepted.
const MAX_MESSAGE: usize = 256 * 1024;

pub trait Io: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Io for T {}

/// `rtsp(s)://host[:port]/path?query`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Url {
    pub secure: bool,
    pub host: String,
    pub port: u16,
    /// Path and query, starting with `/`.
    pub path: String,
}

impl Url {
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let (secure, rest) = if let Some(rest) = text.strip_prefix("rtsps://") {
            (true, rest)
        } else {
            (false, text.strip_prefix("rtsp://")?)
        };
        let (authority, path) = match rest.find('/') {
            Some(i) => (&rest[..i], &rest[i..]),
            None => (rest, "/"),
        };
        let (host, port) = match authority.rsplit_once(':') {
            Some((host, port)) if !host.ends_with(']') || host.starts_with('[') => {
                (host, port.parse().ok()?)
            }
            _ => (authority, if secure { 322 } else { 554 }),
        };
        (!host.is_empty()).then(|| Self {
            secure,
            host: host.trim_matches(['[', ']']).to_owned(),
            port,
            path: path.to_owned(),
        })
    }

    /// The same URL aimed at another host (the address Moli knows the
    /// device by, not the one it announces).
    #[must_use]
    pub fn at(&self, host: &str) -> Self {
        Self {
            host: host.to_owned(),
            ..self.clone()
        }
    }

    #[must_use]
    pub fn text(&self) -> String {
        format!(
            "{}://{}:{}{}",
            if self.secure { "rtsps" } else { "rtsp" },
            self.host,
            self.port,
            self.path
        )
    }
}

/// The video track an SDP describes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct VideoTrack {
    pub payload_type: u8,
    pub control: String,
    pub sps: Option<Vec<u8>>,
    pub pps: Option<Vec<u8>>,
}

/// The first H.264 video section of an SDP.
#[must_use]
pub fn parse_sdp(sdp: &str) -> Option<VideoTrack> {
    let mut track: Option<VideoTrack> = None;
    let mut in_video = false;
    for line in sdp.lines().map(str::trim) {
        if let Some(media) = line.strip_prefix("m=") {
            in_video = track.is_none() && media.starts_with("video ");
            if in_video {
                let pt = media.split_whitespace().nth(3)?.parse().ok()?;
                track = Some(VideoTrack {
                    payload_type: pt,
                    ..VideoTrack::default()
                });
            }
        } else if in_video && let Some(t) = track.as_mut() {
            if let Some(control) = line.strip_prefix("a=control:") {
                control.clone_into(&mut t.control);
            } else if let Some(map) = line.strip_prefix("a=rtpmap:") {
                let (pt, codec) = map.split_once(' ')?;
                if pt.trim().parse() == Ok(t.payload_type)
                    && !codec.to_ascii_uppercase().starts_with("H264/")
                {
                    return None;
                }
            } else if let Some(fmtp) = line.strip_prefix("a=fmtp:") {
                for param in fmtp.split([';', ' ']) {
                    if let Some(sets) = param.trim().strip_prefix("sprop-parameter-sets=") {
                        let mut parts = sets.split(',').filter_map(base64_decode);
                        t.sps = parts.next();
                        t.pps = parts.next();
                    }
                }
            }
        }
    }
    track
}

fn base64_decode(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let (mut acc, mut bits) = (0u32, 0);
    for c in text.trim_end_matches('=').bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        };
        acc = (acc << 6) | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(u8::try_from(acc >> bits).ok()?);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

enum Message {
    Frame(u8, Vec<u8>),
    Response {
        status: u16,
        headers: Vec<(String, String)>,
        body: Vec<u8>,
    },
}

pub struct Client {
    io: Box<dyn Io>,
    buf: Vec<u8>,
    cseq: u32,
    session: Option<String>,
    base: String,
    keepalive: tokio::time::Interval,
}

impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Client").finish_non_exhaustive()
    }
}

impl Client {
    /// Connects (TLS pinned by `tls` for `rtsps://`).
    pub async fn connect(url: &Url, tls: Option<&PinnedTls>) -> anyhow::Result<Self> {
        let io: Box<dyn Io> = if url.secure {
            let tls = tls.context("rtsps needs pinned TLS settings")?;
            Box::new(tls.connect(&url.host, url.port).await?)
        } else {
            let tcp = tokio::time::timeout(
                crate::TIMEOUT,
                TcpStream::connect((url.host.as_str(), url.port)),
            )
            .await
            .context("connect timeout")??;
            let _ = tcp.set_nodelay(true);
            Box::new(tcp)
        };
        Ok(Self::over(io, url.text()))
    }

    /// Over any connection (tests).
    #[must_use]
    pub fn over(io: Box<dyn Io>, base: String) -> Self {
        let mut keepalive = tokio::time::interval(KEEPALIVE);
        keepalive.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        Self {
            io,
            buf: Vec::new(),
            cseq: 0,
            session: None,
            base,
            keepalive,
        }
    }

    /// OPTIONS, DESCRIBE, SETUP of the video track, PLAY.
    pub async fn play_video(&mut self) -> anyhow::Result<VideoTrack> {
        let url = self.base.clone();
        self.request("OPTIONS", &url, &[]).await?;
        let (_, headers, body) = self
            .request("DESCRIBE", &url, &[("Accept", "application/sdp")])
            .await?;
        let sdp = String::from_utf8_lossy(&body).into_owned();
        let track = parse_sdp(&sdp).context("no H.264 video in the stream")?;
        let base = header(&headers, "content-base").map_or_else(|| url.clone(), str::to_owned);
        let control = join(&base, &track.control);
        let (_, headers, _) = self
            .request(
                "SETUP",
                &control,
                &[("Transport", "RTP/AVP/TCP;unicast;interleaved=0-1")],
            )
            .await?;
        self.session = header(&headers, "session")
            .map(|s| s.split(';').next().unwrap_or_default().trim().to_owned());
        self.request("PLAY", &url, &[("Range", "npt=0.000-")])
            .await?;
        self.keepalive.reset();
        Ok(track)
    }

    /// The next RTP packet of the video (interleaved channel 0). Keeps the
    /// session alive meanwhile.
    pub async fn next_packet(&mut self) -> anyhow::Result<Vec<u8>> {
        loop {
            while let Some(message) = self.parse()? {
                if let Message::Frame(0, packet) = message {
                    return Ok(packet);
                }
            }
            let mut chunk = [0u8; 8192];
            tokio::select! {
                read = tokio::time::timeout(IDLE, self.io.read(&mut chunk)) => {
                    match read.context("the stream went silent")?? {
                        0 => bail!("the server closed the stream"),
                        n => self.buf.extend_from_slice(&chunk[..n]),
                    }
                }
                _ = self.keepalive.tick() => {
                    // The answer, if any, is skipped when read.
                    let message = self.message("OPTIONS", &self.base.clone(), &[]);
                    self.io.write_all(message.as_bytes()).await?;
                }
            }
        }
    }

    /// Says goodbye (best effort).
    pub async fn teardown(&mut self) {
        let url = self.base.clone();
        let message = self.message("TEARDOWN", &url, &[]);
        let _ = tokio::time::timeout(
            Duration::from_secs(1),
            self.io.write_all(message.as_bytes()),
        )
        .await;
    }

    fn message(&mut self, method: &str, url: &str, headers: &[(&str, &str)]) -> String {
        self.cseq += 1;
        let mut out = format!("{method} {url} RTSP/1.0\r\nCSeq: {}\r\n", self.cseq);
        if let Some(session) = &self.session {
            let _ = write!(out, "Session: {session}\r\n");
        }
        out.push_str("User-Agent: Moli\r\n");
        for (name, value) in headers {
            let _ = write!(out, "{name}: {value}\r\n");
        }
        out.push_str("\r\n");
        out
    }

    async fn request(
        &mut self,
        method: &str,
        url: &str,
        headers: &[(&str, &str)],
    ) -> anyhow::Result<(u16, Vec<(String, String)>, Vec<u8>)> {
        let message = self.message(method, url, headers);
        self.io.write_all(message.as_bytes()).await?;
        let wait = async {
            loop {
                if let Some(message) = self.parse()? {
                    if let Message::Response {
                        status,
                        headers,
                        body,
                    } = message
                    {
                        return anyhow::Ok((status, headers, body));
                    }
                    continue;
                }
                let mut chunk = [0u8; 8192];
                match self.io.read(&mut chunk).await? {
                    0 => bail!("the server closed the connection"),
                    n => self.buf.extend_from_slice(&chunk[..n]),
                }
            }
        };
        let (status, headers, body) = tokio::time::timeout(ANSWER, wait)
            .await
            .with_context(|| format!("{method}: no answer"))??;
        if !(200..300).contains(&status) {
            bail!("{method}: the server answered {status}");
        }
        Ok((status, headers, body))
    }

    /// One complete message from the buffer, if there is one.
    fn parse(&mut self) -> anyhow::Result<Option<Message>> {
        let Some(&first) = self.buf.first() else {
            return Ok(None);
        };
        if first == b'$' {
            if self.buf.len() < 4 {
                return Ok(None);
            }
            let size = usize::from(u16::from_be_bytes([self.buf[2], self.buf[3]]));
            if self.buf.len() < 4 + size {
                return Ok(None);
            }
            let channel = self.buf[1];
            let data = self.buf[4..4 + size].to_vec();
            self.buf.drain(..4 + size);
            return Ok(Some(Message::Frame(channel, data)));
        }
        let Some(end) = self.buf.windows(4).position(|w| w == b"\r\n\r\n") else {
            if self.buf.len() > MAX_MESSAGE {
                bail!("RTSP header too long");
            }
            return Ok(None);
        };
        let head = String::from_utf8_lossy(&self.buf[..end]).into_owned();
        let mut lines = head.split("\r\n");
        let status = lines
            .next()
            .and_then(|l| l.strip_prefix("RTSP/1.0 "))
            .and_then(|l| l.split(' ').next())
            .and_then(|s| s.parse().ok())
            .context("not an RTSP answer")?;
        let headers: Vec<(String, String)> = lines
            .filter_map(|l| l.split_once(':'))
            .map(|(n, v)| (n.trim().to_ascii_lowercase(), v.trim().to_owned()))
            .collect();
        let length: usize = header(&headers, "content-length")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        if length > MAX_MESSAGE {
            bail!("RTSP body too long");
        }
        if self.buf.len() < end + 4 + length {
            return Ok(None);
        }
        let body = self.buf[end + 4..end + 4 + length].to_vec();
        self.buf.drain(..end + 4 + length);
        Ok(Some(Message::Response {
            status,
            headers,
            body,
        }))
    }
}

fn header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(n, _)| n == name)
        .map(|(_, v)| v.as_str())
}

/// A track's `a=control:` against the session's base URL.
fn join(base: &str, control: &str) -> String {
    if control.starts_with("rtsp://") || control.starts_with("rtsps://") {
        control.to_owned()
    } else if control.is_empty() || control == "*" {
        base.to_owned()
    } else {
        // The query (a token) belongs to the base, not to the track.
        let base = base.split('?').next().unwrap_or(base);
        format!("{}/{}", base.trim_end_matches('/'), control)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SDP: &str = "v=0\r\ns=cam\r\nm=video 0 RTP/AVP 96\r\na=rtpmap:96 H264/90000\r\n\
a=fmtp:96 packetization-mode=1;sprop-parameter-sets=Z2QAKKzZQHgCJ+XARAAAAwAEAAADAPA8YMZY,aOvssg==\r\n\
a=control:trackID=0\r\nm=audio 0 RTP/AVP 97\r\na=rtpmap:97 MPEG4-GENERIC/48000/1\r\na=control:trackID=1\r\n";

    #[test]
    fn urls_are_read_with_their_defaults() {
        let u = Url::parse("rtsps://192.168.1.2:7441/AbC?enableSrtp").unwrap();
        assert!(u.secure);
        assert_eq!((u.host.as_str(), u.port), ("192.168.1.2", 7441));
        assert_eq!(u.path, "/AbC?enableSrtp");
        assert_eq!(Url::parse("rtsp://cam/x").unwrap().port, 554);
        assert_eq!(
            u.at("10.0.0.9").text(),
            "rtsps://10.0.0.9:7441/AbC?enableSrtp"
        );
        assert!(Url::parse("http://x").is_none());
    }

    #[test]
    fn the_video_track_and_its_parameter_sets_come_out_of_the_sdp() {
        let track = parse_sdp(SDP).unwrap();
        assert_eq!(track.payload_type, 96);
        assert_eq!(track.control, "trackID=0");
        let sps = track.sps.unwrap();
        assert_eq!(sps[0], 0x67);
        assert_eq!(crate::h264::picture_size(&sps), Some((1920, 1080)));
        assert_eq!(track.pps.unwrap(), [0x68, 0xEB, 0xEC, 0xB2]);
    }

    #[test]
    fn a_track_is_found_under_the_base_url() {
        assert_eq!(
            join("rtsps://h:1/a?x=1", "trackID=0"),
            "rtsps://h:1/a/trackID=0"
        );
        assert_eq!(join("rtsp://h/a", "*"), "rtsp://h/a");
    }

    #[tokio::test]
    async fn a_session_plays_and_hands_over_the_rtp_packets() {
        use tokio::io::duplex;
        let (client_io, mut server) = duplex(64 * 1024);
        let server_task = tokio::spawn(async move {
            let mut seen = Vec::new();
            let mut buf = vec![0u8; 4096];
            for answer in [
                "RTSP/1.0 200 OK\r\nCSeq: 1\r\n\r\n".to_owned(),
                format!(
                    "RTSP/1.0 200 OK\r\nCSeq: 2\r\nContent-Length: {}\r\n\r\n{SDP}",
                    SDP.len()
                ),
                "RTSP/1.0 200 OK\r\nCSeq: 3\r\nSession: 1234;timeout=60\r\n\r\n".to_owned(),
                "RTSP/1.0 200 OK\r\nCSeq: 4\r\n\r\n".to_owned(),
            ] {
                let n = server.read(&mut buf).await.unwrap();
                seen.push(String::from_utf8_lossy(&buf[..n]).into_owned());
                server.write_all(answer.as_bytes()).await.unwrap();
            }
            // Video on channel 0, something else on 1, in one write.
            let mut media = vec![b'$', 1, 0, 2, 9, 9, b'$', 0, 0, 3, 1, 2, 3];
            media.extend_from_slice(b"RTSP/1.0 200 OK\r\nCSeq: 9\r\n\r\n");
            media.extend_from_slice(&[b'$', 0, 0, 1, 7]);
            server.write_all(&media).await.unwrap();
            seen
        });
        let mut client = Client::over(Box::new(client_io), "rtsp://cam/stream".into());
        let track = client.play_video().await.unwrap();
        assert_eq!(track.payload_type, 96);
        assert_eq!(client.next_packet().await.unwrap(), vec![1, 2, 3]);
        assert_eq!(client.next_packet().await.unwrap(), vec![7]);
        let seen = server_task.await.unwrap();
        assert!(seen[2].starts_with("SETUP rtsp://cam/stream/trackID=0 RTSP/1.0"));
        assert!(seen[2].contains("interleaved=0-1"));
        assert!(seen[3].contains("Session: 1234\r\n"));
    }
}
