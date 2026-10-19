//! The bridge against a controller of our own making: a fake house (a lamp,
//! a thermometer, an alarm panel), a real listener on a free port, and a
//! HomeKit controller written here (SRP client, Ed25519, X25519, the
//! encrypted channel). Pairing with the right and the wrong code, reading
//! the database, an order that goes through, an order the guard holds,
//! choosing devices.

use std::sync::Arc;
use std::time::Duration;

use moli_core::{
    Access, Device, DeviceId, InstanceId, Kind, Origin, PointId, PointSpec, Semantic, Value,
};
use moli_runtime::{BoxFuture, Driver, DriverCtx, Hub, HubOptions, MasterKey, spawn_driver};
use ring::agreement::{self, EphemeralPrivateKey, UnparsedPublicKey, X25519};
use ring::digest::{self, SHA512};
use ring::rand::{SecureRandom, SystemRandom};
use ring::signature::{ED25519, Ed25519KeyPair, KeyPair, UnparsedPublicKey as SignatureKey};
use serde_json::{Value as Json, json};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpStream;
use tokio_util::sync::CancellationToken;

use crate::bigint::Uint;
use crate::crypto::{self, Channel, hkdf32, message_tag};
use crate::srp;
use crate::tlv::{self, Tlv};
use crate::{Config, HomeKit, Policy};

// ---- the house -----------------------------------------------------------

fn spec(key: &str, kind: Kind, write: bool, semantic: Semantic) -> PointSpec {
    PointSpec {
        key: key.into(),
        label: key.into(),
        kind,
        access: Access { read: true, write },
        unit: None,
        semantic,
    }
}

fn device(ctx: &DriverCtx, native: &str, points: Vec<PointSpec>) -> Device {
    Device {
        id: ctx.device_id(native),
        instance: ctx.instance().clone(),
        native_name: native.into(),
        manufacturer: None,
        model: None,
        description: None,
        native_room: None,
        members: Vec::new(),
        points,
    }
}

struct FakeHouse;

impl Driver for FakeHouse {
    fn kind(&self) -> &'static str {
        "fake"
    }

    fn run<'a>(&'a self, ctx: &'a mut DriverCtx) -> BoxFuture<'a, anyhow::Result<()>> {
        Box::pin(async move {
            let percent = Kind::Numeric {
                min: Some(0.0),
                max: Some(100.0),
                step: None,
            };
            ctx.upsert_device(device(
                ctx,
                "lamp",
                vec![
                    spec("on", Kind::Binary, true, Semantic::OnOff),
                    spec("brightness", percent, true, Semantic::Brightness),
                ],
            ));
            ctx.upsert_device(device(
                ctx,
                "thermo",
                vec![spec(
                    "temperature",
                    Kind::Numeric {
                        min: None,
                        max: None,
                        step: None,
                    },
                    false,
                    Semantic::Temperature,
                )],
            ));
            ctx.upsert_device(device(
                ctx,
                "alarm",
                vec![
                    spec("state", Kind::Text, false, Semantic::Other),
                    spec(
                        "mode",
                        Kind::Enum {
                            values: vec!["disarmed".into(), "partial".into(), "total".into()],
                        },
                        true,
                        Semantic::Control,
                    ),
                ],
            ));
            let (lamp, thermo, alarm) = (
                ctx.device_id("lamp"),
                ctx.device_id("thermo"),
                ctx.device_id("alarm"),
            );
            ctx.set_state(&lamp, "on", Value::Bool(false));
            ctx.set_state(&lamp, "brightness", Value::Float(30.0));
            ctx.set_state(&thermo, "temperature", Value::Float(21.5));
            ctx.set_state(&alarm, "state", Value::from("disarmed"));
            ctx.set_state(&alarm, "mode", Value::from("disarmed"));
            ctx.ready();
            while let Some(cmd) = ctx.next_command().await {
                ctx.set_state(&cmd.device.id, &cmd.key, cmd.value.clone());
                cmd.reply(Ok(()));
            }
            Ok(())
        })
    }
}

struct Bridge {
    hub: Hub,
    homekit: HomeKit,
    cancel: CancellationToken,
    dir: std::path::PathBuf,
}

impl Drop for Bridge {
    fn drop(&mut self) {
        self.cancel.cancel();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

async fn bridge(name: &str) -> Bridge {
    bridge_with(name, Arc::new(FakeHouse)).await
}

async fn bridge_with(name: &str, house: Arc<dyn Driver>) -> Bridge {
    let dir = std::env::temp_dir().join(format!("moli-homekit-it-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let hub = Hub::new(HubOptions {
        secrets: Some((dir.join("secrets.enc"), MasterKey::generate().unwrap())),
        guard: moli_runtime::guard::GuardPolicy {
            protected_rooms: vec!["Chambre".into()],
            quiet_hours: None,
            protect_unassigned: false,
            dashboard: moli_runtime::guard::Dashboard::Trusted,
        },
        ..HubOptions::default()
    })
    .unwrap();
    let cancel = CancellationToken::new();
    spawn_driver(&hub, InstanceId::from("fake"), house, cancel.clone());
    for _ in 0..200 {
        if hub.stats().drivers_running == 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let (homekit, _task) = crate::start(
        hub.clone(),
        &dir,
        Config {
            port: 0,
            announce: false,
            ..Config::default()
        },
        cancel.clone(),
    )
    .await
    .expect("the bridge starts");
    Bridge {
        hub,
        homekit,
        cancel,
        dir,
    }
}

// ---- a controller ----------------------------------------------------------

struct Client {
    stream: TcpStream,
    channel: Option<Channel>,
    raw: Vec<u8>,
    plain: Vec<u8>,
}

impl Client {
    async fn connect(port: u16) -> Self {
        Self {
            stream: TcpStream::connect(("127.0.0.1", port)).await.unwrap(),
            channel: None,
            raw: Vec::new(),
            plain: Vec::new(),
        }
    }

    /// One request, one response (events are not subscribed in these tests).
    async fn call(
        &mut self,
        method: &str,
        path: &str,
        content_type: &str,
        body: &[u8],
    ) -> (u16, Vec<u8>) {
        let mut request = format!(
            "{method} {path} HTTP/1.1\r\nHost: test\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\n\r\n",
            body.len()
        )
        .into_bytes();
        request.extend_from_slice(body);
        if let Some(channel) = self.channel.as_mut() {
            request = channel.encrypt(&request);
        }
        self.stream.write_all(&request).await.unwrap();
        loop {
            if let Some(end) = self.plain.windows(4).position(|w| w == b"\r\n\r\n") {
                let head = String::from_utf8_lossy(&self.plain[..end]).into_owned();
                let length: usize = head
                    .lines()
                    .find_map(|l| l.strip_prefix("Content-Length: "))
                    .map_or(0, |n| n.trim().parse().unwrap());
                if self.plain.len() >= end + 4 + length {
                    let status = head.split(' ').nth(1).unwrap().parse().unwrap();
                    let body = self.plain[end + 4..end + 4 + length].to_vec();
                    self.plain.drain(..end + 4 + length);
                    return (status, body);
                }
            }
            let mut chunk = [0u8; 4096];
            let n = tokio::time::timeout(Duration::from_secs(5), self.stream.read(&mut chunk))
                .await
                .expect("the bridge answers")
                .unwrap();
            assert!(n > 0, "the bridge closed the connection");
            self.raw.extend_from_slice(&chunk[..n]);
            match self.channel.as_mut() {
                Some(channel) => {
                    let opened = channel.decrypt(&mut self.raw).expect("frames authenticate");
                    self.plain.extend(opened);
                }
                None => self.plain.append(&mut self.raw),
            }
        }
    }

    async fn tlv(&mut self, path: &str, request: &Tlv) -> Tlv {
        let (status, body) = self
            .call("POST", path, "application/pairing+tlv8", &request.encode())
            .await;
        assert_eq!(status, 200);
        Tlv::parse(&body).unwrap()
    }

    async fn json(&mut self, method: &str, path: &str, body: &Json) -> (u16, Json) {
        let bytes = if body.is_null() {
            Vec::new()
        } else {
            serde_json::to_vec(body).unwrap()
        };
        let (status, body) = self
            .call(method, path, "application/hap+json", &bytes)
            .await;
        (status, serde_json::from_slice(&body).unwrap_or(Json::Null))
    }
}

fn sha(parts: &[&[u8]]) -> [u8; 64] {
    let mut ctx = digest::Context::new(&SHA512);
    for part in parts {
        ctx.update(part);
    }
    ctx.finish().as_ref().try_into().unwrap()
}

fn random<const N: usize>() -> [u8; N] {
    let mut out = [0u8; N];
    SystemRandom::new().fill(&mut out).unwrap();
    out
}

/// A controller that knows its pairing identifier and its signing key.
struct Controller {
    id: String,
    key: Ed25519KeyPair,
}

impl Controller {
    fn new(id: &str) -> Self {
        Self {
            id: id.to_owned(),
            key: Ed25519KeyPair::from_seed_unchecked(&random::<32>()).unwrap(),
        }
    }

    /// The whole of pair-setup; `Err(error code)` where the bridge refused.
    #[allow(clippy::too_many_lines)] // the whole exchange, as the spec lists it
    async fn pair(&self, client: &mut Client, code: &str) -> Result<(), u8> {
        let m2 = client
            .tlv(
                "/pair-setup",
                &Tlv::new().with(tlv::STATE, [1]).with(tlv::METHOD, [0]),
            )
            .await;
        if let Some(e) = m2.byte(tlv::ERROR) {
            return Err(e);
        }
        let (salt, big_b) = (
            m2.get(tlv::SALT).unwrap().to_vec(),
            m2.get(tlv::PUBLIC_KEY).unwrap().to_vec(),
        );
        // SRP, client side.
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
        let u = Uint::from_be_bytes(&sha(&[&big_a, &big_b]));
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
        let m4 = client
            .tlv(
                "/pair-setup",
                &Tlv::new()
                    .with(tlv::STATE, [3])
                    .with(tlv::PUBLIC_KEY, &big_a)
                    .with(tlv::PROOF, m1),
            )
            .await;
        if let Some(e) = m4.byte(tlv::ERROR) {
            return Err(e);
        }
        assert_eq!(m4.get(tlv::PROOF).unwrap(), sha(&[&big_a, &m1, &session]));
        // M5: our identity, signed, sealed.
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
        let ltpk = self.key.public_key().as_ref().to_vec();
        let mut signed = sign_x.to_vec();
        signed.extend_from_slice(self.id.as_bytes());
        signed.extend_from_slice(&ltpk);
        let inner = Tlv::new()
            .with(tlv::IDENTIFIER, self.id.as_bytes())
            .with(tlv::PUBLIC_KEY, &ltpk)
            .with(tlv::SIGNATURE, self.key.sign(&signed).as_ref());
        let sealed = crypto::seal(&encrypt_key, message_tag("PS-Msg05"), &[], &inner.encode());
        let m6 = client
            .tlv(
                "/pair-setup",
                &Tlv::new()
                    .with(tlv::STATE, [5])
                    .with(tlv::ENCRYPTED_DATA, sealed),
            )
            .await;
        if let Some(e) = m6.byte(tlv::ERROR) {
            return Err(e);
        }
        let opened = crypto::open(
            &encrypt_key,
            message_tag("PS-Msg06"),
            &[],
            m6.get(tlv::ENCRYPTED_DATA).unwrap(),
        )
        .expect("M6 authenticates");
        let inner = Tlv::parse(&opened).unwrap();
        // The bridge proves who it is: its signature checks against its key.
        let accessory_x = hkdf32(
            &session,
            "Pair-Setup-Accessory-Sign-Salt",
            "Pair-Setup-Accessory-Sign-Info",
        );
        let mut signed = accessory_x.to_vec();
        signed.extend_from_slice(inner.get(tlv::IDENTIFIER).unwrap());
        signed.extend_from_slice(inner.get(tlv::PUBLIC_KEY).unwrap());
        SignatureKey::new(&ED25519, inner.get(tlv::PUBLIC_KEY).unwrap())
            .verify(&signed, inner.get(tlv::SIGNATURE).unwrap())
            .expect("the bridge's signature");
        Ok(())
    }

    /// pair-verify, then the connection speaks the encrypted channel.
    async fn verify(&self, client: &mut Client) -> Result<(), u8> {
        let rng = SystemRandom::new();
        let ephemeral = EphemeralPrivateKey::generate(&X25519, &rng).unwrap();
        let public = ephemeral.compute_public_key().unwrap().as_ref().to_vec();
        let m2 = client
            .tlv(
                "/pair-verify",
                &Tlv::new()
                    .with(tlv::STATE, [1])
                    .with(tlv::PUBLIC_KEY, &public),
            )
            .await;
        let accessory_pub = m2.get(tlv::PUBLIC_KEY).unwrap().to_vec();
        let shared = agreement::agree_ephemeral(
            ephemeral,
            &UnparsedPublicKey::new(&X25519, &accessory_pub),
            <[u8]>::to_vec,
        )
        .unwrap();
        let key = hkdf32(
            &shared,
            "Pair-Verify-Encrypt-Salt",
            "Pair-Verify-Encrypt-Info",
        );
        let opened = crypto::open(
            &key,
            message_tag("PV-Msg02"),
            &[],
            m2.get(tlv::ENCRYPTED_DATA).unwrap(),
        )
        .unwrap();
        assert!(Tlv::parse(&opened).unwrap().get(tlv::SIGNATURE).is_some());
        let mut signed = public.clone();
        signed.extend_from_slice(self.id.as_bytes());
        signed.extend_from_slice(&accessory_pub);
        let inner = Tlv::new()
            .with(tlv::IDENTIFIER, self.id.as_bytes())
            .with(tlv::SIGNATURE, self.key.sign(&signed).as_ref());
        let sealed = crypto::seal(&key, message_tag("PV-Msg03"), &[], &inner.encode());
        let m4 = client
            .tlv(
                "/pair-verify",
                &Tlv::new()
                    .with(tlv::STATE, [3])
                    .with(tlv::ENCRYPTED_DATA, sealed),
            )
            .await;
        if let Some(e) = m4.byte(tlv::ERROR) {
            return Err(e);
        }
        client.channel = Some(Channel::mirrored(&shared));
        Ok(())
    }
}

/// The `(aid, iid)` of the characteristic of `kind` in the accessory whose
/// serial number is `serial`.
fn find(db: &Json, serial: &str, kind: &str) -> (u64, u64) {
    for accessory in db["accessories"].as_array().unwrap() {
        let has_serial = accessory["services"][0]["characteristics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["type"] == "30" && c["value"] == serial);
        if !has_serial {
            continue;
        }
        for service in accessory["services"].as_array().unwrap() {
            for c in service["characteristics"].as_array().unwrap() {
                if c["type"] == kind {
                    return (
                        accessory["aid"].as_u64().unwrap(),
                        c["iid"].as_u64().unwrap(),
                    );
                }
            }
        }
    }
    panic!("no {kind} on {serial}");
}

async fn wait_for(what: &str, mut ready: impl FnMut() -> bool) {
    for _ in 0..200 {
        if ready() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("never happened: {what}");
}

async fn count(s: &mut Client) -> usize {
    let (_, db) = s.json("GET", "/accessories", &Json::Null).await;
    db["accessories"].as_array().unwrap().len()
}

// ---- the tests ---------------------------------------------------------------

#[tokio::test]
async fn pairing_needs_the_right_code_and_happens_once() {
    let b = bridge("pairing").await;
    let code = b.homekit.status(true).setup_code.unwrap();
    assert_eq!(
        b.homekit.status(false).setup_code,
        None,
        "the code is not shown to everyone"
    );
    let phone = Controller::new("11111111-2222-3333-4444-555555555555");

    // A wrong code is refused, and does not pair anyone.
    let mut client = Client::connect(b.homekit.port()).await;
    assert_eq!(
        phone.pair(&mut client, "000-00-000").await,
        Err(tlv::ERR_AUTHENTICATION)
    );
    assert_eq!(b.homekit.status(false).controllers, 0);

    // The right one pairs.
    let mut client = Client::connect(b.homekit.port()).await;
    phone.pair(&mut client, &code).await.expect("paired");
    assert_eq!(b.homekit.status(false).controllers, 1);

    // Paired: no second pairing, and an unknown phone cannot verify.
    let mut other = Client::connect(b.homekit.port()).await;
    let stranger = Controller::new("99999999-0000-0000-0000-000000000000");
    assert_eq!(
        stranger.pair(&mut other, &code).await,
        Err(tlv::ERR_UNAVAILABLE)
    );
    let mut other = Client::connect(b.homekit.port()).await;
    assert_eq!(
        stranger.verify(&mut other).await,
        Err(tlv::ERR_AUTHENTICATION)
    );

    // Without verification nothing is served.
    let mut plain = Client::connect(b.homekit.port()).await;
    let (status, _) = plain
        .call("GET", "/accessories", "application/hap+json", b"")
        .await;
    assert_eq!(status, 470);

    // The paired phone verifies and reads the database.
    let mut session = Client::connect(b.homekit.port()).await;
    phone.verify(&mut session).await.expect("verified");
    let (status, db) = session.json("GET", "/accessories", &Json::Null).await;
    assert_eq!(status, 200);
    assert_eq!(
        db["accessories"].as_array().unwrap().len(),
        4,
        "the bridge, a lamp, a thermometer, an alarm"
    );

    // Unpairing everyone drops the connection and allows a new pairing.
    b.homekit.forget_pairings();
    assert_eq!(b.homekit.status(false).controllers, 0);
    let mut again = Client::connect(b.homekit.port()).await;
    phone.pair(&mut again, &code).await.expect("paired again");
}

#[tokio::test]
#[allow(clippy::too_many_lines)] // one story: read, write, held, refused
async fn a_paired_phone_reads_writes_and_is_stopped_by_the_guard() {
    let b = bridge("session").await;
    let code = b.homekit.status(true).setup_code.unwrap();
    let phone = Controller::new("phone-1");
    let mut client = Client::connect(b.homekit.port()).await;
    phone.pair(&mut client, &code).await.unwrap();
    let mut s = Client::connect(b.homekit.port()).await;
    phone.verify(&mut s).await.unwrap();
    let (_, db) = s.json("GET", "/accessories", &Json::Null).await;

    // Reading: values come from the hub.
    let (aid, on) = find(&db, "fake:lamp", "25");
    let (_, brightness) = find(&db, "fake:lamp", "8");
    let (status, read) = s
        .json(
            "GET",
            &format!("/characteristics?id={aid}.{on},{aid}.{brightness}"),
            &Json::Null,
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(read["characteristics"][0]["value"], false);
    assert_eq!(read["characteristics"][1]["value"], 30);
    let (taid, temp) = find(&db, "fake:thermo", "11");
    let (_, read) = s
        .json(
            "GET",
            &format!("/characteristics?id={taid}.{temp}"),
            &Json::Null,
        )
        .await;
    assert_eq!(read["characteristics"][0]["value"], 21.5);
    // An unknown characteristic is a status, not a failure.
    let (status, read) = s
        .json("GET", "/characteristics?id=77.77", &Json::Null)
        .await;
    assert_eq!(status, 207);
    assert_eq!(read["characteristics"][0]["status"], -70409);

    // Writing: the lamp goes on, through the hub, journaled as the API.
    let (status, _) = s
        .json(
            "PUT",
            "/characteristics",
            &json!({"characteristics": [{"aid": aid, "iid": on, "value": true}]}),
        )
        .await;
    assert_eq!(status, 204);
    let lamp = PointId::new(&DeviceId::new(&InstanceId::from("fake"), "lamp"), "on");
    wait_for("the lamp is on", || {
        b.hub
            .state(&lamp)
            .is_some_and(|s| s.value == Value::Bool(true))
    })
    .await;
    let entry = b
        .hub
        .journal(5)
        .into_iter()
        .find(|e| e.origin == Origin::Api)
        .expect("journaled as api");
    assert_eq!(entry.actor.as_deref(), Some("homekit"));
    // Read-only and out-of-range writes are refused.
    let (status, bad) = s
        .json(
            "PUT",
            "/characteristics",
            &json!({"characteristics": [{"aid": taid, "iid": temp, "value": 5}]}),
        )
        .await;
    assert_eq!(status, 207);
    assert_eq!(bad["characteristics"][0]["status"], -70404);

    // The alarm: HomeKit may ask, the guard holds the order for a human.
    let (alarm_aid, target) = find(&db, "fake:alarm", "67");
    let (status, _) = s
        .json(
            "PUT",
            "/characteristics",
            &json!({"characteristics": [{"aid": alarm_aid, "iid": target, "value": 2}]}),
        )
        .await;
    assert_eq!(status, 204, "accepted, then shown unchanged");
    let mode = PointId::new(&DeviceId::new(&InstanceId::from("fake"), "alarm"), "mode");
    assert_eq!(
        b.hub.state(&mode).unwrap().value,
        Value::from("disarmed"),
        "not armed"
    );
    let held = b.hub.approvals();
    assert_eq!(held.len(), 1);
    assert_eq!(held[0].origin, Origin::Api);
    assert_eq!(
        held[0].value,
        Value::from("partial"),
        "Night is the panel's partial arming"
    );
    let (_, read) = s
        .json(
            "GET",
            &format!("/characteristics?id={alarm_aid}.{target}"),
            &Json::Null,
        )
        .await;
    assert_eq!(
        read["characteristics"][0]["value"], 3,
        "still disarmed for the phone"
    );
    // A value HomeKit does not offer for this panel is refused.
    let (status, _) = s
        .json(
            "PUT",
            "/characteristics",
            &json!({"characteristics": [{"aid": alarm_aid, "iid": target, "value": 0}]}),
        )
        .await;
    assert_eq!(status, 207);
}

#[tokio::test]
async fn devices_are_shown_or_hidden_one_by_one() {
    let b = bridge("choice").await;
    let code = b.homekit.status(true).setup_code.unwrap();
    let phone = Controller::new("phone-2");
    let mut client = Client::connect(b.homekit.port()).await;
    phone.pair(&mut client, &code).await.unwrap();
    let mut s = Client::connect(b.homekit.port()).await;
    phone.verify(&mut s).await.unwrap();

    assert_eq!(count(&mut s).await, 4);
    let config_before = b.homekit.status(false).exposed;
    assert_eq!(config_before, 3);

    // Nothing by default, except the lamp.
    b.homekit.set_policy(Policy::None);
    b.homekit.set_device("fake:lamp", Some(true));
    wait_for("the database follows", || {
        b.homekit.status(false).exposed == 1
    })
    .await;
    assert_eq!(count(&mut s).await, 2);
    let listed = b.homekit.devices();
    assert!(
        listed
            .iter()
            .any(|d| d.id == "fake:lamp" && d.exposed && d.exception)
    );
    assert!(listed.iter().any(|d| d.id == "fake:alarm" && !d.exposed));
    assert!(b.homekit.knows("fake:alarm"));
    assert!(!b.homekit.knows("fake:nothing"));

    // Everything again, except the alarm.
    b.homekit.set_policy(Policy::All);
    b.homekit.set_device("fake:alarm", Some(false));
    wait_for("the database follows", || {
        b.homekit.status(false).exposed == 2
    })
    .await;
    assert_eq!(count(&mut s).await, 3);
    // Back to the policy: it comes back.
    b.homekit.set_device("fake:alarm", None);
    wait_for("the alarm is back", || b.homekit.status(false).exposed == 3).await;
}

#[tokio::test]
async fn a_reset_makes_a_new_bridge() {
    let b = bridge("reset").await;
    let old_code = b.homekit.status(true).setup_code.unwrap();
    let phone = Controller::new("phone-3");
    let mut client = Client::connect(b.homekit.port()).await;
    phone.pair(&mut client, &old_code).await.unwrap();
    b.homekit.set_device("fake:alarm", Some(false));
    b.homekit.reset().await.unwrap();

    let status = b.homekit.status(true);
    assert_eq!(status.controllers, 0);
    assert_ne!(status.setup_code.unwrap(), old_code);
    // The old code no longer opens it; the old phone is a stranger.
    let mut again = Client::connect(b.homekit.port()).await;
    assert_eq!(
        phone.pair(&mut again, &old_code).await,
        Err(tlv::ERR_AUTHENTICATION)
    );
    let mut again = Client::connect(b.homekit.port()).await;
    assert_eq!(phone.verify(&mut again).await, Err(tlv::ERR_AUTHENTICATION));
    // The new code pairs, and the choice of devices survived.
    let new_code = b.homekit.status(true).setup_code.unwrap();
    let mut fresh = Client::connect(b.homekit.port()).await;
    Controller::new("phone-4")
        .pair(&mut fresh, &new_code)
        .await
        .unwrap();
    assert!(
        b.homekit
            .devices()
            .iter()
            .any(|d| d.id == "fake:alarm" && !d.exposed)
    );
}

// ---- cameras -----------------------------------------------------------------

const SPS: [u8; 4] = [0x67, 0x64, 0x00, 0x28];
const PPS: [u8; 3] = [0x68, 0xEB, 0xEC];

fn idr() -> Vec<u8> {
    std::iter::once(0x65)
        .chain((0..4000).map(|i| (i % 253) as u8))
        .collect()
}

/// A camera that sends a key frame, then small frames, every 10 ms.
#[derive(Debug)]
struct FakeLive;

struct FakeStream {
    info: moli_runtime::media::LiveInfo,
    n: u32,
}

impl moli_runtime::media::LiveStream for FakeStream {
    fn info(&self) -> &moli_runtime::media::LiveInfo {
        &self.info
    }

    fn next(&mut self) -> BoxFuture<'_, anyhow::Result<moli_runtime::media::LiveItem>> {
        use moli_runtime::media::{AudioChunk, LiveItem, VideoFrame};
        Box::pin(async move {
            tokio::time::sleep(Duration::from_millis(5)).await;
            let n = self.n;
            self.n += 1;
            // Every other item is 20 ms of a 1 kHz tone at 48 kHz.
            if n % 2 == 1 {
                let start = (n / 2) * 960;
                let samples = (0..960u32)
                    .map(|i| {
                        let t = f64::from(start + i) / 48_000.0;
                        (10_000.0 * (2.0 * std::f64::consts::PI * 1000.0 * t).sin()) as i16
                    })
                    .collect();
                return Ok(LiveItem::Audio(AudioChunk {
                    rate: 48_000,
                    samples,
                }));
            }
            let frame = n / 2;
            let key = frame.is_multiple_of(30);
            Ok(LiveItem::Video(VideoFrame {
                timestamp: 90_000 + frame * 3000,
                nals: if key {
                    vec![SPS.to_vec(), PPS.to_vec(), idr()]
                } else {
                    vec![
                        std::iter::once(0x41)
                            .chain([0x98])
                            .chain(std::iter::repeat_n(0x80, 198))
                            .collect(),
                    ]
                },
                keyframe: key,
            }))
        })
    }
}

impl moli_runtime::media::LiveSource for FakeLive {
    fn open(
        &self,
        _want: moli_runtime::media::LiveWant,
    ) -> BoxFuture<'_, anyhow::Result<Box<dyn moli_runtime::media::LiveStream>>> {
        Box::pin(async {
            Ok(Box::new(FakeStream {
                info: moli_runtime::media::LiveInfo {
                    width: 1280,
                    height: 720,
                    sps: SPS.to_vec(),
                    pps: PPS.to_vec(),
                    audio_rate: Some(48_000),
                },
                n: 0,
            }) as Box<dyn moli_runtime::media::LiveStream>)
        })
    }
}

#[derive(Debug)]
struct FakeJpeg;

impl moli_runtime::media::SnapshotSource for FakeJpeg {
    fn snapshot(&self) -> BoxFuture<'_, anyhow::Result<moli_runtime::media::Image>> {
        Box::pin(async {
            Ok(moli_runtime::media::Image {
                content_type: "image/jpeg".into(),
                bytes: vec![0xFF, 0xD8, 1, 2, 3, 0xFF, 0xD9],
            })
        })
    }
}

/// Two cameras (with a motion point each); one is in a protected room.
struct FakeCameras;

impl Driver for FakeCameras {
    fn kind(&self) -> &'static str {
        "fake"
    }

    fn run<'a>(&'a self, ctx: &'a mut DriverCtx) -> BoxFuture<'a, anyhow::Result<()>> {
        Box::pin(async move {
            for (native, room) in [("cam", None), ("camp", Some("Chambre"))] {
                let mut d = device(
                    ctx,
                    native,
                    vec![spec("motion", Kind::Binary, false, Semantic::Occupancy)],
                );
                d.native_room = room.map(Into::into);
                let id = d.id.clone();
                ctx.upsert_device(d);
                ctx.provide_live(&id, Arc::new(FakeLive));
                ctx.provide_snapshots(&id, Arc::new(FakeJpeg));
                ctx.set_state(&id, "motion", Value::Bool(false));
            }
            ctx.ready();
            while let Some(cmd) = ctx.next_command().await {
                cmd.reply(Ok(()));
            }
            Ok(())
        })
    }
}

fn b64_of(tlv: &Tlv) -> Json {
    json!(crate::camera::b64(&tlv.encode()))
}

fn le16(n: u16) -> [u8; 2] {
    n.to_le_bytes()
}

#[tokio::test]
#[allow(clippy::too_many_lines)] // one scenario, start to end
async fn a_phone_watches_a_camera_in_srtp_and_the_guard_keeps_protected_rooms_out() {
    let b = bridge_with("camera", Arc::new(FakeCameras)).await;
    let code = b.homekit.status(true).setup_code.unwrap();
    let phone = Controller::new("phone-cam");
    let mut client = Client::connect(b.homekit.port()).await;
    phone.pair(&mut client, &code).await.unwrap();
    let mut s = Client::connect(b.homekit.port()).await;
    phone.verify(&mut s).await.unwrap();
    wait_for("both cameras are known", || b.homekit.devices().len() == 4).await;

    // The dashboard lists the picture of each camera apart from its motion
    // sensor; the one in a protected room is marked, and never shown.
    let list = b.homekit.devices();
    let camera = |id: &str| list.iter().find(|d| d.id == id).unwrap();
    assert_eq!(camera("fake:cam#camera").kinds, ["camera"]);
    assert_eq!(camera("fake:cam").kinds, ["motion"]);
    assert!(!camera("fake:cam#camera").protected);
    assert!(camera("fake:camp#camera").protected);
    wait_for("the database is built", || {
        b.homekit.status(false).exposed == 3
    })
    .await;
    let (_, db) = s.json("GET", "/accessories", &Json::Null).await;
    let serials: Vec<&str> = db["accessories"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|a| a["services"][0]["characteristics"][4]["value"].as_str())
        .collect();
    assert!(serials.contains(&"fake:cam#camera"));
    assert!(
        serials.contains(&"fake:cam"),
        "its motion sensor stays apart"
    );
    assert!(
        serials.contains(&"fake:camp"),
        "the sensor of the protected one is a sensor"
    );
    assert!(
        !serials.contains(&"fake:camp#camera"),
        "its video never leaves"
    );

    // What the camera supports.
    let (aid, supported) = find(&db, "fake:cam#camera", "114");
    let (_, status_iid) = find(&db, "fake:cam#camera", "120");
    let (_, setup_iid) = find(&db, "fake:cam#camera", "118");
    let (_, select_iid) = find(&db, "fake:cam#camera", "117");
    let (_, read) = s
        .json(
            "GET",
            &format!("/characteristics?id={aid}.{supported}"),
            &Json::Null,
        )
        .await;
    let config = Tlv::parse(
        &crate::camera::unb64(read["characteristics"][0]["value"].as_str().unwrap()).unwrap(),
    )
    .unwrap();
    let config = Tlv::parse(config.get(1).unwrap()).unwrap();
    assert_eq!(config.byte(1), Some(0), "H.264");
    assert_eq!(config.all(3).count(), 5, "five picture sizes");

    // The phone sets a stream up: its address, its keys.
    let udp = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let port = udp.local_addr().unwrap().port();
    let session: [u8; 16] = random();
    let (key, salt): ([u8; 16], [u8; 14]) = (random(), random());
    let srtp = Tlv::new().with(1, [0]).with(2, key).with(3, salt);
    let audio_udp = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let (audio_key, audio_salt): ([u8; 16], [u8; 14]) = (random(), random());
    let audio_srtp = Tlv::new()
        .with(1, [0])
        .with(2, audio_key)
        .with(3, audio_salt);
    let setup = Tlv::new()
        .with(1, session)
        .with(
            3,
            Tlv::new()
                .with(1, [0])
                .with(2, "127.0.0.1")
                .with(3, le16(port))
                .with(4, le16(audio_udp.local_addr().unwrap().port()))
                .encode(),
        )
        .with(4, srtp.encode())
        .with(5, audio_srtp.encode());
    let (status, _) = s
        .json(
            "PUT",
            "/characteristics",
            &json!({"characteristics": [{"aid": aid, "iid": setup_iid, "value": b64_of(&setup)}]}),
        )
        .await;
    assert_eq!(status, 204);
    let (_, read) = s
        .json(
            "GET",
            &format!("/characteristics?id={aid}.{setup_iid}"),
            &Json::Null,
        )
        .await;
    let answer = Tlv::parse(
        &crate::camera::unb64(read["characteristics"][0]["value"].as_str().unwrap()).unwrap(),
    )
    .unwrap();
    assert_eq!(answer.get(1), Some(&session[..]));
    assert_eq!(answer.byte(2), Some(0), "accepted");
    let address = Tlv::parse(answer.get(3).unwrap()).unwrap();
    assert_eq!(address.get(2), Some(&b"127.0.0.1"[..]));
    let ssrc = u32::from_le_bytes(answer.get(6).unwrap().try_into().unwrap());
    assert_eq!(
        Tlv::parse(answer.get(4).unwrap()).unwrap().get(2),
        Some(&key[..])
    );

    // It starts the stream: 720p, payload type 99, a packet size.
    let select = Tlv::new()
        .with(1, Tlv::new().with(1, session).with(2, [1]).encode())
        .with(
            2,
            Tlv::new()
                .with(1, [0])
                .with(
                    3,
                    Tlv::new()
                        .with(1, le16(1280))
                        .with(2, le16(720))
                        .with(3, [30])
                        .encode(),
                )
                .with(
                    4,
                    Tlv::new()
                        .with(1, [99])
                        // The phone proposes another SSRC: the one this bridge announced wins.
                        .with(2, (ssrc ^ 0x00FF_00FF).to_le_bytes())
                        .with(3, le16(2000))
                        .with(4, 0.5f32.to_le_bytes())
                        .with(5, le16(1378))
                        .encode(),
                )
                .encode(),
        )
        .with(
            3,
            Tlv::new()
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
                        .with(1, [110])
                        .with(2, 0x0BAD_F00Du32.to_le_bytes())
                        .with(3, le16(24))
                        .with(4, 0.5f32.to_le_bytes())
                        .encode(),
                )
                .with(4, [0])
                .encode(),
        );
    let (status, _) = s
        .json(
            "PUT",
            "/characteristics",
            &json!({"characteristics": [{"aid": aid, "iid": select_iid, "value": b64_of(&select)}]}),
        )
        .await;
    assert_eq!(status, 204);

    // SRTP arrives; opened with the phone's key it is a decodable stream:
    // parameter sets, then the key frame cut to the packet size.
    let receiver = crate::srtp::Sender::new(&key, &salt);
    let mut depacketizer = moli_net::h264::Depacketizer::new();
    let mut frames = Vec::new();
    let (mut reports, mut markers) = (0, 0);
    let mut buf = vec![0u8; 2048];
    while frames.len() < 8 {
        let (n, _) = tokio::time::timeout(Duration::from_secs(5), udp.recv_from(&mut buf))
            .await
            .expect("the bridge sends video")
            .unwrap();
        if buf[1] == 200 {
            // The sender report that opens the stream (RTCP is not RTP).
            reports += 1;
            continue;
        }
        let clear = receiver.open_rtp(&buf[..n], 0).expect("the tag is valid");
        assert!(n <= 1378, "within the packet size the phone gave");
        assert_eq!(clear[1] & 0x7F, 99);
        assert_eq!(u32::from_be_bytes(clear[8..12].try_into().unwrap()), ssrc);
        markers += usize::from(clear[1] & 0x80 != 0);
        frames.extend(depacketizer.push(&clear));
    }
    assert_eq!(reports, 1, "a sender report comes first");

    // The sound: Opus in SRTP with the audio keys, on the audio port, under
    // the audio SSRC the bridge announced; it decodes to the tone.
    let audio_ssrc = u32::from_le_bytes(answer.get(7).unwrap().try_into().unwrap());
    let audio_receiver = crate::srtp::Sender::new(&audio_key, &audio_salt);
    let mut decoder = opus_rs::OpusDecoder::new(16_000, 1).unwrap();
    let mut samples_out: Vec<f32> = Vec::new();
    let mut heard = 0;
    while heard < 20 {
        let (n, _) = tokio::time::timeout(Duration::from_secs(5), audio_udp.recv_from(&mut buf))
            .await
            .expect("the bridge sends sound")
            .unwrap();
        if buf[1] == 200 {
            continue; // a sender report
        }
        let clear = audio_receiver
            .open_rtp(&buf[..n], 0)
            .expect("the audio tag is valid");
        assert_eq!(clear[1] & 0x7F, 110);
        assert_eq!(
            u32::from_be_bytes(clear[8..12].try_into().unwrap()),
            audio_ssrc
        );
        let mut pcm = vec![0f32; 320];
        let got = decoder.decode(&clear[12..], 320, &mut pcm).unwrap();
        samples_out.extend_from_slice(&pcm[..got]);
        heard += 1;
    }
    let peak = samples_out.iter().fold(0f32, |m, v| m.max(v.abs()));
    assert!(
        peak > 0.05 || peak > 1_500.0,
        "the tone is there: peak {peak}"
    );
    assert!(frames[0].keyframe, "starts on a key frame");
    assert_eq!(frames[0].nals, vec![SPS.to_vec(), PPS.to_vec(), idr()]);
    assert!(!frames[1].keyframe);
    // A stream of several frames: the clock only moves forward and every
    // frame is closed by a marker.
    assert!(
        frames
            .windows(2)
            .all(|w| w[1].timestamp.wrapping_sub(w[0].timestamp).cast_signed() > 0),
        "timestamps are monotonic"
    );
    assert!(markers + 1 >= frames.len(), "a marker per frame");

    // It is in use for the phone, a picture is available, and ending it
    // stops the video.
    let (_, read) = s
        .json(
            "GET",
            &format!("/characteristics?id={aid}.{status_iid}"),
            &Json::Null,
        )
        .await;
    let status = Tlv::parse(
        &crate::camera::unb64(read["characteristics"][0]["value"].as_str().unwrap()).unwrap(),
    )
    .unwrap();
    assert_eq!(status.byte(1), Some(1), "in use while it streams");
    let (code, jpeg) = s
        .call(
            "POST",
            "/resource",
            "application/hap+json",
            json!({"aid": aid, "resource-type": "image", "image-width": 320, "image-height": 180})
                .to_string()
                .as_bytes(),
        )
        .await;
    assert_eq!(code, 200);
    assert_eq!(&jpeg[..2], &[0xFF, 0xD8]);
    let end = Tlv::new().with(1, Tlv::new().with(1, session).with(2, [0]).encode());
    let (status, _) = s
        .json(
            "PUT",
            "/characteristics",
            &json!({"characteristics": [{"aid": aid, "iid": select_iid, "value": b64_of(&end)}]}),
        )
        .await;
    assert_eq!(status, 204);
    tokio::time::sleep(Duration::from_millis(200)).await;
    while udp.try_recv_from(&mut buf).is_ok() {}
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(udp.try_recv_from(&mut buf).is_err(), "no more video");

    // The switch of the dashboard removes the camera, not its sensor.
    b.homekit.set_device("fake:cam#camera", Some(false));
    wait_for("the camera leaves", || b.homekit.status(false).exposed == 2).await;
    let (_, db) = s.json("GET", "/accessories", &Json::Null).await;
    assert_eq!(db["accessories"].as_array().unwrap().len(), 3);
}
