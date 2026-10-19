//! One HAP connection: the pairing exchanges (`/pair-setup`, `/pair-verify`),
//! then, once the channel is encrypted, the accessory database and the
//! characteristics (read, write, notifications) and the pairings list.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use moli_core::{Origin, PointId, Value};
use moli_runtime::CommandError;
use ring::agreement::{self, EphemeralPrivateKey, UnparsedPublicKey, X25519};
use ring::rand::SystemRandom;
use ring::signature::{ED25519, KeyPair, UnparsedPublicKey as SignatureKey};
use serde_json::{Value as Json, json};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpStream;
use tokio_util::sync::CancellationToken;

use crate::accessories::{
    self, Bind, Characteristic, ColorPart, OK, ST_INVALID, ST_NOT_FOUND, ST_NOTIFY_UNSUPPORTED,
    ST_READ_ONLY, ST_WRITE_ONLY, hex_to_hs, hs_to_hex, mode_of_arm_target,
};
use crate::crypto::{self, Channel, hkdf32, message_tag};
use crate::http::{self, Parsed, Request, Response};
use crate::setup::{hex, unhex};
use crate::store::Pairing;
use crate::tlv::{self, Tlv};
use crate::{MAX_FAILURES, Shared, srp};

/// Simultaneous connections the bridge accepts.
pub const MAX_CONNECTIONS: usize = 24;
/// A connection that has not authenticated is dropped after this silence.
const IDLE_UNVERIFIED: Duration = Duration::from_secs(60);
/// The name the journal shows for orders that came through HomeKit.
const ACTOR: &str = "homekit";

/// Hue, saturation and the result slots of one colour point's writes.
type Colors = HashMap<PointId, (Option<f64>, Option<f64>, Vec<usize>)>;

enum Setup {
    Challenged(srp::Server),
    Proved([u8; 64]),
}

struct Verify {
    controller_pub: Vec<u8>,
    accessory_pub: Vec<u8>,
    shared: Vec<u8>,
    key: [u8; 32],
}

struct Conn {
    shared: Arc<Shared>,
    id: u64,
    channel: Option<Channel>,
    controller: Option<String>,
    /// The channel to switch to once the current response is written.
    upgrade: Option<(Channel, String)>,
    setup: Option<Setup>,
    verify: Option<Verify>,
    subscribed: HashSet<(u64, u64)>,
}

impl Drop for Conn {
    fn drop(&mut self) {
        let mut owner = self.shared.setup_owner.lock().expect("setup lock");
        if *owner == Some(self.id) {
            *owner = None;
        }
    }
}

pub(crate) async fn serve(shared: Arc<Shared>, stream: TcpStream, cancel: CancellationToken) {
    let _ = stream.set_nodelay(true);
    let id = shared.new_conn_id();
    let mut conn = Conn {
        shared,
        id,
        channel: None,
        controller: None,
        upgrade: None,
        setup: None,
        verify: None,
        subscribed: HashSet::new(),
    };
    conn.run(stream, cancel).await;
}

fn tlv_reply(state: u8) -> Tlv {
    Tlv::new().with(tlv::STATE, [state])
}

fn tlv_error(state: u8, error: u8) -> Response {
    Response::tlv(tlv_reply(state).with(tlv::ERROR, [error]).encode())
}

fn status_json(status: i32) -> Response {
    Response::json(470, &json!({ "status": status }))
}

impl Conn {
    async fn run(&mut self, mut stream: TcpStream, cancel: CancellationToken) {
        let mut updates = self.shared.updates.subscribe();
        let mut kicks = self.shared.kick.subscribe();
        let mut inbuf: Vec<u8> = Vec::new();
        let mut plain: Vec<u8> = Vec::new();
        let mut chunk = [0u8; 4096];
        let mut last_read = tokio::time::Instant::now();
        loop {
            let silent = async {
                if self.channel.is_none() {
                    tokio::time::sleep_until(last_read + IDLE_UNVERIFIED).await;
                } else {
                    std::future::pending::<()>().await;
                }
            };
            tokio::select! {
                () = cancel.cancelled() => return,
                () = silent => return,
                kick = kicks.recv() => match kick {
                    Ok(None) | Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
                    Ok(Some(who)) if self.controller.as_deref() == Some(who.as_str()) => return,
                    _ => {}
                },
                update = updates.recv() => {
                    let mut batch: HashSet<(u64, u64)> = HashSet::new();
                    match update {
                        Ok(pair) => { batch.insert(pair); }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                            batch.extend(self.subscribed.iter().copied());
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
                    }
                    while let Ok(pair) = updates.try_recv() {
                        batch.insert(pair);
                    }
                    if !self.send_events(&mut stream, &batch).await {
                        return;
                    }
                }
                read = stream.read(&mut chunk) => match read {
                    Ok(0) | Err(_) => return,
                    Ok(n) => {
                        last_read = tokio::time::Instant::now();
                        inbuf.extend_from_slice(&chunk[..n]);
                        if !self.pump(&mut stream, &mut inbuf, &mut plain).await {
                            return;
                        }
                    }
                }
            }
        }
    }

    /// Answers every complete request in the buffers; `false` drops the
    /// connection.
    async fn pump(
        &mut self,
        stream: &mut TcpStream,
        inbuf: &mut Vec<u8>,
        plain: &mut Vec<u8>,
    ) -> bool {
        loop {
            if let Some(channel) = self.channel.as_mut() {
                match channel.decrypt(inbuf) {
                    Ok(more) => plain.extend(more),
                    Err(()) => return false,
                }
            }
            let source: &mut Vec<u8> = if self.channel.is_some() { plain } else { inbuf };
            match http::parse(source) {
                Parsed::Incomplete => return true,
                Parsed::Bad => return false,
                Parsed::Done(request, used) => {
                    source.drain(..used);
                    let line = format!("{} {}?{}", request.method, request.path, request.query);
                    let body_len = request.body.len();
                    let response = self.handle(request).await;
                    // What the Home app asked and what it was told: the way to
                    // see where it gives up on an accessory. No body is logged.
                    tracing::info!(
                        conn = self.id,
                        request = %line,
                        body = body_len,
                        status = response.status,
                        answer = response.body.len(),
                        "homekit"
                    );
                    let mut bytes = response.encode();
                    if let Some(channel) = self.channel.as_mut() {
                        bytes = channel.encrypt(&bytes);
                    }
                    if stream.write_all(&bytes).await.is_err() {
                        return false;
                    }
                    if let Some((channel, controller)) = self.upgrade.take() {
                        self.channel = Some(channel);
                        self.controller = Some(controller);
                    }
                }
            }
        }
    }

    async fn send_events(&mut self, stream: &mut TcpStream, batch: &HashSet<(u64, u64)>) -> bool {
        if self.channel.is_none() || self.subscribed.is_empty() {
            return true;
        }
        let catalog = self.shared.catalog();
        let mut items = Vec::new();
        for &(aid, iid) in batch {
            if !self.subscribed.contains(&(aid, iid)) {
                continue;
            }
            let Some(c) = catalog.find(aid, iid) else {
                continue;
            };
            if let Ok(value) = self.shared.read(c) {
                items.push(json!({ "aid": aid, "iid": iid, "value": value }));
            }
        }
        if items.is_empty() {
            return true;
        }
        let message = http::event(&json!({ "characteristics": items }));
        let Some(channel) = self.channel.as_mut() else {
            return true;
        };
        stream.write_all(&channel.encrypt(&message)).await.is_ok()
    }

    async fn handle(&mut self, request: Request) -> Response {
        match (request.method.as_str(), request.path.as_str()) {
            ("POST", "/pair-setup") => self.pair_setup(&request.body),
            ("POST", "/pair-verify") => self.pair_verify(&request.body),
            ("POST", "/identify") if !self.shared.paired() => Response::empty(204),
            _ if self.channel.is_none() => status_json(-70401),
            ("GET", "/accessories") => self.accessories(),
            ("GET", "/characteristics") => self.read_characteristics(&request.query),
            ("PUT", "/characteristics") => self.write_characteristics(&request.body).await,
            ("POST", "/pairings") => self.pairings(&request.body),
            _ => Response::empty(404),
        }
    }

    // ---- pair-setup -----------------------------------------------------------

    fn pair_setup(&mut self, body: &[u8]) -> Response {
        let Some(request) = Tlv::parse(body) else {
            return tlv_error(2, tlv::ERR_UNKNOWN);
        };
        match request.byte(tlv::STATE) {
            Some(1) => self.setup_start(),
            Some(3) => self.setup_proof(&request),
            Some(5) => self.setup_exchange(&request),
            _ => tlv_error(2, tlv::ERR_UNKNOWN),
        }
    }

    fn setup_start(&mut self) -> Response {
        if self.shared.paired() {
            return tlv_error(2, tlv::ERR_UNAVAILABLE);
        }
        if self.shared.failures.load(Ordering::Relaxed) >= MAX_FAILURES {
            return tlv_error(2, tlv::ERR_MAX_TRIES);
        }
        {
            let mut owner = self.shared.setup_owner.lock().expect("setup lock");
            match *owner {
                Some(other) if other != self.id => return tlv_error(2, tlv::ERR_BUSY),
                _ => *owner = Some(self.id),
            }
        }
        let challenge = srp::Server::new(&self.shared.setup_code());
        let reply = tlv_reply(2)
            .with(tlv::PUBLIC_KEY, challenge.public_key())
            .with(tlv::SALT, challenge.salt());
        self.setup = Some(Setup::Challenged(challenge));
        Response::tlv(reply.encode())
    }

    fn setup_proof(&mut self, request: &Tlv) -> Response {
        let Some(Setup::Challenged(mut challenge)) = self.setup.take() else {
            return tlv_error(4, tlv::ERR_UNKNOWN);
        };
        let (Some(a), Some(m1)) = (request.get(tlv::PUBLIC_KEY), request.get(tlv::PROOF)) else {
            return tlv_error(4, tlv::ERR_UNKNOWN);
        };
        let Some(m2) = challenge.verify(a, m1) else {
            self.shared.failures.fetch_add(1, Ordering::Relaxed);
            self.release_setup();
            tracing::warn!("homekit: a pairing attempt gave the wrong setup code");
            return tlv_error(4, tlv::ERR_AUTHENTICATION);
        };
        let key = *challenge.session_key().expect("set by verify");
        self.setup = Some(Setup::Proved(key));
        Response::tlv(tlv_reply(4).with(tlv::PROOF, m2).encode())
    }

    fn setup_exchange(&mut self, request: &Tlv) -> Response {
        let Some(Setup::Proved(key)) = self.setup.take() else {
            return tlv_error(6, tlv::ERR_UNKNOWN);
        };
        self.release_setup();
        let session = hkdf32(&key, "Pair-Setup-Encrypt-Salt", "Pair-Setup-Encrypt-Info");
        let opened = request
            .get(tlv::ENCRYPTED_DATA)
            .and_then(|data| crypto::open(&session, message_tag("PS-Msg05"), &[], data))
            .and_then(|plain| Tlv::parse(&plain));
        let Some(inner) = opened else {
            return tlv_error(6, tlv::ERR_AUTHENTICATION);
        };
        let (Some(id), Some(ltpk), Some(signature)) = (
            inner.get(tlv::IDENTIFIER),
            inner.get(tlv::PUBLIC_KEY),
            inner.get(tlv::SIGNATURE),
        ) else {
            return tlv_error(6, tlv::ERR_UNKNOWN);
        };
        let controller_x = hkdf32(
            &key,
            "Pair-Setup-Controller-Sign-Salt",
            "Pair-Setup-Controller-Sign-Info",
        );
        let mut signed = controller_x.to_vec();
        signed.extend_from_slice(id);
        signed.extend_from_slice(ltpk);
        if SignatureKey::new(&ED25519, ltpk)
            .verify(&signed, signature)
            .is_err()
        {
            return tlv_error(6, tlv::ERR_AUTHENTICATION);
        }
        let Ok(id) = std::str::from_utf8(id) else {
            return tlv_error(6, tlv::ERR_UNKNOWN);
        };
        let added = self.shared.add_pairing(Pairing {
            id: id.to_owned(),
            ltpk: hex(ltpk),
            admin: true,
        });
        if !added {
            return tlv_error(6, tlv::ERR_UNKNOWN);
        }
        self.shared.failures.store(0, Ordering::Relaxed);
        tracing::info!("homekit: a controller is paired");
        let accessory_x = hkdf32(
            &key,
            "Pair-Setup-Accessory-Sign-Salt",
            "Pair-Setup-Accessory-Sign-Info",
        );
        let device_id = self.shared.device_id();
        let identity = self.shared.identity();
        let public = identity.public_key().as_ref().to_vec();
        let mut signed = accessory_x.to_vec();
        signed.extend_from_slice(device_id.as_bytes());
        signed.extend_from_slice(&public);
        let signature = identity.sign(&signed);
        let inner = Tlv::new()
            .with(tlv::IDENTIFIER, device_id.as_bytes())
            .with(tlv::PUBLIC_KEY, &public)
            .with(tlv::SIGNATURE, signature.as_ref());
        let sealed = crypto::seal(&session, message_tag("PS-Msg06"), &[], &inner.encode());
        Response::tlv(tlv_reply(6).with(tlv::ENCRYPTED_DATA, sealed).encode())
    }

    fn release_setup(&self) {
        let mut owner = self.shared.setup_owner.lock().expect("setup lock");
        if *owner == Some(self.id) {
            *owner = None;
        }
    }

    // ---- pair-verify ------------------------------------------------------------

    fn pair_verify(&mut self, body: &[u8]) -> Response {
        let Some(request) = Tlv::parse(body) else {
            return tlv_error(2, tlv::ERR_UNKNOWN);
        };
        match request.byte(tlv::STATE) {
            Some(1) => self.verify_start(&request),
            Some(3) => self.verify_finish(&request),
            _ => tlv_error(2, tlv::ERR_UNKNOWN),
        }
    }

    fn verify_start(&mut self, request: &Tlv) -> Response {
        let Some(controller_pub) = request.get(tlv::PUBLIC_KEY).filter(|k| k.len() == 32) else {
            return tlv_error(2, tlv::ERR_UNKNOWN);
        };
        let rng = SystemRandom::new();
        let Ok(ephemeral) = EphemeralPrivateKey::generate(&X25519, &rng) else {
            return tlv_error(2, tlv::ERR_UNKNOWN);
        };
        let Ok(accessory_pub) = ephemeral.compute_public_key().map(|k| k.as_ref().to_vec()) else {
            return tlv_error(2, tlv::ERR_UNKNOWN);
        };
        let Ok(shared) = agreement::agree_ephemeral(
            ephemeral,
            &UnparsedPublicKey::new(&X25519, controller_pub),
            <[u8]>::to_vec,
        ) else {
            return tlv_error(2, tlv::ERR_UNKNOWN);
        };
        let key = hkdf32(
            &shared,
            "Pair-Verify-Encrypt-Salt",
            "Pair-Verify-Encrypt-Info",
        );
        let device_id = self.shared.device_id();
        let mut signed = accessory_pub.clone();
        signed.extend_from_slice(device_id.as_bytes());
        signed.extend_from_slice(controller_pub);
        let signature = self.shared.identity().sign(&signed);
        let inner = Tlv::new()
            .with(tlv::IDENTIFIER, device_id.as_bytes())
            .with(tlv::SIGNATURE, signature.as_ref());
        let sealed = crypto::seal(&key, message_tag("PV-Msg02"), &[], &inner.encode());
        let reply = tlv_reply(2)
            .with(tlv::PUBLIC_KEY, &accessory_pub)
            .with(tlv::ENCRYPTED_DATA, sealed);
        self.verify = Some(Verify {
            controller_pub: controller_pub.to_vec(),
            accessory_pub,
            shared,
            key,
        });
        Response::tlv(reply.encode())
    }

    fn verify_finish(&mut self, request: &Tlv) -> Response {
        let Some(verify) = self.verify.take() else {
            return tlv_error(4, tlv::ERR_UNKNOWN);
        };
        let opened = request
            .get(tlv::ENCRYPTED_DATA)
            .and_then(|data| crypto::open(&verify.key, message_tag("PV-Msg03"), &[], data))
            .and_then(|plain| Tlv::parse(&plain));
        let Some(inner) = opened else {
            return tlv_error(4, tlv::ERR_AUTHENTICATION);
        };
        let (Some(id), Some(signature)) = (inner.get(tlv::IDENTIFIER), inner.get(tlv::SIGNATURE))
        else {
            return tlv_error(4, tlv::ERR_AUTHENTICATION);
        };
        let pairing = std::str::from_utf8(id)
            .ok()
            .and_then(|id| self.shared.pairing(id));
        let Some((pairing, ltpk)) = pairing.and_then(|p| unhex(&p.ltpk).map(|k| (p, k))) else {
            return tlv_error(4, tlv::ERR_AUTHENTICATION);
        };
        let mut signed = verify.controller_pub.clone();
        signed.extend_from_slice(id);
        signed.extend_from_slice(&verify.accessory_pub);
        if SignatureKey::new(&ED25519, &ltpk)
            .verify(&signed, signature)
            .is_err()
        {
            return tlv_error(4, tlv::ERR_AUTHENTICATION);
        }
        self.upgrade = Some((Channel::new(&verify.shared), pairing.id));
        Response::tlv(tlv_reply(4).encode())
    }

    // ---- pairings ---------------------------------------------------------------

    fn pairings(&mut self, body: &[u8]) -> Response {
        let admin = self
            .controller
            .as_deref()
            .and_then(|id| self.shared.pairing(id))
            .is_some_and(|p| p.admin);
        if !admin {
            return tlv_error(2, tlv::ERR_AUTHENTICATION);
        }
        let Some(request) = Tlv::parse(body).filter(|t| t.byte(tlv::STATE) == Some(1)) else {
            return tlv_error(2, tlv::ERR_UNKNOWN);
        };
        let identifier = request
            .get(tlv::IDENTIFIER)
            .and_then(|id| std::str::from_utf8(id).ok());
        match (request.byte(tlv::METHOD), identifier) {
            // Add
            (Some(3), Some(id)) => {
                let Some(ltpk) = request.get(tlv::PUBLIC_KEY).filter(|k| k.len() == 32) else {
                    return tlv_error(2, tlv::ERR_UNKNOWN);
                };
                let admin = request.byte(tlv::PERMISSIONS) == Some(1);
                let added = self.shared.add_pairing(Pairing {
                    id: id.to_owned(),
                    ltpk: hex(ltpk),
                    admin,
                });
                if added {
                    Response::tlv(tlv_reply(2).encode())
                } else {
                    tlv_error(2, tlv::ERR_UNKNOWN)
                }
            }
            // Remove
            (Some(4), Some(id)) => {
                self.shared.remove_pairing(id);
                Response::tlv(tlv_reply(2).encode())
            }
            // List
            (Some(5), _) => {
                let mut reply = tlv_reply(2);
                for (i, p) in self.shared.pairings().iter().enumerate() {
                    if i > 0 {
                        reply = reply.with(tlv::SEPARATOR, []);
                    }
                    reply = reply
                        .with(tlv::IDENTIFIER, p.id.as_bytes())
                        .with(tlv::PUBLIC_KEY, unhex(&p.ltpk).unwrap_or_default())
                        .with(tlv::PERMISSIONS, [u8::from(p.admin)]);
                }
                Response::tlv(reply.encode())
            }
            _ => tlv_error(2, tlv::ERR_UNKNOWN),
        }
    }

    // ---- accessories and characteristics ------------------------------------------

    fn accessories(&self) -> Response {
        let catalog = self.shared.catalog();
        let shared = &self.shared;
        let body = catalog.render(&|c: &Characteristic| {
            if c.perms.contains(&"pr") {
                // A readable characteristic always has a value in the
                // database (the Home app rejects one without); a device
                // that does not answer shows a neutral value until it does.
                Some(
                    shared
                        .read(c)
                        .unwrap_or_else(|_| accessories::placeholder(c)),
                )
            } else {
                None
            }
        });
        Response::json(200, &body)
    }

    fn read_characteristics(&self, query: &str) -> Response {
        let Some(ids) = http::parse_ids(query) else {
            return Response::empty(400);
        };
        let (meta, perms, kind, events) = (
            http::flag(query, "meta"),
            http::flag(query, "perms"),
            http::flag(query, "type"),
            http::flag(query, "ev"),
        );
        let catalog = self.shared.catalog();
        let mut failed = false;
        let mut items: Vec<Json> = Vec::new();
        for (aid, iid) in ids {
            let outcome = match catalog.find(aid, iid) {
                None => Err(ST_NOT_FOUND),
                Some(c) if !c.perms.contains(&"pr") => Err(ST_WRITE_ONLY),
                Some(c) => self.shared.read(c).map(|v| (c, v)),
            };
            match outcome {
                Ok((c, value)) => {
                    let mut item = json!({ "aid": aid, "iid": iid, "value": value, "status": 0 });
                    let map = item.as_object_mut().expect("object");
                    if perms {
                        map.insert("perms".into(), json!(c.perms));
                    }
                    if kind {
                        map.insert("type".into(), json!(c.kind));
                    }
                    if events {
                        map.insert("ev".into(), json!(self.subscribed.contains(&(aid, iid))));
                    }
                    if meta {
                        map.insert("format".into(), json!(c.format));
                        if let Some(u) = c.meta.unit {
                            map.insert("unit".into(), json!(u));
                        }
                        if let Some(v) = c.meta.min {
                            map.insert("minValue".into(), json!(v));
                        }
                        if let Some(v) = c.meta.max {
                            map.insert("maxValue".into(), json!(v));
                        }
                        if let Some(v) = c.meta.step {
                            map.insert("minStep".into(), json!(v));
                        }
                    }
                    items.push(item);
                }
                Err(status) => {
                    failed = true;
                    items.push(json!({ "aid": aid, "iid": iid, "status": status }));
                }
            }
        }
        if failed {
            Response::json(207, &json!({ "characteristics": items }))
        } else {
            // Without errors the status members are noise.
            for item in &mut items {
                item.as_object_mut().expect("object").remove("status");
            }
            Response::json(200, &json!({ "characteristics": items }))
        }
    }

    async fn write_characteristics(&mut self, body: &[u8]) -> Response {
        let Ok(parsed) = serde_json::from_slice::<Json>(body) else {
            return Response::empty(400);
        };
        let Some(entries) = parsed["characteristics"].as_array() else {
            return Response::empty(400);
        };
        let catalog = self.shared.catalog();
        let mut results: Vec<(u64, u64, i32)> = Vec::new();
        // Hue and saturation arrive as two writes of one colour.
        let mut colors: Colors = HashMap::new();
        for entry in entries {
            let (Some(aid), Some(iid)) = (entry["aid"].as_u64(), entry["iid"].as_u64()) else {
                return Response::empty(400);
            };
            let index = results.len();
            let Some(c) = catalog.find(aid, iid) else {
                results.push((aid, iid, ST_NOT_FOUND));
                continue;
            };
            let mut status = OK;
            if let Some(subscribe) = entry["ev"].as_bool() {
                if !c.perms.contains(&"ev") {
                    status = ST_NOTIFY_UNSUPPORTED;
                } else if subscribe {
                    self.subscribed.insert((aid, iid));
                } else {
                    self.subscribed.remove(&(aid, iid));
                }
            }
            if let Some(value) = entry.get("value").filter(|v| !v.is_null()) {
                if !c.perms.contains(&"pw") {
                    status = ST_READ_ONLY;
                } else if status == OK {
                    status = self.apply(c, value, index, &mut colors).await;
                }
            }
            results.push((aid, iid, status));
        }
        for (point, (hue, saturation, indexes)) in colors {
            let (h0, s0) = match self.shared.hub.state(&point).map(|s| s.value) {
                Some(Value::Text(t)) => hex_to_hs(&t).unwrap_or((0.0, 0.0)),
                _ => (0.0, 0.0),
            };
            let hex = hs_to_hex(hue.unwrap_or(h0), saturation.unwrap_or(s0));
            let status = self.command(&point, Value::Text(hex.as_str().into())).await;
            for i in indexes {
                results[i].2 = status;
            }
        }
        if results.iter().all(|r| r.2 == OK) {
            return Response::empty(204);
        }
        let items: Vec<Json> = results
            .iter()
            .map(|(aid, iid, status)| json!({ "aid": aid, "iid": iid, "status": status }))
            .collect();
        Response::json(207, &json!({ "characteristics": items }))
    }

    /// One written value; the status to answer.
    async fn apply(
        &self,
        c: &Characteristic,
        value: &Json,
        index: usize,
        colors: &mut Colors,
    ) -> i32 {
        match &c.bind {
            Bind::Identify => OK,
            Bind::Static(_) | Bind::ArmCurrent { .. } => ST_READ_ONLY,
            Bind::Point(point, conv) => match accessories::from_hap(*conv, value) {
                Some(v) => self.command(point, v).await,
                None => ST_INVALID,
            },
            Bind::ArmTarget(point) => match value.as_i64().and_then(mode_of_arm_target) {
                Some(mode) => self.command(point, Value::Text(mode.into())).await,
                None => ST_INVALID,
            },
            Bind::Color(point, part) => {
                let Some(n) = value.as_f64().filter(|n| n.is_finite()) else {
                    return ST_INVALID;
                };
                let entry = colors.entry(point.clone()).or_default();
                match part {
                    ColorPart::Hue => entry.0 = Some(n),
                    ColorPart::Saturation => entry.1 = Some(n),
                }
                entry.2.push(index);
                OK
            }
        }
    }

    /// An order to the hub, as origin `Api`: the guard applies. A held
    /// order is accepted (it waits for a human in the dashboard) and the
    /// controllers are shown the unchanged value again.
    async fn command(&self, point: &PointId, value: Value) -> i32 {
        let result = self
            .shared
            .hub
            .command(point, value, Origin::Api, Some(ACTOR.to_owned()))
            .await;
        match result {
            Ok(()) => OK,
            Err(CommandError::NeedsApproval { .. }) => {
                self.shared.refresh(point);
                OK
            }
            Err(CommandError::Invalid(_)) => ST_INVALID,
            Err(CommandError::ReadOnly) => ST_READ_ONLY,
            Err(CommandError::UnknownPoint) => ST_NOT_FOUND,
            Err(e) => {
                tracing::debug!(error = %e, "homekit: an order failed");
                accessories::ST_COMMUNICATION
            }
        }
    }
}
