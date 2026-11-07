//! An Apple TV as a Moli device (`kind = "appletv"`), over the Companion
//! protocol an iPhone's remote uses: power (it wakes the TV through
//! HDMI-CEC when the Apple TV is set to), the remote's keys, apps opened by
//! their bundle identifier or a link, play and pause, volume when the TV
//! or amplifier lets the Apple TV set it.
//!
//! Pairing is done once from the dashboard: « Appairage » on yes, the Apple
//! TV shows a 4-digit code, written in « Code PIN ». The pairing then lives
//! in the secret store (`pairing`).

// Wire formats: every cast is of a value bounded by a length check or a
// protocol limit.
#![allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]

use std::net::{IpAddr, SocketAddr};

mod companion;
mod opack;
use std::time::Duration;

use moli_core::{Access, Device, DeviceId, Kind, PointSpec, Semantic, Value};
use moli_runtime::{BoxFuture, CommandRequest, Driver, DriverCtx};
use serde::Deserialize;
use tokio::time::Instant;

use crate::companion::{Companion, Event, find_port};
use crate::opack::Opack;
use moli_homekit::pairing::{Error, Pairing};
use moli_homekit::tlv::{self, Tlv};

const PAIRING: &str = "pairing";

const HEARTBEAT: Duration = Duration::from_secs(30);
const FIRST_WAIT: Duration = Duration::from_secs(2);
const MAX_WAIT: Duration = Duration::from_secs(60);
/// How long the code shown on the TV is waited for.
const CODE_LIFE: Duration = Duration::from_secs(120);

/// The remote's keys, and their HID numbers.
const KEYS: [(&str, i64); 15] = [
    ("up", 1),
    ("down", 2),
    ("left", 3),
    ("right", 4),
    ("menu", 5),
    ("select", 6),
    ("home", 7),
    ("volume_up", 8),
    ("volume_down", 9),
    ("play_pause", 14),
    ("channel_up", 15),
    ("channel_down", 16),
    ("guide", 17),
    ("page_up", 18),
    ("page_down", 19),
];
const SLEEP: i64 = 12;
const WAKE: i64 = 13;

const MEDIA: [(&str, i64); 4] = [("play", 1), ("pause", 2), ("next", 3), ("previous", 4)];
const GET_VOLUME: i64 = 5;
const SET_VOLUME: i64 = 6;

/// `_mcF` bits: what the playing app accepts now.
const CAN_PLAY: i64 = 0x1;
const CAN_PAUSE: i64 = 0x2;
const CAN_VOLUME: i64 = 0x100;

/// Driver configuration (`[driver.options]` in `moli.toml`).
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// The Apple TV's address (an IP: reserve it on the router).
    pub host: IpAddr,
    /// Its Companion port; found by asking the Apple TV when absent (it
    /// changes when it restarts).
    #[serde(default)]
    pub port: Option<u16>,
    /// The device's name in Moli.
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Debug)]
pub struct AppleTv {
    config: Config,
}

impl AppleTv {
    #[must_use]
    pub fn new(config: Config) -> Self {
        Self { config }
    }
}

impl Driver for AppleTv {
    fn kind(&self) -> &'static str {
        "appletv"
    }

    fn run<'a>(&'a self, ctx: &'a mut DriverCtx) -> BoxFuture<'a, anyhow::Result<()>> {
        Box::pin(run(&self.config, ctx))
    }
}

fn spec(key: &str, label: &str, kind: Kind, read: bool, write: bool) -> PointSpec {
    PointSpec {
        key: key.into(),
        label: moli_i18n::tr!(label).into(),
        semantic: if key == "power" {
            Semantic::OnOff
        } else if key == "volume" {
            Semantic::infer("volume", None)
        } else {
            Semantic::Other
        },
        kind,
        access: Access { read, write },
        unit: None,
    }
}

fn enumeration<'a>(values: impl IntoIterator<Item = &'a str>) -> Kind {
    Kind::Enum {
        values: values.into_iter().map(Into::into).collect(),
    }
}

fn name_of(config: &Config) -> String {
    config.name.clone().unwrap_or_else(|| "Apple TV".to_owned())
}

fn device(ctx: &DriverCtx, id: &DeviceId, config: &Config, points: Vec<PointSpec>) -> Device {
    Device {
        id: id.clone(),
        instance: ctx.instance().clone(),
        native_name: name_of(config).into(),
        manufacturer: Some("Apple".into()),
        model: Some("Apple TV".into()),
        description: None,
        native_room: None,
        members: Vec::new(),
        points,
    }
}

fn paired_points() -> Vec<PointSpec> {
    vec![
        spec("power", "pilotes.appletv.allumee", Kind::Binary, true, true),
        spec(
            "state",
            "pilotes.appletv.etat",
            enumeration(["asleep", "screensaver", "awake", "idle"]),
            true,
            false,
        ),
        // A press, not a state: nothing to read back.
        spec(
            "key",
            "pilotes.appletv.touche",
            enumeration(KEYS.iter().map(|(k, _)| *k)),
            false,
            true,
        ),
        // Written: opens that app (its bundle identifier) or link.
        spec(
            "app",
            "pilotes.appletv.application",
            Kind::Text,
            false,
            true,
        ),
        spec(
            "media_state",
            "pilotes.appletv.lecture",
            enumeration(["playing", "paused", "idle"]),
            true,
            false,
        ),
        spec(
            "media_control",
            "pilotes.appletv.lecture_commande",
            enumeration(MEDIA.iter().map(|(k, _)| *k)),
            false,
            true,
        ),
        spec(
            "volume",
            "pilotes.appletv.volume",
            Kind::Numeric {
                min: Some(0.0),
                max: Some(100.0),
                step: Some(1.0),
            },
            true,
            true,
        ),
    ]
}

fn pairing_points() -> Vec<PointSpec> {
    vec![
        spec(
            "pairing",
            "pilotes.appletv.appairage_point",
            Kind::Binary,
            true,
            true,
        ),
        spec("pin", "pilotes.appletv.code_pin", Kind::Text, false, true),
    ]
}

async fn address(config: &Config) -> Result<SocketAddr, Error> {
    let port = match config.port {
        Some(port) => port,
        None => find_port(config.host).await?,
    };
    Ok(SocketAddr::new(config.host, port))
}

async fn run(config: &Config, ctx: &mut DriverCtx) -> anyhow::Result<()> {
    if !ctx.can_store_secrets() {
        ctx.wait_for(moli_i18n::tr!("pilotes.appletv.coffre_requis"));
        ctx.cancelled().await;
        return Ok(());
    }
    let id = ctx.device_id(&config.host.to_string());
    let mut wait = FIRST_WAIT;
    loop {
        let pairing = match ctx.secret(PAIRING).as_deref().and_then(Pairing::decode) {
            Some(pairing) => pairing,
            None => match pair(config, ctx, &id).await? {
                Some(pairing) => pairing,
                None => return Ok(()),
            },
        };
        match session(config, ctx, &id, &pairing, &mut wait).await {
            Stop::Done => return Ok(()),
            // The Apple TV forgot Moli (reset, removed from its remotes).
            Stop::Forgotten => {
                tracing::warn!(instance = %ctx.instance(), "apple tv: pairing refused, pairing again");
                ctx.forget_secret(PAIRING).await?;
            }
            Stop::Fail(e) => {
                tracing::warn!(instance = %ctx.instance(), error = %e, wait_s = wait.as_secs(), "apple tv unreachable");
                ctx.set_availability(&id, false);
                tokio::select! {
                    () = ctx.cancelled() => return Ok(()),
                    () = tokio::time::sleep(wait) => {}
                }
                wait = (wait * 2).min(MAX_WAIT);
            }
        }
    }
}

/// A code asked for: the connection it lives on, and what the TV sent.
struct Pending {
    companion: Companion,
    m2: Tlv,
    until: Instant,
}

/// Pairs the Apple TV, a person giving the code; the pairing is stored.
/// `None`: the driver is stopping.
async fn pair(
    config: &Config,
    ctx: &mut DriverCtx,
    id: &DeviceId,
) -> anyhow::Result<Option<Pairing>> {
    ctx.upsert_device(device(ctx, id, config, pairing_points()));
    ctx.set_availability(id, true);
    ctx.set_state(id, "pairing", Value::Bool(false));
    ctx.wait_for(moli_i18n::tr!(
        "pilotes.appletv.a_appairer",
        instance = ctx.instance()
    ));
    let mut pending: Option<Pending> = None;
    loop {
        let Some(command) = ctx.next_command().await else {
            return Ok(None);
        };
        let result = match (&*command.key, &command.value) {
            ("pairing", Value::Bool(true)) => match ask_code(config).await {
                Ok(asked) => {
                    ctx.wait_for(moli_i18n::tr!("pilotes.appletv.code_affiche"));
                    pending = Some(asked);
                    Ok(())
                }
                Err(e) => Err(moli_i18n::tr!(
                    "pilotes.appletv.appairage_refuse",
                    error = e.to_string()
                )),
            },
            ("pairing", Value::Bool(false)) => {
                pending = None;
                Ok(())
            }
            ("pin", Value::Text(code)) => {
                let code = code.trim().to_owned();
                if code.len() != 4 || !code.bytes().all(|b| b.is_ascii_digit()) {
                    Err(moli_i18n::tr!("pilotes.appletv.code_chiffres"))
                } else {
                    match pending.take() {
                        None => Err(moli_i18n::tr!("pilotes.appletv.appairage_pas_ouvert")),
                        Some(asked) if Instant::now() >= asked.until => {
                            Err(moli_i18n::tr!("pilotes.appletv.code_expire"))
                        }
                        Some(mut asked) => {
                            match asked.companion.pair_finish(&asked.m2, &code, "Moli").await {
                                Ok(pairing) => {
                                    ctx.store_secret(PAIRING, &pairing.encode()).await?;
                                    tracing::info!(instance = %ctx.instance(), "apple tv paired");
                                    ctx.set_state(id, "pairing", Value::Bool(false));
                                    command.reply(Ok(()));
                                    return Ok(Some(pairing));
                                }
                                // A wrong code closes this attempt on the TV.
                                Err(Error::Refused(tlv::ERR_AUTHENTICATION)) => {
                                    Err(moli_i18n::tr!("pilotes.appletv.code_faux"))
                                }
                                Err(e) => Err(moli_i18n::tr!(
                                    "pilotes.appletv.appairage_refuse",
                                    error = e.to_string()
                                )),
                            }
                        }
                    }
                }
            }
            ("pairing", _) => Err(moli_i18n::tr!("pilotes.appletv.oui_non")),
            _ => Err(moli_i18n::tr!("pilotes.appletv.pas_appairee")),
        };
        ctx.set_state(id, "pairing", Value::Bool(pending.is_some()));
        if pending.is_none() {
            ctx.wait_for(moli_i18n::tr!(
                "pilotes.appletv.a_appairer",
                instance = ctx.instance()
            ));
        }
        command.reply(result);
    }
}

/// Opens pair-setup: the Apple TV shows its code.
async fn ask_code(config: &Config) -> Result<Pending, Error> {
    let mut companion = Companion::open(address(config).await?).await?;
    let m2 = companion.pair_start().await?;
    Ok(Pending {
        companion,
        m2,
        until: Instant::now() + CODE_LIFE,
    })
}

enum Stop {
    Done,
    Forgotten,
    Fail(Error),
}

/// What the client says it is (an iPhone's remote, as pyatv does): the
/// Apple TV pushes its power state only to a client with an identifier.
fn system_info(pairing: &Pairing) -> Opack {
    let digest = ring::digest::digest(&ring::digest::SHA256, pairing.controller().as_bytes());
    let bytes = &digest.as_ref()[..6];
    let rp_id = opack::hex(bytes);
    let device_id = bytes
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(":");
    Opack::dict([
        ("_bf", Opack::Int(0)),
        ("_cf", Opack::Int(512)),
        ("_clFl", Opack::Int(128)),
        ("_i", Opack::Str(rp_id)),
        ("_idsID", Opack::str(pairing.controller())),
        ("_pubID", Opack::Str(device_id)),
        ("_sf", Opack::Int(256)),
        ("_sv", Opack::str("170.18")),
        ("model", Opack::str("iPhone14,3")),
        ("name", Opack::str("Moli")),
    ])
}

/// One connection: verify, start the remote's session, then listen.
async fn session(
    config: &Config,
    ctx: &mut DriverCtx,
    id: &DeviceId,
    pairing: &Pairing,
    wait: &mut Duration,
) -> Stop {
    let mut tv = match open(config, pairing).await {
        Ok(tv) => tv,
        Err(Error::Refused(tlv::ERR_AUTHENTICATION) | Error::WrongAccessory) => {
            return Stop::Forgotten;
        }
        Err(e) => return Stop::Fail(e),
    };
    ctx.upsert_device(device(ctx, id, config, paired_points()));
    match tv.request("FetchAttentionState", Opack::dict([])).await {
        Ok(answer) => set_attention(ctx, id, &answer),
        Err(e) => return Stop::Fail(e),
    }
    if let Ok(apps) = tv
        .request("FetchLaunchableApplicationsEvent", Opack::dict([]))
        .await
    {
        let list: Vec<String> = apps
            .entries()
            .iter()
            .filter_map(|(k, v)| Some(format!("{}={}", v.as_str()?, k.as_str()?)))
            .collect();
        tracing::info!(instance = %ctx.instance(), count = list.len(), apps = %list.join(", "), "apple tv apps");
    }
    ctx.set_availability(id, true);
    ctx.ready();
    *wait = FIRST_WAIT;
    tracing::info!(instance = %ctx.instance(), "apple tv connected");
    loop {
        tokio::select! {
            command = ctx.next_command() => {
                let Some(command) = command else { return Stop::Done };
                if let Err(e) = order(&mut tv, ctx, id, command).await {
                    return Stop::Fail(e);
                }
            }
            event = tv.next_event() => match event {
                Ok(event) => {
                    if let Err(e) = heard(&mut tv, ctx, id, &event).await {
                        return Stop::Fail(e);
                    }
                }
                Err(e) => return Stop::Fail(e),
            },
            () = tokio::time::sleep(HEARTBEAT) => {
                match tv.request("FetchAttentionState", Opack::dict([])).await {
                    Ok(answer) => set_attention(ctx, id, &answer),
                    Err(e) => return Stop::Fail(e),
                }
            }
        }
    }
}

/// The connection, verified, with the remote's session started.
async fn open(config: &Config, pairing: &Pairing) -> Result<Companion, Error> {
    let mut tv = Companion::open(address(config).await?).await?;
    tv.verify(pairing).await?;
    tv.request("_systemInfo", system_info(pairing)).await?;
    tv.request(
        "_touchStart",
        Opack::dict([
            ("_height", Opack::Float(1000.0)),
            ("_tFl", Opack::Int(0)),
            ("_width", Opack::Float(1000.0)),
        ]),
    )
    .await?;
    tv.request(
        "_sessionStart",
        Opack::dict([
            ("_srvT", Opack::str("com.apple.tvremoteservices")),
            ("_sid", Opack::Int(i64::from(random_u32()))),
        ]),
    )
    .await?;
    // Older systems do not know it: not needed.
    let _ = tv.request("TVRCSessionStart", Opack::dict([])).await;
    for event in ["_iMC", "SystemStatus", "TVSystemStatus"] {
        tv.event(
            "_interest",
            Opack::dict([("_regEvents", Opack::Array(vec![Opack::str(event)]))]),
        )
        .await?;
    }
    Ok(tv)
}

fn random_u32() -> u32 {
    use ring::rand::SecureRandom as _;
    let mut b = [0u8; 4];
    ring::rand::SystemRandom::new()
        .fill(&mut b)
        .expect("system randomness");
    u32::from_le_bytes(b)
}

fn attention_of(state: i64) -> Option<&'static str> {
    Some(match state {
        1 => "asleep",
        2 => "screensaver",
        3 => "awake",
        4 => "idle",
        _ => return None,
    })
}

fn set_attention(ctx: &DriverCtx, id: &DeviceId, content: &Opack) {
    let Some(state) = content
        .get("state")
        .and_then(Opack::as_int)
        .and_then(attention_of)
    else {
        return;
    };
    ctx.set_state(id, "state", Value::Text(state.into()));
    ctx.set_state(id, "power", Value::Bool(state != "asleep"));
}

fn media_state_of(flags: i64) -> &'static str {
    if flags & CAN_PAUSE != 0 {
        "playing"
    } else if flags & CAN_PLAY != 0 {
        "paused"
    } else {
        "idle"
    }
}

/// An event pushed by the Apple TV.
async fn heard(
    tv: &mut Companion,
    ctx: &DriverCtx,
    id: &DeviceId,
    event: &Event,
) -> Result<(), Error> {
    tracing::debug!(instance = %ctx.instance(), name = %event.name, content = %event.content.to_json(), "apple tv event");
    match event.name.as_str() {
        "SystemStatus" | "TVSystemStatus" => set_attention(ctx, id, &event.content),
        "_iMC" => {
            let flags = event
                .content
                .get("_mcF")
                .and_then(Opack::as_int)
                .unwrap_or(0);
            ctx.set_state(id, "media_state", Value::Text(media_state_of(flags).into()));
            if flags & CAN_VOLUME != 0 {
                let answer = tv
                    .request("_mcc", Opack::dict([("_mcc", Opack::Int(GET_VOLUME))]))
                    .await;
                match answer {
                    Ok(answer) => set_volume(ctx, id, &answer),
                    Err(Error::Refused(_)) => {}
                    Err(e) => return Err(e),
                }
            }
        }
        "_iMCVolume" | "_volumeChanged" => set_volume(ctx, id, &event.content),
        _ => {}
    }
    Ok(())
}

fn set_volume(ctx: &DriverCtx, id: &DeviceId, content: &Opack) {
    if let Some(volume) = content.get("_vol").and_then(Opack::as_float) {
        ctx.set_state(id, "volume", Value::Float((volume * 100.0).round()));
    }
}

/// A press of the remote: down, then up.
async fn press(tv: &mut Companion, code: i64) -> Result<(), Error> {
    for state in [1, 2] {
        tv.request(
            "_hidC",
            Opack::dict([("_hBtS", Opack::Int(state)), ("_hidC", Opack::Int(code))]),
        )
        .await?;
    }
    Ok(())
}

/// A command; `Err` only when the connection is lost (the order is then
/// answered as failed).
async fn order(
    tv: &mut Companion,
    ctx: &DriverCtx,
    id: &DeviceId,
    command: CommandRequest,
) -> Result<(), Error> {
    if command.abandoned() {
        command.reply(Err(moli_i18n::tr!("pilotes.appletv.trop_tard")));
        return Ok(());
    }
    let (key, value) = (command.key.clone(), command.value.clone());
    let sent = match (&*key, &value) {
        ("power", Value::Bool(on)) => {
            // Only the release, as Apple's remote does for these two.
            let code = if *on { WAKE } else { SLEEP };
            let sent = tv
                .request(
                    "_hidC",
                    Opack::dict([("_hBtS", Opack::Int(2)), ("_hidC", Opack::Int(code))]),
                )
                .await;
            if sent.is_ok() {
                ctx.set_state(id, "power", Value::Bool(*on));
            }
            sent.map(drop)
        }
        ("key", Value::Text(key)) => {
            let Some((_, code)) = KEYS.iter().find(|(k, _)| **k == **key) else {
                command.reply(Err(moli_i18n::tr!(
                    "pilotes.appletv.touche_inconnue",
                    key = key
                )));
                return Ok(());
            };
            press(tv, *code).await
        }
        ("app", Value::Text(app)) => {
            let app = app.trim();
            let key = if app.contains(':') {
                "_urlS"
            } else {
                "_bundleID"
            };
            tracing::info!(instance = %ctx.instance(), app, "apple tv: opening");
            tv.request("_launchApp", Opack::dict([(key, Opack::str(app))]))
                .await
                .map(drop)
        }
        ("media_control", Value::Text(what)) => {
            let Some((_, code)) = MEDIA.iter().find(|(k, _)| **k == **what) else {
                command.reply(Err(moli_i18n::tr!("pilotes.appletv.lecture_mots")));
                return Ok(());
            };
            tv.request("_mcc", Opack::dict([("_mcc", Opack::Int(*code))]))
                .await
                .map(drop)
        }
        ("volume", value) => {
            let Some(level) = value.as_f64() else {
                command.reply(Err(moli_i18n::tr!(
                    "pilotes.appletv.ne_se_regle_pas",
                    point = "volume"
                )));
                return Ok(());
            };
            tv.request(
                "_mcc",
                Opack::dict([
                    ("_mcc", Opack::Int(SET_VOLUME)),
                    ("_vol", Opack::Float((level / 100.0).clamp(0.0, 1.0))),
                ]),
            )
            .await
            .map(drop)
        }
        (point, _) => {
            command.reply(Err(moli_i18n::tr!(
                "pilotes.appletv.ne_se_regle_pas",
                point = point
            )));
            return Ok(());
        }
    };
    match sent {
        Ok(()) => {
            command.reply(Ok(()));
            Ok(())
        }
        Err(Error::Refused(code)) => {
            command.reply(Err(moli_i18n::tr!("pilotes.appletv.refuse", code = code)));
            Ok(())
        }
        Err(e) => {
            command.reply(Err(e.to_string()));
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_playing_state_comes_from_what_the_app_accepts() {
        assert_eq!(media_state_of(CAN_PAUSE | CAN_VOLUME), "playing");
        assert_eq!(media_state_of(CAN_PLAY), "paused");
        assert_eq!(media_state_of(0), "idle");
    }

    #[test]
    fn attention_states_are_named() {
        assert_eq!(attention_of(1), Some("asleep"));
        assert_eq!(attention_of(3), Some("awake"));
        assert_eq!(attention_of(9), None);
    }

    #[test]
    fn the_config_needs_only_the_host() {
        let config: Config =
            serde_json::from_value(serde_json::json!({ "host": "192.168.1.20" })).unwrap();
        assert_eq!(config.port, None);
        assert_eq!(name_of(&config), "Apple TV");
    }
    /// Against a real Apple TV, by hand: `APPLETV_HOST=… APPLETV_DIR=…
    /// cargo test -p moli-homekit live_ -- --ignored --nocapture`.
    /// `live_pair` shows the code on the TV and waits for `<dir>/pin`;
    /// `live_session` runs `APPLETV_DO` (`state`, `wake`, `sleep`, a key, `app:<id>`).
    fn live_env() -> (Config, std::path::PathBuf) {
        let host = std::env::var("APPLETV_HOST")
            .expect("APPLETV_HOST")
            .parse()
            .unwrap();
        let dir = std::env::var("APPLETV_DIR").expect("APPLETV_DIR").into();
        (
            Config {
                host,
                port: None,
                name: None,
            },
            dir,
        )
    }

    #[tokio::test]
    #[ignore = "needs a real Apple TV and a person reading its code"]
    async fn live_pair() {
        let (config, dir) = live_env();
        let at = address(&config).await.unwrap();
        println!("companion at {at}");
        let mut asked = ask_code(&config).await.unwrap();
        println!(
            "code shown on the TV; waiting for {}",
            dir.join("pin").display()
        );
        let code = loop {
            if let Ok(code) = std::fs::read_to_string(dir.join("pin")) {
                break code.trim().to_owned();
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        };
        let pairing = asked
            .companion
            .pair_finish(&asked.m2, &code, "Moli")
            .await
            .unwrap();
        std::fs::write(dir.join("pairing"), pairing.encode()).unwrap();
        println!("paired with {}", pairing.accessory());
    }

    #[tokio::test]
    #[ignore = "needs a real, paired Apple TV"]
    async fn live_session() {
        let (config, dir) = live_env();
        let pairing =
            Pairing::decode(&std::fs::read_to_string(dir.join("pairing")).unwrap()).unwrap();
        let mut tv = open(&config, &pairing).await.unwrap();
        let state = tv
            .request("FetchAttentionState", Opack::dict([]))
            .await
            .unwrap();
        println!("attention: {}", state.to_json());
        let what = std::env::var("APPLETV_DO").unwrap_or_else(|_| "state".into());
        match what.as_str() {
            "state" => {
                let apps = tv
                    .request("FetchLaunchableApplicationsEvent", Opack::dict([]))
                    .await
                    .unwrap();
                println!("apps: {}", apps.to_json());
            }
            "wake" | "sleep" => {
                let code = if what == "wake" { WAKE } else { SLEEP };
                let r = tv
                    .request(
                        "_hidC",
                        Opack::dict([("_hBtS", Opack::Int(2)), ("_hidC", Opack::Int(code))]),
                    )
                    .await;
                println!("{what}: {r:?}");
            }
            app if app.starts_with("app:") => {
                let app = &app[4..];
                let key = if app.contains(':') {
                    "_urlS"
                } else {
                    "_bundleID"
                };
                let r = tv
                    .request("_launchApp", Opack::dict([(key, Opack::str(app))]))
                    .await;
                println!("launch {app}: {r:?}");
            }
            key => {
                let code = KEYS.iter().find(|(k, _)| *k == key).expect("a key").1;
                println!("{key}: {:?}", press(&mut tv, code).await);
            }
        }
        let seconds: u64 = std::env::var("APPLETV_LISTEN")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(8);
        let until = tokio::time::Instant::now() + Duration::from_secs(seconds);
        while let Ok(event) = tokio::time::timeout_at(until, tv.next_event()).await {
            let event = event.unwrap();
            println!("event {}: {}", event.name, event.content.to_json());
        }
        let state = tv
            .request("FetchAttentionState", Opack::dict([]))
            .await
            .unwrap();
        println!("attention after: {}", state.to_json());
    }
}
