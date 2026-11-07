//! Apple's Companion protocol (`_companion-link._tcp`), the one an iPhone's
//! remote control speaks to an Apple TV: frames of OPACK over TCP, paired
//! once with the 4-digit code the TV shows (HAP's pair-setup), then
//! encrypted with ChaCha20-Poly1305 after HAP's pair-verify.
//!
//! A frame is `[type][length: 3 bytes, big endian][payload]`; once
//! encrypted, the length counts the 16-byte tag and the 4-byte header is the
//! additional data. A message is a dictionary: `_i` its name, `_t` its kind
//! (1 event, 2 request, 3 response), `_x` the number a response repeats,
//! `_c` its content.

use std::collections::VecDeque;
use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{TcpStream, UdpSocket};

use crate::opack::Opack;
use moli_homekit::pairing::{
    Error, Identity, Pairing, SetupProof, Verify, counter_nonce, hkdf32, open_nonce, seal_nonce,
};
use moli_homekit::tlv::{self, Tlv};

const ANSWER: Duration = Duration::from_secs(10);
/// The person reads the code on the TV and types it: pair-setup's second
/// step waits for them elsewhere, this is only the TV's own answer.
const PAIR_ANSWER: Duration = Duration::from_secs(30);

const PS_START: u8 = 3;
const PS_NEXT: u8 = 4;
const PV_START: u8 = 5;
const PV_NEXT: u8 = 6;
const E_OPACK: u8 = 8;

const EVENT: i64 = 1;
const REQUEST: i64 = 2;
const RESPONSE: i64 = 3;

/// TLV8 item of pair-setup's M5 carrying the controller's name (OPACK).
const TLV_NAME: u8 = 0x11;
/// TLV8 item of the sequence number in Companion (HAP's `State`).
const SEQ: u8 = tlv::STATE;

/// The largest frame accepted (an app list, artwork are well below).
const MAX_FRAME: usize = 1 << 20;

/// A message the TV pushes: its name and content.
#[derive(Clone, Debug, PartialEq)]
pub struct Event {
    pub name: String,
    pub content: Opack,
}

/// The session keys, one counter per direction.
struct Keys {
    output: [u8; 32],
    input: [u8; 32],
    sent: u64,
    received: u64,
}

pub struct Companion {
    stream: TcpStream,
    keys: Option<Keys>,
    buffer: Vec<u8>,
    xid: i64,
    events: VecDeque<Event>,
}

impl std::fmt::Debug for Companion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Companion")
            .field("encrypted", &self.keys.is_some())
            .finish_non_exhaustive()
    }
}

fn random_u32() -> u32 {
    use ring::rand::SecureRandom as _;
    let mut b = [0u8; 4];
    ring::rand::SystemRandom::new()
        .fill(&mut b)
        .expect("system randomness");
    u32::from_le_bytes(b)
}

impl Companion {
    pub async fn open(address: SocketAddr) -> Result<Self, Error> {
        let stream = tokio::time::timeout(ANSWER, TcpStream::connect(address))
            .await
            .map_err(|_| Error::Timeout)??;
        stream.set_nodelay(true)?;
        Ok(Self {
            stream,
            keys: None,
            buffer: Vec::new(),
            // Any start will do; the TV only repeats it.
            xid: i64::from(random_u32() & 0xFFFF),
            events: VecDeque::new(),
        })
    }

    async fn send_frame(&mut self, kind: u8, payload: &[u8]) -> Result<(), Error> {
        let mut frame = Vec::with_capacity(payload.len() + 20);
        match self.keys.as_mut() {
            Some(keys) if !payload.is_empty() => {
                let len = payload.len() + 16;
                let header = [kind, (len >> 16) as u8, (len >> 8) as u8, len as u8];
                frame.extend_from_slice(&header);
                frame.extend(seal_nonce(
                    &keys.output,
                    counter_nonce(keys.sent),
                    &header,
                    payload,
                ));
                keys.sent += 1;
            }
            _ => {
                let len = payload.len();
                frame.extend_from_slice(&[kind, (len >> 16) as u8, (len >> 8) as u8, len as u8]);
                frame.extend_from_slice(payload);
            }
        }
        self.stream.write_all(&frame).await?;
        Ok(())
    }

    async fn read_frame(&mut self) -> Result<(u8, Vec<u8>), Error> {
        loop {
            if self.buffer.len() >= 4 {
                let len = (usize::from(self.buffer[1]) << 16)
                    | (usize::from(self.buffer[2]) << 8)
                    | usize::from(self.buffer[3]);
                if len > MAX_FRAME {
                    return Err(Error::Protocol);
                }
                if self.buffer.len() >= 4 + len {
                    let frame: Vec<u8> = self.buffer.drain(..4 + len).collect();
                    let (header, body) = frame.split_at(4);
                    let payload = match self.keys.as_mut() {
                        Some(keys) if !body.is_empty() => {
                            let plain =
                                open_nonce(&keys.input, counter_nonce(keys.received), header, body)
                                    .ok_or(Error::Protocol)?;
                            keys.received += 1;
                            plain
                        }
                        _ => body.to_vec(),
                    };
                    return Ok((header[0], payload));
                }
            }
            let mut chunk = [0u8; 8192];
            let n = self.stream.read(&mut chunk).await?;
            if n == 0 {
                return Err(Error::Network(std::io::ErrorKind::UnexpectedEof.into()));
            }
            self.buffer.extend_from_slice(&chunk[..n]);
        }
    }

    /// One pairing step: an OPACK frame out, the TLV of the next one back.
    async fn auth(
        &mut self,
        kind: u8,
        answer_kind: u8,
        body: Opack,
        wait: Duration,
    ) -> Result<Tlv, Error> {
        self.send_frame(kind, &body.encode()).await?;
        let deadline = tokio::time::Instant::now() + wait;
        loop {
            let (got, payload) = tokio::time::timeout_at(deadline, self.read_frame())
                .await
                .map_err(|_| Error::Timeout)??;
            if got != answer_kind {
                continue;
            }
            let message = Opack::decode(&payload).ok_or(Error::Protocol)?;
            let data = message
                .get("_pd")
                .and_then(Opack::as_bytes)
                .ok_or(Error::Protocol)?;
            let answer = Tlv::parse(data).ok_or(Error::Protocol)?;
            if let Some(e) = answer.byte(tlv::ERROR) {
                return Err(Error::Refused(e));
            }
            return Ok(answer);
        }
    }

    /// pair-setup's first step: the TV shows a 4-digit code and waits for
    /// [`Self::pair_finish`] on this same connection.
    pub async fn pair_start(&mut self) -> Result<Tlv, Error> {
        let m1 = Tlv::new().with(tlv::METHOD, [0]).with(SEQ, [1]);
        self.auth(
            PS_START,
            PS_NEXT,
            Opack::dict([("_pd", Opack::Bytes(m1.encode())), ("_pwTy", Opack::Int(1))]),
            PAIR_ANSWER,
        )
        .await
    }

    /// The code the TV shows: the pairing is made (or refused, error 2
    /// for a wrong code).
    pub async fn pair_finish(
        &mut self,
        m2: &Tlv,
        code: &str,
        name: &str,
    ) -> Result<Pairing, Error> {
        let code: String = code.chars().filter(char::is_ascii_digit).collect();
        let srp = SetupProof::new(m2, &code)?;
        let m3 = Tlv::new()
            .with(SEQ, [3])
            .with(tlv::PUBLIC_KEY, &srp.big_a)
            .with(tlv::PROOF, srp.m1);
        let m4 = self
            .auth(PS_NEXT, PS_NEXT, setup_frame(&m3), PAIR_ANSWER)
            .await?;
        srp.check(&m4)?;
        let identity = Identity::new();
        let info = Opack::dict([("name", Opack::str(name))]).encode();
        let m5 = Tlv::new().with(SEQ, [5]).with(
            tlv::ENCRYPTED_DATA,
            identity.sealed(&srp, &[(TLV_NAME, info)]),
        );
        let m6 = self
            .auth(PS_NEXT, PS_NEXT, setup_frame(&m5), PAIR_ANSWER)
            .await?;
        identity.pairing(&srp, &m6)
    }

    /// pair-verify, then everything is encrypted.
    pub async fn verify(&mut self, pairing: &Pairing) -> Result<(), Error> {
        let verify = Verify::start()?;
        let m1 = Tlv::new()
            .with(SEQ, [1])
            .with(tlv::PUBLIC_KEY, verify.public());
        let m2 = self
            .auth(
                PV_START,
                PV_NEXT,
                Opack::dict([("_pd", Opack::Bytes(m1.encode())), ("_auTy", Opack::Int(4))]),
                ANSWER,
            )
            .await?;
        let (sealed, shared) = verify.finish(&m2, pairing)?;
        let m3 = Tlv::new().with(SEQ, [3]).with(tlv::ENCRYPTED_DATA, sealed);
        self.auth(
            PV_NEXT,
            PV_NEXT,
            Opack::dict([("_pd", Opack::Bytes(m3.encode()))]),
            ANSWER,
        )
        .await?;
        self.keys = Some(Keys {
            output: hkdf32(&shared, "", "ClientEncrypt-main"),
            input: hkdf32(&shared, "", "ServerEncrypt-main"),
            sent: 0,
            received: 0,
        });
        Ok(())
    }

    async fn send_message(&mut self, name: &str, kind: i64, content: Opack) -> Result<i64, Error> {
        let xid = self.xid;
        self.xid += 1;
        let message = Opack::dict([
            ("_i", Opack::str(name)),
            ("_t", Opack::Int(kind)),
            ("_x", Opack::Int(xid)),
            ("_c", content),
        ]);
        self.send_frame(E_OPACK, &message.encode()).await?;
        Ok(xid)
    }

    /// A message read off the stream: a response (its `_x`) or an event,
    /// queued. `None`: something else (a no-op frame, a request).
    async fn read_message(&mut self) -> Result<Option<(i64, Opack)>, Error> {
        let (kind, payload) = self.read_frame().await?;
        if kind != E_OPACK {
            return Ok(None);
        }
        let message = Opack::decode(&payload).ok_or(Error::Protocol)?;
        let content = message.get("_c").cloned().unwrap_or(Opack::Null);
        match message.get("_t").and_then(Opack::as_int) {
            Some(EVENT) => {
                if let Some(name) = message.get("_i").and_then(Opack::as_str) {
                    self.events.push_back(Event {
                        name: name.to_owned(),
                        content,
                    });
                }
                Ok(None)
            }
            Some(RESPONSE) => {
                let xid = message.get("_x").and_then(Opack::as_int).unwrap_or(-1);
                Ok(Some((xid, message)))
            }
            _ => Ok(None),
        }
    }

    /// A request and its response's content. An error the TV reports
    /// (`_em`, `_ec`) is `Refused`.
    pub async fn request(&mut self, name: &str, content: Opack) -> Result<Opack, Error> {
        let xid = self.send_message(name, REQUEST, content).await?;
        let deadline = tokio::time::Instant::now() + ANSWER;
        loop {
            let read = tokio::time::timeout_at(deadline, self.read_message())
                .await
                .map_err(|_| Error::Timeout)??;
            let Some((got, message)) = read else { continue };
            if got != xid {
                continue;
            }
            if message.get("_em").is_some() || message.get("_ec").is_some() {
                tracing::debug!(name, answer = %message.to_json(), "companion request refused");
                let code = message.get("_ec").and_then(Opack::as_int).unwrap_or(1);
                return Err(Error::Refused(
                    u8::try_from(code.clamp(1, 255)).unwrap_or(1),
                ));
            }
            return Ok(message.get("_c").cloned().unwrap_or(Opack::Null));
        }
    }

    /// An event Moli sends (no answer).
    pub async fn event(&mut self, name: &str, content: Opack) -> Result<(), Error> {
        self.send_message(name, EVENT, content).await.map(drop)
    }

    /// The next event the TV pushes.
    pub async fn next_event(&mut self) -> Result<Event, Error> {
        loop {
            if let Some(event) = self.events.pop_front() {
                return Ok(event);
            }
            self.read_message().await?;
        }
    }
}

fn setup_frame(tlv: &Tlv) -> Opack {
    Opack::dict([
        ("_pd", Opack::Bytes(tlv.encode())),
        ("_pwTy", Opack::Int(1)),
    ])
}

/// The Companion port of the device at `host`, asked to the device itself
/// (a unicast mDNS question, RFC 6762 §5.5): it changes when the TV
/// restarts.
pub async fn find_port(host: IpAddr) -> Result<u16, Error> {
    const SERVICE: &str = "_companion-link._tcp.local";
    let socket = UdpSocket::bind(if host.is_ipv4() {
        "0.0.0.0:0"
    } else {
        "[::]:0"
    })
    .await?;
    let mut query = vec![0x12, 0x34, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0];
    for label in SERVICE.split('.') {
        query.push(label.len() as u8);
        query.extend_from_slice(label.as_bytes());
    }
    // PTR, class IN with the « unicast response » bit.
    query.extend_from_slice(&[0, 0, 12, 0x80, 1]);
    for _ in 0..3 {
        socket.send_to(&query, (host, 5353)).await?;
        let mut buf = vec![0u8; 9000];
        let Ok(read) =
            tokio::time::timeout(Duration::from_secs(2), socket.recv_from(&mut buf)).await
        else {
            continue;
        };
        let (n, from) = read?;
        if from.ip() != host {
            continue;
        }
        if let Some(port) = srv_port(&buf[..n], SERVICE) {
            return Ok(port);
        }
    }
    Err(Error::Timeout)
}

/// A DNS name at `at` (compression pointers followed) and where it ends.
fn read_name(packet: &[u8], mut at: usize) -> Option<(String, usize)> {
    let mut labels: Vec<String> = Vec::new();
    let mut end = None;
    for _ in 0..64 {
        let len = *packet.get(at)?;
        if len & 0xC0 == 0xC0 {
            let low = *packet.get(at + 1)?;
            end.get_or_insert(at + 2);
            at = (usize::from(len & 0x3F) << 8) | usize::from(low);
        } else if len == 0 {
            return Some((labels.join("."), end.unwrap_or(at + 1)));
        } else {
            let start = at + 1;
            let label = packet.get(start..start + usize::from(len))?;
            labels.push(String::from_utf8_lossy(label).into_owned());
            at = start + usize::from(len);
        }
    }
    None
}

/// The port of the first SRV record of `service` in an mDNS answer.
fn srv_port(packet: &[u8], service: &str) -> Option<u16> {
    if packet.len() < 12 || packet[2] & 0x80 == 0 {
        return None;
    }
    let count = |i: usize| usize::from(u16::from_be_bytes([packet[i], packet[i + 1]]));
    let questions = count(4);
    let records = count(6) + count(8) + count(10);
    let mut at = 12;
    for _ in 0..questions {
        let (_, next) = read_name(packet, at)?;
        at = next + 4;
    }
    for _ in 0..records {
        let (name, next) = read_name(packet, at)?;
        let kind = u16::from_be_bytes([*packet.get(next)?, *packet.get(next + 1)?]);
        let len = usize::from(u16::from_be_bytes([
            *packet.get(next + 8)?,
            *packet.get(next + 9)?,
        ]));
        let data = next + 10;
        if kind == 33 && name.to_lowercase().ends_with(service) {
            return Some(u16::from_be_bytes([
                *packet.get(data + 4)?,
                *packet.get(data + 5)?,
            ]));
        }
        at = data + len;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_srv_record_gives_the_port() {
        // A response: no question, one PTR answer, one SRV additional.
        let mut p = vec![0, 0, 0x84, 0, 0, 0, 0, 1, 0, 0, 0, 1];
        let name = |p: &mut Vec<u8>, labels: &[&str]| {
            for l in labels {
                p.push(l.len() as u8);
                p.extend_from_slice(l.as_bytes());
            }
            p.push(0);
        };
        name(&mut p, &["_companion-link", "_tcp", "local"]);
        p.extend_from_slice(&[0, 12, 0, 1, 0, 0, 0, 10, 0, 2, 0xC0, 12]);
        name(&mut p, &["Living", "_companion-link", "_tcp", "local"]);
        p.extend_from_slice(&[
            0, 33, 0x80, 1, 0, 0, 0, 120, 0, 8, 0, 0, 0, 0, 0xC0, 0x01, 0xC0, 12,
        ]);
        assert_eq!(srv_port(&p, "_companion-link._tcp.local"), Some(49153));
        assert_eq!(srv_port(&p[..20], "_companion-link._tcp.local"), None);
    }

    /// A TV's side of the encrypted frames, to check both directions.
    #[tokio::test]
    async fn encrypted_messages_and_events_cross_the_link() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let shared = [5u8; 32];
        let tv = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let input = hkdf32(&shared, "", "ClientEncrypt-main");
            let output = hkdf32(&shared, "", "ServerEncrypt-main");
            let mut header = [0u8; 4];
            socket.read_exact(&mut header).await.unwrap();
            let len = (usize::from(header[1]) << 16)
                | (usize::from(header[2]) << 8)
                | usize::from(header[3]);
            let mut body = vec![0u8; len];
            socket.read_exact(&mut body).await.unwrap();
            let plain = open_nonce(&input, counter_nonce(0), &header, &body).unwrap();
            let request = Opack::decode(&plain).unwrap();
            assert_eq!(request.get("_i"), Some(&Opack::str("FetchAttentionState")));
            let xid = request.get("_x").cloned().unwrap();
            let send = |n: u64, message: Opack| {
                let payload = message.encode();
                let len = payload.len() + 16;
                let header = [E_OPACK, 0, (len >> 8) as u8, len as u8];
                let mut frame = header.to_vec();
                frame.extend(seal_nonce(&output, counter_nonce(n), &header, &payload));
                frame
            };
            let event = send(
                0,
                Opack::dict([
                    ("_i", Opack::str("TVSystemStatus")),
                    ("_t", Opack::Int(EVENT)),
                    ("_c", Opack::dict([("state", Opack::Int(1))])),
                ]),
            );
            let response = send(
                1,
                Opack::dict([
                    ("_t", Opack::Int(RESPONSE)),
                    ("_x", xid),
                    ("_c", Opack::dict([("state", Opack::Int(3))])),
                ]),
            );
            socket.write_all(&event).await.unwrap();
            socket.write_all(&response).await.unwrap();
            socket.flush().await.unwrap();
        });
        let mut companion = Companion::open(address).await.unwrap();
        companion.keys = Some(Keys {
            output: hkdf32(&shared, "", "ClientEncrypt-main"),
            input: hkdf32(&shared, "", "ServerEncrypt-main"),
            sent: 0,
            received: 0,
        });
        let answer = companion
            .request("FetchAttentionState", Opack::dict([]))
            .await
            .unwrap();
        assert_eq!(answer.get("state"), Some(&Opack::Int(3)));
        let event = companion.next_event().await.unwrap();
        assert_eq!(event.name, "TVSystemStatus");
        assert_eq!(event.content.get("state"), Some(&Opack::Int(1)));
        tv.await.unwrap();
    }
}
