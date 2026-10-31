//! A HomeKit accessory as a Moli device (`kind = "homekit"`):
//! Moli pairs with it like the Home app would, then hears its events over
//! the local network, with no cloud in between. A doorbell's press becomes
//! the `ring` point within a fraction of a second.
//!
//! The setup code is given once (`secrets set <instance> setup_code`); the
//! pairing then lives in the secret store (`pairing`) and the code is
//! forgotten. An accessory paired to the Home app refuses Moli: it must be
//! removed from Home first.

use std::sync::Arc;
use std::time::Duration;

use moli_core::{Access, Device, DeviceId, Kind, PointSpec, Semantic, Value};
use moli_runtime::{BoxFuture, Driver, DriverCtx};
use serde::Deserialize;
use serde_json::Value as Json;

use crate::controller::{Connection, Error, Event, Pairing, short_type};
use crate::stream::Camera;
use crate::tlv;

const SETUP_CODE: &str = "setup_code";
const PAIRING: &str = "pairing";

/// A silent connection is checked this often: a dead one is found within
/// a minute, not at the next press.
const HEARTBEAT: Duration = Duration::from_secs(30);
const FIRST_WAIT: Duration = Duration::from_secs(2);
const MAX_WAIT: Duration = Duration::from_secs(60);

const ACCESSORY_INFORMATION: &str = "3E";
const DOORBELL: &str = "121";
const NAME: &str = "23";
const MANUFACTURER: &str = "20";
const MODEL: &str = "21";
const SWITCH_EVENT: &str = "73";
const MOTION: &str = "22";
const CAMERA: &str = "110";
const SPEAKER: &str = "113";
const SELECTED: &str = "117";
const SETUP_ENDPOINTS: &str = "118";

/// Driver configuration (`[driver.options]` in `moli.toml`).
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// `host:port` of the accessory (its `_hap._tcp` service).
    pub address: String,
}

#[derive(Debug)]
pub struct Accessory {
    config: Config,
}

impl Accessory {
    #[must_use]
    pub fn new(config: Config) -> Self {
        Self { config }
    }
}

impl Driver for Accessory {
    fn kind(&self) -> &'static str {
        "homekit"
    }

    fn run<'a>(&'a self, ctx: &'a mut DriverCtx) -> BoxFuture<'a, anyhow::Result<()>> {
        Box::pin(run(&self.config, ctx))
    }
}

/// What one characteristic is to Moli.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Role {
    /// A press (a doorbell's button, a programmable switch): a pulse.
    Press,
    /// Something seen (motion): a lasting yes or no.
    Motion,
}

/// The accessory as read from its database.
#[derive(Clone, Debug, PartialEq)]
struct Layout {
    name: String,
    manufacturer: Option<String>,
    model: Option<String>,
    /// `(aid, iid)`, the point key, its role.
    watched: Vec<((u64, u64), &'static str, Role)>,
    /// Its camera's stream management, if it films.
    camera: Option<crate::stream::Ids>,
    /// It has a speaker (a person can answer through it).
    speaker: bool,
}

fn text_of(service: &Json, kind: &str) -> Option<String> {
    service["characteristics"]
        .as_array()?
        .iter()
        .find(|c| c["type"].as_str().map(short_type).as_deref() == Some(kind))?["value"]
        .as_str()
        .map(str::to_owned)
}

/// Reads what Moli shows of the first accessory: its name and the
/// characteristics worth an event.
fn layout_of(db: &Json) -> Option<Layout> {
    let accessory = db["accessories"].as_array()?.first()?;
    let aid = accessory["aid"].as_u64()?;
    let services = accessory["services"].as_array()?;
    let info = services
        .iter()
        .find(|s| s["type"].as_str().map(short_type).as_deref() == Some(ACCESSORY_INFORMATION));
    let mut watched = Vec::new();
    let mut camera = None;
    let mut speaker = false;
    for service in services {
        let kind = service["type"].as_str().map(short_type);
        speaker |= kind.as_deref() == Some(SPEAKER);
        if kind.as_deref() == Some(CAMERA) && camera.is_none() {
            let iid_of = |t: &str| {
                service["characteristics"]
                    .as_array()?
                    .iter()
                    .find(|c| c["type"].as_str().map(short_type).as_deref() == Some(t))?["iid"]
                    .as_u64()
            };
            if let (Some(setup), Some(selected)) = (iid_of(SETUP_ENDPOINTS), iid_of(SELECTED)) {
                camera = Some(crate::stream::Ids {
                    aid,
                    setup,
                    selected,
                });
            }
        }
        let doorbell = service["type"].as_str().map(short_type).as_deref() == Some(DOORBELL);
        for c in service["characteristics"].as_array().into_iter().flatten() {
            let Some(iid) = c["iid"].as_u64() else {
                continue;
            };
            let notifies = c["perms"]
                .as_array()
                .is_some_and(|p| p.iter().any(|p| p == "ev"));
            if !notifies {
                continue;
            }
            let role = match c["type"].as_str().map(short_type).as_deref() {
                Some(SWITCH_EVENT) if doorbell => ("ring", Role::Press),
                Some(SWITCH_EVENT) => ("button", Role::Press),
                Some(MOTION) => ("motion", Role::Motion),
                _ => continue,
            };
            if watched.iter().all(|(_, key, _)| *key != role.0) {
                watched.push(((aid, iid), role.0, role.1));
            }
        }
    }
    Some(Layout {
        name: info
            .and_then(|i| text_of(i, NAME))
            .unwrap_or_else(|| "HomeKit".to_owned()),
        manufacturer: info.and_then(|i| text_of(i, MANUFACTURER)),
        model: info.and_then(|i| text_of(i, MODEL)),
        watched,
        camera,
        speaker,
    })
}

/// What the accessory offers, for the log: each service and its
/// characteristics (short types), and the camera's configuration TLVs.
fn summary_of(db: &Json) -> String {
    let mut out = Vec::new();
    for accessory in db["accessories"].as_array().into_iter().flatten() {
        for service in accessory["services"].as_array().into_iter().flatten() {
            let kind = service["type"].as_str().map(short_type).unwrap_or_default();
            let chars: Vec<String> = service["characteristics"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|c| {
                    let t = c["type"].as_str().map(short_type).unwrap_or_default();
                    match t.as_str() {
                        // Supported video, audio, RTP configurations.
                        "114" | "115" | "116" => {
                            format!("{t}={}", c["value"].as_str().unwrap_or(""))
                        }
                        _ => t,
                    }
                })
                .collect();
            out.push(format!("{kind}[{}]", chars.join(" ")));
        }
    }
    out.join(" ")
}

fn device_of(ctx: &DriverCtx, native: &str, layout: &Layout) -> Device {
    let points = layout
        .watched
        .iter()
        .map(|(_, key, role)| PointSpec {
            key: (*key).into(),
            label: moli_i18n::tr!(match *key {
                "ring" => "pilotes.homekit.sonnerie",
                "button" => "pilotes.homekit.bouton",
                _ => "pilotes.homekit.mouvement",
            })
            .into(),
            semantic: if *role == Role::Motion {
                Semantic::Occupancy
            } else {
                Semantic::Other
            },
            kind: Kind::Binary,
            access: Access {
                read: true,
                write: false,
            },
            unit: None,
        })
        .collect();
    Device {
        id: ctx.device_id(native),
        instance: ctx.instance().clone(),
        native_name: layout.name.as_str().into(),
        manufacturer: layout.manufacturer.as_deref().map(Into::into),
        model: layout.model.as_deref().map(Into::into),
        description: None,
        native_room: None,
        members: Vec::new(),
        points,
    }
}

/// The device's native id: the pairing identifier, without its colons.
fn native_of(pairing: &Pairing) -> String {
    pairing.accessory().replace(':', "").to_lowercase()
}

async fn park(ctx: &DriverCtx, reason: String) -> anyhow::Result<()> {
    ctx.wait_for(reason);
    ctx.cancelled().await;
    Ok(())
}

/// Why a session ends.
enum Stop {
    Park(String),
    Fail(Error),
    Done,
}

/// The pairing, made now with the setup code when there is none yet.
async fn pairing(config: &Config, ctx: &DriverCtx) -> Result<Pairing, Stop> {
    if let Some(pairing) = ctx.secret(PAIRING).as_deref().and_then(Pairing::decode) {
        return Ok(pairing);
    }
    let Some(code) = ctx.secret(SETUP_CODE) else {
        return Err(Stop::Park(moli_i18n::tr!(
            "pilotes.homekit.code_manquant",
            instance = ctx.instance()
        )));
    };
    let mut connection = Connection::open(&config.address)
        .await
        .map_err(Stop::Fail)?;
    match connection.pair(&code).await {
        Ok(pairing) => {
            ctx.store_secret(PAIRING, &pairing.encode())
                .await
                .map_err(|e| Stop::Fail(Error::Network(e)))?;
            let _ = ctx.forget_secret(SETUP_CODE).await;
            tracing::info!(instance = %ctx.instance(), "homekit accessory paired");
            Ok(pairing)
        }
        // Never tried again by itself: the accessory locks after a few
        // wrong codes.
        Err(Error::Refused(e)) => {
            let _ = ctx.forget_secret(SETUP_CODE).await;
            let key = match e {
                tlv::ERR_AUTHENTICATION => "pilotes.homekit.code_faux",
                tlv::ERR_UNAVAILABLE => "pilotes.homekit.deja_appaire",
                _ => "pilotes.homekit.appairage_refuse",
            };
            Err(Stop::Park(moli_i18n::tr!(key, instance = ctx.instance())))
        }
        Err(e) => Err(Stop::Fail(e)),
    }
}

async fn run(config: &Config, ctx: &mut DriverCtx) -> anyhow::Result<()> {
    if !ctx.can_store_secrets() {
        return park(ctx, moli_i18n::tr!("pilotes.homekit.coffre_requis")).await;
    }
    let mut wait = FIRST_WAIT;
    let mut known: Option<DeviceId> = None;
    loop {
        let stop = match pairing(config, ctx).await {
            Ok(pairing) => session(config, &pairing, ctx, &mut known, &mut wait).await,
            Err(stop) => stop,
        };
        match stop {
            Stop::Done => return Ok(()),
            Stop::Park(reason) => return park(ctx, reason).await,
            Stop::Fail(e) => {
                tracing::warn!(instance = %ctx.instance(), error = %e, wait_s = wait.as_secs(), "homekit accessory unreachable");
                if let Some(id) = &known {
                    ctx.set_availability(id, false);
                }
                tokio::select! {
                    () = ctx.cancelled() => return Ok(()),
                    () = tokio::time::sleep(wait) => {}
                }
                wait = (wait * 2).min(MAX_WAIT);
            }
        }
    }
}

/// One connection: verify, read the database, subscribe, then listen.
async fn session(
    config: &Config,
    pairing: &Pairing,
    ctx: &mut DriverCtx,
    known: &mut Option<DeviceId>,
    wait: &mut Duration,
) -> Stop {
    let mut connection = match Connection::open(&config.address).await {
        Ok(c) => c,
        Err(e) => return Stop::Fail(e),
    };
    match connection.verify(pairing).await {
        Ok(()) => {}
        // The accessory forgot Moli (reset, paired again elsewhere).
        Err(Error::Refused(tlv::ERR_AUTHENTICATION) | Error::WrongAccessory) => {
            return Stop::Park(moli_i18n::tr!(
                "pilotes.homekit.appairage_perdu",
                instance = ctx.instance()
            ));
        }
        Err(e) => return Stop::Fail(e),
    }
    let db = match connection.accessories().await {
        Ok(db) => db,
        Err(e) => return Stop::Fail(e),
    };
    let Some(layout) = layout_of(&db) else {
        return Stop::Fail(Error::Protocol);
    };
    tracing::info!(instance = %ctx.instance(), services = %summary_of(&db), "homekit accessory database");
    let native = native_of(pairing);
    let id = ctx.device_id(&native);
    ctx.upsert_device(device_of(ctx, &native, &layout));
    if let Some(ids) = layout.camera {
        let camera = Arc::new(Camera::new(&config.address, pairing.clone(), ids));
        ctx.provide_live(&id, camera.clone());
        ctx.provide_snapshots(&id, camera.clone());
        if layout.speaker {
            ctx.provide_talk(&id, camera);
        }
    }
    for (_, key, _) in &layout.watched {
        ctx.set_state(&id, key, Value::Bool(false));
    }
    let ids: Vec<(u64, u64)> = layout.watched.iter().map(|(at, _, _)| *at).collect();
    if let Err(e) = connection.subscribe(&ids).await {
        return Stop::Fail(e);
    }
    ctx.set_availability(&id, true);
    ctx.ready();
    *known = Some(id.clone());
    *wait = FIRST_WAIT;
    tracing::info!(instance = %ctx.instance(), watched = ids.len(), "homekit accessory listening");
    let heartbeat_at = ids.first().copied();
    loop {
        let event = tokio::select! {
            // Ends (None) when the system stops.
            command = ctx.next_command() => {
                let Some(command) = command else { return Stop::Done };
                command.reply(Err(moli_i18n::tr!("pilotes.homekit.lecture_seule")));
                continue;
            }
            event = connection.next_event() => event,
            () = tokio::time::sleep(HEARTBEAT) => {
                if let Some(at) = heartbeat_at
                    && let Err(e) = connection.read(&[at]).await
                {
                    return Stop::Fail(e);
                }
                continue;
            }
        };
        match event {
            Ok(event) => answer(ctx, &id, &layout, &event),
            Err(e) => return Stop::Fail(e),
        }
    }
}

/// What an event means: a press is a pulse, motion a lasting state.
fn answer(ctx: &DriverCtx, id: &DeviceId, layout: &Layout, event: &Event) {
    let Some((_, key, role)) = layout
        .watched
        .iter()
        .find(|(at, _, _)| *at == (event.aid, event.iid))
    else {
        return;
    };
    match role {
        Role::Press => {
            // A single press is 0; a double or a long one 1 or 2. A
            // null value (some accessories send one at subscription) is
            // not a press.
            if event.value.is_null() {
                return;
            }
            tracing::info!(instance = %ctx.instance(), key, value = %event.value, "homekit press");
            ctx.pulse_state(id, key, Value::Bool(true));
            ctx.set_state(id, key, Value::Bool(false));
        }
        Role::Motion => {
            let seen = event
                .value
                .as_bool()
                .or_else(|| event.value.as_u64().map(|n| n != 0));
            if let Some(seen) = seen {
                ctx.set_state(id, key, Value::Bool(seen));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn doorbell_db() -> Json {
        json!({ "accessories": [{ "aid": 1, "services": [
            { "type": "3E", "iid": 1, "characteristics": [
                { "type": "23", "iid": 2, "perms": ["pr"], "value": "Doorbell" },
                { "type": "20", "iid": 3, "perms": ["pr"], "value": "Netatmo" },
                { "type": "21", "iid": 4, "perms": ["pr"], "value": "NDB" }
            ]},
            { "type": "00000121-0000-1000-8000-0026BB765291", "iid": 10, "characteristics": [
                { "type": "00000073-0000-1000-8000-0026BB765291", "iid": 11, "perms": ["pr", "ev"], "value": null }
            ]},
            { "type": "85", "iid": 20, "characteristics": [
                { "type": "22", "iid": 21, "perms": ["pr", "ev"], "value": false }
            ]},
            { "type": "89", "iid": 30, "characteristics": [
                { "type": "73", "iid": 31, "perms": ["pr"], "value": null }
            ]}
        ]}]})
    }

    #[test]
    fn a_doorbell_is_read_as_a_ring_and_motion() {
        let layout = layout_of(&doorbell_db()).unwrap();
        assert_eq!(layout.name, "Doorbell");
        assert_eq!(layout.manufacturer.as_deref(), Some("Netatmo"));
        assert_eq!(layout.camera, None);
        assert_eq!(
            layout.watched,
            vec![
                ((1, 11), "ring", Role::Press),
                ((1, 21), "motion", Role::Motion)
            ]
        );
    }

    #[test]
    fn an_empty_database_is_nothing() {
        assert_eq!(layout_of(&json!({ "accessories": [] })), None);
    }
}
