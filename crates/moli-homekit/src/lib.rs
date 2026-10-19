//! A HomeKit bridge: Moli announces itself as a HAP (HomeKit Accessory
//! Protocol, over IP) bridge that people add in the Home app, and shows the
//! devices they chose, device by device.
//!
//! It is a *surface*, like the REST API and MCP, not a driver: it reads the
//! hub's devices and values, follows its events to push HAP notifications,
//! and sends orders back to the hub as origin `Api`, so Moli's guard applies
//! as to any other agent. An order the guard holds (the alarm, a protected
//! room, quiet hours) becomes an approval request in the dashboard; HomeKit
//! shows the unchanged state until a human approves.
//!
//! The setup code and the bridge's private key live in Moli's encrypted
//! store (`homekit/setup_code`, `homekit/identity_seed`); the controllers'
//! public keys, the per-device choices and the identifiers are in
//! `data/homekit.json`. Nothing here ever logs a code or a key.
//!
//! The pieces: `bigint` and `srp` (pair-setup's SRP-6a), `crypto` (HKDF,
//! ChaCha20-Poly1305, the encrypted channel), `tlv`, `http`, `session` (one
//! connection: pairing, verification, characteristics), `accessories` (the
//! mapping of devices to services), `mdns` (the `_hap._tcp` announcement),
//! `store` (what is kept on disk), `setup` (code, identifiers, QR payload),
//! `camera`, `rtp` and `srtp` (live video of cameras, relayed as SRTP).

// Wire formats and unit conversions: every cast is of a value bounded by
// construction (a clamp, a length check, a protocol limit), and the
// big-number and cryptographic code keeps the names of its specifications.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::many_single_char_names
)]

mod accessories;
mod bigint;
mod camera;
mod crypto;
mod http;
mod mdns;
mod rtp;
mod session;
mod setup;
mod srp;
mod srtp;
mod store;
mod tlv;

#[cfg(test)]
mod tests;

use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use moli_core::{DeviceId, Event, InstanceId, PointId};
use moli_runtime::Hub;
use ring::digest;
use ring::rand::{SecureRandom, SystemRandom};
use ring::signature::Ed25519KeyPair;
use serde::{Deserialize, Serialize};
use serde_json::Value as Json;
use tokio::sync::{Notify, broadcast, watch};
use tokio_util::sync::CancellationToken;

use accessories::{Aids, Catalog, Characteristic, Entry};
pub use store::Policy;
use store::{Pairing, Saved};

/// The secret store namespace of the bridge.
const NAMESPACE: &str = "homekit";
const SECRET_CODE: &str = "setup_code";
const SECRET_IDENTITY: &str = "identity_seed";

/// `[homekit]` in `moli.toml`. Present: the bridge runs.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// TCP port of the bridge.
    #[serde(default = "default_port")]
    pub port: u16,
    /// The bridge's name in the Home app.
    #[serde(default = "default_name")]
    pub name: String,
    /// The address announced on the network, when the machine's default
    /// route is not the right one.
    #[serde(default)]
    pub address: Option<IpAddr>,
    /// `false`: do not announce on the network (tests).
    #[serde(default = "yes")]
    pub announce: bool,
}

fn yes() -> bool {
    true
}

fn default_port() -> u16 {
    51827
}

fn default_name() -> String {
    "Moli".to_owned()
}

impl Default for Config {
    fn default() -> Self {
        Self {
            port: default_port(),
            name: default_name(),
            address: None,
            announce: true,
        }
    }
}

/// What the mDNS announcement says of the bridge's state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Announcement {
    pub config_number: u32,
    pub paired: bool,
    /// The bridge's pairing identifier (`AA:BB:CC:DD:EE:FF`).
    pub device_id: String,
    /// `sh` of the advertisement.
    pub setup_hash: String,
}

impl Announcement {
    fn of(saved: &Saved) -> Self {
        Self {
            config_number: saved.config_number,
            paired: saved.paired(),
            device_id: saved.device_id.clone(),
            setup_hash: setup::setup_hash(&saved.setup_id, &saved.device_id),
        }
    }
}

/// Everything the connections and the maintenance task share.
pub(crate) struct Shared {
    pub hub: Hub,
    pub config: Config,
    path: PathBuf,
    saved: Mutex<Saved>,
    last_written: Mutex<Vec<u8>>,
    catalog: RwLock<Arc<Catalog>>,
    identity: RwLock<Arc<Ed25519KeyPair>>,
    code: RwLock<String>,
    /// A characteristic whose value (maybe) changed: `(aid, iid)`.
    pub updates: broadcast::Sender<(u64, u64)>,
    /// A controller was removed (its identifier), or everyone (`None`).
    pub kick: broadcast::Sender<Option<String>>,
    pub announce: watch::Sender<Announcement>,
    rebuild: Notify,
    /// Camera streams being negotiated or relayed.
    pub(crate) streams: camera::Streams,
    /// The connection currently pairing, if any.
    pub setup_owner: Mutex<Option<u64>>,
    pub failures: AtomicU32,
    next_conn: AtomicU64,
    /// The port the listener got (the configured one, or the system's for 0).
    port: u16,
}

impl std::fmt::Debug for Shared {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Shared").finish_non_exhaustive()
    }
}

/// Pair-setup attempts allowed before the bridge refuses (HAP says 100).
pub(crate) const MAX_FAILURES: u32 = 100;

impl Shared {
    pub fn catalog(&self) -> Arc<Catalog> {
        self.catalog.read().expect("catalog lock").clone()
    }

    pub fn new_conn_id(&self) -> u64 {
        self.next_conn.fetch_add(1, Ordering::Relaxed)
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn setup_code(&self) -> String {
        self.code.read().expect("code lock").clone()
    }

    pub fn identity(&self) -> Arc<Ed25519KeyPair> {
        self.identity.read().expect("identity lock").clone()
    }

    pub fn device_id(&self) -> String {
        self.saved.lock().expect("saved lock").device_id.clone()
    }

    pub fn paired(&self) -> bool {
        self.saved.lock().expect("saved lock").paired()
    }

    pub fn pairing(&self, id: &str) -> Option<Pairing> {
        let saved = self.saved.lock().expect("saved lock");
        saved.pairings.iter().find(|p| p.id == id).cloned()
    }

    pub fn pairings(&self) -> Vec<Pairing> {
        self.saved.lock().expect("saved lock").pairings.clone()
    }

    /// Adds or updates a controller; `false` when the identifier is known
    /// with another key.
    pub fn add_pairing(&self, pairing: Pairing) -> bool {
        let mut saved = self.saved.lock().expect("saved lock");
        match saved.pairings.iter_mut().find(|p| p.id == pairing.id) {
            Some(existing) if existing.ltpk != pairing.ltpk => return false,
            Some(existing) => existing.admin = pairing.admin,
            None => saved.pairings.push(pairing),
        }
        self.persist(&saved);
        self.announce_from(&saved);
        true
    }

    pub fn remove_pairing(&self, id: &str) {
        let mut saved = self.saved.lock().expect("saved lock");
        saved.pairings.retain(|p| p.id != id);
        self.persist(&saved);
        self.announce_from(&saved);
        drop(saved);
        let _ = self.kick.send(Some(id.to_owned()));
    }

    fn announce_from(&self, saved: &Saved) {
        let next = Announcement::of(saved);
        self.announce.send_if_modified(|current| {
            let changed = *current != next;
            *current = next;
            changed
        });
    }

    fn persist(&self, saved: &Saved) {
        let bytes = serde_json::to_vec_pretty(saved).unwrap_or_default();
        let mut last = self.last_written.lock().expect("lock");
        if *last == bytes {
            return;
        }
        match saved.save(&self.path) {
            Ok(()) => *last = bytes,
            Err(e) => tracing::error!(error = %e, "homekit: cannot write homekit.json"),
        }
    }

    /// Pushes the current value of a Moli point to the controllers that
    /// watch the characteristics it feeds.
    pub fn refresh(&self, point: &PointId) {
        for pair in self.catalog().watching(point) {
            let _ = self.updates.send(*pair);
        }
    }

    // ---- values -------------------------------------------------------------

    fn state_text(&self, point: &PointId) -> Option<String> {
        match self.hub.state(point)?.value {
            moli_core::Value::Text(t) => Some(t.to_string()),
            _ => None,
        }
    }

    fn offline(&self, point: &PointId) -> bool {
        point
            .split()
            .and_then(|(device, _)| self.hub.device(&device))
            .is_none_or(|view| view.online == Some(false))
    }

    fn offline_device(&self, device: &DeviceId) -> bool {
        self.hub
            .device(device)
            .is_none_or(|view| view.online == Some(false))
    }

    /// The value of a characteristic, or a HAP status.
    pub fn read(&self, c: &Characteristic) -> Result<Json, i32> {
        use accessories::{
            Bind, ST_COMMUNICATION, ST_WRITE_ONLY, arm_current_of, arm_target_of, hex_to_hs, to_hap,
        };
        match &c.bind {
            Bind::Static(v) => Ok(v.clone()),
            Bind::Identify => Err(ST_WRITE_ONLY),
            Bind::Camera { device, what } => {
                use camera::CameraChar::{
                    Selected, Setup, Status, SupportedAudio, SupportedRtp, SupportedVideo,
                };
                let tlv = match what {
                    SupportedVideo => camera::supported_video(),
                    SupportedAudio => camera::supported_audio(),
                    SupportedRtp => camera::supported_rtp(),
                    // Read back on the connection that wrote them.
                    Selected | Setup => Vec::new(),
                    Status => {
                        if self.offline_device(device) {
                            return Err(ST_COMMUNICATION);
                        }
                        let unavailable = self.hub.protected_room(device).is_some();
                        camera::streaming_status(if unavailable {
                            2
                        } else {
                            self.streams.status_of(device)
                        })
                    }
                };
                Ok(Json::String(camera::b64(&tlv)))
            }
            Bind::Point(point, conv) => {
                if self.offline(point) {
                    return Err(ST_COMMUNICATION);
                }
                // A device that answers but has no value for this point (a
                // lamp in colour mode has no colour temperature) is not
                // unresponsive: the Home app would show the whole lamp so.
                Ok(self
                    .hub
                    .state(point)
                    .and_then(|sample| to_hap(*conv, &sample.value))
                    .unwrap_or_else(|| accessories::placeholder(c)))
            }
            Bind::Color(point, part) => {
                if self.offline(point) {
                    return Err(ST_COMMUNICATION);
                }
                // A lamp that never told its colour is white to HomeKit.
                let (hue, saturation) = self
                    .state_text(point)
                    .and_then(|text| hex_to_hs(&text))
                    .unwrap_or((0.0, 0.0));
                Ok(match part {
                    accessories::ColorPart::Hue => serde_json::json!(hue.round() as i64),
                    accessories::ColorPart::Saturation => {
                        serde_json::json!(saturation.round() as i64)
                    }
                })
            }
            Bind::ArmCurrent { state, mode } => {
                let anchor = state.as_ref().or(mode.as_ref()).ok_or(ST_COMMUNICATION)?;
                if self.offline(anchor) {
                    return Err(ST_COMMUNICATION);
                }
                let from_state = state
                    .as_ref()
                    .and_then(|p| self.state_text(p))
                    .and_then(|s| arm_current_of(&s));
                let from_mode = || {
                    mode.as_ref()
                        .and_then(|p| self.state_text(p))
                        .and_then(|m| arm_target_of(&m))
                };
                from_state
                    .or_else(from_mode)
                    .map(|n| serde_json::json!(n))
                    .ok_or(ST_COMMUNICATION)
            }
            Bind::ArmTarget(point) => {
                if self.offline(point) {
                    return Err(ST_COMMUNICATION);
                }
                self.state_text(point)
                    .and_then(|m| arm_target_of(&m))
                    .map(|n| serde_json::json!(n))
                    .ok_or(ST_COMMUNICATION)
            }
        }
    }

    // ---- the database ---------------------------------------------------------

    /// Rebuilds the accessory database from the hub and the choices; raises
    /// `c#` when its structure changed.
    fn rebuild_now(&self) {
        let snapshot = self.hub.snapshot();
        let mut saved = self.saved.lock().expect("saved lock");
        // Refresh the cache of what is exposed (the database must not shrink
        // while drivers are still starting), and forget what is not.
        for view in &snapshot.devices {
            let id = view.device.id.as_str();
            let live = self.hub.has_live(&view.device.id);
            if saved.exposed(id) && !accessories::kinds_of(&view.device).is_empty() {
                saved.devices.insert(
                    id.to_owned(),
                    Entry::new(view.device.clone(), &view.label, live),
                );
            }
            // Video of a protected room never leaves the house: no accessory.
            if live {
                let key = accessories::camera_key(&view.device.id);
                if saved.exposed(&key) && self.hub.protected_room(&view.device.id).is_none() {
                    saved
                        .devices
                        .insert(key, Entry::camera(view.device.clone(), &view.label));
                } else {
                    saved.devices.remove(&key);
                }
            }
        }
        let keep: Vec<String> = saved
            .devices
            .keys()
            .filter(|id| !saved.exposed(id))
            .cloned()
            .collect();
        for id in keep {
            saved.devices.remove(&id);
        }
        let entries: Vec<Entry> = saved.devices.values().cloned().collect();
        let mut aids = Aids {
            assigned: std::mem::take(&mut saved.aids),
            next: saved.next_aid,
        };
        let policy_saved = saved.clone();
        let catalog = Catalog::build(
            &self.config.name,
            &entries,
            |key| policy_saved.exposed(key),
            &mut aids,
        );
        saved.aids = aids.assigned;
        saved.next_aid = aids.next;
        let structure = digest::digest(&digest::SHA512, catalog.structure().to_string().as_bytes());
        let structure = setup::hex(&structure.as_ref()[..16]);
        if structure != saved.structure {
            saved.structure = structure;
            saved.config_number = saved.config_number.checked_add(1).unwrap_or(1);
            tracing::info!(
                accessories = catalog.accessories.len() - 1,
                config_number = saved.config_number,
                database = %catalog.structure(),
                "homekit: the accessory database changed"
            );
        }
        self.persist(&saved);
        self.announce_from(&saved);
        drop(saved);
        *self.catalog.write().expect("catalog lock") = Arc::new(catalog);
    }

    fn forget_device(&self, id: &DeviceId) {
        let mut saved = self.saved.lock().expect("saved lock");
        saved.devices.remove(id.as_str());
        saved.devices.remove(&accessories::camera_key(id));
    }
}

/// The running bridge, as the dashboard's API sees it.
#[derive(Clone, Debug)]
pub struct HomeKit(Arc<Shared>);

/// The bridge's state, for the settings page.
#[derive(Debug, Serialize)]
pub struct Status {
    pub name: String,
    pub port: u16,
    pub policy: Policy,
    /// People's phones (controllers) paired.
    pub controllers: usize,
    /// Devices shown in HomeKit now.
    pub exposed: usize,
    /// More devices were chosen than HomeKit takes.
    pub truncated: bool,
    /// `123-45-678`, only when `reveal` was asked for.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub setup_code: Option<String>,
    /// `X-HM://…`, what the QR code holds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub setup_uri: Option<String>,
}

/// One device in the settings list.
#[allow(clippy::struct_excessive_bools)] // flags the dashboard shows as they are
#[derive(Debug, Serialize)]
pub struct DeviceEntry {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub room: Option<String>,
    /// What HomeKit shows it as (`light`, `temperature`, `alarm`…).
    pub kinds: Vec<&'static str>,
    pub exposed: bool,
    /// The choice departs from the default policy.
    pub exception: bool,
    /// Not in the hub right now (its driver is down or starting).
    pub absent: bool,
    /// A camera of a protected room: never shown to HomeKit, whatever the choice.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub protected: bool,
}

impl HomeKit {
    /// The TCP port the bridge listens on.
    pub fn port(&self) -> u16 {
        self.0.port()
    }

    pub fn status(&self, reveal: bool) -> Status {
        let saved = self.0.saved.lock().expect("saved lock").clone();
        let catalog = self.0.catalog();
        Status {
            name: self.0.config.name.clone(),
            port: self.0.port,
            policy: saved.policy,
            controllers: saved.pairings.len(),
            exposed: catalog.accessories.len().saturating_sub(1),
            truncated: catalog.truncated,
            setup_code: reveal.then(|| self.0.setup_code()),
            setup_uri: reveal.then(|| {
                setup::uri(
                    &self.0.setup_code(),
                    &saved.setup_id,
                    setup::CATEGORY_BRIDGE,
                )
            }),
        }
    }

    /// Every device HomeKit has a place for, with whether it is shown.
    pub fn devices(&self) -> Vec<DeviceEntry> {
        let saved = self.0.saved.lock().expect("saved lock").clone();
        let snapshot = self.0.hub.snapshot();
        let mut out: Vec<DeviceEntry> = Vec::new();
        for view in &snapshot.devices {
            let kinds = accessories::kinds_of(&view.device);
            let live = self.0.hub.has_live(&view.device.id);
            let room = view
                .label
                .room
                .clone()
                .or_else(|| view.device.native_room.as_deref().map(str::to_owned));
            if !kinds.is_empty() {
                let id = view.device.id.as_str().to_owned();
                out.push(DeviceEntry {
                    name: Entry::new(view.device.clone(), &view.label, live).name,
                    room: room.clone(),
                    kinds: kinds.into_iter().map(accessories::Kinds::name).collect(),
                    exposed: saved.exposed(&id),
                    exception: saved.exceptions.contains_key(&id),
                    absent: false,
                    protected: false,
                    id,
                });
            }
            if live {
                let id = accessories::camera_key(&view.device.id);
                out.push(DeviceEntry {
                    name: Entry::camera(view.device.clone(), &view.label).name,
                    room,
                    kinds: vec![accessories::Kinds::Camera.name()],
                    exposed: saved.exposed(&id),
                    exception: saved.exceptions.contains_key(&id),
                    absent: false,
                    protected: self.0.hub.protected_room(&view.device.id).is_some(),
                    id,
                });
            }
        }
        for (id, entry) in &saved.devices {
            if out.iter().any(|d| &d.id == id) {
                continue;
            }
            out.push(DeviceEntry {
                id: id.clone(),
                name: entry.name.clone(),
                room: entry.device.native_room.as_deref().map(str::to_owned),
                kinds: if entry.camera {
                    vec![accessories::Kinds::Camera.name()]
                } else {
                    accessories::kinds_of(&entry.device)
                        .into_iter()
                        .map(accessories::Kinds::name)
                        .collect()
                },
                exposed: saved.exposed(id),
                exception: saved.exceptions.contains_key(id),
                absent: true,
                protected: false,
            });
        }
        out.sort_by(|a, b| {
            a.room
                .cmp(&b.room)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
                .then_with(|| a.id.cmp(&b.id))
        });
        out
    }

    /// The default for devices without a choice of their own.
    pub fn set_policy(&self, policy: Policy) {
        {
            let mut saved = self.0.saved.lock().expect("saved lock");
            saved.policy = policy;
            self.0.persist(&saved);
        }
        self.0.rebuild.notify_one();
    }

    /// A device's own choice; `None` goes back to the policy.
    pub fn set_device(&self, id: &str, exposed: Option<bool>) {
        {
            let mut saved = self.0.saved.lock().expect("saved lock");
            match exposed {
                Some(choice) => {
                    saved.exceptions.insert(id.to_owned(), choice);
                }
                None => {
                    saved.exceptions.remove(id);
                }
            }
            // A choice equal to the policy is no exception.
            let policy = saved.policy.exposes();
            saved.exceptions.retain(|_, v| *v != policy);
            self.0.persist(&saved);
        }
        self.0.rebuild.notify_one();
    }

    /// Whether a device the settings list knows is exposable (for the API
    /// to refuse unknown identifiers).
    pub fn knows(&self, id: &str) -> bool {
        self.devices().iter().any(|d| d.id == id)
    }

    /// A new bridge in the eyes of HomeKit: new identity key, pairing
    /// identifier, setup code and setup id; every phone is unpaired and
    /// the bridge can be added again, on any iPhone, as a new accessory. The
    /// choice of devices is kept.
    pub async fn reset(&self) -> Result<(), StartError> {
        let shared = &self.0;
        let instance = InstanceId::from(NAMESPACE);
        let code = setup::new_code();
        let seed = random_bytes::<32>();
        let identity = Ed25519KeyPair::from_seed_unchecked(&seed)
            .map_err(|_| StartError::Io("cannot make an identity key".to_owned()))?;
        for (name, value) in [
            (SECRET_CODE, code.clone()),
            (SECRET_IDENTITY, setup::hex(&seed)),
        ] {
            shared
                .hub
                .store_secret(&instance, name, &value)
                .await
                .map_err(|e| StartError::Io(e.to_string()))?;
        }
        {
            let mut saved = shared.saved.lock().expect("saved lock");
            saved.device_id = setup::new_device_id();
            saved.setup_id = setup::new_setup_id();
            saved.pairings.clear();
            saved.config_number = 1;
            saved.structure.clear();
            *shared.identity.write().expect("identity lock") = Arc::new(identity);
            *shared.code.write().expect("code lock") = code;
            shared.persist(&saved);
            shared.announce_from(&saved);
        }
        let _ = shared.kick.send(None);
        shared.rebuild.notify_one();
        Ok(())
    }

    /// Unpairs every phone: they must add the bridge again.
    pub fn forget_pairings(&self) {
        {
            let mut saved = self.0.saved.lock().expect("saved lock");
            saved.pairings.clear();
            self.0.persist(&saved);
            self.0.announce_from(&saved);
        }
        let _ = self.0.kick.send(None);
    }
}

/// Why the bridge did not start.
#[derive(Debug, thiserror::Error)]
pub enum StartError {
    #[error("no secret store: HomeKit needs MOLI_MASTER_KEY to keep its setup code")]
    NoSecrets,
    #[error("{0}")]
    Io(String),
}

fn random_bytes<const N: usize>() -> [u8; N] {
    let mut out = [0u8; N];
    SystemRandom::new()
        .fill(&mut out)
        .expect("system randomness");
    out
}

/// Starts the bridge: its identity and code from the secret store, the
/// database, the listener, the announcement. The task ends with `cancel`.
pub async fn start(
    hub: Hub,
    data_dir: &Path,
    config: Config,
    cancel: CancellationToken,
) -> Result<(HomeKit, tokio::task::JoinHandle<()>), StartError> {
    if !hub.has_secret_store() {
        return Err(StartError::NoSecrets);
    }
    let instance = InstanceId::from(NAMESPACE);
    let code = if let Some(code) = hub
        .secret(&instance, SECRET_CODE)
        .filter(|c| setup::is_code(c))
    {
        code
    } else {
        let code = setup::new_code();
        hub.store_secret(&instance, SECRET_CODE, &code)
            .await
            .map_err(|e| StartError::Io(e.to_string()))?;
        code
    };
    let seed = if let Some(seed) = hub
        .secret(&instance, SECRET_IDENTITY)
        .and_then(|s| setup::unhex(&s))
        .filter(|s| s.len() == 32)
    {
        seed
    } else {
        let seed = random_bytes::<32>();
        hub.store_secret(&instance, SECRET_IDENTITY, &setup::hex(&seed))
            .await
            .map_err(|e| StartError::Io(e.to_string()))?;
        seed.to_vec()
    };
    let identity = Ed25519KeyPair::from_seed_unchecked(&seed)
        .map_err(|_| StartError::Io("the bridge's identity key is damaged".to_owned()))?;
    let path = data_dir.join("homekit.json");
    let saved =
        Saved::load(&path).map_err(|e| StartError::Io(format!("{}: {e}", path.display())))?;
    // A new identity under old pairings would be refused by every phone.
    let (updates, _) = broadcast::channel(256);
    let (kick, _) = broadcast::channel(16);
    let (announce, announcement) = watch::channel(Announcement::of(&saved));
    let listener = tokio::net::TcpListener::bind(("0.0.0.0", config.port))
        .await
        .map_err(|e| StartError::Io(format!("cannot listen on port {}: {e}", config.port)))?;
    let port = listener
        .local_addr()
        .map_err(|e| StartError::Io(e.to_string()))?
        .port();
    let shared = Arc::new(Shared {
        hub,
        config,
        path,
        last_written: Mutex::new(Vec::new()),
        saved: Mutex::new(saved),
        catalog: RwLock::new(Arc::new(Catalog::default())),
        identity: RwLock::new(Arc::new(identity)),
        code: RwLock::new(code),
        updates,
        kick,
        announce,
        rebuild: Notify::new(),
        streams: camera::Streams::new(cancel.clone()),
        setup_owner: Mutex::new(None),
        failures: AtomicU32::new(0),
        next_conn: AtomicU64::new(1),
        port,
    });
    shared.rebuild_now();
    let handle = HomeKit(shared.clone());
    let task = tokio::spawn(run(shared, listener, announcement, cancel));
    Ok((handle, task))
}

async fn run(
    shared: Arc<Shared>,
    listener: tokio::net::TcpListener,
    announcement: watch::Receiver<Announcement>,
    cancel: CancellationToken,
) {
    tracing::info!(
        port = shared.port,
        name = %shared.config.name,
        "homekit: bridge ready"
    );
    let service = mdns::Service {
        name: shared.config.name.clone(),
        port: shared.port,
        address: shared.config.address,
    };
    let mdns = tokio::spawn({
        let (announce, cancel) = (shared.config.announce, cancel.clone());
        async move {
            if announce {
                mdns::run(service, announcement, cancel).await;
            }
        }
    });
    let maintain = tokio::spawn(maintain(shared.clone(), cancel.clone()));
    let slots = Arc::new(tokio::sync::Semaphore::new(session::MAX_CONNECTIONS));
    loop {
        tokio::select! {
            () = cancel.cancelled() => break,
            accepted = listener.accept() => match accepted {
                Ok((stream, _)) => {
                    let Ok(permit) = slots.clone().try_acquire_owned() else {
                        continue; // too many: the stream is dropped
                    };
                    let (shared, cancel) = (shared.clone(), cancel.clone());
                    tokio::spawn(async move {
                        session::serve(shared, stream, cancel).await;
                        drop(permit);
                    });
                }
                Err(e) => {
                    tracing::warn!(error = %e, "homekit: accept failed");
                    tokio::time::sleep(Duration::from_millis(200)).await;
                }
            }
        }
    }
    let _ = tokio::join!(mdns, maintain);
}

/// Follows the hub: pushes value changes to the controllers, and rebuilds
/// the database (after a pause, drivers publish in bursts) when devices or
/// names change.
async fn maintain(shared: Arc<Shared>, cancel: CancellationToken) {
    let mut events = shared.hub.subscribe();
    let mut due: Option<tokio::time::Instant> = Some(tokio::time::Instant::now());
    loop {
        let wait = async {
            match due {
                Some(at) => tokio::time::sleep_until(at).await,
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            () = cancel.cancelled() => break,
            () = shared.rebuild.notified() => {
                due = Some(tokio::time::Instant::now() + Duration::from_millis(200));
            }
            () = wait => {
                due = None;
                shared.rebuild_now();
            }
            event = events.recv() => match event {
                Ok(Event::State { point, .. }) => shared.refresh(&point),
                Ok(Event::DeviceUpserted { .. } | Event::LabelChanged { .. }) => {
                    due.get_or_insert(tokio::time::Instant::now() + Duration::from_secs(2));
                }
                Ok(Event::DeviceRemoved { device }) => {
                    shared.forget_device(&device);
                    due.get_or_insert(tokio::time::Instant::now() + Duration::from_secs(2));
                }
                Ok(_) => {}
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    due.get_or_insert(tokio::time::Instant::now() + Duration::from_secs(2));
                    // Values were missed: every watcher reads them again.
                    let catalog = shared.catalog();
                    for accessory in &catalog.accessories {
                        for service in &accessory.services {
                            for c in &service.chars {
                                let _ = shared.updates.send((accessory.aid, c.iid));
                            }
                        }
                    }
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    }
}
