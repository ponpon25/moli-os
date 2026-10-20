//! Hitachi air conditioners (airCloud Go), through the owner's own account:
//! the units' state arrives on the app's notification service, orders go
//! through the app's REST API. Nothing touches the units or the LAN.
//!
//! The account is a real one, so the driver is cautious:
//! - the password is tried once per start: a refusal parks the driver
//!   (`login_locked`) until a person lifts it, never a retry loop;
//! - identifier and password live in the secret store only;
//! - the session (30 minutes) is renewed by signing in again, as the app does;
//! - the units are discovered from the account, never listed in the config;
//! - an order carries the unit's whole state (the cloud takes no partial
//!   order), built from the last state the cloud reported.
//!
//! Nothing here is published by Hitachi: see `api.rs`.

mod api;
mod model;
mod orders;
mod stomp;

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use moli_core::{Access, Device, DeviceId, Kind, PointSpec, Semantic, Unit as Measure, Value};
use moli_runtime::{BoxFuture, CommandRequest, Driver, DriverCtx};
use serde::Deserialize;
use tokio::time::Instant;

use api::{ApiError, Client, Event, Feed};
use model::{Capabilities, Change, Mode, Unit};
use orders::Orders;

const USERNAME: &str = "username";
const PASSWORD: &str = "password";
/// Present once the account refused the sign-in: nothing signs in again
/// until a person removes it.
const LOCKED: &str = "login_locked";

const MIN_POLL: u64 = 30;
/// A silent feed is dead after this: the service speaks every 10 s.
const SILENCE: Duration = Duration::from_secs(45);
/// After a failure, in steps of this, up to `MAX_WAIT`.
const STEP_WAIT: Duration = Duration::from_secs(30);
const MAX_WAIT: Duration = Duration::from_secs(5 * 60);
/// A unit asked again soon after an order is read again twice: it takes a
/// few seconds to follow.
const AFTER_ORDER: [Duration; 2] = [Duration::from_secs(3), Duration::from_secs(10)];

/// Driver configuration (`[driver.options]` in `moli.toml`). The identifier
/// and the password are not here: they live in the secret store.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// The home to use (its number), when the account has several.
    #[serde(default)]
    pub family: Option<i64>,
    /// Seconds between two requests for a fresh picture (30 at least).
    #[serde(default = "default_poll")]
    pub poll_secs: u64,
}

fn default_poll() -> u64 {
    60
}

#[derive(Debug)]
pub struct Hitachi {
    config: Config,
}

impl Hitachi {
    #[must_use]
    pub fn new(config: Config) -> Self {
        Self { config }
    }
}

impl Driver for Hitachi {
    fn kind(&self) -> &'static str {
        "hitachi"
    }

    fn run<'a>(&'a self, ctx: &'a mut DriverCtx) -> BoxFuture<'a, anyhow::Result<()>> {
        Box::pin(run(&self.config, ctx))
    }
}

/// Why a session ends.
enum Stop {
    /// A person must act: shown everywhere, nothing else is tried.
    Park(String),
    /// Something passing: the loop waits, then starts again.
    Fail(anyhow::Error),
    /// The system stops.
    Done,
    /// The token is about to end: sign in again.
    Renew,
}

fn passing(e: ApiError) -> Stop {
    Stop::Fail(anyhow::Error::new(e))
}

async fn park(ctx: &DriverCtx, reason: String) -> anyhow::Result<()> {
    ctx.wait_for(reason);
    ctx.cancelled().await;
    Ok(())
}

fn missing(ctx: &DriverCtx) -> String {
    moli_i18n::tr!(
        "pilotes.hitachi.identifiants_manquants",
        instance = ctx.instance()
    )
}

fn spec(
    key: &str,
    label: String,
    kind: Kind,
    write: bool,
    semantic: Semantic,
    unit: Option<Measure>,
) -> PointSpec {
    PointSpec {
        key: key.into(),
        label: label.into(),
        semantic,
        kind,
        access: Access { read: true, write },
        unit,
    }
}

fn device_of(ctx: &DriverCtx, unit: &Unit, range: (f64, f64)) -> Device {
    Device {
        id: ctx.device_id(&unit.native_id()),
        instance: ctx.instance().clone(),
        native_name: unit.name.clone().into(),
        manufacturer: Some("Hitachi".into()),
        model: Some(unit.model().into()),
        description: None,
        native_room: None,
        members: Vec::new(),
        points: vec![
            spec(
                "on",
                moli_i18n::tr!("pilotes.hitachi.marche"),
                Kind::Binary,
                true,
                Semantic::OnOff,
                None,
            ),
            spec(
                "mode",
                moli_i18n::tr!("pilotes.hitachi.mode"),
                Kind::Enum {
                    values: model::mode_values(),
                },
                true,
                Semantic::Other,
                None,
            ),
            spec(
                "target_temperature",
                moli_i18n::tr!("pilotes.hitachi.consigne"),
                Kind::Numeric {
                    min: Some(range.0),
                    max: Some(range.1),
                    step: Some(0.5),
                },
                true,
                Semantic::Config,
                Some(Measure::Celsius),
            ),
            spec(
                "temperature",
                moli_i18n::tr!("pilotes.hitachi.temperature"),
                Kind::Numeric {
                    min: None,
                    max: None,
                    step: None,
                },
                false,
                Semantic::Temperature,
                Some(Measure::Celsius),
            ),
            spec(
                "fan_speed",
                moli_i18n::tr!("pilotes.hitachi.ventilation"),
                Kind::Enum {
                    values: model::fan_values(),
                },
                true,
                Semantic::Config,
                None,
            ),
            spec(
                "swing",
                moli_i18n::tr!("pilotes.hitachi.oscillation"),
                Kind::Text,
                false,
                Semantic::Other,
                None,
            ),
            spec(
                "last_order",
                moli_i18n::tr!("pilotes.hitachi.dernier_ordre"),
                Kind::Text,
                false,
                Semantic::Other,
                None,
            ),
            spec(
                "order_failed",
                moli_i18n::tr!("pilotes.hitachi.ordre_en_echec"),
                Kind::Binary,
                false,
                Semantic::Other,
                None,
            ),
            spec(
                "fault",
                moli_i18n::tr!("pilotes.hitachi.defaut"),
                Kind::Binary,
                false,
                Semantic::Other,
                None,
            ),
        ],
    }
}

/// Publishes what the cloud says about a unit.
fn publish(ctx: &DriverCtx, id: &DeviceId, unit: &Unit) {
    ctx.set_state(id, "on", Value::Bool(unit.power));
    if let Some(mode) = unit.mode {
        ctx.set_state(id, "mode", Value::Text(mode.moli().into()));
    }
    if let Some(t) = unit.target {
        ctx.set_state(id, "target_temperature", Value::Float(t));
    }
    if let Some(t) = unit.room {
        ctx.set_state(id, "temperature", Value::Float(t));
    }
    if let Some(fan) = model::fan_to_moli(&unit.fan) {
        ctx.set_state(id, "fan_speed", Value::Text(fan.into()));
    }
    ctx.set_state(id, "swing", Value::Text(unit.swing.to_lowercase().into()));
    ctx.set_state(id, "fault", Value::Bool(unit.fault));
    ctx.set_availability(id, unit.online);
}

/// Everything the driver keeps between two messages.
struct Fleet {
    /// The last state the cloud reported, by the unit's native identifier.
    units: HashMap<String, Unit>,
    orders: Orders,
    caps: Capabilities,
}

impl Fleet {
    /// Takes a message of the notification service in, and publishes it.
    fn absorb(&mut self, ctx: &DriverCtx, message: &serde_json::Value) {
        let units = model::units_of(message);
        let whole = matches!(
            message["notificationType"].as_str(),
            Some("ON_CONNECT" | "REFRESH_ALL")
        );
        let range = self.caps.overall();
        let now = Instant::now();
        for unit in units.iter().filter(|u| !u.thing.is_empty()) {
            let native = unit.native_id();
            let id = ctx.device_id(&native);
            if !self.units.contains_key(&native) {
                ctx.upsert_device(device_of(ctx, unit, range));
                tracing::info!(instance = %ctx.instance(), model = unit.model(), "hitachi unit found");
            }
            publish(ctx, &id, unit);
            self.orders.observe(&native, unit, now);
            self.units.insert(native, unit.clone());
        }
        if whole && !units.is_empty() {
            let present: Vec<String> = units.iter().map(Unit::native_id).collect();
            let gone: Vec<String> = self
                .units
                .keys()
                .filter(|k| !present.contains(k))
                .cloned()
                .collect();
            for native in gone {
                ctx.remove_device(&ctx.device_id(&native));
                self.units.remove(&native);
                self.orders.forget(&native);
            }
        }
    }
}

/// A session for a call, renewed when it is about to end.
async fn sign_in(client: &mut Client, ctx: &DriverCtx) -> Result<(), Stop> {
    if client.token().is_some() {
        return Ok(());
    }
    match client.sign_in().await {
        Ok(()) => Ok(()),
        Err(ApiError::Credentials(why)) => {
            // Recorded before parking: a restart must not try again.
            ctx.store_secret(LOCKED, "1")
                .await
                .map_err(|e| Stop::Fail(e.into()))?;
            Err(Stop::Park(moli_i18n::tr!(
                "pilotes.hitachi.connexion_refusee",
                why = why,
                instance = ctx.instance()
            )))
        }
        Err(e) => Err(passing(e)),
    }
}

async fn run(config: &Config, ctx: &mut DriverCtx) -> anyhow::Result<()> {
    if !ctx.can_store_secrets() {
        let problem = ctx.secrets_problem().map_or_else(
            || moli_i18n::tr!("pilotes.hitachi.coffre_indisponible"),
            str::to_owned,
        );
        let reason = moli_i18n::tr!("pilotes.hitachi.coffre_requis", problem = problem);
        return park(ctx, reason).await;
    }
    let (Some(username), Some(password)) = (ctx.secret(USERNAME), ctx.secret(PASSWORD)) else {
        return park(ctx, missing(ctx)).await;
    };
    if ctx.secret(LOCKED).is_some() {
        let reason = moli_i18n::tr!(
            "pilotes.hitachi.connexion_suspendue",
            instance = ctx.instance()
        );
        return park(ctx, reason).await;
    }
    let mut client = Client::new(&username, &password);
    let mut fleet = Fleet {
        units: HashMap::new(),
        orders: Orders::default(),
        caps: Capabilities::default(),
    };
    let mut failures = 0u32;
    loop {
        match session(config, &mut client, ctx, &mut fleet, &mut failures).await {
            Stop::Done => return Ok(()),
            Stop::Park(reason) => return park(ctx, reason).await,
            Stop::Renew => {}
            Stop::Fail(e) => {
                failures += 1;
                let wait = (STEP_WAIT * failures).min(MAX_WAIT);
                tracing::warn!(instance = %ctx.instance(), error = %format!("{e:#}"), wait_s = wait.as_secs(), "hitachi session failed");
                if failures >= 2 {
                    for native in fleet.units.keys() {
                        ctx.set_availability(&ctx.device_id(native), false);
                    }
                }
                tokio::select! {
                    () = ctx.cancelled() => return Ok(()),
                    () = tokio::time::sleep(wait) => {}
                }
            }
        }
    }
}

/// An order on its way: what it was for and how it ended.
type Flight = Pin<Box<dyn Future<Output = (String, Unit, Result<(), ApiError>)> + Send>>;

/// One session: sign in, find the home, follow the feed and take orders,
/// until the token is about to end or something breaks.
#[allow(clippy::too_many_lines)]
async fn session(
    config: &Config,
    client: &mut Client,
    ctx: &mut DriverCtx,
    fleet: &mut Fleet,
    failures: &mut u32,
) -> Stop {
    if let Err(stop) = sign_in(client, ctx).await {
        return stop;
    }
    let account = match client.account(config.family).await {
        Ok(account) => account,
        Err(e) => return passing(e),
    };
    match client.capabilities(account.family).await {
        Ok(caps) => fleet.caps = caps,
        // Not fatal: the common ground applies.
        Err(e) => {
            tracing::warn!(instance = %ctx.instance(), error = %e, "hitachi: model limits unknown");
        }
    }
    let Some(token) = client.bearer() else {
        return Stop::Renew;
    };
    let mut feed = match Feed::open(&token, account).await {
        Ok(feed) => feed,
        Err(e) => return Stop::Fail(e),
    };
    tracing::info!(instance = %ctx.instance(), "hitachi feed open");
    ctx.ready();
    *failures = 0;

    let poll = Duration::from_secs(config.poll_secs.max(MIN_POLL));
    let mut next_poll = Instant::now() + poll;
    let mut quick: Vec<Instant> = Vec::new();
    let mut heard = Instant::now();
    // At most one order on its way: the cloud takes them one by one.
    let mut flight: Option<Flight> = None;
    // An order that met an ended session goes once more.
    let mut retried = false;
    loop {
        let now = Instant::now();
        if flight.is_none() && fleet.orders.is_empty() && now >= client.renew_at().into() {
            return Stop::Renew;
        }
        if heard.elapsed() > SILENCE {
            return Stop::Fail(anyhow::anyhow!("the notification service went silent"));
        }
        let due_quick = quick.iter().any(|t| *t <= now);
        if due_quick || now >= next_poll {
            quick.retain(|t| *t > now);
            if now >= next_poll {
                next_poll = now + poll;
            }
            if let Err(e) = feed.refresh().await {
                return Stop::Fail(e);
            }
        }
        // The order that is due goes, when none is on its way.
        if flight.is_none()
            && let Some((native, unit)) = fleet.orders.take_due(now)
        {
            if let Err(stop) = sign_in(client, ctx).await {
                return stop;
            }
            let Some(token) = client.bearer() else {
                return Stop::Renew;
            };
            note_order(ctx, &native, None);
            let sent = unit.clone();
            flight = Some(Box::pin(async move {
                let result = api::send_order(token, sent, account.family).await;
                (native, unit, result)
            }));
        }
        // While an order is on its way the next one waits: no wake-up for it.
        let wake = match (&flight, fleet.orders.next_due()) {
            (None, Some(due)) => due,
            _ => now + Duration::from_secs(5),
        };
        tokio::select! {
            command = ctx.next_command() => {
                let Some(command) = command else { return Stop::Done };
                accept(ctx, fleet, command);
            }
            (native, unit, result) = async {
                match flight.as_mut() {
                    Some(f) => f.await,
                    None => std::future::pending().await,
                }
            } => {
                flight = None;
                let at = Instant::now();
                match result {
                    Ok(()) => {
                        retried = false;
                        tracing::info!(instance = %ctx.instance(), "hitachi order taken");
                        fleet.orders.sent(&native, &unit, at);
                        note_order(ctx, &native, Some(Ok(())));
                        quick.extend(AFTER_ORDER.iter().map(|d| at + *d));
                    }
                    Err(ApiError::SessionExpired) if !retried => {
                        // The token ended early: sign in again, send once more.
                        retried = true;
                        client.forget_session();
                        fleet.orders.requeue(&native, unit, at);
                    }
                    Err(e) => {
                        retried = false;
                        tracing::warn!(instance = %ctx.instance(), error = %e, "hitachi order failed");
                        note_order(ctx, &native, Some(Err(e.to_string())));
                        // What the unit really does is read again.
                        quick.extend(AFTER_ORDER.iter().map(|d| at + *d));
                    }
                }
            }
            event = feed.next(Duration::from_secs(5)) => match event {
                Ok(Event::Quiet) => {}
                Ok(Event::Alive) => heard = Instant::now(),
                Ok(Event::Units(message)) => {
                    heard = Instant::now();
                    fleet.absorb(ctx, &message);
                }
                Err(e) => return Stop::Fail(e),
            },
            () = tokio::time::sleep_until(wake) => {}
        }
    }
}

/// Says on the unit what became of its last order.
fn note_order(ctx: &DriverCtx, native: &str, outcome: Option<Result<(), String>>) {
    let id = ctx.device_id(native);
    let (text, failed) = match outcome {
        None => (moli_i18n::tr!("pilotes.hitachi.ordre_envoye"), false),
        Some(Ok(())) => (moli_i18n::tr!("pilotes.hitachi.ordre_pris"), false),
        Some(Err(why)) => (
            moli_i18n::tr!("pilotes.hitachi.ordre_refuse", why = why),
            true,
        ),
    };
    ctx.set_state(&id, "last_order", Value::Text(text.into()));
    ctx.set_state(&id, "order_failed", Value::Bool(failed));
}

/// What an order asks, or why it is not an order.
fn change_of(command: &CommandRequest) -> Result<Change, String> {
    let text = |v: &Value| match v {
        Value::Text(t) => Ok(t.to_string()),
        _ => Err(moli_i18n::tr!("pilotes.hitachi.valeur_texte")),
    };
    match &*command.key {
        "on" => match command.value {
            Value::Bool(on) => Ok(Change::Power(on)),
            _ => Err(moli_i18n::tr!("pilotes.hitachi.valeur_booleenne")),
        },
        "mode" => {
            let word = text(&command.value)?;
            Mode::from_moli(&word)
                .map(Change::Mode)
                .ok_or_else(|| moli_i18n::tr!("pilotes.hitachi.mode_inconnu", mode = word))
        }
        "target_temperature" => command
            .value
            .as_f64()
            .map(Change::Target)
            .ok_or_else(|| moli_i18n::tr!("pilotes.hitachi.valeur_numerique")),
        "fan_speed" => {
            let word = text(&command.value)?;
            model::fan_to_cloud(&word)
                .map(|f| Change::Fan(f.to_owned()))
                .ok_or_else(|| moli_i18n::tr!("pilotes.hitachi.ventilation_inconnue", speed = word))
        }
        other => Err(moli_i18n::tr!(
            "pilotes.hitachi.point_non_ecrivable",
            point = other
        )),
    }
}

/// Takes an order in: checked now against what the unit will be, answered at
/// once (the hub waits only a few seconds, the cloud takes longer), queued to
/// go once the person stops tapping. What becomes of it shows on the unit
/// (`last_order`, `order_failed`).
fn accept(ctx: &DriverCtx, fleet: &mut Fleet, command: CommandRequest) {
    let change = match change_of(&command) {
        Ok(change) => change,
        Err(why) => return command.reply(Err(why)),
    };
    let found = fleet
        .units
        .keys()
        .find(|native| ctx.device_id(native) == command.device.id)
        .cloned();
    let Some(native) = found else {
        return command.reply(Err(moli_i18n::tr!("pilotes.hitachi.appareil_inconnu")));
    };
    let now = Instant::now();
    let cloud = &fleet.units[&native];
    if !cloud.online {
        return command.reply(Err(moli_i18n::tr!("pilotes.hitachi.hors_ligne")));
    }
    let base = fleet.orders.projected(&native, cloud, now);
    let next = match model::apply(&base, &fleet.caps, &change) {
        Ok(next) => next,
        Err(why) => {
            return command.reply(Err(moli_i18n::tr!(
                "pilotes.hitachi.ordre_refuse",
                why = why
            )));
        }
    };
    if next != base {
        fleet.orders.enqueue(&native, next, &change, now);
    }
    command.reply(Ok(()));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options_are_checked() {
        let bad: Result<Config, _> = serde_json::from_str(r#"{"password":"nope"}"#);
        assert!(bad.is_err(), "a password never goes in moli.toml");
        let good: Config = serde_json::from_str("{}").unwrap();
        assert_eq!(good.poll_secs, 60);
        assert!(good.family.is_none());
    }
}
