//! The other side of HomeKit: Moli as a *controller*, like the Home app. It
//! pairs with an accessory once (its 8-digit setup code), then opens an
//! encrypted session to it, reads its database and subscribes to its events
//! (a doorbell's press arrives over the local network, with no cloud).
//!
//! An accessory has one owner: paired to the Home app, it refuses anyone
//! else. It must be removed from Home first; Moli can then show it to Home
//! again through its own bridge.

use std::collections::VecDeque;
use std::time::Duration;

use ring::agreement::{self, EphemeralPrivateKey, UnparsedPublicKey, X25519};
use ring::digest::{self, SHA512};
use ring::rand::{SecureRandom, SystemRandom};
use ring::signature::{ED25519, Ed25519KeyPair, KeyPair, UnparsedPublicKey as SignatureKey};
use serde_json::Value as Json;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpStream;

use crate::bigint::Uint;
use crate::crypto::{self, Channel, hkdf32, message_tag};
use crate::setup::{hex, unhex};
use crate::srp;
use crate::tlv::{self, Tlv};

/// How long an accessory may take to answer one request.
const ANSWER: Duration = Duration::from_secs(10);
/// SRP takes a while on a small accessory.
const PAIR_ANSWER: Duration = Duration::from_secs(40);

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("accessory unreachable: {0}")]
    Network(#[from] std::io::Error),
    #[error("the accessory did not answer in time")]
    Timeout,
    #[error("the accessory answered something unreadable")]
    Protocol,
    /// The accessory's own refusal (`kTLVError_*`): 2 wrong code or unknown
    /// controller, 5 too many tries, 6 already paired, 7 busy.
    #[error("the accessory refused (error {0})")]
    Refused(u8),
    #[error("the accessory is not the one paired")]
    WrongAccessory,
    #[error("HTTP {0}")]
    Status(u16),
}

/// What a pairing leaves: who Moli is to the accessory, and who the
/// accessory is. Kept as one line in the secret store.
#[derive(Clone, PartialEq, Eq)]
pub struct Pairing {
    controller: String,
    seed: [u8; 32],
    accessory: String,
    accessory_key: Vec<u8>,
}

impl std::fmt::Debug for Pairing {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pairing")
            .field("accessory", &self.accessory)
            .finish_non_exhaustive()
    }
}

impl Pairing {
    /// `controller|seed|accessory|accessory key`, hex for the keys.
    #[must_use]
    pub fn encode(&self) -> String {
        format!(
            "{}|{}|{}|{}",
            self.controller,
            hex(&self.seed),
            self.accessory,
            hex(&self.accessory_key)
        )
    }

    #[must_use]
    pub fn decode(text: &str) -> Option<Self> {
        let mut parts = text.trim().split('|');
        let controller = parts.next()?.to_owned();
        let seed = unhex(parts.next()?)?.try_into().ok()?;
        let accessory = parts.next()?.to_owned();
        let accessory_key = unhex(parts.next()?)?;
        (parts.next().is_none() && accessory_key.len() == 32).then_some(Self {
            controller,
            seed,
            accessory,
            accessory_key,
        })
    }

    /// The accessory's pairing identifier (`AA:BB:…`, its `id=` on mDNS).
    #[must_use]
    pub fn accessory(&self) -> &str {
        &self.accessory
    }

    fn key(&self) -> Ed25519KeyPair {
        Ed25519KeyPair::from_seed_unchecked(&self.seed).expect("a 32-byte seed")
    }
}

/// An event pushed by the accessory: one characteristic's new value.
#[derive(Clone, Debug, PartialEq)]
pub struct Event {
    pub aid: u64,
    pub iid: u64,
    pub value: Json,
}

/// One message read from the accessory.
enum Message {
    Response(u16, Vec<u8>),
    Event(Vec<u8>),
}

/// A connection to an accessory, plain until pair-verify, then encrypted.
pub struct Connection {
    stream: TcpStream,
    channel: Option<Channel>,
    raw: Vec<u8>,
    plain: Vec<u8>,
    events: VecDeque<Event>,
}

impl std::fmt::Debug for Connection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Connection")
            .field("encrypted", &self.channel.is_some())
            .finish_non_exhaustive()
    }
}

fn sha(parts: &[&[u8]]) -> [u8; 64] {
    let mut ctx = digest::Context::new(&SHA512);
    for part in parts {
        ctx.update(part);
    }
    ctx.finish().as_ref().try_into().expect("64 bytes")
}

fn random<const N: usize>() -> [u8; N] {
    let mut out = [0u8; N];
    SystemRandom::new()
        .fill(&mut out)
        .expect("system randomness");
    out
}

/// A random controller identifier, shaped like the ones phones use.
fn new_controller_id() -> String {
    let b = random::<16>();
    format!(
        "{}-{}-{}-{}-{}",
        hex(&b[..4]),
        hex(&b[4..6]),
        hex(&b[6..8]),
        hex(&b[8..10]),
        hex(&b[10..])
    )
    .to_uppercase()
}

/// Reads one HTTP-like message (`HTTP/1.1 …` or `EVENT/1.0 …`) off the
/// front of `buffer`, chunked bodies included; `None` while incomplete.
fn take_message(buffer: &mut Vec<u8>) -> Option<Result<Message, Error>> {
    let end = buffer.windows(4).position(|w| w == b"\r\n\r\n")?;
    let head = String::from_utf8_lossy(&buffer[..end]).into_owned();
    let mut lines = head.lines();
    let first = lines.next().unwrap_or_default();
    let mut words = first.split(' ');
    let protocol = words.next().unwrap_or_default().to_owned();
    let Some(status) = words.next().and_then(|s| s.parse::<u16>().ok()) else {
        return Some(Err(Error::Protocol));
    };
    let header = |name: &str| {
        head.lines().skip(1).find_map(|l| {
            let (k, v) = l.split_once(':')?;
            k.trim()
                .eq_ignore_ascii_case(name)
                .then(|| v.trim().to_owned())
        })
    };
    let start = end + 4;
    let (body, used) =
        if header("Transfer-Encoding").is_some_and(|v| v.eq_ignore_ascii_case("chunked")) {
            let mut body = Vec::new();
            let mut at = start;
            loop {
                let line_end = buffer[at..].windows(2).position(|w| w == b"\r\n")? + at;
                let size_text = String::from_utf8_lossy(&buffer[at..line_end]).into_owned();
                let Ok(size) =
                    usize::from_str_radix(size_text.split(';').next().unwrap_or("").trim(), 16)
                else {
                    return Some(Err(Error::Protocol));
                };
                let data = line_end + 2;
                if buffer.len() < data + size + 2 {
                    return None;
                }
                body.extend_from_slice(&buffer[data..data + size]);
                at = data + size + 2;
                if size == 0 {
                    break (body, at);
                }
            }
        } else {
            let length = header("Content-Length")
                .and_then(|v| v.parse().ok())
                .unwrap_or(0);
            if buffer.len() < start + length {
                return None;
            }
            (buffer[start..start + length].to_vec(), start + length)
        };
    buffer.drain(..used);
    Some(Ok(if protocol.starts_with("EVENT") {
        Message::Event(body)
    } else {
        Message::Response(status, body)
    }))
}

/// The characteristics of an `EVENT` body.
fn events_of(body: &[u8]) -> Vec<Event> {
    let Ok(json) = serde_json::from_slice::<Json>(body) else {
        return Vec::new();
    };
    json["characteristics"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|c| {
            Some(Event {
                aid: c["aid"].as_u64()?,
                iid: c["iid"].as_u64()?,
                value: c.get("value").cloned().unwrap_or(Json::Null),
            })
        })
        .collect()
}

impl Connection {
    pub async fn open(address: &str) -> Result<Self, Error> {
        let stream = tokio::time::timeout(ANSWER, TcpStream::connect(address))
            .await
            .map_err(|_| Error::Timeout)??;
        stream.set_nodelay(true)?;
        Ok(Self {
            stream,
            channel: None,
            raw: Vec::new(),
            plain: Vec::new(),
            events: VecDeque::new(),
        })
    }

    async fn read_message(&mut self) -> Result<Message, Error> {
        loop {
            if let Some(message) = take_message(&mut self.plain) {
                return message;
            }
            let mut chunk = [0u8; 4096];
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
        }
    }

    /// One request, its response; events read meanwhile wait their turn.
    async fn call(
        &mut self,
        method: &str,
        path: &str,
        content_type: &str,
        body: &[u8],
        wait: Duration,
    ) -> Result<(u16, Vec<u8>), Error> {
        let mut request = format!(
            "{method} {path} HTTP/1.1\r\nHost: accessory\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\n\r\n",
            body.len()
        )
        .into_bytes();
        request.extend_from_slice(body);
        if let Some(channel) = self.channel.as_mut() {
            request = channel.encrypt(&request);
        }
        self.stream.write_all(&request).await?;
        let deadline = tokio::time::Instant::now() + wait;
        loop {
            match tokio::time::timeout_at(deadline, self.read_message())
                .await
                .map_err(|_| Error::Timeout)??
            {
                Message::Response(status, body) => return Ok((status, body)),
                Message::Event(body) => self.events.extend(events_of(&body)),
            }
        }
    }

    async fn tlv(&mut self, path: &str, request: &Tlv, wait: Duration) -> Result<Tlv, Error> {
        let (status, body) = self
            .call(
                "POST",
                path,
                "application/pairing+tlv8",
                &request.encode(),
                wait,
            )
            .await?;
        let answer = Tlv::parse(&body).ok_or(Error::Protocol)?;
        if let Some(e) = answer.byte(tlv::ERROR) {
            return Err(Error::Refused(e));
        }
        if status != 200 {
            return Err(Error::Status(status));
        }
        Ok(answer)
    }

    /// A JSON request on the encrypted session.
    pub async fn json(
        &mut self,
        method: &str,
        path: &str,
        body: Option<&Json>,
    ) -> Result<Json, Error> {
        let bytes = body
            .map(|b| serde_json::to_vec(b).expect("JSON"))
            .unwrap_or_default();
        let (status, body) = self
            .call(method, path, "application/hap+json", &bytes, ANSWER)
            .await?;
        if !(200..300).contains(&status) {
            return Err(Error::Status(status));
        }
        if body.is_empty() {
            return Ok(Json::Null);
        }
        serde_json::from_slice(&body).map_err(|_| Error::Protocol)
    }

    /// The address of Moli on the link to the accessory (where it sends
    /// a stream back).
    pub fn local_ip(&self) -> Result<std::net::IpAddr, Error> {
        Ok(self.stream.local_addr()?.ip())
    }

    /// The accessory's own address.
    pub fn peer_ip(&self) -> Result<std::net::IpAddr, Error> {
        Ok(self.stream.peer_addr()?.ip())
    }

    /// Writes a TLV characteristic (base64), then reads it back: how a
    /// camera answers `SetupEndpoints`.
    pub async fn write_tlv(
        &mut self,
        aid: u64,
        iid: u64,
        value: &[u8],
        read_back: bool,
    ) -> Result<Vec<u8>, Error> {
        let encoded = crate::camera::b64(value);
        self.json(
            "PUT",
            "/characteristics",
            Some(&serde_json::json!({ "characteristics": [{ "aid": aid, "iid": iid, "value": encoded }] })),
        )
        .await?;
        if !read_back {
            return Ok(Vec::new());
        }
        let answer = self.read(&[(aid, iid)]).await?;
        let text = answer["characteristics"][0]["value"]
            .as_str()
            .ok_or(Error::Protocol)?;
        crate::camera::unb64(text).ok_or(Error::Protocol)
    }

    /// A still image of a camera (`POST /resource`), JPEG.
    pub async fn snapshot(&mut self, aid: u64, width: u32, height: u32) -> Result<Vec<u8>, Error> {
        let body = serde_json::json!({
            "aid": aid,
            "resource-type": "image",
            "image-width": width,
            "image-height": height,
        });
        let (status, bytes) = self
            .call(
                "POST",
                "/resource",
                "application/hap+json",
                &serde_json::to_vec(&body).expect("JSON"),
                PAIR_ANSWER,
            )
            .await?;
        if status != 200 || bytes.is_empty() {
            return Err(Error::Status(status));
        }
        Ok(bytes)
    }

    /// The accessory database (`GET /accessories`).
    pub async fn accessories(&mut self) -> Result<Json, Error> {
        self.json("GET", "/accessories", None).await
    }

    /// Asks for the events of these characteristics.
    pub async fn subscribe(&mut self, ids: &[(u64, u64)]) -> Result<(), Error> {
        let list: Vec<Json> = ids
            .iter()
            .map(|(aid, iid)| serde_json::json!({ "aid": aid, "iid": iid, "ev": true }))
            .collect();
        self.json(
            "PUT",
            "/characteristics",
            Some(&serde_json::json!({ "characteristics": list })),
        )
        .await
        .map(drop)
    }

    /// Reads characteristics (`aid.iid` pairs), for a heartbeat or a refresh.
    pub async fn read(&mut self, ids: &[(u64, u64)]) -> Result<Json, Error> {
        let list: Vec<String> = ids.iter().map(|(a, i)| format!("{a}.{i}")).collect();
        self.json(
            "GET",
            &format!("/characteristics?id={}", list.join(",")),
            None,
        )
        .await
    }

    /// The next event the accessory pushes.
    pub async fn next_event(&mut self) -> Result<Event, Error> {
        loop {
            if let Some(event) = self.events.pop_front() {
                return Ok(event);
            }
            match self.read_message().await? {
                Message::Event(body) => self.events.extend(events_of(&body)),
                // A response nobody waits for (a late one): dropped.
                Message::Response(..) => {}
            }
        }
    }

    /// pair-setup with the setup code (`123-45-678` or `12345678`): the
    /// accessory must be free (removed from the Home app).
    #[allow(clippy::too_many_lines)] // the whole exchange, as the spec lists it
    pub async fn pair(&mut self, code: &str) -> Result<Pairing, Error> {
        let digits: String = code.chars().filter(char::is_ascii_digit).collect();
        if digits.len() != 8 {
            return Err(Error::Refused(tlv::ERR_AUTHENTICATION));
        }
        let code = format!("{}-{}-{}", &digits[..3], &digits[3..5], &digits[5..]);
        let m2 = self
            .tlv(
                "/pair-setup",
                &Tlv::new().with(tlv::STATE, [1]).with(tlv::METHOD, [0]),
                PAIR_ANSWER,
            )
            .await?;
        let salt = m2.get(tlv::SALT).ok_or(Error::Protocol)?.to_vec();
        let big_b = m2.get(tlv::PUBLIC_KEY).ok_or(Error::Protocol)?.to_vec();
        // SRP-6a, client side.
        let n = srp::modulus();
        let g = Uint::from_u64(5);
        let pad = |u: &Uint| u.to_be_bytes_padded(srp::KEY_LEN);
        let a = Uint::from_be_bytes(&random::<32>());
        let big_a = pad(&g.pow_mod(&a, &n));
        let x = Uint::from_be_bytes(&sha(&[
            &salt,
            &sha(&[format!("Pair-Setup:{code}").as_bytes()]),
        ]));
        let v = g.pow_mod(&x, &n);
        let k = Uint::from_be_bytes(&sha(&[&pad(&n), &pad(&g)]));
        let u = Uint::from_be_bytes(&sha(&[&big_a, &pad(&Uint::from_be_bytes(&big_b))]));
        let base = Uint::from_be_bytes(&big_b)
            .add(&n)
            .sub(&k.mul_mod(&v, &n))
            .rem(&n);
        let secret = base.pow_mod(&a.add(&u.mul(&x)), &n);
        let session = sha(&[&pad(&secret)]);
        let group: Vec<u8> = sha(&[&n.to_be_bytes()])
            .iter()
            .zip(sha(&[&[5u8]]))
            .map(|(p, q)| p ^ q)
            .collect();
        let m1 = sha(&[
            &group,
            &sha(&[b"Pair-Setup"]),
            &salt,
            &big_a,
            &big_b,
            &session,
        ]);
        let m4 = self
            .tlv(
                "/pair-setup",
                &Tlv::new()
                    .with(tlv::STATE, [3])
                    .with(tlv::PUBLIC_KEY, &big_a)
                    .with(tlv::PROOF, m1),
                PAIR_ANSWER,
            )
            .await?;
        if m4.get(tlv::PROOF) != Some(&sha(&[&big_a, &m1, &session])[..]) {
            return Err(Error::WrongAccessory);
        }
        // M5: Moli's identity, signed and sealed.
        let seed = random::<32>();
        let controller = new_controller_id();
        let key = Ed25519KeyPair::from_seed_unchecked(&seed).expect("a 32-byte seed");
        let encrypt_key = hkdf32(
            &session,
            "Pair-Setup-Encrypt-Salt",
            "Pair-Setup-Encrypt-Info",
        );
        let sign_x = hkdf32(
            &session,
            "Pair-Setup-Controller-Sign-Salt",
            "Pair-Setup-Controller-Sign-Info",
        );
        let ltpk = key.public_key().as_ref().to_vec();
        let mut signed = sign_x.to_vec();
        signed.extend_from_slice(controller.as_bytes());
        signed.extend_from_slice(&ltpk);
        let inner = Tlv::new()
            .with(tlv::IDENTIFIER, controller.as_bytes())
            .with(tlv::PUBLIC_KEY, &ltpk)
            .with(tlv::SIGNATURE, key.sign(&signed).as_ref());
        let sealed = crypto::seal(&encrypt_key, message_tag("PS-Msg05"), &[], &inner.encode());
        let m6 = self
            .tlv(
                "/pair-setup",
                &Tlv::new()
                    .with(tlv::STATE, [5])
                    .with(tlv::ENCRYPTED_DATA, sealed),
                PAIR_ANSWER,
            )
            .await?;
        let opened = crypto::open(
            &encrypt_key,
            message_tag("PS-Msg06"),
            &[],
            m6.get(tlv::ENCRYPTED_DATA).ok_or(Error::Protocol)?,
        )
        .ok_or(Error::WrongAccessory)?;
        let inner = Tlv::parse(&opened).ok_or(Error::Protocol)?;
        let accessory = inner.get(tlv::IDENTIFIER).ok_or(Error::Protocol)?;
        let accessory_key = inner.get(tlv::PUBLIC_KEY).ok_or(Error::Protocol)?;
        let accessory_x = hkdf32(
            &session,
            "Pair-Setup-Accessory-Sign-Salt",
            "Pair-Setup-Accessory-Sign-Info",
        );
        let mut signed = accessory_x.to_vec();
        signed.extend_from_slice(accessory);
        signed.extend_from_slice(accessory_key);
        SignatureKey::new(&ED25519, accessory_key)
            .verify(&signed, inner.get(tlv::SIGNATURE).ok_or(Error::Protocol)?)
            .map_err(|_| Error::WrongAccessory)?;
        Ok(Pairing {
            controller,
            seed,
            accessory: String::from_utf8_lossy(accessory).into_owned(),
            accessory_key: accessory_key.to_vec(),
        })
    }

    /// pair-verify: both sides prove who they are, then the session is
    /// encrypted.
    pub async fn verify(&mut self, pairing: &Pairing) -> Result<(), Error> {
        let rng = SystemRandom::new();
        let ephemeral =
            EphemeralPrivateKey::generate(&X25519, &rng).map_err(|_| Error::Protocol)?;
        let public = ephemeral
            .compute_public_key()
            .map_err(|_| Error::Protocol)?
            .as_ref()
            .to_vec();
        let m2 = self
            .tlv(
                "/pair-verify",
                &Tlv::new()
                    .with(tlv::STATE, [1])
                    .with(tlv::PUBLIC_KEY, &public),
                ANSWER,
            )
            .await?;
        let accessory_public = m2.get(tlv::PUBLIC_KEY).ok_or(Error::Protocol)?.to_vec();
        let shared = agreement::agree_ephemeral(
            ephemeral,
            &UnparsedPublicKey::new(&X25519, &accessory_public),
            <[u8]>::to_vec,
        )
        .map_err(|_| Error::Protocol)?;
        let key = hkdf32(
            &shared,
            "Pair-Verify-Encrypt-Salt",
            "Pair-Verify-Encrypt-Info",
        );
        let opened = crypto::open(
            &key,
            message_tag("PV-Msg02"),
            &[],
            m2.get(tlv::ENCRYPTED_DATA).ok_or(Error::Protocol)?,
        )
        .ok_or(Error::WrongAccessory)?;
        let inner = Tlv::parse(&opened).ok_or(Error::Protocol)?;
        // The accessory is the one paired: its long-term key signs this.
        if inner.get(tlv::IDENTIFIER) != Some(pairing.accessory.as_bytes()) {
            return Err(Error::WrongAccessory);
        }
        let mut signed = accessory_public.clone();
        signed.extend_from_slice(pairing.accessory.as_bytes());
        signed.extend_from_slice(&public);
        SignatureKey::new(&ED25519, &pairing.accessory_key)
            .verify(&signed, inner.get(tlv::SIGNATURE).ok_or(Error::Protocol)?)
            .map_err(|_| Error::WrongAccessory)?;
        let mut signed = public.clone();
        signed.extend_from_slice(pairing.controller.as_bytes());
        signed.extend_from_slice(&accessory_public);
        let inner = Tlv::new()
            .with(tlv::IDENTIFIER, pairing.controller.as_bytes())
            .with(tlv::SIGNATURE, pairing.key().sign(&signed).as_ref());
        let sealed = crypto::seal(&key, message_tag("PV-Msg03"), &[], &inner.encode());
        self.tlv(
            "/pair-verify",
            &Tlv::new()
                .with(tlv::STATE, [3])
                .with(tlv::ENCRYPTED_DATA, sealed),
            ANSWER,
        )
        .await?;
        self.channel = Some(Channel::mirrored(&shared));
        Ok(())
    }
}

/// HomeKit's short type for a full UUID (`00000073-0000-1000-8000-…` → `73`).
#[must_use]
pub fn short_type(kind: &str) -> String {
    let head = kind
        .strip_suffix("-0000-1000-8000-0026BB765291")
        .unwrap_or(kind);
    head.trim_start_matches('0').to_uppercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pairings_round_trip_and_hide_their_keys() {
        let pairing = Pairing {
            controller: "AAAA".into(),
            seed: [7; 32],
            accessory: "AA:BB:CC:DD:EE:FF".into(),
            accessory_key: vec![9; 32],
        };
        assert_eq!(Pairing::decode(&pairing.encode()), Some(pairing.clone()));
        assert!(!format!("{pairing:?}").contains("0707"));
        assert_eq!(Pairing::decode("a|b|c"), None);
    }

    #[test]
    fn messages_are_cut_out_of_the_stream() {
        let event = br#"{"characteristics":[{"aid":1,"iid":10,"value":0}]}"#;
        let mut buffer = format!(
            "EVENT/1.0 200 OK\r\nContent-Type: application/hap+json\r\nContent-Length: {}\r\n\r\n",
            event.len()
        )
        .into_bytes();
        buffer.extend_from_slice(event);
        buffer.extend_from_slice(b"HTTP/1.1 204 No Content\r\n\r\n");
        let Some(Ok(Message::Event(body))) = take_message(&mut buffer) else {
            panic!("an event first");
        };
        assert_eq!(
            events_of(&body),
            vec![Event {
                aid: 1,
                iid: 10,
                value: Json::from(0)
            }]
        );
        assert!(matches!(
            take_message(&mut buffer),
            Some(Ok(Message::Response(204, _)))
        ));
        assert!(take_message(&mut buffer).is_none());
    }

    #[test]
    fn chunked_bodies_are_put_back_together() {
        let mut buffer =
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n2\r\nde\r\n0\r\n\r\n".to_vec();
        let Some(Ok(Message::Response(200, body))) = take_message(&mut buffer) else {
            panic!("a response");
        };
        assert_eq!(body, b"abcde");
        assert!(buffer.is_empty());
        let mut partial = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nab".to_vec();
        assert!(take_message(&mut partial).is_none());
    }

    #[test]
    fn types_are_shortened() {
        assert_eq!(short_type("00000073-0000-1000-8000-0026BB765291"), "73");
        assert_eq!(short_type("121"), "121");
    }
}
