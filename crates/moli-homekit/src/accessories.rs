//! What HomeKit sees: Moli's devices mapped to HAP accessories, services
//! and characteristics, with stable identifiers, and the conversion of
//! values both ways.
//!
//! The mapping is by *meaning* (`Semantic`), never by brand: a writable
//! on/off point is a light (with brightness, colour temperature, colour when
//! the device has them) or a switch; read-only temperature, humidity,
//! motion, contact, light level, leak and smoke points are sensors; a
//! `Control` point `mode` offering `disarmed`/`partial`/`total` is a
//! security system (`partial` is HomeKit's *Night* arming).

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use moli_core::{Device, DeviceId, Kind, Label, PointId, PointSpec, Semantic, Value};
use serde::{Deserialize, Serialize};
use serde_json::{Value as Json, json};

use crate::camera::CameraChar;

/// Most bridged accessories HomeKit accepts behind one bridge.
pub const MAX_ACCESSORIES: usize = 140;
/// Status codes of HAP (`HAPStatus`).
pub const OK: i32 = 0;
pub const ST_COMMUNICATION: i32 = -70402;
pub const ST_READ_ONLY: i32 = -70404;
pub const ST_WRITE_ONLY: i32 = -70405;
pub const ST_NOTIFY_UNSUPPORTED: i32 = -70406;
pub const ST_NOT_FOUND: i32 = -70409;
pub const ST_INVALID: i32 = -70410;

// Service types (short UUIDs).
const SVC_INFO: &str = "3E";
const SVC_PROTOCOL: &str = "A2";
const SVC_LIGHT: &str = "43";
const SVC_SWITCH: &str = "49";
const SVC_OUTLET: &str = "47";
const SVC_TEMPERATURE: &str = "8A";
const SVC_HUMIDITY: &str = "82";
const SVC_MOTION: &str = "85";
const SVC_CONTACT: &str = "80";
const SVC_LIGHT_LEVEL: &str = "84";
const SVC_LEAK: &str = "83";
const SVC_SMOKE: &str = "87";
const SVC_SECURITY: &str = "7E";
const SVC_CAMERA: &str = "110";

// Characteristic types.
const C_IDENTIFY: &str = "14";
const C_MANUFACTURER: &str = "20";
const C_MODEL: &str = "21";
const C_NAME: &str = "23";
const C_SERIAL: &str = "30";
const C_FIRMWARE: &str = "52";
const C_VERSION: &str = "37";
const C_ON: &str = "25";
const C_BRIGHTNESS: &str = "08";
const C_COLOR_TEMP: &str = "CE";
const C_HUE: &str = "13";
const C_SATURATION: &str = "2F";
const C_OUTLET_IN_USE: &str = "26";
const C_TEMPERATURE: &str = "11";
const C_HUMIDITY: &str = "10";
const C_MOTION: &str = "22";
const C_CONTACT: &str = "6A";
const C_LIGHT_LEVEL: &str = "6B";
const C_LEAK: &str = "70";
const C_SMOKE: &str = "76";
const C_SEC_CURRENT: &str = "66";
const C_SEC_TARGET: &str = "67";
const C_SUPPORTED_VIDEO: &str = "114";
const C_SUPPORTED_AUDIO: &str = "115";
const C_SUPPORTED_RTP: &str = "116";
const C_SELECTED_RTP: &str = "117";
const C_SETUP_ENDPOINTS: &str = "118";
const C_STREAMING_STATUS: &str = "120";

const READ_NOTIFY: &[&str] = &["pr", "ev"];
const READ_WRITE_NOTIFY: &[&str] = &["pr", "pw", "ev"];
const READ_ONLY: &[&str] = &["pr"];
const READ_WRITE: &[&str] = &["pr", "pw"];

/// HomeKit's security states.
const ARM_STAY: i64 = 0;
const ARM_AWAY: i64 = 1;
const ARM_NIGHT: i64 = 2;
const ARM_DISARMED: i64 = 3;
const ARM_TRIGGERED: i64 = 4;

/// What a device is to HomeKit, for the dashboard's list.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Kinds {
    Light,
    Switch,
    Outlet,
    Temperature,
    Humidity,
    Motion,
    Contact,
    LightLevel,
    Leak,
    Smoke,
    Alarm,
    /// Live video (a camera of its own, apart from the device's sensors).
    Camera,
}

impl Kinds {
    pub fn name(self) -> &'static str {
        match self {
            Self::Light => "light",
            Self::Switch => "switch",
            Self::Outlet => "outlet",
            Self::Temperature => "temperature",
            Self::Humidity => "humidity",
            Self::Motion => "motion",
            Self::Contact => "contact",
            Self::LightLevel => "light_level",
            Self::Leak => "leak",
            Self::Smoke => "smoke",
            Self::Alarm => "alarm",
            Self::Camera => "camera",
        }
    }
}

/// How a characteristic gets and takes its value.
#[derive(Clone, Debug, PartialEq)]
pub enum Bind {
    Static(Json),
    Identify,
    /// A Moli point, converted by `Conv`.
    Point(PointId, Conv),
    /// Hue or saturation of a `#rrggbb` colour point.
    Color(PointId, ColorPart),
    /// The alarm's current state (its `state` point, else its `mode`).
    ArmCurrent {
        state: Option<PointId>,
        mode: Option<PointId>,
    },
    ArmTarget(PointId),
    /// A characteristic of a camera's stream management.
    Camera {
        device: DeviceId,
        what: CameraChar,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorPart {
    Hue,
    Saturation,
}

/// Value conversion between Moli and HAP.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Conv {
    Bool,
    /// Moli's `lo..hi` ⇄ HAP's 0–100 %.
    Percent {
        lo: f64,
        hi: f64,
    },
    /// An integer in the same unit (mired).
    Int,
    /// Moli `true` (closed) ⇄ HAP 0 (`CONTACT_DETECTED`).
    Contact,
    /// Moli `true` ⇄ HAP 1 (leak, smoke detected).
    Detected,
    /// Lux, never below HAP's minimum of 0.0001.
    Lux,
    /// A number HAP bounds (a temperature in °C, a humidity in %).
    Clamped {
        lo: f64,
        hi: f64,
    },
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Meta {
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub step: Option<f64>,
    pub unit: Option<&'static str>,
    pub valid_values: Option<Vec<i64>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Characteristic {
    pub iid: u64,
    pub kind: &'static str,
    pub format: &'static str,
    pub perms: &'static [&'static str],
    pub bind: Bind,
    pub meta: Meta,
    pub description: &'static str,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Service {
    pub iid: u64,
    pub kind: &'static str,
    pub chars: Vec<Characteristic>,
}

#[derive(Clone, Debug)]
pub struct Accessory {
    pub aid: u64,
    pub services: Vec<Service>,
}

/// A service before identifiers are given.
struct ServiceSpec {
    kind: &'static str,
    chars: Vec<CharSpec>,
}

struct CharSpec {
    kind: &'static str,
    format: &'static str,
    perms: &'static [&'static str],
    bind: Bind,
    meta: Meta,
    description: &'static str,
}

fn spec(
    kind: &'static str,
    format: &'static str,
    perms: &'static [&'static str],
    bind: Bind,
    description: &'static str,
) -> CharSpec {
    CharSpec {
        kind,
        format,
        perms,
        bind,
        meta: Meta::default(),
        description,
    }
}

impl CharSpec {
    fn meta(mut self, meta: Meta) -> Self {
        self.meta = meta;
        self
    }
}

fn name_char(name: &str) -> CharSpec {
    spec(
        C_NAME,
        "string",
        READ_ONLY,
        Bind::Static(json!(name)),
        "Name",
    )
}

fn percent_meta() -> Meta {
    Meta {
        min: Some(0.0),
        max: Some(100.0),
        step: Some(1.0),
        unit: Some("percentage"),
        ..Meta::default()
    }
}

fn numeric(spec: &PointSpec) -> (f64, f64) {
    match spec.kind {
        Kind::Numeric { min, max, .. } => (min.unwrap_or(0.0), max.unwrap_or(100.0)),
        _ => (0.0, 100.0),
    }
}

/// The part of a point key that tells *which* light it belongs to on a
/// multi-light device: `state_l1` → `_l1`, `light2.on` → `light2.`.
fn tag(key: &str) -> &str {
    for base in ["color_temp", "brightness", "color", "state", "on"] {
        if key == base {
            return "";
        }
        if let Some(rest) = key.strip_prefix(base)
            && rest.starts_with('_')
        {
            return rest;
        }
        if let Some(prefix) = key.strip_suffix(base) {
            return prefix;
        }
    }
    key
}

fn writable(p: &PointSpec) -> bool {
    p.access.write
}

/// The services a device offers HomeKit, and what they are. Empty: nothing
/// HomeKit has a place for.
#[allow(clippy::too_many_lines)] // one block per kind of device
fn services_of(device: &Device, name: &str, live: bool) -> (Vec<ServiceSpec>, Vec<Kinds>) {
    let mut services = Vec::new();
    let mut kinds = Vec::new();
    let id = |key: &str| PointId::new(&device.id, key);

    // The alarm panel.
    if let Some(mode) = device.points.iter().find(|p| {
        &*p.key == "mode"
            && p.semantic == Semantic::Control
            && writable(p)
            && matches!(&p.kind, Kind::Enum { values } if values.iter().any(|v| &**v == "disarmed"))
    }) {
        let state = device.point("state").map(|p| id(&p.key));
        services.push(ServiceSpec {
            kind: SVC_SECURITY,
            chars: vec![
                name_char(name),
                spec(
                    C_SEC_CURRENT,
                    "uint8",
                    READ_NOTIFY,
                    Bind::ArmCurrent {
                        state,
                        mode: Some(id(&mode.key)),
                    },
                    "Security System Current State",
                )
                .meta(Meta {
                    valid_values: Some(vec![
                        ARM_STAY,
                        ARM_AWAY,
                        ARM_NIGHT,
                        ARM_DISARMED,
                        ARM_TRIGGERED,
                    ]),
                    ..Meta::default()
                }),
                spec(
                    C_SEC_TARGET,
                    "uint8",
                    READ_WRITE_NOTIFY,
                    Bind::ArmTarget(id(&mode.key)),
                    "Security System Target State",
                )
                .meta(Meta {
                    // « Stay » has no counterpart in the panel.
                    valid_values: Some(vec![ARM_AWAY, ARM_NIGHT, ARM_DISARMED]),
                    ..Meta::default()
                }),
            ],
        });
        kinds.push(Kinds::Alarm);
    }

    // Lights and switches.
    let has_power = device.points.iter().any(|p| p.semantic == Semantic::Power);
    let switches: Vec<&PointSpec> = device
        .points
        .iter()
        .filter(|p| p.semantic == Semantic::OnOff && writable(p) && p.kind == Kind::Binary)
        .collect();
    let single = switches.len() == 1;
    for on in &switches {
        let mine = |p: &&PointSpec| single || tag(&p.key) == tag(&on.key);
        let brightness = device
            .points
            .iter()
            .filter(|p| p.semantic == Semantic::Brightness && writable(p))
            .find(mine);
        let color_temp = device
            .points
            .iter()
            .filter(|p| p.semantic == Semantic::ColorTemp && writable(p))
            .find(mine);
        let color = device
            .points
            .iter()
            .filter(|p| p.semantic == Semantic::Color && writable(p) && p.kind == Kind::Text)
            .find(mine);
        let dimmable = brightness.is_some() || color_temp.is_some() || color.is_some();
        let suffix = if switches.len() > 1 {
            format!(" {}", on.label)
        } else {
            String::new()
        };
        let label = format!("{name}{suffix}");
        let mut chars = vec![
            name_char(&label),
            spec(
                C_ON,
                "bool",
                READ_WRITE_NOTIFY,
                Bind::Point(id(&on.key), Conv::Bool),
                "On",
            ),
        ];
        if let Some(b) = brightness {
            let (lo, hi) = numeric(b);
            chars.push(
                spec(
                    C_BRIGHTNESS,
                    "int",
                    READ_WRITE_NOTIFY,
                    Bind::Point(id(&b.key), Conv::Percent { lo, hi }),
                    "Brightness",
                )
                .meta(percent_meta()),
            );
        }
        if let Some(ct) = color_temp {
            let (lo, hi) = numeric(ct);
            chars.push(
                spec(
                    C_COLOR_TEMP,
                    "uint32",
                    READ_WRITE_NOTIFY,
                    Bind::Point(id(&ct.key), Conv::Int),
                    "Color Temperature",
                )
                .meta(Meta {
                    min: Some(lo.max(50.0)),
                    max: Some(hi.min(900.0)),
                    step: Some(1.0),
                    ..Meta::default()
                }),
            );
        }
        if let Some(c) = color {
            chars.push(
                spec(
                    C_HUE,
                    "float",
                    READ_WRITE_NOTIFY,
                    Bind::Color(id(&c.key), ColorPart::Hue),
                    "Hue",
                )
                .meta(Meta {
                    min: Some(0.0),
                    max: Some(360.0),
                    step: Some(1.0),
                    unit: Some("arcdegrees"),
                    ..Meta::default()
                }),
            );
            chars.push(
                spec(
                    C_SATURATION,
                    "float",
                    READ_WRITE_NOTIFY,
                    Bind::Color(id(&c.key), ColorPart::Saturation),
                    "Saturation",
                )
                .meta(percent_meta()),
            );
        }
        if dimmable {
            services.push(ServiceSpec {
                kind: SVC_LIGHT,
                chars,
            });
            kinds.push(Kinds::Light);
        } else if has_power {
            chars.push(spec(
                C_OUTLET_IN_USE,
                "bool",
                READ_NOTIFY,
                Bind::Static(json!(true)),
                "Outlet In Use",
            ));
            services.push(ServiceSpec {
                kind: SVC_OUTLET,
                chars,
            });
            kinds.push(Kinds::Outlet);
        } else {
            services.push(ServiceSpec {
                kind: SVC_SWITCH,
                chars,
            });
            kinds.push(Kinds::Switch);
        }
    }

    // Sensors: read-only points only (a writable temperature is a setpoint).
    let sensors: Vec<&PointSpec> = device
        .points
        .iter()
        .filter(|p| p.access.read && !writable(p))
        .collect();
    let many = sensors.iter().filter(|p| sensor_kind(p).is_some()).count() > 1;
    for p in sensors {
        let Some(kind) = sensor_kind(p) else { continue };
        let label = if many || live || !services.is_empty() {
            format!("{name} {}", p.label)
        } else {
            name.to_owned()
        };
        let point = id(&p.key);
        let (svc, ch) = match kind {
            Kinds::Temperature => (
                SVC_TEMPERATURE,
                spec(
                    C_TEMPERATURE,
                    "float",
                    READ_NOTIFY,
                    Bind::Point(
                        point,
                        Conv::Clamped {
                            lo: -270.0,
                            hi: 100.0,
                        },
                    ),
                    "Current Temperature",
                )
                .meta(Meta {
                    min: Some(-270.0),
                    max: Some(100.0),
                    step: Some(0.1),
                    unit: Some("celsius"),
                    ..Meta::default()
                }),
            ),
            Kinds::Humidity => (
                SVC_HUMIDITY,
                spec(
                    C_HUMIDITY,
                    "float",
                    READ_NOTIFY,
                    Bind::Point(point, Conv::Clamped { lo: 0.0, hi: 100.0 }),
                    "Current Relative Humidity",
                )
                .meta(Meta {
                    min: Some(0.0),
                    max: Some(100.0),
                    step: Some(1.0),
                    unit: Some("percentage"),
                    ..Meta::default()
                }),
            ),
            Kinds::Motion => (
                SVC_MOTION,
                spec(
                    C_MOTION,
                    "bool",
                    READ_NOTIFY,
                    Bind::Point(point, Conv::Bool),
                    "Motion Detected",
                ),
            ),
            Kinds::Contact => (
                SVC_CONTACT,
                spec(
                    C_CONTACT,
                    "uint8",
                    READ_NOTIFY,
                    Bind::Point(point, Conv::Contact),
                    "Contact Sensor State",
                ),
            ),
            Kinds::LightLevel => (
                SVC_LIGHT_LEVEL,
                spec(
                    C_LIGHT_LEVEL,
                    "float",
                    READ_NOTIFY,
                    Bind::Point(point, Conv::Lux),
                    "Current Ambient Light Level",
                )
                .meta(Meta {
                    min: Some(0.0001),
                    max: Some(100_000.0),
                    unit: Some("lux"),
                    ..Meta::default()
                }),
            ),
            Kinds::Leak => (
                SVC_LEAK,
                spec(
                    C_LEAK,
                    "uint8",
                    READ_NOTIFY,
                    Bind::Point(point, Conv::Detected),
                    "Leak Detected",
                ),
            ),
            Kinds::Smoke => (
                SVC_SMOKE,
                spec(
                    C_SMOKE,
                    "uint8",
                    READ_NOTIFY,
                    Bind::Point(point, Conv::Detected),
                    "Smoke Detected",
                ),
            ),
            _ => continue,
        };
        services.push(ServiceSpec {
            kind: svc,
            chars: vec![name_char(&label), ch],
        });
        kinds.push(kind);
    }
    kinds.sort();
    kinds.dedup();
    (services, kinds)
}

fn sensor_kind(p: &PointSpec) -> Option<Kinds> {
    let numeric = matches!(p.kind, Kind::Numeric { .. });
    let binary = p.kind == Kind::Binary;
    match p.semantic {
        Semantic::Temperature if numeric => Some(Kinds::Temperature),
        Semantic::Humidity if numeric => Some(Kinds::Humidity),
        Semantic::Illuminance if numeric => Some(Kinds::LightLevel),
        Semantic::Occupancy if binary => Some(Kinds::Motion),
        Semantic::Contact if binary => Some(Kinds::Contact),
        Semantic::WaterLeak if binary => Some(Kinds::Leak),
        Semantic::Smoke if binary => Some(Kinds::Smoke),
        _ => None,
    }
}

/// What HomeKit would show of a device (the dashboard lists these).
pub fn kinds_of(device: &Device) -> Vec<Kinds> {
    services_of(device, "", false).1
}

/// The stream management service of a camera.
fn camera_services(device: &DeviceId) -> Vec<ServiceSpec> {
    let chars = [
        (
            C_SUPPORTED_VIDEO,
            READ_ONLY,
            CameraChar::SupportedVideo,
            "Supported Video Stream Configuration",
        ),
        (
            C_SUPPORTED_AUDIO,
            READ_ONLY,
            CameraChar::SupportedAudio,
            "Supported Audio Stream Configuration",
        ),
        (
            C_SUPPORTED_RTP,
            READ_ONLY,
            CameraChar::SupportedRtp,
            "Supported RTP Configuration",
        ),
        (
            C_SELECTED_RTP,
            READ_WRITE,
            CameraChar::Selected,
            "Selected RTP Stream Configuration",
        ),
        (
            C_SETUP_ENDPOINTS,
            READ_WRITE,
            CameraChar::Setup,
            "Setup Endpoints",
        ),
        (
            C_STREAMING_STATUS,
            READ_NOTIFY,
            CameraChar::Status,
            "Streaming Status",
        ),
    ]
    .into_iter()
    .map(|(kind, perms, what, description)| {
        spec(
            kind,
            "tlv8",
            perms,
            Bind::Camera {
                device: device.clone(),
                what,
            },
            description,
        )
    })
    .collect();
    vec![ServiceSpec {
        kind: SVC_CAMERA,
        chars,
    }]
}

/// The key of a device's camera among HomeKit's choices: the device itself
/// is its sensors, this is its picture.
pub fn camera_key(device: &DeviceId) -> String {
    format!("{}#camera", device.as_str())
}

/// The accessory database and its indexes.
#[derive(Debug, Default)]
pub struct Catalog {
    pub accessories: Vec<Accessory>,
    by_point: HashMap<PointId, Vec<(u64, u64)>>,
    index: HashMap<(u64, u64), (usize, usize, usize)>,
    /// More devices were chosen than HomeKit takes.
    pub truncated: bool,
}

/// Stable accessory identifiers: a device keeps its `aid` for good.
#[derive(Debug, Default)]
pub struct Aids {
    pub assigned: BTreeMap<String, u64>,
    pub next: u64,
}

impl Aids {
    fn get(&mut self, id: &str) -> u64 {
        if let Some(aid) = self.assigned.get(id) {
            return *aid;
        }
        // 1 is the bridge.
        let aid = self.next.max(2);
        self.next = aid + 1;
        self.assigned.insert(id.to_owned(), aid);
        aid
    }
}

/// A device as HomeKit is told of it: its description and the name people
/// gave it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub name: String,
    pub device: Arc<Device>,
    /// This is the camera accessory of the device (its live video).
    #[serde(default)]
    pub camera: bool,
    /// The device has live video, shown apart as its camera.
    #[serde(default)]
    pub live: bool,
}

impl Entry {
    pub fn new(device: Arc<Device>, label: &Label, live: bool) -> Self {
        let name = label
            .name
            .as_deref()
            .filter(|n| !n.trim().is_empty())
            .unwrap_or(&device.native_name);
        // HAP names stay short and plain.
        let name: String = name.trim().chars().take(64).collect();
        Self {
            name: if name.is_empty() {
                "Moli".to_owned()
            } else {
                name
            },
            device,
            camera: false,
            live,
        }
    }

    /// The camera accessory of a device with live video.
    pub fn camera(device: Arc<Device>, label: &Label) -> Self {
        Self {
            camera: true,
            live: true,
            ..Self::new(device, label, true)
        }
    }

    /// What the choices are keyed by.
    pub fn key(&self) -> String {
        if self.camera {
            camera_key(&self.device.id)
        } else {
            self.device.id.as_str().to_owned()
        }
    }
}

impl Catalog {
    /// The database for the devices `exposed` says yes to. Sorted by
    /// `aid`, so a given house gives the same database each time.
    pub fn build(
        bridge_name: &str,
        entries: &[Entry],
        exposed: impl Fn(&str) -> bool,
        aids: &mut Aids,
    ) -> Self {
        let mut accessories = vec![bridge(bridge_name)];
        let mut chosen: Vec<(u64, &Entry, Vec<ServiceSpec>)> = Vec::new();
        let mut candidates: Vec<(String, &Entry)> = entries
            .iter()
            .map(|e| (e.key(), e))
            .filter(|(key, _)| exposed(key))
            .collect();
        candidates.sort_by(|a, b| a.0.cmp(&b.0));
        let mut truncated = false;
        for (key, view) in candidates {
            let services = if view.camera {
                camera_services(&view.device.id)
            } else {
                services_of(&view.device, &view.name, view.live).0
            };
            if services.is_empty() {
                continue;
            }
            if chosen.len() >= MAX_ACCESSORIES {
                truncated = true;
                break;
            }
            chosen.push((aids.get(&key), view, services));
        }
        chosen.sort_by_key(|(aid, _, _)| *aid);
        for (aid, view, services) in chosen {
            accessories.push(accessory(aid, view, services));
        }
        let mut catalog = Self {
            accessories,
            truncated,
            ..Self::default()
        };
        catalog.reindex();
        catalog
    }

    fn reindex(&mut self) {
        self.by_point.clear();
        self.index.clear();
        for (a, accessory) in self.accessories.iter().enumerate() {
            for (s, service) in accessory.services.iter().enumerate() {
                for (c, ch) in service.chars.iter().enumerate() {
                    self.index.insert((accessory.aid, ch.iid), (a, s, c));
                    let points: Vec<&PointId> = match &ch.bind {
                        Bind::Point(p, _) | Bind::Color(p, _) | Bind::ArmTarget(p) => vec![p],
                        Bind::ArmCurrent { state, mode } => {
                            state.iter().chain(mode.iter()).collect()
                        }
                        Bind::Static(_) | Bind::Identify | Bind::Camera { .. } => Vec::new(),
                    };
                    for p in points {
                        self.by_point
                            .entry(p.clone())
                            .or_default()
                            .push((accessory.aid, ch.iid));
                    }
                }
            }
        }
    }

    pub fn find(&self, aid: u64, iid: u64) -> Option<&Characteristic> {
        let (a, s, c) = *self.index.get(&(aid, iid))?;
        Some(&self.accessories[a].services[s].chars[c])
    }

    /// The `(aid, iid)` of a camera's streaming status.
    pub fn camera_status(&self, device: &DeviceId) -> Option<(u64, u64)> {
        self.accessories.iter().find_map(|a| {
            a.services
                .iter()
                .flat_map(|s| &s.chars)
                .find_map(|c| match &c.bind {
                    Bind::Camera {
                        device: d,
                        what: CameraChar::Status,
                    } if d == device => Some((a.aid, c.iid)),
                    _ => None,
                })
        })
    }

    /// The device whose camera accessory `aid` is.
    pub fn camera_device(&self, aid: u64) -> Option<&DeviceId> {
        self.accessories
            .iter()
            .find(|a| a.aid == aid)?
            .services
            .iter()
            .flat_map(|s| &s.chars)
            .find_map(|c| match &c.bind {
                Bind::Camera { device, .. } => Some(device),
                _ => None,
            })
    }

    /// The characteristics a Moli point feeds.
    pub fn watching(&self, point: &PointId) -> &[(u64, u64)] {
        self.by_point.get(point).map_or(&[], Vec::as_slice)
    }

    /// The accessories with the structure only (what `c#` is about).
    pub fn structure(&self) -> Json {
        self.render(&|_| None)
    }

    /// `/accessories`: the database with each readable value.
    pub fn render(&self, value: &dyn Fn(&Characteristic) -> Option<Json>) -> Json {
        let accessories: Vec<Json> = self
            .accessories
            .iter()
            .map(|a| {
                let services: Vec<Json> = a
                    .services
                    .iter()
                    .map(|s| {
                        let chars: Vec<Json> = s
                            .chars
                            .iter()
                            .map(|c| characteristic_json(c, value(c)))
                            .collect();
                        json!({"iid": s.iid, "type": s.kind, "characteristics": chars})
                    })
                    .collect();
                json!({"aid": a.aid, "services": services})
            })
            .collect();
        json!({ "accessories": accessories })
    }
}

fn characteristic_json(c: &Characteristic, value: Option<Json>) -> Json {
    let mut out = json!({
        "iid": c.iid,
        "type": c.kind,
        "perms": c.perms,
        "format": c.format,
        "description": c.description,
    });
    let map = out.as_object_mut().expect("object");
    if let Some(v) = value {
        map.insert("value".into(), v);
    }
    let m = &c.meta;
    if let Some(v) = m.min {
        map.insert("minValue".into(), number(v, c.format));
    }
    if let Some(v) = m.max {
        map.insert("maxValue".into(), number(v, c.format));
    }
    if let Some(v) = m.step {
        map.insert("minStep".into(), number(v, c.format));
    }
    if let Some(u) = m.unit {
        map.insert("unit".into(), json!(u));
    }
    if let Some(v) = &m.valid_values {
        map.insert("valid-values".into(), json!(v));
    }
    out
}

/// Integers stay integers in the metadata of integer formats.
fn number(v: f64, format: &str) -> Json {
    // Whole numbers stay whole, as other HomeKit accessories print them
    // (360, not 360.0): the Home app is strict about its database.
    if matches!(format, "int" | "uint8" | "uint16" | "uint32" | "uint64")
        || (v - v.trunc()).abs() < f64::EPSILON
    {
        json!(v as i64)
    } else {
        json!(v)
    }
}

fn bridge(name: &str) -> Accessory {
    let mut next = 1;
    let mut iid = || {
        next += 1;
        next - 1
    };
    let info = info_service(
        "Moli",
        "Moli OS",
        "bridge",
        env!("CARGO_PKG_VERSION"),
        name,
        &mut iid,
    );
    let protocol = Service {
        iid: iid(),
        kind: SVC_PROTOCOL,
        chars: vec![Characteristic {
            iid: iid(),
            kind: C_VERSION,
            format: "string",
            perms: READ_ONLY,
            bind: Bind::Static(json!("1.1.0")),
            meta: Meta::default(),
            description: "Version",
        }],
    };
    Accessory {
        aid: 1,
        services: vec![info, protocol],
    }
}

fn info_service(
    manufacturer: &str,
    model: &str,
    serial: &str,
    firmware: &str,
    name: &str,
    iid: &mut impl FnMut() -> u64,
) -> Service {
    let service_iid = iid();
    let chars = [
        (C_IDENTIFY, "bool", &["pw"][..], Bind::Identify, "Identify"),
        (
            C_MANUFACTURER,
            "string",
            READ_ONLY,
            Bind::Static(json!(manufacturer)),
            "Manufacturer",
        ),
        (
            C_MODEL,
            "string",
            READ_ONLY,
            Bind::Static(json!(model)),
            "Model",
        ),
        (
            C_NAME,
            "string",
            READ_ONLY,
            Bind::Static(json!(name)),
            "Name",
        ),
        (
            C_SERIAL,
            "string",
            READ_ONLY,
            Bind::Static(json!(serial)),
            "Serial Number",
        ),
        (
            C_FIRMWARE,
            "string",
            READ_ONLY,
            Bind::Static(json!(firmware)),
            "Firmware Revision",
        ),
    ]
    .into_iter()
    .map(|(kind, format, perms, bind, description)| Characteristic {
        iid: iid(),
        kind,
        format,
        perms,
        bind,
        meta: Meta::default(),
        description,
    })
    .collect();
    Service {
        iid: service_iid,
        kind: SVC_INFO,
        chars,
    }
}

fn accessory(aid: u64, view: &Entry, specs: Vec<ServiceSpec>) -> Accessory {
    let device = &view.device;
    let mut next = 1;
    let mut iid = || {
        next += 1;
        next - 1
    };
    let name = view.name.clone();
    // The maker is always Moli: the Home app treats a bridge that claims
    // another brand's name (Signify, Philips Hue…) as an accessory out of
    // compliance. The real maker and model go in the model.
    let model = [device.manufacturer.as_deref(), device.model.as_deref()]
        .into_iter()
        .flatten()
        .filter(|part| !part.trim().is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let mut services = vec![info_service(
        "Moli",
        if model.is_empty() { "Moli" } else { &model },
        &view.key(),
        "1.0.0",
        &name,
        &mut iid,
    )];
    for service in specs {
        let service_iid = iid();
        let chars = service
            .chars
            .into_iter()
            .map(|c| Characteristic {
                iid: iid(),
                kind: c.kind,
                format: c.format,
                perms: c.perms,
                bind: c.bind,
                meta: c.meta,
                description: c.description,
            })
            .collect();
        services.push(Service {
            iid: service_iid,
            kind: service.kind,
            chars,
        });
    }
    Accessory { aid, services }
}

/// A neutral value for a readable characteristic whose device has not
/// answered (yet): what the database carries instead of nothing.
pub fn placeholder(c: &Characteristic) -> Json {
    if c.format == "tlv8" {
        return json!("");
    }
    if matches!(c.bind, Bind::ArmCurrent { .. } | Bind::ArmTarget(_)) {
        return json!(ARM_DISARMED);
    }
    let low = c.meta.min.unwrap_or(0.0);
    match c.format {
        "bool" => json!(false),
        "string" => json!(""),
        "float" => json!(low),
        _ => json!(low as i64),
    }
}

// ---- values ------------------------------------------------------------

/// Moli value → HAP JSON.
pub fn to_hap(conv: Conv, value: &Value) -> Option<Json> {
    Some(match conv {
        Conv::Bool => json!(as_bool(value)?),
        Conv::Percent { lo, hi } => {
            let v = value.as_f64()?;
            let span = (hi - lo).max(f64::EPSILON);
            json!(((v - lo) * 100.0 / span).round().clamp(0.0, 100.0) as i64)
        }
        Conv::Int => json!(value.as_f64()?.round() as i64),
        Conv::Clamped { lo, hi } => json!(value.as_f64()?.clamp(lo, hi)),
        Conv::Contact => json!(i64::from(!as_bool(value)?)),
        Conv::Detected => json!(i64::from(as_bool(value)?)),
        Conv::Lux => json!(value.as_f64()?.clamp(0.0001, 100_000.0)),
    })
}

fn as_bool(value: &Value) -> Option<bool> {
    match value {
        Value::Bool(b) => Some(*b),
        Value::Int(i) => Some(*i != 0),
        _ => None,
    }
}

/// HAP JSON → Moli value (before the point's own validation).
pub fn from_hap(conv: Conv, json: &Json) -> Option<Value> {
    match conv {
        Conv::Bool => {
            let b = match json {
                Json::Bool(b) => *b,
                Json::Number(n) => n.as_i64()? != 0,
                _ => return None,
            };
            Some(Value::Bool(b))
        }
        Conv::Percent { lo, hi } => {
            let pct = json.as_f64()?.clamp(0.0, 100.0);
            Some(Value::Float(lo + pct * (hi - lo) / 100.0))
        }
        Conv::Int => Some(Value::Int(json.as_f64()?.round() as i64)),
        Conv::Lux | Conv::Clamped { .. } => Some(Value::Float(json.as_f64()?)),
        Conv::Contact | Conv::Detected => None,
    }
}

/// The alarm's mode ⇄ HomeKit's target state. `partial` is the *Night*
/// arming, as the dashboard shows it.
pub fn arm_target_of(mode: &str) -> Option<i64> {
    match mode {
        "disarmed" => Some(ARM_DISARMED),
        "partial" => Some(ARM_NIGHT),
        "total" => Some(ARM_AWAY),
        _ => None,
    }
}

pub fn mode_of_arm_target(target: i64) -> Option<&'static str> {
    match target {
        ARM_DISARMED => Some("disarmed"),
        ARM_NIGHT => Some("partial"),
        ARM_AWAY => Some("total"),
        _ => None,
    }
}

/// The alarm's current state from the panel's own words.
pub fn arm_current_of(state: &str) -> Option<i64> {
    match state {
        "disarmed" => Some(ARM_DISARMED),
        "triggered" | "alarm" => Some(ARM_TRIGGERED),
        s if s.starts_with("partial") => Some(ARM_NIGHT),
        s if s.starts_with("total") => Some(ARM_AWAY),
        "perimeter" | "annex" => Some(ARM_STAY),
        _ => None,
    }
}

// ---- colour ------------------------------------------------------------

/// `#rrggbb` → (hue °, saturation %).
#[allow(clippy::float_cmp)] // `max` is one of the three channels, compared as such
pub fn hex_to_hs(hex: &str) -> Option<(f64, f64)> {
    let digits = hex.strip_prefix('#')?;
    if digits.len() != 6 {
        return None;
    }
    let channel = |i: usize| {
        u8::from_str_radix(&digits[i..i + 2], 16)
            .ok()
            .map(|v| f64::from(v) / 255.0)
    };
    let (r, g, b) = (channel(0)?, channel(2)?, channel(4)?);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let delta = max - min;
    let hue = if delta == 0.0 {
        0.0
    } else if max == r {
        60.0 * (((g - b) / delta) % 6.0)
    } else if max == g {
        60.0 * ((b - r) / delta + 2.0)
    } else {
        60.0 * ((r - g) / delta + 4.0)
    };
    let hue = if hue < 0.0 { hue + 360.0 } else { hue };
    let saturation = if max == 0.0 { 0.0 } else { delta / max * 100.0 };
    Some((hue, saturation))
}

/// (hue °, saturation %) → `#rrggbb` at full brightness (the lamp's own
/// brightness is a separate point).
pub fn hs_to_hex(hue: f64, saturation: f64) -> String {
    let s = (saturation / 100.0).clamp(0.0, 1.0);
    let h = hue.rem_euclid(360.0) / 60.0;
    let c = s;
    let x = c * (1.0 - (h % 2.0 - 1.0).abs());
    let m = 1.0 - c;
    let (r, g, b) = match h as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let byte = |v: f64| ((v + m) * 255.0).round().clamp(0.0, 255.0) as u8;
    format!("#{:02x}{:02x}{:02x}", byte(r), byte(g), byte(b))
}

#[cfg(test)]
mod tests {
    use super::*;
    use moli_core::{Access, InstanceId};

    fn point(key: &str, kind: Kind, write: bool, semantic: Semantic) -> PointSpec {
        PointSpec {
            key: key.into(),
            label: key.into(),
            kind,
            access: Access { read: true, write },
            unit: None,
            semantic,
        }
    }

    fn range(lo: f64, hi: f64) -> Kind {
        Kind::Numeric {
            min: Some(lo),
            max: Some(hi),
            step: None,
        }
    }

    fn view(native: &str, points: Vec<PointSpec>) -> Entry {
        let instance = InstanceId::from("test");
        let device = Arc::new(Device {
            id: DeviceId::new(&instance, native),
            instance,
            native_name: native.into(),
            manufacturer: None,
            model: None,
            description: None,
            native_room: None,
            members: Vec::new(),
            points,
        });
        Entry::new(device, &Label::default(), false)
    }

    fn catalog(views: &[Entry]) -> Catalog {
        Catalog::build("Moli", views, |_| true, &mut Aids::default())
    }

    fn types(c: &Catalog, aid: u64) -> Vec<&'static str> {
        c.accessories
            .iter()
            .find(|a| a.aid == aid)
            .unwrap()
            .services
            .iter()
            .map(|s| s.kind)
            .collect()
    }

    #[test]
    fn a_dimmable_colour_lamp_is_a_lightbulb() {
        let lamp = view(
            "lamp",
            vec![
                point("on", Kind::Binary, true, Semantic::OnOff),
                point("brightness", range(0.0, 100.0), true, Semantic::Brightness),
                point("color_temp", range(153.0, 500.0), true, Semantic::ColorTemp),
                point("color", Kind::Text, true, Semantic::Color),
                point("color.x", range(0.0, 1.0), false, Semantic::Color),
            ],
        );
        let c = catalog(std::slice::from_ref(&lamp));
        assert_eq!(types(&c, 2), ["3E", "43"]);
        let light = &c.accessories[1].services[1];
        let kinds: Vec<_> = light.chars.iter().map(|ch| ch.kind).collect();
        assert_eq!(kinds, ["23", "25", "08", "CE", "13", "2F"]);
        assert_eq!(kinds_of(&lamp.device), [Kinds::Light]);
    }

    #[test]
    fn a_plain_relay_is_a_switch_and_a_metering_one_an_outlet() {
        let relay = view(
            "a_relay",
            vec![point("state", Kind::Binary, true, Semantic::OnOff)],
        );
        let plug = view(
            "b_plug",
            vec![
                point("state", Kind::Binary, true, Semantic::OnOff),
                point("power", range(0.0, 3000.0), false, Semantic::Power),
            ],
        );
        let c = catalog(&[relay, plug]);
        assert_eq!(types(&c, 2), ["3E", "49"]);
        assert_eq!(types(&c, 3), ["3E", "47"]);
    }

    #[test]
    fn dimmers_attach_to_the_right_light_of_a_two_gang_device() {
        let device = view(
            "two",
            vec![
                point("state_l1", Kind::Binary, true, Semantic::OnOff),
                point("state_l2", Kind::Binary, true, Semantic::OnOff),
                point(
                    "brightness_l2",
                    range(0.0, 254.0),
                    true,
                    Semantic::Brightness,
                ),
            ],
        );
        let c = catalog(&[device]);
        assert_eq!(types(&c, 2), ["3E", "49", "43"]);
        let light = &c.accessories[1].services[2];
        assert!(light.chars.iter().any(|ch| ch.kind == C_BRIGHTNESS));
        assert_eq!(c.accessories[1].services[1].chars.len(), 2);
    }

    #[test]
    fn hue_style_keys_group_by_prefix() {
        assert_eq!(tag("on"), "");
        assert_eq!(tag("brightness"), "");
        assert_eq!(tag("light2.on"), "light2.");
        assert_eq!(tag("light2.color_temp"), "light2.");
        assert_eq!(tag("state_l1"), "_l1");
        assert_eq!(tag("color_temp_l1"), "_l1");
        assert_eq!(tag("color.x"), "color.x");
    }

    #[test]
    fn sensors_map_by_meaning() {
        let sensor = view(
            "multi",
            vec![
                point("occupancy", Kind::Binary, false, Semantic::Occupancy),
                point(
                    "temperature",
                    range(-20.0, 60.0),
                    false,
                    Semantic::Temperature,
                ),
                point(
                    "illuminance",
                    range(0.0, 10000.0),
                    false,
                    Semantic::Illuminance,
                ),
                point("humidity", range(0.0, 100.0), false, Semantic::Humidity),
                point("setpoint", range(5.0, 30.0), true, Semantic::Temperature),
                point("battery", range(0.0, 100.0), false, Semantic::Battery),
            ],
        );
        let c = catalog(std::slice::from_ref(&sensor));
        assert_eq!(types(&c, 2), ["3E", "85", "8A", "84", "82"]);
        assert_eq!(
            kinds_of(&sensor.device),
            [
                Kinds::Temperature,
                Kinds::Humidity,
                Kinds::Motion,
                Kinds::LightLevel
            ]
        );
        let door = view(
            "door",
            vec![point("contact", Kind::Binary, false, Semantic::Contact)],
        );
        let leak = view(
            "leak",
            vec![point(
                "water_leak",
                Kind::Binary,
                false,
                Semantic::WaterLeak,
            )],
        );
        let c = catalog(&[door, leak]);
        assert_eq!(types(&c, 2), ["3E", "80"]);
        assert_eq!(types(&c, 3), ["3E", "83"]);
    }

    #[test]
    fn things_homekit_has_no_place_for_are_left_out() {
        let camera = view(
            "cam",
            vec![point("recording", Kind::Binary, false, Semantic::Other)],
        );
        let c = catalog(&[camera]);
        assert_eq!(c.accessories.len(), 1);
    }

    #[test]
    fn the_alarm_is_a_security_system_with_night_for_partial() {
        let mut mode = point(
            "mode",
            Kind::Enum {
                values: vec!["disarmed".into(), "partial".into(), "total".into()],
            },
            true,
            Semantic::Control,
        );
        mode.access.read = true;
        let alarm = view(
            "alarm",
            vec![point("state", Kind::Text, false, Semantic::Other), mode],
        );
        let c = catalog(std::slice::from_ref(&alarm));
        assert_eq!(types(&c, 2), ["3E", "7E"]);
        let target = c.accessories[1].services[1]
            .chars
            .iter()
            .find(|ch| ch.kind == C_SEC_TARGET)
            .unwrap();
        assert!(c.find(2, target.iid).is_some());
        assert_eq!(target.meta.valid_values, Some(vec![1, 2, 3]));
        assert_eq!(kinds_of(&alarm.device), [Kinds::Alarm]);
        assert_eq!(arm_target_of("partial"), Some(2));
        assert_eq!(arm_target_of("total"), Some(1));
        assert_eq!(arm_target_of("disarmed"), Some(3));
        assert_eq!(mode_of_arm_target(2), Some("partial"));
        assert_eq!(mode_of_arm_target(0), None);
        assert_eq!(arm_current_of("partial_night_perimeter"), Some(2));
        assert_eq!(arm_current_of("total_annex"), Some(1));
        assert_eq!(arm_current_of("perimeter"), Some(0));
        assert_eq!(arm_current_of("mystery"), None);
    }

    #[test]
    fn identifiers_are_stable_and_follow_the_device() {
        let a = view(
            "a",
            vec![point("state", Kind::Binary, true, Semantic::OnOff)],
        );
        let b = view(
            "b",
            vec![point("state", Kind::Binary, true, Semantic::OnOff)],
        );
        let mut aids = Aids::default();
        let first = Catalog::build("Moli", &[a.clone(), b.clone()], |_| true, &mut aids);
        assert_eq!(first.accessories[1].aid, 2);
        assert_eq!(first.accessories[2].aid, 3);
        // `a` leaves, then comes back: it keeps its place; `b` keeps its.
        let only_b = Catalog::build(
            "Moli",
            &[a.clone(), b.clone()],
            |key| key.ends_with(":b"),
            &mut aids,
        );
        assert_eq!(only_b.accessories.len(), 2);
        assert_eq!(only_b.accessories[1].aid, 3);
        let again = Catalog::build("Moli", &[a, b], |_| true, &mut aids);
        assert_eq!(again.accessories[1].aid, 2);
        // Same house, same database.
        assert_eq!(first.structure(), again.structure());
    }

    #[test]
    fn the_bridge_comes_first_with_its_protocol_version() {
        let c = catalog(&[]);
        let json = c.structure();
        let bridge = &json["accessories"][0];
        assert_eq!(bridge["aid"], 1);
        assert_eq!(bridge["services"][0]["iid"], 1);
        assert_eq!(bridge["services"][0]["type"], "3E");
        assert_eq!(bridge["services"][1]["type"], "A2");
    }

    #[test]
    fn at_most_140_accessories() {
        let views: Vec<_> = (0..150)
            .map(|i| {
                view(
                    &format!("d{i:03}"),
                    vec![point("state", Kind::Binary, true, Semantic::OnOff)],
                )
            })
            .collect();
        let c = catalog(&views);
        assert_eq!(c.accessories.len(), 1 + MAX_ACCESSORIES);
        assert!(c.truncated);
    }

    #[test]
    fn values_convert_both_ways() {
        let pct = Conv::Percent { lo: 0.0, hi: 254.0 };
        assert_eq!(to_hap(pct, &Value::Float(127.0)), Some(json!(50)));
        assert_eq!(from_hap(pct, &json!(100)), Some(Value::Float(254.0)));
        assert_eq!(to_hap(Conv::Bool, &Value::Bool(true)), Some(json!(true)));
        assert_eq!(from_hap(Conv::Bool, &json!(0)), Some(Value::Bool(false)));
        assert_eq!(from_hap(Conv::Bool, &json!("yes")), None);
        // closed contact = detected = 0
        assert_eq!(to_hap(Conv::Contact, &Value::Bool(true)), Some(json!(0)));
        assert_eq!(to_hap(Conv::Contact, &Value::Bool(false)), Some(json!(1)));
        assert_eq!(to_hap(Conv::Detected, &Value::Bool(true)), Some(json!(1)));
        assert_eq!(to_hap(Conv::Lux, &Value::Float(0.0)), Some(json!(0.0001)));
        assert_eq!(to_hap(Conv::Lux, &Value::from("x")), None);
        // HAP bounds a temperature: a reading outside is clamped, not refused.
        let bounded = Conv::Clamped {
            lo: -270.0,
            hi: 100.0,
        };
        assert_eq!(to_hap(bounded, &Value::Float(220.0)), Some(json!(100.0)));
        assert_eq!(to_hap(bounded, &Value::Int(21)), Some(json!(21.0)));
        assert_eq!(to_hap(Conv::Int, &Value::Float(366.4)), Some(json!(366)));
    }

    #[test]
    fn the_maker_is_moli_whatever_the_device_says() {
        let mut lamp = view(
            "lamp",
            vec![point("on", Kind::Binary, true, Semantic::OnOff)],
        );
        let mut device = (*lamp.device).clone();
        device.manufacturer = Some("Signify Netherlands B.V.".into());
        device.model = Some("LCL001".into());
        lamp.device = Arc::new(device);
        let c = catalog(&[lamp]);
        let info = &c.accessories[1].services[0].chars;
        assert_eq!(info[1].bind, Bind::Static(json!("Moli")));
        assert_eq!(
            info[2].bind,
            Bind::Static(json!("Signify Netherlands B.V. LCL001"))
        );
    }

    #[test]
    fn a_silent_device_still_has_a_value_in_the_database() {
        let sensor = view(
            "quiet",
            vec![point("occupancy", Kind::Binary, false, Semantic::Occupancy)],
        );
        let c = catalog(&[sensor]);
        let json = c.render(&|ch| Some(placeholder(ch)));
        let chars = json["accessories"][1]["services"][1]["characteristics"]
            .as_array()
            .unwrap();
        assert_eq!(chars[1]["value"], false);
        assert_eq!(
            placeholder(&c.accessories[1].services[0].chars[0]),
            json!(false)
        );
    }

    #[test]
    fn colours_round_trip_through_hue_and_saturation() {
        let (h, s) = hex_to_hs("#ff0000").unwrap();
        assert_eq!((h.round(), s.round()), (0.0, 100.0));
        let (h, s) = hex_to_hs("#00ff00").unwrap();
        assert_eq!((h.round(), s.round()), (120.0, 100.0));
        assert_eq!(hs_to_hex(240.0, 100.0), "#0000ff");
        assert_eq!(hs_to_hex(0.0, 0.0), "#ffffff");
        assert_eq!(hs_to_hex(60.0, 100.0), "#ffff00");
        for hex in ["#ff8000", "#12ab34", "#ffffff"] {
            let (h, s) = hex_to_hs(hex).unwrap();
            let back = hex_to_hs(&hs_to_hex(h, s)).unwrap();
            assert!(
                (back.0 - h).abs() < 1.5 && (back.1 - s).abs() < 1.5,
                "{hex}"
            );
        }
        assert_eq!(hex_to_hs("red"), None);
    }

    #[test]
    fn names_come_from_the_label_first() {
        let v = view(
            "native",
            vec![point("state", Kind::Binary, true, Semantic::OnOff)],
        );
        assert_eq!(v.name, "native");
        let labelled = Entry::new(
            v.device,
            &Label {
                name: Some("Salon".into()),
                room: None,
            },
            false,
        );
        assert_eq!(labelled.name, "Salon");
    }

    #[test]
    fn a_point_feeds_the_characteristics_that_read_it() {
        let lamp = view(
            "lamp",
            vec![
                point("on", Kind::Binary, true, Semantic::OnOff),
                point("color", Kind::Text, true, Semantic::Color),
            ],
        );
        let c = catalog(std::slice::from_ref(&lamp));
        let color = PointId::new(&lamp.device.id, "color");
        // hue and saturation both
        assert_eq!(c.watching(&color).len(), 2);
        assert!(
            c.watching(&PointId::new(&lamp.device.id, "nope"))
                .is_empty()
        );
    }
}
