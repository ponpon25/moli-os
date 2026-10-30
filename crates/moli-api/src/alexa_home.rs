//! Alexa Smart Home: the house's devices in the Alexa app, so « Alexa,
//! allume la télé » needs no « demande à Moli ». Amazon sends its
//! directives (Smart Home API v3) to the skill's AWS Lambda, which only
//! relays them here, `POST /api/alexa`, with a token shared with Moli (the
//! `alexa-skill/token` secret). The one way in from the Internet besides a
//! phone's report; on the name given to Amazon (`[alexa] host`), nothing
//! else answers.
//!
//! Which devices Alexa sees is chosen in the dashboard (Réglages →
//! Intégrations), kept in `alexa.json` next to the other data: by default
//! every real device, not the helpers (virtual switches such as a
//! presence). The name Alexa hears is the device's voice name.
//!
//! An order goes to the hub as origin `api`, actor « alexa »: the guard
//! applies, as for HomeKit (held in a protected room or at quiet hours,
//! told to Alexa as done, approved in the dashboard).

use std::collections::BTreeMap;
use std::path::{Path as FsPath, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use axum::Extension;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use moli_core::{InstanceId, Kind, Origin, PointId, PointSpec, Semantic, Value};
use moli_runtime::{CommandError, DeviceView, DriverView, Hub};
use serde::{Deserialize, Serialize};
use serde_json::{Value as Json, json};

use crate::caller::Caller;
use crate::session::Sessions;

/// Where the shared token lives: `moli-os secrets set alexa-skill token`.
pub const INSTANCE: &str = "alexa-skill";
pub const TOKEN: &str = "token";
const ACTOR: &str = "alexa";

/// `[alexa]` in `moli.toml`.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AlexaConfig {
    /// The name Amazon reaches Moli by (through the home's reverse proxy):
    /// on it, only `/api/alexa` answers.
    #[serde(default)]
    pub host: Option<String>,
}

/// The skill as the API sees it: its settings, the household's choice of
/// devices, the last directive received.
#[derive(Debug)]
pub(crate) struct Alexa {
    config: AlexaConfig,
    path: Option<PathBuf>,
    /// Per-device choices that depart from the default.
    choices: Mutex<BTreeMap<String, bool>>,
    /// When Amazon last reached Moli (ms), 0: never since start.
    last: AtomicU64,
}

#[derive(Default, Serialize, Deserialize)]
struct Saved {
    #[serde(default)]
    exceptions: BTreeMap<String, bool>,
}

impl Alexa {
    /// The choices are read from `alexa.json` in `data_dir`; none: every
    /// device follows the default.
    pub(crate) fn open(config: AlexaConfig, data_dir: Option<&FsPath>) -> Self {
        let path = data_dir.map(|d| d.join("alexa.json"));
        let saved: Saved = path
            .as_deref()
            .and_then(|p| std::fs::read(p).ok())
            .and_then(|bytes| match serde_json::from_slice(&bytes) {
                Ok(saved) => Some(saved),
                Err(e) => {
                    tracing::error!(error = %e, "alexa: alexa.json unreadable, defaults used");
                    None
                }
            })
            .unwrap_or_default();
        Self {
            config,
            path,
            choices: Mutex::new(saved.exceptions),
            last: AtomicU64::new(0),
        }
    }

    fn exposed(&self, view: &DeviceView, drivers: &[DriverView]) -> bool {
        let id = view.device.id.as_str();
        self.choices
            .lock()
            .expect("alexa choices")
            .get(id)
            .copied()
            .unwrap_or_else(|| by_default(view, drivers))
    }

    /// A device's own choice; `None` goes back to the default.
    fn choose(&self, id: &str, default: bool, exposed: Option<bool>) -> std::io::Result<()> {
        let mut choices = self.choices.lock().expect("alexa choices");
        match exposed {
            Some(choice) if choice != default => {
                choices.insert(id.to_owned(), choice);
            }
            _ => {
                choices.remove(id);
            }
        }
        let Some(path) = &self.path else {
            return Ok(());
        };
        let bytes = serde_json::to_vec_pretty(&Saved {
            exceptions: choices.clone(),
        })
        .map_err(std::io::Error::other)?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, bytes)?;
        std::fs::rename(&tmp, path)
    }
}

/// Without a choice: every real device, not a helper's virtual switch.
fn by_default(view: &DeviceView, drivers: &[DriverView]) -> bool {
    !drivers
        .iter()
        .any(|d| d.instance == view.device.instance && d.kind == "helpers")
}

#[derive(Clone)]
pub(crate) struct AlexaGate(pub(crate) Option<Arc<Alexa>>);

/// Whether `host` (a `Host` header, port or not) is the name given to Amazon.
pub(crate) fn is_skill_host(config: Option<&AlexaConfig>, host: &str) -> bool {
    let name = host.rsplit_once(':').map_or(host, |(name, port)| {
        if port.bytes().all(|b| b.is_ascii_digit()) {
            name
        } else {
            host
        }
    });
    config
        .and_then(|c| c.host.as_deref())
        .is_some_and(|h| h.eq_ignore_ascii_case(name))
}

/// `POST /api/alexa`: one directive, relayed by the skill's Lambda.
pub(crate) async fn directive(
    State(hub): State<Hub>,
    Extension(AlexaGate(alexa)): Extension<AlexaGate>,
    headers: HeaderMap,
    axum::Json(request): axum::Json<Json>,
) -> Response {
    let Some(alexa) = alexa else {
        return (StatusCode::NOT_FOUND, "alexa is not configured").into_response();
    };
    let expected = hub.secret(&InstanceId::from(INSTANCE), TOKEN);
    let given = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::trim);
    if !token_matches(expected.as_deref(), given) {
        tracing::warn!("alexa: token refused");
        return (StatusCode::UNAUTHORIZED, "token refused").into_response();
    }
    alexa.last.store(moli_core::now_ms(), Ordering::Relaxed);
    axum::Json(handle(&hub, &alexa, &request).await).into_response()
}

fn refuse(status: StatusCode, message: &str) -> Response {
    (status, axum::Json(json!({ "error": message }))).into_response()
}

/// `GET /api/integrations/alexa`: the skill's state and every device Alexa
/// has a place for, with the names Alexa hears and whether it is shown.
pub(crate) async fn settings(
    State(hub): State<Hub>,
    Extension(AlexaGate(alexa)): Extension<AlexaGate>,
) -> Response {
    let Some(alexa) = alexa else {
        return axum::Json(json!({ "enabled": false })).into_response();
    };
    let snapshot = hub.snapshot();
    let all = endpoints(&snapshot.devices, |_| true);
    let mut devices: Vec<Json> = Vec::new();
    for view in &snapshot.devices {
        let names: Vec<&str> = all
            .iter()
            .filter(|e| e.view.device.id == view.device.id)
            .map(|e| e.name.as_str())
            .collect();
        if names.is_empty() {
            continue;
        }
        let id = view.device.id.as_str();
        let default = by_default(view, &snapshot.drivers);
        let exception = alexa
            .choices
            .lock()
            .expect("alexa choices")
            .contains_key(id);
        devices.push(json!({
            "id": id,
            "names": names,
            "exposed": alexa.exposed(view, &snapshot.drivers),
            "default": default,
            "exception": exception,
        }));
    }
    let last = alexa.last.load(Ordering::Relaxed);
    axum::Json(json!({
        "enabled": true,
        "host": alexa.config.host,
        "token": hub.secret(&InstanceId::from(INSTANCE), TOKEN).is_some(),
        "last": (last > 0).then_some(last),
        "devices": devices,
    }))
    .into_response()
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Choice {
    exposed: Option<bool>,
}

/// `PUT /api/integrations/alexa/{id}` `{ exposed: true | false | null }`
/// (`null`: back to the default). A person's (code): it decides what
/// Amazon sees.
pub(crate) async fn choose(
    State(hub): State<Hub>,
    Extension(AlexaGate(alexa)): Extension<AlexaGate>,
    Extension(humans): Extension<Arc<Sessions>>,
    caller: Caller,
    headers: HeaderMap,
    Path(id): Path<String>,
    axum::Json(body): axum::Json<Choice>,
) -> Response {
    let Some(alexa) = alexa else {
        return refuse(
            StatusCode::SERVICE_UNAVAILABLE,
            &moli_i18n::tr!("serveur.alexa.absent"),
        );
    };
    if !humans.is_human(&headers, &caller.key) {
        return refuse(
            StatusCode::FORBIDDEN,
            &moli_i18n::tr!("serveur.alexa.humain"),
        );
    }
    let snapshot = hub.snapshot();
    let Some(view) = snapshot
        .devices
        .iter()
        .find(|v| v.device.id.as_str() == id)
        .filter(|v| !endpoints(std::slice::from_ref(*v), |_| true).is_empty())
    else {
        return refuse(
            StatusCode::NOT_FOUND,
            &moli_i18n::tr!("serveur.alexa.appareil_inconnu"),
        );
    };
    let default = by_default(view, &snapshot.drivers);
    if let Err(e) = alexa.choose(&id, default, body.exposed) {
        tracing::error!(error = %e, "alexa: cannot write alexa.json");
        return refuse(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string());
    }
    tracing::info!(device = %id, exposed = ?body.exposed, "alexa: a device's choice changed");
    axum::Json(json!({ "id": id, "exposed": body.exposed.unwrap_or(default) })).into_response()
}

/// Compared through their digests: no early exit on the first difference.
pub(crate) fn token_matches(expected: Option<&str>, given: Option<&str>) -> bool {
    let (Some(expected), Some(given)) = (expected, given) else {
        return false;
    };
    if expected.len() < 16 {
        return false;
    }
    let digest = |s: &str| ring::digest::digest(&ring::digest::SHA256, s.as_bytes());
    digest(expected).as_ref() == digest(given).as_ref()
}

async fn handle(hub: &Hub, alexa: &Alexa, request: &Json) -> Json {
    let directive = &request["directive"];
    let head = &directive["header"];
    let namespace = head["namespace"].as_str().unwrap_or_default();
    let name = head["name"].as_str().unwrap_or_default();
    let correlation = head["correlationToken"].as_str();
    let snapshot = hub.snapshot();
    let endpoints = endpoints(&snapshot.devices, |v| alexa.exposed(v, &snapshot.drivers));
    match (namespace, name) {
        ("Alexa.Discovery", "Discover") => {
            let names: Vec<&str> = endpoints.iter().map(|e| e.name.as_str()).collect();
            tracing::info!(count = names.len(), names = ?names, "alexa: discovery");
            discovery(&endpoints)
        }
        ("Alexa.Authorization", "AcceptGrant") => json!({
            "event": {
                "header": header("Alexa.Authorization", "AcceptGrant.Response", None),
                "payload": {},
            }
        }),
        _ => {
            let id = directive["endpoint"]["endpointId"]
                .as_str()
                .unwrap_or_default();
            let Some(endpoint) = endpoints.iter().find(|e| e.id == id) else {
                return error(id, correlation, "NO_SUCH_ENDPOINT", "unknown endpoint");
            };
            if namespace == "Alexa" && name == "ReportState" {
                return answer(endpoint, "StateReport", correlation);
            }
            if endpoint.view.online == Some(false)
                && !(namespace == "Alexa.PowerController" && name == "TurnOn")
            {
                // Asleep: only « switch on » reaches it (a TV wakes up).
                return error(id, correlation, "ENDPOINT_UNREACHABLE", "offline");
            }
            let result = order(hub, endpoint, namespace, name, &directive["payload"]).await;
            tracing::info!(endpoint = %endpoint.name, directive = %format!("{namespace}.{name}"), ok = result.is_ok(), "alexa: a directive");
            match result {
                Ok(()) => answer(endpoint, "Response", correlation),
                Err((kind, message)) => error(id, correlation, kind, &message),
            }
        }
    }
}

/// One Alexa device: an on/off point and what goes with it (a light's
/// brightness and colours, a TV's volume).
struct Endpoint<'a> {
    id: String,
    view: &'a DeviceView,
    on: &'a PointSpec,
    /// `on` is a dimmer's level with no switch of its own (a Z-Wave
    /// multilevel switch): on above its minimum.
    level: bool,
    brightness: Option<&'a PointSpec>,
    color_temp: Option<&'a PointSpec>,
    color: Option<&'a PointSpec>,
    volume: Option<&'a PointSpec>,
    mute: Option<&'a PointSpec>,
    name: String,
    category: &'static str,
}

impl Endpoint<'_> {
    fn point(&self, spec: &PointSpec) -> PointId {
        PointId::new(&self.view.device.id, &spec.key)
    }

    fn value(&self, spec: &PointSpec) -> Option<&Value> {
        self.view.state.get(&spec.key).map(|s| &s.value)
    }

    fn time(&self, spec: &PointSpec) -> String {
        let ms = self
            .view
            .state
            .get(&spec.key)
            .map_or_else(moli_core::now_ms, |s| s.ts);
        iso_time(ms)
    }
}

/// The part of a key that tells which light it belongs to on a device with
/// several: `state_l1` → `_l1`, `light2.on` → `light2.`.
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

fn endpoints(views: &[DeviceView], shown: impl Fn(&DeviceView) -> bool) -> Vec<Endpoint<'_>> {
    let mut out = Vec::new();
    for view in views.iter().filter(|v| shown(v)) {
        let device = &view.device;
        let writable = |p: &&PointSpec| p.access.write;
        let switches: Vec<&PointSpec> = device
            .points
            .iter()
            .filter(|p| p.semantic == Semantic::OnOff && p.access.write && p.kind == Kind::Binary)
            // A speaker's play/pause is no switch (and Alexa knows the
            // speakers through their own skills).
            .filter(|p| &*p.key != "playing")
            .collect();
        let single = switches.len() == 1;
        let has_power = device.points.iter().any(|p| p.semantic == Semantic::Power);
        let base = view.label.spoken(&device.native_name).trim().to_owned();
        for on in &switches {
            // The device's own on/off (a TV's power) carries its name and
            // its volume; the others (its Ambilight) are named after it.
            let main = single || matches!(&*on.key, "power" | "state" | "on");
            let mine = |p: &&PointSpec| single || tag(&p.key) == tag(&on.key);
            let find = |semantic: Semantic| {
                device
                    .points
                    .iter()
                    .filter(writable)
                    .filter(|p| p.semantic == semantic)
                    .find(mine)
            };
            let brightness = find(Semantic::Brightness);
            let color_temp = find(Semantic::ColorTemp);
            let color = find(Semantic::Color).filter(|p| p.kind == Kind::Text);
            // A TV, a receiver: its volume and mute (single on/off only).
            let by_key = |key: &str| {
                device
                    .points
                    .iter()
                    .filter(writable)
                    .find(|p| main && &*p.key == key)
            };
            let volume = by_key("volume").filter(|p| matches!(p.kind, Kind::Numeric { .. }));
            let mute = by_key("mute").filter(|p| p.kind == Kind::Binary);
            let category = if volume.is_some() {
                "TV"
            } else if brightness.is_some() || color_temp.is_some() || color.is_some() {
                "LIGHT"
            } else if has_power {
                "SMARTPLUG"
            } else {
                "SWITCH"
            };
            let name = if main {
                base.clone()
            } else {
                format!("{base} {}", on.label)
            };
            out.push(Endpoint {
                id: hex(PointId::new(&device.id, &on.key).as_str()),
                view,
                on,
                level: false,
                brightness,
                color_temp,
                color,
                volume,
                mute,
                name,
                category,
            });
        }
        // A dimmer with no switch (a Z-Wave multilevel switch): its level
        // is the switch, 0 meaning off.
        let level = device.points.iter().find(|p| {
            p.semantic == Semantic::Brightness
                && p.access.write
                && matches!(p.kind, Kind::Numeric { .. })
        });
        if let (true, Some(level)) = (switches.is_empty(), level) {
            out.push(Endpoint {
                id: hex(PointId::new(&device.id, &level.key).as_str()),
                view,
                on: level,
                level: true,
                brightness: Some(level),
                color_temp: None,
                color: None,
                volume: None,
                mute: None,
                name: base,
                category: "LIGHT",
            });
        }
    }
    out
}

/// A dimmer's level read as on or off.
fn level_on(spec: &PointSpec, value: &Value) -> Option<bool> {
    Some(value.as_f64()? > range(spec).0)
}

/// Alexa's endpoint ids take few characters: the point id, in hex.
fn hex(text: &str) -> String {
    use std::fmt::Write as _;
    text.bytes()
        .fold(String::with_capacity(text.len() * 2), |mut out, b| {
            let _ = write!(out, "{b:02x}");
            out
        })
}

fn capability(interface: &str, properties: &[&str]) -> Json {
    let mut cap = json!({ "type": "AlexaInterface", "interface": interface, "version": "3" });
    if !properties.is_empty() {
        cap["properties"] = json!({
            "supported": properties.iter().map(|p| json!({ "name": p })).collect::<Vec<_>>(),
            "proactivelyReported": false,
            "retrievable": true,
        });
    }
    cap
}

fn discovery(endpoints: &[Endpoint<'_>]) -> Json {
    let list: Vec<Json> = endpoints
        .iter()
        .map(|e| {
            let mut caps = vec![
                capability("Alexa", &[]),
                capability("Alexa.PowerController", &["powerState"]),
                capability("Alexa.EndpointHealth", &["connectivity"]),
            ];
            if e.brightness.is_some() {
                caps.push(capability("Alexa.BrightnessController", &["brightness"]));
            }
            if e.color.is_some() {
                caps.push(capability("Alexa.ColorController", &["color"]));
            }
            if e.color_temp.is_some() {
                caps.push(capability(
                    "Alexa.ColorTemperatureController",
                    &["colorTemperatureInKelvin"],
                ));
            }
            if e.volume.is_some() {
                let props: &[&str] = if e.mute.is_some() {
                    &["volume", "muted"]
                } else {
                    &["volume"]
                };
                caps.push(capability("Alexa.Speaker", props));
            }
            let device = &e.view.device;
            let description = match (&e.view.label.room, &device.model) {
                (Some(room), _) => format!("Moli · {room}"),
                (None, Some(model)) => format!("Moli · {model}"),
                (None, None) => "Moli".to_owned(),
            };
            json!({
                "endpointId": e.id,
                "manufacturerName": device.manufacturer.as_deref().unwrap_or("Moli"),
                "friendlyName": e.name,
                "description": description,
                "displayCategories": [e.category],
                "capabilities": caps,
            })
        })
        .collect();
    json!({
        "event": {
            "header": header("Alexa.Discovery", "Discover.Response", None),
            "payload": { "endpoints": list },
        }
    })
}

fn header(namespace: &str, name: &str, correlation: Option<&str>) -> Json {
    let mut head = json!({
        "namespace": namespace,
        "name": name,
        "payloadVersion": "3",
        "messageId": message_id(),
    });
    if let Some(token) = correlation {
        head["correlationToken"] = json!(token);
    }
    head
}

fn message_id() -> String {
    use std::fmt::Write as _;
    let mut bytes = [0u8; 16];
    let _ = ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut bytes);
    bytes.iter().fold(String::with_capacity(32), |mut out, b| {
        let _ = write!(out, "{b:02x}");
        out
    })
}

fn error(endpoint: &str, correlation: Option<&str>, kind: &str, message: &str) -> Json {
    json!({
        "event": {
            "header": header("Alexa", "ErrorResponse", correlation),
            "endpoint": { "endpointId": endpoint },
            "payload": { "type": kind, "message": message },
        }
    })
}

fn answer(endpoint: &Endpoint<'_>, name: &str, correlation: Option<&str>) -> Json {
    json!({
        "event": {
            "header": header("Alexa", name, correlation),
            "endpoint": { "endpointId": endpoint.id },
            "payload": {},
        },
        "context": { "properties": properties(endpoint) },
    })
}

fn property(namespace: &str, name: &str, value: Json, time: String) -> Json {
    let mut property = json!({
        "namespace": namespace,
        "name": name,
        "uncertaintyInMilliseconds": 500,
    });
    property["value"] = value;
    property["timeOfSample"] = Json::String(time);
    property
}

fn properties(e: &Endpoint<'_>) -> Vec<Json> {
    let mut out = Vec::new();
    let online = e.view.online != Some(false);
    out.push(property(
        "Alexa.EndpointHealth",
        "connectivity",
        json!({ "value": if online { "OK" } else { "UNREACHABLE" } }),
        iso_time(moli_core::now_ms()),
    ));
    let on = e.value(e.on).and_then(|v| {
        if e.level {
            level_on(e.on, v)
        } else {
            as_bool(v)
        }
    });
    if let Some(on) = on {
        out.push(property(
            "Alexa.PowerController",
            "powerState",
            json!(if on { "ON" } else { "OFF" }),
            e.time(e.on),
        ));
    }
    if let Some(b) = e.brightness
        && let Some(pct) = e.value(b).and_then(|v| percent(b, v))
    {
        out.push(property(
            "Alexa.BrightnessController",
            "brightness",
            json!(pct),
            e.time(b),
        ));
    }
    if let Some(c) = e.color
        && let Some(Value::Text(hex)) = e.value(c)
        && let Some((hue, saturation, brightness)) = hex_to_hsb(hex)
    {
        out.push(property(
            "Alexa.ColorController",
            "color",
            json!({ "hue": hue, "saturation": saturation, "brightness": brightness }),
            e.time(c),
        ));
    }
    if let Some(ct) = e.color_temp
        && let Some(mired) = e.value(ct).and_then(Value::as_f64).filter(|m| *m > 0.0)
    {
        out.push(property(
            "Alexa.ColorTemperatureController",
            "colorTemperatureInKelvin",
            json!(kelvin_of(mired)),
            e.time(ct),
        ));
    }
    if let Some(v) = e.volume
        && let Some(pct) = e.value(v).and_then(|value| percent(v, value))
    {
        out.push(property("Alexa.Speaker", "volume", json!(pct), e.time(v)));
    }
    if let Some(m) = e.mute
        && let Some(muted) = e.value(m).and_then(as_bool)
    {
        out.push(property("Alexa.Speaker", "muted", json!(muted), e.time(m)));
    }
    out
}

type Refusal = (&'static str, String);

#[allow(clippy::too_many_lines)] // one arm per directive
async fn order(
    hub: &Hub,
    e: &Endpoint<'_>,
    namespace: &str,
    name: &str,
    payload: &Json,
) -> Result<(), Refusal> {
    let invalid = |what: &str| ("INVALID_VALUE", what.to_owned());
    let unsupported = || ("INVALID_DIRECTIVE", format!("{namespace}.{name}"));
    let number = |key: &str| payload[key].as_f64().filter(|n| n.is_finite());
    match (namespace, name) {
        // A dimmer's level: full on, its minimum off (as HomeKit does).
        ("Alexa.PowerController", "TurnOn") if e.level => {
            send(hub, e.point(e.on), from_percent(e.on, 100.0)).await
        }
        ("Alexa.PowerController", "TurnOff") if e.level => {
            send(hub, e.point(e.on), from_percent(e.on, 0.0)).await
        }
        ("Alexa.PowerController", "TurnOn") => send(hub, e.point(e.on), Value::Bool(true)).await,
        ("Alexa.PowerController", "TurnOff") => send(hub, e.point(e.on), Value::Bool(false)).await,
        ("Alexa.BrightnessController", verb) => {
            let b = e.brightness.ok_or_else(unsupported)?;
            let pct = match verb {
                "SetBrightness" => number("brightness").ok_or_else(|| invalid("brightness"))?,
                "AdjustBrightness" => {
                    let delta =
                        number("brightnessDelta").ok_or_else(|| invalid("brightnessDelta"))?;
                    let now = e
                        .value(b)
                        .and_then(|v| percent(b, v))
                        .map_or(0.0, f64::from);
                    now + delta
                }
                _ => return Err(unsupported()),
            };
            send(hub, e.point(b), from_percent(b, pct)).await
        }
        ("Alexa.ColorController", "SetColor") => {
            let c = e.color.ok_or_else(unsupported)?;
            let color = &payload["color"];
            let (Some(hue), Some(saturation)) =
                (color["hue"].as_f64(), color["saturation"].as_f64())
            else {
                return Err(invalid("color"));
            };
            send(
                hub,
                e.point(c),
                Value::Text(hs_to_hex(hue, saturation).into()),
            )
            .await
        }
        ("Alexa.ColorTemperatureController", verb) => {
            let ct = e.color_temp.ok_or_else(unsupported)?;
            let now = e
                .value(ct)
                .and_then(Value::as_f64)
                .filter(|m| *m > 0.0)
                .map_or(4000.0, |mired| 1_000_000.0 / mired);
            let kelvin = match verb {
                "SetColorTemperature" => number("colorTemperatureInKelvin")
                    .ok_or_else(|| invalid("colorTemperatureInKelvin"))?,
                "IncreaseColorTemperature" => now + 500.0,
                "DecreaseColorTemperature" => now - 500.0,
                _ => return Err(unsupported()),
            };
            let (lo, hi) = range(ct);
            let mired = (1_000_000.0 / kelvin.clamp(1000.0, 10_000.0))
                .clamp(lo, hi)
                .round();
            send(hub, e.point(ct), integral(ct, mired)).await
        }
        ("Alexa.Speaker", "SetVolume" | "AdjustVolume") => {
            let v = e.volume.ok_or_else(unsupported)?;
            let pct = number("volume").ok_or_else(|| invalid("volume"))?;
            let pct = if name == "AdjustVolume" {
                let now = e
                    .value(v)
                    .and_then(|value| percent(v, value))
                    .map_or(0.0, f64::from);
                // « Plus fort » without a number: a few steps.
                let step = if payload["volumeDefault"].as_bool() == Some(true) {
                    pct.signum() * 5.0
                } else {
                    pct
                };
                now + step
            } else {
                pct
            };
            send(hub, e.point(v), from_percent(v, pct)).await
        }
        ("Alexa.Speaker", "SetMute") => {
            let m = e.mute.ok_or_else(unsupported)?;
            let mute = payload["mute"].as_bool().ok_or_else(|| invalid("mute"))?;
            send(hub, e.point(m), Value::Bool(mute)).await
        }
        _ => Err(unsupported()),
    }
}

/// To the hub, as origin `api`: a held order is told to Alexa as done (it
/// waits for a human, as HomeKit's do).
async fn send(hub: &Hub, point: PointId, value: Value) -> Result<(), Refusal> {
    match hub
        .command(&point, value, Origin::Api, Some(ACTOR.to_owned()))
        .await
    {
        Ok(()) | Err(CommandError::NeedsApproval { .. }) => Ok(()),
        Err(CommandError::Invalid(e)) => Err(("INVALID_VALUE", e.to_string())),
        Err(e) => {
            tracing::debug!(%point, error = %e, "alexa: an order failed");
            Err(("ENDPOINT_UNREACHABLE", e.to_string()))
        }
    }
}

fn as_bool(value: &Value) -> Option<bool> {
    match value {
        Value::Bool(b) => Some(*b),
        Value::Int(i) => Some(*i != 0),
        _ => None,
    }
}

fn range(spec: &PointSpec) -> (f64, f64) {
    match spec.kind {
        Kind::Numeric { min, max, .. } => (min.unwrap_or(0.0), max.unwrap_or(100.0)),
        _ => (0.0, 100.0),
    }
}

#[allow(clippy::cast_possible_truncation)] // clamped to 0..=100
fn percent(spec: &PointSpec, value: &Value) -> Option<i32> {
    let (lo, hi) = range(spec);
    let span = (hi - lo).max(f64::EPSILON);
    Some(
        ((value.as_f64()? - lo) * 100.0 / span)
            .round()
            .clamp(0.0, 100.0) as i32,
    )
}

fn from_percent(spec: &PointSpec, pct: f64) -> Value {
    let (lo, hi) = range(spec);
    let raw = lo + pct.clamp(0.0, 100.0) * (hi - lo) / 100.0;
    integral(spec, raw)
}

/// Whole steps stay whole (a TV's volume takes integers), and so does a
/// range of whole bounds without a step (a Z-Wave level, 0–99).
#[allow(clippy::cast_possible_truncation)] // a point's own range
fn integral(spec: &PointSpec, raw: f64) -> Value {
    match spec.kind {
        Kind::Numeric {
            step: Some(step), ..
        } if step.fract() == 0.0 => Value::Int(raw.round() as i64),
        Kind::Numeric {
            step: None,
            min: Some(min),
            max: Some(max),
        } if min.fract() == 0.0 && max.fract() == 0.0 => Value::Int(raw.round() as i64),
        _ => Value::Float(raw),
    }
}

#[allow(clippy::cast_possible_truncation)] // a light's few thousand kelvins
fn kelvin_of(mired: f64) -> i64 {
    (1_000_000.0 / mired).round() as i64
}

/// `#rrggbb` → (hue °, saturation 0–1, brightness 0–1).
#[allow(clippy::many_single_char_names)] // the usual names of the formula
fn hex_to_hsb(hex: &str) -> Option<(f64, f64, f64)> {
    let digits = hex.strip_prefix('#')?;
    if digits.len() != 6 {
        return None;
    }
    let channel = |i: usize| {
        u8::from_str_radix(digits.get(i..i + 2)?, 16)
            .ok()
            .map(|v| f64::from(v) / 255.0)
    };
    let (r, g, b) = (channel(0)?, channel(2)?, channel(4)?);
    let max = r.max(g).max(b);
    let delta = max - r.min(g).min(b);
    let hue = if delta == 0.0 {
        0.0
    } else if (max - r).abs() < f64::EPSILON {
        60.0 * (((g - b) / delta).rem_euclid(6.0))
    } else if (max - g).abs() < f64::EPSILON {
        60.0 * ((b - r) / delta + 2.0)
    } else {
        60.0 * ((r - g) / delta + 4.0)
    };
    let saturation = if max == 0.0 { 0.0 } else { delta / max };
    Some((hue, saturation, max))
}

/// (hue °, saturation 0–1) → `#rrggbb` at full brightness (a lamp's
/// brightness is its own point).
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::many_single_char_names
)] // clamped; the usual names of the formula
fn hs_to_hex(hue: f64, saturation: f64) -> String {
    let c = saturation.clamp(0.0, 1.0);
    let h = hue.rem_euclid(360.0) / 60.0;
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

/// Milliseconds since 1970 → `2026-10-26T15:04:05.123Z`.
#[allow(
    clippy::cast_possible_wrap,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
fn iso_time(ms: u64) -> String {
    let secs = (ms / 1000) as i64;
    let (days, rest) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Civil date from days (H. Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60,
        ms % 1000
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dimmer_without_switch_is_a_light() {
        // A Z-Wave multilevel switch: one level, 0–99, 0 meaning off.
        let view = DeviceView {
            device: Arc::new(moli_core::Device {
                id: "zwave:a-40".into(),
                instance: "zwave".into(),
                native_name: "Salon".into(),
                manufacturer: None,
                model: None,
                description: None,
                native_room: None,
                members: Vec::new(),
                points: vec![PointSpec {
                    key: "level".into(),
                    label: "Niveau".into(),
                    // As Z-Wave JS declares it: no step.
                    kind: Kind::Numeric {
                        min: Some(0.0),
                        max: Some(99.0),
                        step: None,
                    },
                    access: moli_core::Access {
                        read: true,
                        write: true,
                    },
                    unit: None,
                    semantic: Semantic::Brightness,
                }],
            }),
            label: moli_core::Label::default(),
            online: Some(true),
            state: std::iter::once((
                Arc::from("level"),
                moli_core::Sample {
                    value: Value::Int(30),
                    ts: 0,
                },
            ))
            .collect(),
            camera: false,
            printer: false,
        };
        let views = [view];
        let all = endpoints(&views, |_| true);
        let [e] = all.as_slice() else {
            panic!("one endpoint")
        };
        assert!(e.level);
        assert_eq!(e.category, "LIGHT");
        assert_eq!(e.name, "Salon");
        let found = discovery(&all).to_string();
        assert!(
            found.contains("Alexa.PowerController") && found.contains("Alexa.BrightnessController")
        );
        let props = properties(e);
        assert!(
            props
                .iter()
                .any(|p| p["name"] == "powerState" && p["value"] == "ON")
        );
        assert!(
            props
                .iter()
                .any(|p| p["name"] == "brightness" && p["value"] == 30)
        );
        assert_eq!(from_percent(e.on, 100.0), Value::Int(99));
        assert_eq!(from_percent(e.on, 30.0), Value::Int(30), "29.7, whole");
        assert_eq!(from_percent(e.on, 0.0), Value::Int(0));
        assert_eq!(level_on(e.on, &Value::Int(0)), Some(false));
    }

    #[test]
    fn times_are_iso() {
        assert_eq!(iso_time(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(iso_time(1_709_251_200_123), "2024-03-01T00:00:00.123Z");
    }

    #[test]
    fn colors_go_both_ways() {
        assert_eq!(hs_to_hex(0.0, 1.0), "#ff0000");
        assert_eq!(hs_to_hex(120.0, 1.0), "#00ff00");
        let (h, s, b) = hex_to_hsb("#0000ff").unwrap();
        assert!((h - 240.0).abs() < 0.01 && (s - 1.0).abs() < 0.01 && (b - 1.0).abs() < 0.01);
    }

    #[test]
    fn the_skill_host_alone_is_restricted() {
        let config = AlexaConfig {
            host: Some("alexa.example.org".into()),
        };
        assert!(is_skill_host(Some(&config), "alexa.example.org"));
        assert!(is_skill_host(Some(&config), "ALEXA.example.org:443"));
        assert!(!is_skill_host(Some(&config), "moli.example.org"));
        assert!(!is_skill_host(None, "alexa.example.org"));
    }

    #[test]
    fn choices_are_kept_and_only_when_they_differ() {
        let dir = std::env::temp_dir().join(format!("moli-alexa-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let alexa = Alexa::open(AlexaConfig::default(), Some(&dir));
        alexa.choose("hue:a", true, Some(false)).unwrap();
        alexa.choose("hue:b", true, Some(true)).unwrap();
        let again = Alexa::open(AlexaConfig::default(), Some(&dir));
        let kept = again.choices.lock().unwrap().clone();
        assert_eq!(kept, BTreeMap::from([("hue:a".to_owned(), false)]));
        again.choose("hue:a", true, None).unwrap();
        assert!(
            Alexa::open(AlexaConfig::default(), Some(&dir))
                .choices
                .lock()
                .unwrap()
                .is_empty()
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn tokens_must_match_and_be_long() {
        let token = "0123456789abcdef0123";
        assert!(token_matches(Some(token), Some(token)));
        assert!(!token_matches(Some(token), Some("0123456789abcdef0124")));
        assert!(!token_matches(Some(token), None));
        assert!(!token_matches(None, Some(token)));
        assert!(!token_matches(Some("short"), Some("short")));
    }
}
