//! A remote-control session over AirPlay 2 (port 7000), as an iPhone opens
//! one to choose where an Apple TV's sound goes: pair-verify with the
//! pairing already made, then RTSP on the encrypted control channel, an
//! event channel the Apple TV talks on, and a data stream carrying MRP
//! messages. Through it, Moli reads the Apple TV's audio outputs and adds or
//! removes AirPlay speakers.

use std::fmt::Write as _;
use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use moli_homekit::pairing::{Channel, Error, Pairing, Verify, hkdf32};
use moli_homekit::tlv::{self, Tlv};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, watch};

use crate::bplist::Plist;
use crate::mrp::{self, Change, Output};

const PORT: u16 = 7000;
const ANSWER: Duration = Duration::from_secs(10);
/// What an iPhone sends to keep the session.
const FEEDBACK: Duration = Duration::from_secs(2);
const USER_AGENT: &str = "AirPlay/550.10";
const BPLIST: &str = "application/x-apple-binary-plist";

fn random<const N: usize>() -> [u8; N] {
    use ring::rand::SecureRandom as _;
    let mut b = [0u8; N];
    ring::rand::SystemRandom::new()
        .fill(&mut b)
        .expect("system randomness");
    b
}

fn uuid() -> String {
    let b = random::<16>();
    let h = crate::opack::hex(&b).to_uppercase();
    format!(
        "{}-{}-{}-{}-{}",
        &h[..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..]
    )
}

/// A response or a request read off a stream: first line, headers, body.
#[derive(Debug)]
struct Message {
    first: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Message {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    /// The status of a response (`RTSP/1.0 200 OK`).
    fn status(&self) -> Option<u16> {
        self.first.split(' ').nth(1)?.parse().ok()
    }
}

/// Cuts one message off the front of `buffer`; `None` while incomplete.
fn take(buffer: &mut Vec<u8>) -> Option<Message> {
    let end = buffer.windows(4).position(|w| w == b"\r\n\r\n")?;
    let head = String::from_utf8_lossy(&buffer[..end]).into_owned();
    let mut lines = head.split("\r\n");
    let first = lines.next().unwrap_or_default().to_owned();
    let headers: Vec<(String, String)> = lines
        .filter_map(|l| {
            let (k, v) = l.split_once(':')?;
            Some((k.trim().to_owned(), v.trim().to_owned()))
        })
        .collect();
    let length: usize = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, v)| v.parse().ok())
        .unwrap_or(0);
    if buffer.len() < end + 4 + length {
        return None;
    }
    let body = buffer[end + 4..end + 4 + length].to_vec();
    buffer.drain(..end + 4 + length);
    Some(Message {
        first,
        headers,
        body,
    })
}

/// A TCP stream, plain or HAP-encrypted, read message by message.
struct Link {
    stream: TcpStream,
    channel: Option<Channel>,
    raw: Vec<u8>,
    plain: Vec<u8>,
}

impl Link {
    async fn connect(address: SocketAddr) -> Result<Self, Error> {
        let stream = tokio::time::timeout(ANSWER, TcpStream::connect(address))
            .await
            .map_err(|_| Error::Timeout)??;
        stream.set_nodelay(true)?;
        Ok(Self {
            stream,
            channel: None,
            raw: Vec::new(),
            plain: Vec::new(),
        })
    }

    async fn write(&mut self, bytes: &[u8]) -> Result<(), Error> {
        let wire = match self.channel.as_mut() {
            Some(channel) => channel.encrypt(bytes),
            None => bytes.to_vec(),
        };
        self.stream.write_all(&wire).await?;
        Ok(())
    }

    /// More bytes into `plain` (cancel-safe: only the read awaits).
    async fn fill(&mut self) -> Result<(), Error> {
        let mut chunk = [0u8; 8192];
        let n = self.stream.read(&mut chunk).await?;
        if n == 0 {
            return Err(Error::Network(std::io::ErrorKind::UnexpectedEof.into()));
        }
        self.raw.extend_from_slice(&chunk[..n]);
        match self.channel.as_mut() {
            Some(channel) => {
                let opened = channel
                    .decrypt(&mut self.raw)
                    .map_err(|()| Error::Protocol)?;
                self.plain.extend(opened);
            }
            None => self.plain.append(&mut self.raw),
        }
        Ok(())
    }

    async fn message(&mut self) -> Result<Message, Error> {
        loop {
            if let Some(message) = take(&mut self.plain) {
                return Ok(message);
            }
            self.fill().await?;
        }
    }
}

/// The control connection: HTTP for pair-verify, then RTSP.
struct Control {
    link: Link,
    cseq: u32,
    uri: String,
    dacp: String,
    active_remote: u32,
}

impl Control {
    async fn request(
        &mut self,
        method: &str,
        uri: Option<&str>,
        protocol: &str,
        content_type: &str,
        body: &[u8],
    ) -> Result<Message, Error> {
        self.cseq += 1;
        let mut head = format!(
            "{method} {} {protocol}\r\nCSeq: {}\r\nUser-Agent: {USER_AGENT}\r\nDACP-ID: {}\r\nActive-Remote: {}\r\nClient-Instance: {}\r\nContent-Length: {}\r\n",
            uri.unwrap_or(&self.uri),
            self.cseq,
            self.dacp,
            self.active_remote,
            self.dacp,
            body.len()
        );
        if !content_type.is_empty() {
            let _ = write!(head, "Content-Type: {content_type}\r\n");
        }
        if protocol.starts_with("HTTP") {
            head.push_str("X-Apple-HKP: 3\r\nConnection: keep-alive\r\n");
        }
        head.push_str("\r\n");
        let mut bytes = head.into_bytes();
        bytes.extend_from_slice(body);
        self.link.write(&bytes).await?;
        let answer = tokio::time::timeout(ANSWER, self.link.message())
            .await
            .map_err(|_| Error::Timeout)??;
        match answer.status() {
            Some(200..=299) => Ok(answer),
            Some(status) => Err(Error::Status(status)),
            None => Err(Error::Protocol),
        }
    }

    async fn verify_step(&mut self, tlv: &Tlv) -> Result<Tlv, Error> {
        let answer = self
            .request(
                "POST",
                Some("/pair-verify"),
                "HTTP/1.1",
                "application/octet-stream",
                &tlv.encode(),
            )
            .await;
        let answer = match answer {
            Ok(a) => a,
            // An Apple TV that does not know Moli answers 470.
            Err(Error::Status(470)) => return Err(Error::Refused(tlv::ERR_AUTHENTICATION)),
            Err(e) => return Err(e),
        };
        let tlv = Tlv::parse(&answer.body).ok_or(Error::Protocol)?;
        if let Some(e) = tlv.byte(tlv::ERROR) {
            return Err(Error::Refused(e));
        }
        Ok(tlv)
    }

    async fn setup(&mut self, body: &Plist) -> Result<Plist, Error> {
        let answer = self
            .request("SETUP", None, "RTSP/1.0", BPLIST, &body.encode())
            .await?;
        Plist::decode(&answer.body).ok_or(Error::Protocol)
    }
}

/// What the session reports.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct State {
    /// The Apple TV's current audio outputs (itself included).
    pub outputs: Vec<Output>,
    pub connected: bool,
}

/// A running session: orders in, outputs out.
#[derive(Debug)]
pub struct Handle {
    orders: mpsc::Sender<(Change, Vec<String>)>,
    pub state: watch::Receiver<State>,
}

impl Handle {
    pub async fn change(&self, change: Change, uids: Vec<String>) -> bool {
        self.orders.send((change, uids)).await.is_ok()
    }
}

/// Opens the session; the returned task keeps it until it fails or the
/// handle is dropped.
#[allow(clippy::too_many_lines)] // the setup, step by step, as an iPhone does it
pub async fn open(
    host: IpAddr,
    pairing: &Pairing,
) -> Result<(Handle, tokio::task::JoinHandle<Result<(), Error>>), Error> {
    let mut control = Control {
        link: Link::connect(SocketAddr::new(host, PORT)).await?,
        cseq: 0,
        uri: String::new(),
        dacp: crate::opack::hex(&random::<8>()).to_uppercase(),
        active_remote: u32::from_le_bytes(random::<4>()),
    };
    let local = control.link.stream.local_addr()?.ip();
    control.uri = format!("rtsp://{local}/{}", u32::from_le_bytes(random::<4>()));
    // pair-verify, then the control channel is encrypted.
    let verify = Verify::start()?;
    let m2 = control
        .verify_step(
            &Tlv::new()
                .with(tlv::STATE, [1])
                .with(tlv::PUBLIC_KEY, verify.public()),
        )
        .await?;
    let (sealed, shared) = verify.finish(&m2, pairing)?;
    control
        .verify_step(
            &Tlv::new()
                .with(tlv::STATE, [3])
                .with(tlv::ENCRYPTED_DATA, sealed),
        )
        .await?;
    control.link.channel = Some(Channel::mirrored(&shared).lenient());
    // The event channel (the Apple TV talks on it; it must be there).
    let device_id = {
        let b = random::<6>();
        b.iter()
            .map(|x| format!("{x:02X}"))
            .collect::<Vec<_>>()
            .join(":")
    };
    let answer = control
        .setup(&Plist::dict([
            ("isRemoteControlOnly", Plist::Bool(true)),
            ("osName", Plist::str("iPhone OS")),
            ("sourceVersion", Plist::str("550.10")),
            ("timingProtocol", Plist::str("None")),
            ("model", Plist::str("iPhone14,3")),
            ("deviceID", Plist::Str(device_id.clone())),
            ("osVersion", Plist::str("17.0")),
            ("osBuildVersion", Plist::str("21A329")),
            ("macAddress", Plist::Str(device_id)),
            ("sessionUUID", Plist::Str(uuid())),
            ("name", Plist::str("Moli")),
        ]))
        .await?;
    let event_port = answer
        .get("eventPort")
        .and_then(Plist::as_int)
        .and_then(|p| u16::try_from(p).ok())
        .ok_or(Error::Protocol)?;
    let mut events = Link::connect(SocketAddr::new(host, event_port)).await?;
    // Read and write swapped: the Apple TV is the one asking here.
    events.channel = Some(
        Channel::from_keys(
            hkdf32(&shared, "Events-Salt", "Events-Write-Encryption-Key"),
            hkdf32(&shared, "Events-Salt", "Events-Read-Encryption-Key"),
        )
        .lenient(),
    );
    control.request("RECORD", None, "RTSP/1.0", "", &[]).await?;
    // The data stream, with a seed of its own in its keys.
    let seed = u64::from_le_bytes(random::<8>()) >> 1;
    let answer = control
        .setup(&Plist::dict([(
            "streams",
            Plist::Array(vec![Plist::dict([
                ("controlType", Plist::Int(2)),
                ("channelID", Plist::Str(uuid())),
                ("seed", Plist::Int(i64::try_from(seed).unwrap_or(0))),
                ("clientUUID", Plist::Str(uuid())),
                ("type", Plist::Int(130)),
                ("wantsDedicatedSocket", Plist::Bool(true)),
                (
                    "clientTypeUUID",
                    Plist::str("1910A70F-DBC0-4242-AF95-115DB30604E1"),
                ),
            ])]),
        )]))
        .await?;
    let data_port = answer
        .get("streams")
        .and_then(Plist::as_array)
        .and_then(|s| s.first())
        .and_then(|s| s.get("dataPort"))
        .and_then(Plist::as_int)
        .and_then(|p| u16::try_from(p).ok())
        .ok_or(Error::Protocol)?;
    let mut data = Link::connect(SocketAddr::new(host, data_port)).await?;
    let salt = format!("DataStream-Salt{seed}");
    data.channel = Some(
        Channel::from_keys(
            hkdf32(&shared, &salt, "DataStream-Input-Encryption-Key"),
            hkdf32(&shared, &salt, "DataStream-Output-Encryption-Key"),
        )
        .lenient(),
    );
    let mut stream = DataStream {
        link: data,
        seqno: 0x1_0000_0000 + u64::from(u32::from_le_bytes(random::<4>())),
    };
    let client = pairing.controller().to_owned();
    stream
        .send(&mrp::device_info(&uuid(), &client, "Moli"))
        .await?;
    stream.send(&mrp::set_connection_state(&uuid())).await?;
    stream.send(&mrp::client_updates_config(&uuid())).await?;
    let (orders, rx) = mpsc::channel(8);
    let (tx, state) = watch::channel(State {
        outputs: Vec::new(),
        connected: true,
    });
    let task = tokio::spawn(run(control, events, stream, rx, tx));
    Ok((Handle { orders, state }, task))
}

/// The data stream: 32-byte frames (`size`, `sync`/`rply`, `comm`,
/// sequence number) around a binary plist `{params: {data: …}}`.
struct DataStream {
    link: Link,
    seqno: u64,
}

impl DataStream {
    fn frame(kind: &[u8], command: &[u8], seqno: u64, payload: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(32 + payload.len());
        out.extend_from_slice(
            &u32::try_from(32 + payload.len())
                .unwrap_or(u32::MAX)
                .to_be_bytes(),
        );
        let mut tag = [0u8; 12];
        tag[..kind.len()].copy_from_slice(kind);
        out.extend_from_slice(&tag);
        let mut cmd = [0u8; 4];
        cmd[..command.len()].copy_from_slice(command);
        out.extend_from_slice(&cmd);
        out.extend_from_slice(&seqno.to_be_bytes());
        out.extend_from_slice(&[0; 4]);
        out.extend_from_slice(payload);
        out
    }

    async fn send(&mut self, message: &[u8]) -> Result<(), Error> {
        let payload = Plist::dict([(
            "params",
            Plist::dict([("data", Plist::Data(mrp::join(&[message.to_vec()])))]),
        )])
        .encode();
        self.seqno += 1;
        let frame = Self::frame(b"sync", b"comm", self.seqno, &payload);
        self.link.write(&frame).await
    }

    /// The next frame's MRP messages (a `sync` is answered with a `rply`).
    async fn next(&mut self) -> Result<Vec<Vec<u8>>, Error> {
        loop {
            if self.link.plain.len() >= 32 {
                let size = usize::try_from(u32::from_be_bytes(
                    self.link.plain[..4].try_into().expect("4 bytes"),
                ))
                .unwrap_or(usize::MAX);
                if !(32..=1 << 22).contains(&size) {
                    return Err(Error::Protocol);
                }
                if self.link.plain.len() >= size {
                    let frame: Vec<u8> = self.link.plain.drain(..size).collect();
                    if frame[4..8] == *b"sync" {
                        let seqno = u64::from_be_bytes(frame[20..28].try_into().expect("8 bytes"));
                        let reply = Self::frame(b"rply", b"", seqno, &[]);
                        self.link.write(&reply).await?;
                    }
                    let data = Plist::decode(&frame[32..])
                        .and_then(|p| p.get("params")?.get("data")?.as_data().map(<[u8]>::to_vec));
                    return Ok(data
                        .map(|d| mrp::split(&d).into_iter().map(<[u8]>::to_vec).collect())
                        .unwrap_or_default());
                }
            }
            self.link.fill().await?;
        }
    }
}

#[allow(clippy::too_many_lines)] // the session's four sources, side by side
async fn run(
    mut control: Control,
    mut events: Link,
    mut data: DataStream,
    mut orders: mpsc::Receiver<(Change, Vec<String>)>,
    state: watch::Sender<State>,
) -> Result<(), Error> {
    let mut feedback = tokio::time::interval(FEEDBACK);
    let result = loop {
        tokio::select! {
            order = orders.recv() => {
                let Some((change, uids)) = order else { break Ok(()) };
                tracing::info!(?change, uids = ?uids, "airplay: changing outputs");
                if let Err(e) = data.send(&mrp::modify_outputs(&uuid(), change, &uids)).await {
                    break Err(e);
                }
            }
            messages = data.next() => match messages {
                Ok(messages) => {
                    for m in messages {
                        let Some((kind, body)) = mrp::open(&m) else { continue };
                        tracing::debug!(kind, "airplay: mrp message");
                        if kind == mrp::UPDATE_OUTPUT_DEVICE.0 && let Some(body) = body {
                            let outputs = mrp::outputs_of(body);
                            tracing::info!(outputs = ?outputs.iter().map(|o| o.name.as_str()).collect::<Vec<_>>(), "airplay: outputs");
                            state.send_modify(|s| s.outputs = outputs);
                        }
                    }
                }
                Err(e) => {
                    break Err(e);
                }
            },
            request = events.message() => match request {
                Ok(request) => {
                    let protocol = request.first.rsplit(' ').next().unwrap_or("RTSP/1.0").to_owned();
                    let mut reply = format!("{protocol} 200 OK\r\nContent-Length: 0\r\nAudio-Latency: 0\r\n");
                    for name in ["Server", "CSeq"] {
                        if let Some(v) = request.header(name) {
                            let _ = write!(reply, "{name}: {v}\r\n");
                        }
                    }
                    reply.push_str("\r\n");
                    if let Err(e) = events.write(reply.as_bytes()).await {
                        break Err(e);
                    }
                }
                Err(e) => break Err(e),
            },
            _ = feedback.tick() => {
                if let Err(e) = control.request("POST", Some("/feedback"), "RTSP/1.0", "", &[]).await {
                    tracing::debug!(error = %e, "airplay: feedback failed");
                    break Err(e);
                }
            }
        }
    };
    state.send_modify(|s| s.connected = false);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_are_cut_out_of_the_stream() {
        let mut buffer =
            b"RTSP/1.0 200 OK\r\nCSeq: 2\r\nContent-Length: 3\r\n\r\nabcPOST /x RTSP/1.0\r\n\r\n"
                .to_vec();
        let first = take(&mut buffer).unwrap();
        assert_eq!(first.status(), Some(200));
        assert_eq!(first.header("cseq"), Some("2"));
        assert_eq!(first.body, b"abc");
        let second = take(&mut buffer).unwrap();
        assert_eq!(second.first, "POST /x RTSP/1.0");
        assert!(take(&mut buffer).is_none());
    }

    #[test]
    fn data_frames_have_a_32_byte_header() {
        let frame = DataStream::frame(b"sync", b"comm", 7, b"xyz");
        assert_eq!(frame.len(), 35);
        assert_eq!(&frame[..4], &35u32.to_be_bytes());
        assert_eq!(&frame[4..8], b"sync");
        assert_eq!(&frame[16..20], b"comm");
        assert_eq!(&frame[20..28], &7u64.to_be_bytes());
    }
}
