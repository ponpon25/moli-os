//! Netatmo Smart Video Doorbell, through the owner's own Netatmo Connect
//! app: a ring and a visitor at the door become points of the doorbell,
//! which automations can answer (a chime on a speaker, a notification).
//!
//! The doorbell is read, never commanded. Two ways to hear of an event:
//! - Netatmo's *webhook* (real time): the driver gives Netatmo the address
//!   `{public_url}/api/netatmo/{webhook_token}`; a push there only wakes the
//!   driver, which then reads the events from the (authenticated) API, so a
//!   forged push can ring nothing;
//! - *polling*, always on as a safety net (every 10 s without a webhook,
//!   every minute with one); the rate limit (500 calls an hour per user) sets
//!   the shortest period.
//!
//! The credentials are cautious ones:
//! - the app's identifier and secret and the refresh token live in the secret
//!   store only;
//! - the refresh token is single-use: the new one is stored before the old
//!   one is forgotten, and a refusal parks the driver (`login_locked`) until
//!   a person lifts it, never a retry loop;
//! - the doorbells are discovered from the account, never listed in the config.

mod api;

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use moli_core::{Access, Device, DeviceId, Kind, PointSpec, Semantic, Value};
use moli_runtime::{BoxFuture, Driver, DriverCtx};
use serde::Deserialize;
use serde_json::Value as Json;
use tokio::sync::Notify;
use tokio::time::Instant;

use api::{ApiError, Bell, Client};

const CLIENT_ID: &str = "client_id";
const CLIENT_SECRET: &str = "client_secret";
const REFRESH_TOKEN: &str = "refresh_token";
/// Present once Netatmo refused the credentials: nothing exchanges again
/// until a person removes it.
const LOCKED: &str = "login_locked";
/// The secret part of the webhook's address: random, 16 characters at least.
pub const WEBHOOK_TOKEN: &str = "webhook_token";

/// With a webhook, the poll is only a safety net.
const SAFETY_POLL: u64 = 60;
/// After a push, the events are read again at these delays: Netatmo's list
/// may lag a moment behind its push.
const AFTER_PUSH: [Duration; 2] = [Duration::from_secs(2), Duration::from_secs(6)];
/// The webhook address is given again this often.
const WEBHOOK_EVERY: Duration = Duration::from_secs(3600);

/// 500 calls an hour per user: one every 8 s is 450, and the doorbell's
/// status is asked far less often.
const MIN_POLL: u64 = 8;
const STATUS_EVERY: Duration = Duration::from_secs(120);
const STEP_WAIT: Duration = Duration::from_secs(30);
const MAX_WAIT: Duration = Duration::from_secs(5 * 60);
/// How many events a poll asks for.
const EVENTS: u32 = 30;
/// An event older than this when first seen (after an outage) is recorded
/// but not announced: nobody is at the door any more.
const FRESH_SECS: i64 = 120;

/// Driver configuration (`[driver.options]` in `moli.toml`). The app's
/// identifier, its secret and the token are not here: they live in the
/// secret store.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Seconds between two requests for the latest events (8 at least).
    #[serde(default = "default_poll")]
    pub poll_secs: u64,
    /// The name the Internet reaches Moli by (`https://…`, no path): Netatmo
    /// pushes events to `{public_url}/api/netatmo/{webhook_token}`. Absent:
    /// polling only.
    #[serde(default)]
    pub public_url: Option<String>,
}

fn default_poll() -> u64 {
    10
}

/// What the API holds of the driver: where a push from Netatmo lands.
#[derive(Clone, Debug)]
pub struct Hook {
    instance: String,
    wake: Arc<Notify>,
}

impl Hook {
    /// The instance whose secret store holds the webhook token.
    #[must_use]
    pub fn instance(&self) -> &str {
        &self.instance
    }

    /// Netatmo pushed something: the driver reads the events now. Pushes
    /// that come close together make one wake-up.
    pub fn wake(&self) {
        self.wake.notify_one();
    }
}

#[derive(Debug)]
pub struct Netatmo {
    config: Config,
    wake: Arc<Notify>,
}

impl Netatmo {
    #[must_use]
    pub fn new(config: Config) -> Self {
        Self {
            config,
            wake: Arc::new(Notify::new()),
        }
    }

    /// The driver and the handle the API gives pushes to.
    #[must_use]
    pub fn open(config: Config, instance: &str) -> (Hook, Self) {
        let driver = Self::new(config);
        let hook = Hook {
            instance: instance.to_owned(),
            wake: driver.wake.clone(),
        };
        (hook, driver)
    }
}

impl Driver for Netatmo {
    fn kind(&self) -> &'static str {
        "netatmo"
    }

    fn run<'a>(&'a self, ctx: &'a mut DriverCtx) -> BoxFuture<'a, anyhow::Result<()>> {
        Box::pin(run(&self.config, &self.wake, ctx))
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
}

fn passing(e: ApiError) -> Stop {
    Stop::Fail(anyhow::Error::new(e))
}

async fn park(ctx: &DriverCtx, reason: String) -> anyhow::Result<()> {
    ctx.wait_for(reason);
    ctx.cancelled().await;
    Ok(())
}

/// What an event is for the person at home.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Occasion {
    /// Someone pressed the button.
    Ring,
    /// Someone is at the door (the camera saw a person).
    Visitor,
    /// Anything else the doorbell reports (calls answered or missed, a
    /// vehicle…): only written in `last_event`.
    Other,
}

/// Netatmo's own names are not published in one place and the push and the
/// poll disagree on prefixes: anything that says "call" and starts with
/// "incoming" is a ring.
fn occasion(kind: &str) -> Occasion {
    let kind = kind.to_ascii_lowercase();
    if kind.contains("incoming_call") || kind == "ring" || kind == "doorbell" {
        Occasion::Ring
    } else if matches!(kind.as_str(), "human" | "person" | "person_home") {
        Occasion::Visitor
    } else {
        Occasion::Other
    }
}

/// One event of a poll.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Event {
    /// Netatmo's identifier, or `type:time` when it gave none.
    key: String,
    /// Netatmo's identifier alone: the same event gains content, so it is
    /// answered once whatever the key says.
    base: String,
    module: Option<String>,
    kind: String,
    /// What the event says it holds (`subevents`): a detection is typed
    /// `outdoor`, with `human` inside, often added after the event first
    /// appears.
    subs: Vec<String>,
    /// The calls inside the event: (identifier, type, time). Netatmo puts
    /// several presses close together in one event, each its own call.
    calls: Vec<(String, String, i64)>,
    /// The names of the event's fields, to see what Netatmo really sends.
    fields: String,
    time: i64,
}

impl Event {
    /// A ring says so in its own type, or the doorbell's event carries a
    /// call (`incoming_call`, then `accepted_call` or `missed_call` as it
    /// goes on); a person is seen as a type or inside an `outdoor` detection.
    fn occasion(&self) -> Occasion {
        let calls = self.subs.iter().any(|sub| sub.ends_with("_call"));
        match occasion(&self.kind) {
            Occasion::Other if calls => Occasion::Ring,
            Occasion::Other => self
                .subs
                .iter()
                .map(|sub| occasion(sub))
                .find(|o| *o != Occasion::Other)
                .unwrap_or(Occasion::Other),
            known => known,
        }
    }
}

fn events_of(list: &[Json]) -> Vec<Event> {
    list.iter()
        .filter_map(|e| {
            let kind = e["type"].as_str()?.to_owned();
            let time = e["time"].as_i64().unwrap_or(0);
            let base = e["id"]
                .as_str()
                .map_or_else(|| format!("{kind}:{time}"), str::to_owned);
            let subs: Vec<String> = ["subevents", "subevent_list", "event_list"]
                .iter()
                .flat_map(|list| e[*list].as_array().into_iter().flatten())
                .filter_map(|sub| sub["type"].as_str().map(str::to_owned))
                .collect();
            let key = if subs.is_empty() {
                base.clone()
            } else {
                format!("{base}+{}", subs.join("+"))
            };
            let calls: Vec<(String, String, i64)> = ["subevents", "subevent_list", "event_list"]
                .iter()
                .flat_map(|list| e[*list].as_array().into_iter().flatten())
                .filter_map(|sub| {
                    let kind = sub["type"].as_str()?;
                    let when = sub["time"].as_i64().unwrap_or(time);
                    kind.ends_with("_call").then(|| {
                        let id = sub["id"]
                            .as_str()
                            .map_or_else(|| format!("{kind}:{when}"), str::to_owned);
                        (id, kind.to_owned(), when)
                    })
                })
                .collect();
            let fields = e
                .as_object()
                .map(|o| o.keys().cloned().collect::<Vec<_>>().join(","))
                .unwrap_or_default();
            Some(Event {
                key,
                base: base.clone(),
                module: e["module_id"].as_str().map(str::to_owned),
                kind,
                subs,
                calls,
                fields,
                time,
            })
        })
        .collect()
}

fn spec(key: &str, label: String, kind: Kind, semantic: Semantic) -> PointSpec {
    PointSpec {
        key: key.into(),
        label: label.into(),
        semantic,
        kind,
        access: Access {
            read: true,
            write: false,
        },
        unit: None,
    }
}

fn device_of(ctx: &DriverCtx, bell: &Bell) -> Device {
    Device {
        id: ctx.device_id(&bell.id),
        instance: ctx.instance().clone(),
        native_name: bell.name.clone().into(),
        manufacturer: Some("Netatmo".into()),
        model: Some("Smart Video Doorbell".into()),
        description: None,
        native_room: None,
        members: Vec::new(),
        points: vec![
            spec(
                "ring",
                moli_i18n::tr!("pilotes.netatmo.sonnerie"),
                Kind::Binary,
                Semantic::Other,
            ),
            spec(
                "visitor",
                moli_i18n::tr!("pilotes.netatmo.visiteur"),
                Kind::Binary,
                Semantic::Other,
            ),
            spec(
                "last_event",
                moli_i18n::tr!("pilotes.netatmo.dernier_evenement"),
                Kind::Text,
                Semantic::Other,
            ),
        ],
    }
}

/// Everything the driver keeps between two sessions: the events already
/// seen, so that nothing rings twice or rings again after an outage.
#[derive(Default)]
struct Seen {
    keys: HashSet<String>,
    /// False until the first poll: what is already there is history.
    primed: bool,
}

impl Seen {
    /// The events not seen before, oldest first. The first poll only
    /// records what is there.
    fn fresh(&mut self, events: Vec<Event>) -> Vec<Event> {
        let present: HashSet<String> = events.iter().map(|e| e.key.clone()).collect();
        let mut new: Vec<Event> = if self.primed {
            events
                .into_iter()
                .filter(|e| !self.keys.contains(&e.key))
                .collect()
        } else {
            Vec::new()
        };
        new.sort_by_key(|e| e.time);
        // Only what Netatmo still lists is remembered: the set stays small.
        self.keys.retain(|k| present.contains(k));
        self.keys.extend(present);
        self.primed = true;
        new
    }
}

/// What the driver keeps from one session to the next.
#[derive(Default)]
struct Keep {
    seen: Seen,
    /// The events already rung for, by Netatmo's identifier.
    rung: HashSet<String>,
    known: Vec<Bell>,
    failures: u32,
}

/// When the press happened, if this event holds a press not rung for yet:
/// every call is rung for once, and a call that only says it was answered
/// or missed rings only when its event never told of the press itself.
fn new_ring(event: &Event, rung: &mut HashSet<String>) -> Option<i64> {
    let marker = format!("event:{}", event.base);
    if event.calls.is_empty() {
        // A ring that is the event's own type.
        return rung.insert(marker).then_some(event.time);
    }
    let told = event
        .calls
        .iter()
        .any(|(_, kind, _)| kind.contains("incoming"));
    let mut pressed = None;
    for (id, kind, when) in &event.calls {
        if !rung.insert(id.clone()) {
            continue;
        }
        if kind.contains("incoming") || (!told && !rung.contains(&marker)) {
            pressed = Some(*when);
        }
    }
    if pressed.is_some() {
        rung.insert(marker);
    }
    pressed
}

/// Says what an event is on the doorbell it came from.
fn announce(ctx: &DriverCtx, bells: &[Bell], event: &Event, now: i64, rung: &mut HashSet<String>) {
    // An event with no module is the home's only doorbell's.
    let bell = event
        .module
        .as_ref()
        .and_then(|m| bells.iter().find(|b| &b.id == m))
        .or_else(|| (bells.len() == 1).then(|| &bells[0]));
    let Some(bell) = bell else {
        return;
    };
    let id: DeviceId = ctx.device_id(&bell.id);
    ctx.set_state(&id, "last_event", Value::Text(event.kind.clone().into()));
    if now - event.time > FRESH_SECS {
        return;
    }
    match event.occasion() {
        Occasion::Ring => {
            let Some(pressed) = new_ring(event, rung) else {
                return;
            };
            tracing::info!(instance = %ctx.instance(), subs = %event.subs.join(","), age_s = now - pressed, "netatmo doorbell rung");
            pulse(ctx, &id, "ring");
        }
        Occasion::Visitor => {
            tracing::info!(instance = %ctx.instance(), kind = %event.kind, subs = %event.subs.join(","), age_s = now - event.time, "netatmo visitor");
            pulse(ctx, &id, "visitor");
        }
        Occasion::Other => {
            tracing::info!(instance = %ctx.instance(), kind = %event.kind, subs = %event.subs.join(","), age_s = now - event.time, fields = %event.fields, "netatmo event");
        }
    }
}

/// An occurrence: the point goes to yes, then back to no at once. The
/// automations only see a change, so a point left at yes would let the next
/// ring go unnoticed.
fn pulse(ctx: &DriverCtx, id: &DeviceId, key: &str) {
    ctx.pulse_state(id, key, Value::Bool(true));
    ctx.set_state(id, key, Value::Bool(false));
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

/// A session for a call, renewed when it is about to end. The new refresh
/// token is stored before anything else is done.
async fn sign_in(client: &mut Client, ctx: &DriverCtx) -> Result<(), Stop> {
    if client.has_session() {
        return Ok(());
    }
    match client.exchange().await {
        Ok(refresh) => ctx
            .store_secret(REFRESH_TOKEN, &refresh)
            .await
            .map_err(|e| Stop::Fail(anyhow::Error::new(e).context("refresh token not stored"))),
        Err(ApiError::Credentials(why)) => {
            // Recorded before parking: a restart must not try again.
            ctx.store_secret(LOCKED, "1")
                .await
                .map_err(|e| Stop::Fail(e.into()))?;
            Err(Stop::Park(moli_i18n::tr!(
                "pilotes.netatmo.connexion_refusee",
                why = why,
                instance = ctx.instance()
            )))
        }
        Err(e) => Err(passing(e)),
    }
}

async fn run(config: &Config, wake: &Notify, ctx: &mut DriverCtx) -> anyhow::Result<()> {
    if !ctx.can_store_secrets() {
        let problem = ctx.secrets_problem().map_or_else(
            || moli_i18n::tr!("pilotes.netatmo.coffre_indisponible"),
            str::to_owned,
        );
        let reason = moli_i18n::tr!("pilotes.netatmo.coffre_requis", problem = problem);
        return park(ctx, reason).await;
    }
    let (Some(id), Some(secret), Some(token)) = (
        ctx.secret(CLIENT_ID),
        ctx.secret(CLIENT_SECRET),
        ctx.secret(REFRESH_TOKEN),
    ) else {
        let reason = moli_i18n::tr!(
            "pilotes.netatmo.identifiants_manquants",
            instance = ctx.instance()
        );
        return park(ctx, reason).await;
    };
    if ctx.secret(LOCKED).is_some() {
        let reason = moli_i18n::tr!(
            "pilotes.netatmo.connexion_suspendue",
            instance = ctx.instance()
        );
        return park(ctx, reason).await;
    }
    let mut client = Client::new(&id, &secret, &token);
    let mut keep = Keep::default();
    loop {
        match session(config, wake, &mut client, ctx, &mut keep).await {
            Stop::Done => return Ok(()),
            Stop::Park(reason) => return park(ctx, reason).await,
            Stop::Fail(e) => {
                keep.failures += 1;
                let wait = (STEP_WAIT * keep.failures).min(MAX_WAIT);
                tracing::warn!(instance = %ctx.instance(), error = %format!("{e:#}"), wait_s = wait.as_secs(), "netatmo session failed");
                if keep.failures >= 2 {
                    for bell in &keep.known {
                        ctx.set_availability(&ctx.device_id(&bell.id), false);
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

/// One session: exchange the token, find the doorbells, then poll their
/// events until something breaks.
#[allow(clippy::too_many_lines)]
async fn session(
    config: &Config,
    wake: &Notify,
    client: &mut Client,
    ctx: &mut DriverCtx,
    keep: &mut Keep,
) -> Stop {
    if let Err(stop) = sign_in(client, ctx).await {
        return stop;
    }
    let bells = match client.doorbells().await {
        Ok(bells) => bells,
        Err(e) => return passing(e),
    };
    for gone in keep.known.iter().filter(|k| !bells.contains(k)) {
        ctx.remove_device(&ctx.device_id(&gone.id));
    }
    for bell in &bells {
        if !keep.known.contains(bell) {
            ctx.upsert_device(device_of(ctx, bell));
            // At rest: a value left at yes by an earlier run is not a ring.
            for key in ["ring", "visitor"] {
                ctx.set_state(&ctx.device_id(&bell.id), key, Value::Bool(false));
            }
            tracing::info!(instance = %ctx.instance(), "netatmo doorbell found");
        }
    }
    keep.known.clone_from(&bells);
    if bells.is_empty() {
        tracing::warn!(instance = %ctx.instance(), "netatmo: no doorbell in the account");
    }
    ctx.ready();
    keep.failures = 0;

    let hook_url = webhook_url(config, ctx);
    let mut hooked = false;
    let mut next_hook = Instant::now();
    let mut quick: Vec<Instant> = Vec::new();
    let mut next_status = Instant::now();
    let mut soft = 0u32;
    loop {
        if let Err(stop) = sign_in(client, ctx).await {
            return stop;
        }
        if let Some(url) = &hook_url
            && Instant::now() >= next_hook
        {
            match client.add_webhook(url).await {
                Ok(()) => {
                    if !hooked {
                        tracing::info!(instance = %ctx.instance(), "netatmo webhook registered");
                    }
                    hooked = true;
                    next_hook = Instant::now() + WEBHOOK_EVERY;
                }
                Err(ApiError::SessionExpired) => client.forget_session(),
                Err(e) => {
                    // Not fatal: the poll goes on at its fast pace.
                    tracing::warn!(instance = %ctx.instance(), error = %e, "netatmo webhook not registered");
                    hooked = false;
                    next_hook = Instant::now() + STEP_WAIT * 4;
                }
            }
        }
        let poll = if hooked {
            Duration::from_secs(SAFETY_POLL)
        } else {
            Duration::from_secs(config.poll_secs.max(MIN_POLL))
        };
        let mut homes: Vec<&str> = bells.iter().map(|b| b.home.as_str()).collect();
        homes.sort_unstable();
        homes.dedup();
        for home in homes {
            match client.events(home, EVENTS).await {
                Ok(list) => {
                    soft = 0;
                    let now = unix_now();
                    for event in keep.seen.fresh(events_of(&list)) {
                        announce(ctx, &bells, &event, now, &mut keep.rung);
                    }
                }
                Err(ApiError::SessionExpired) => client.forget_session(),
                // Netatmo answers 503 now and then, often right after a push:
                // the next read catches up, so a session is not worth ending.
                Err(e @ (ApiError::Refused(_) | ApiError::Network(_))) if soft < 3 => {
                    soft += 1;
                    tracing::warn!(instance = %ctx.instance(), error = %e, "netatmo events not read");
                }
                Err(e) => return passing(e),
            }
            if Instant::now() >= next_status {
                match client.reachable(home).await {
                    Ok(list) => {
                        for bell in bells.iter().filter(|b| b.home == home) {
                            let up = list
                                .iter()
                                .find(|(id, _)| *id == bell.id)
                                .is_none_or(|(_, up)| *up);
                            ctx.set_availability(&ctx.device_id(&bell.id), up);
                        }
                    }
                    Err(ApiError::SessionExpired) => client.forget_session(),
                    Err(e @ (ApiError::Refused(_) | ApiError::Network(_))) => {
                        tracing::warn!(instance = %ctx.instance(), error = %e, "netatmo status not read");
                    }
                    Err(e) => return passing(e),
                }
            }
        }
        if Instant::now() >= next_status {
            next_status = Instant::now() + STATUS_EVERY;
        }
        // Until the next poll, a push or a command wakes the loop.
        let deadline = Instant::now() + poll;
        loop {
            let now = Instant::now();
            quick.retain(|t| *t > now);
            let until = quick
                .iter()
                .copied()
                .min()
                .map_or(deadline, |t| t.min(deadline));
            tokio::select! {
                command = ctx.next_command() => {
                    // Nothing is written on a doorbell.
                    let Some(command) = command else { return Stop::Done };
                    command.reply(Err(moli_i18n::tr!("pilotes.netatmo.lecture_seule")));
                    continue;
                }
                () = wake.notified() => {
                    tracing::info!(instance = %ctx.instance(), "netatmo push received");
                    let at = Instant::now();
                    quick.extend(AFTER_PUSH.iter().map(|d| at + *d));
                }
                () = tokio::time::sleep_until(until) => {}
            }
            break;
        }
    }
}

/// The address Netatmo is given, when the driver has what it takes.
fn webhook_url(config: &Config, ctx: &DriverCtx) -> Option<String> {
    let base = config.public_url.as_deref()?.trim_end_matches('/');
    let Some(token) = ctx.secret(WEBHOOK_TOKEN) else {
        tracing::warn!(instance = %ctx.instance(), "netatmo: public_url set but no webhook_token secret: polling only");
        return None;
    };
    Some(format!("{base}/api/netatmo/{token}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Vec<Event> {
        let answer: Json = serde_json::from_str(include_str!(
            "../../../integrations/netatmo/fixtures/events.json"
        ))
        .unwrap();
        events_of(answer["body"]["home"]["events"].as_array().unwrap())
    }

    #[test]
    fn options_are_checked() {
        let bad: Result<Config, _> = serde_json::from_str(r#"{"client_secret":"nope"}"#);
        assert!(bad.is_err(), "a secret never goes in moli.toml");
        let good: Config = serde_json::from_str("{}").unwrap();
        assert_eq!(good.poll_secs, 10);
        assert!(good.public_url.is_none());
    }

    #[test]
    fn rings_are_told_from_the_rest() {
        assert_eq!(occasion("incoming_call"), Occasion::Ring);
        assert_eq!(occasion("NDB-incoming_call"), Occasion::Ring);
        assert_eq!(occasion("human"), Occasion::Visitor);
        assert_eq!(occasion("missed_call"), Occasion::Other);
        assert_eq!(occasion("accepted_call"), Occasion::Other);
    }

    #[test]
    fn a_person_seen_in_a_detection_is_a_visitor() {
        let list: Vec<Json> = serde_json::from_str(
            r#"[{"id":"a","type":"outdoor","time":1,"subevents":[{"type":"human"}]},
                {"id":"b","type":"outdoor","time":2,"subevent_list":[{"type":"vehicle"}]},
                {"id":"c","type":"outdoor","time":3}]"#,
        )
        .unwrap();
        let events = events_of(&list);
        assert_eq!(events[0].occasion(), Occasion::Visitor);
        assert_eq!(events[1].occasion(), Occasion::Other);
        assert_eq!(events[2].occasion(), Occasion::Other);
        assert_eq!(events[0].subs, ["human"]);
        assert_eq!(
            events[0].key, "a+human",
            "an event that gains content is seen again"
        );
        assert_eq!(events[0].base, "a");
    }

    #[test]
    fn a_call_inside_a_detection_is_a_ring() {
        let list: Vec<Json> = serde_json::from_str(
            r#"[{"id":"a","type":"outdoor","time":1,"subevents":[{"type":"accepted_call"}]},
                {"id":"b","type":"outdoor","time":2,"subevents":[{"type":"incoming_call"}]},
                {"id":"c","type":"outdoor","time":3,"subevents":[{"type":"missed_call"}]}]"#,
        )
        .unwrap();
        for event in events_of(&list) {
            assert_eq!(event.occasion(), Occasion::Ring, "{}", event.base);
        }
    }

    #[test]
    fn two_presses_in_one_event_ring_twice_and_an_answer_does_not() {
        let list = |subs: &str| -> Vec<Event> {
            let text =
                format!(r#"[{{"id":"e","type":"outdoor","time":100,"subevents":[{subs}]}}]"#);
            events_of(&serde_json::from_str::<Vec<Json>>(&text).unwrap())
        };
        let mut rung = HashSet::new();
        let first = list(r#"{"id":"s1","type":"incoming_call","time":110}"#);
        assert_eq!(new_ring(&first[0], &mut rung), Some(110));
        assert_eq!(new_ring(&first[0], &mut rung), None, "once per call");
        // The visitor presses again in the same event.
        let second = list(
            r#"{"id":"s1","type":"incoming_call","time":110},{"id":"s2","type":"incoming_call","time":140}"#,
        );
        assert_eq!(new_ring(&second[0], &mut rung), Some(140));
        // Someone answers: no new ring.
        let answered = list(
            r#"{"id":"s1","type":"incoming_call","time":110},{"id":"s2","type":"incoming_call","time":140},{"id":"s3","type":"accepted_call","time":150}"#,
        );
        assert_eq!(new_ring(&answered[0], &mut rung), None);
        // An answered call whose press was never seen still rings, once.
        let mut rung = HashSet::new();
        let late = list(r#"{"id":"s9","type":"accepted_call","time":120}"#);
        assert_eq!(new_ring(&late[0], &mut rung), Some(120));
        assert_eq!(new_ring(&late[0], &mut rung), None);
    }

    #[test]
    fn history_is_not_announced_and_news_is_once() {
        let events = fixture();
        assert_eq!(events.len(), 3);
        let mut seen = Seen::default();
        // The first poll only records.
        assert!(seen.fresh(events.clone()).is_empty());
        // The same list again: nothing new.
        assert!(seen.fresh(events.clone()).is_empty());
        // A new ring appears at the top.
        let mut more = events;
        more.insert(
            0,
            Event {
                key: "new".into(),
                base: "new".into(),
                module: None,
                kind: "incoming_call".into(),
                subs: Vec::new(),
                calls: Vec::new(),
                fields: String::new(),
                time: 1_800_000_000,
            },
        );
        let fresh = seen.fresh(more.clone());
        assert_eq!(fresh.len(), 1);
        assert_eq!(fresh[0].key, "new");
        assert!(seen.fresh(more).is_empty(), "once only");
    }

    #[test]
    fn an_event_without_identifier_gets_one() {
        let list: Vec<Json> =
            serde_json::from_str(r#"[{"type":"incoming_call","time":42}]"#).unwrap();
        let events = events_of(&list);
        assert_eq!(events[0].key, "incoming_call:42");
    }
}
