//! Verisure alarm, through the web API of the mobile app: the panel's state
//! (read from the panel, every couple of minutes) and arm / disarm orders.
//!
//! The account is a real one with a real alarm, so the driver is cautious:
//! - one login attempt, ever, until a person lifts the lock (a wrong
//!   password counts toward a lock of the account);
//! - the second factor (an SMS) goes through the secret store, typed by a
//!   person, never read from or written to a log;
//! - each refresh token is stored the moment it arrives (it is single-use);
//! - arm and disarm are `control` points: the guard holds them for a human
//!   whatever their origin, the household's dashboard included.
//!
//! Nothing here is published by Verisure: see `api.rs`.

mod api;
mod model;

use std::time::Duration;

use moli_core::{Access, Device, Kind, PointSpec, Semantic, Value};
use moli_runtime::{BoxFuture, CommandRequest, Driver, DriverCtx};
use serde::Deserialize;

use api::{ApiError, Client, DeviceStep, LoginOutcome, Tokens};
use model::Installation;

const USERNAME: &str = "username";
const PASSWORD: &str = "password";
const DEVICE: &str = "device_id";
const REFRESH: &str = "refresh_token";
const OTP_HASH: &str = "otp_hash";
const OTP_CODE: &str = "otp_code";
const OTP_PHONE: &str = "otp_phone";
/// Present once a login was tried and not judged by the network alone:
/// nothing logs in again until a person removes it.
const LOCKED: &str = "login_locked";

const MIN_POLL: u64 = 60;
/// After a firewall block.
const WAF_BACKOFF: Duration = Duration::from_secs(10 * 60);
/// After a rate limit.
const RATE_BACKOFF: Duration = Duration::from_secs(5 * 60);
/// After anything else.
const ERROR_BACKOFF: Duration = Duration::from_secs(2 * 60);
/// A panel asked again soon after an order: it needs a moment to settle.
const AFTER_ORDER: Duration = Duration::from_secs(20);

/// Driver configuration (`[driver.options]` in `moli.toml`). The identifier
/// and the password are not here: they live in the secret store.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default = "default_country")]
    pub country: String,
    /// The installation to use (its number), when the account has several.
    #[serde(default)]
    pub installation: Option<String>,
    /// Seconds between two readings of the panel (60 at least: Verisure's
    /// firewall counts requests).
    #[serde(default = "default_poll")]
    pub poll_secs: u64,
}

fn default_country() -> String {
    "FR".into()
}

fn default_poll() -> u64 {
    120
}

#[derive(Debug)]
pub struct Verisure {
    config: Config,
}

impl Verisure {
    #[must_use]
    pub fn new(config: Config) -> Self {
        Self { config }
    }
}

impl Driver for Verisure {
    fn kind(&self) -> &'static str {
        "verisure"
    }

    fn run<'a>(&'a self, ctx: &'a mut DriverCtx) -> BoxFuture<'a, anyhow::Result<()>> {
        Box::pin(run(&self.config, ctx))
    }
}

/// Why the driver stops going forward.
enum Stop {
    /// A person must act: shown everywhere, nothing else is tried.
    Park(String),
    /// Something passing: the supervisor starts again, with its backoff.
    Fail(anyhow::Error),
}

impl From<std::io::Error> for Stop {
    fn from(e: std::io::Error) -> Self {
        Self::Fail(e.into())
    }
}

/// An API error that is not a person's problem.
fn passing(e: ApiError) -> Stop {
    Stop::Fail(anyhow::Error::new(e))
}

async fn park(ctx: &DriverCtx, reason: String) -> anyhow::Result<()> {
    ctx.wait_for(reason);
    ctx.cancelled().await;
    Ok(())
}

fn secrets_problem(ctx: &DriverCtx) -> String {
    ctx.secrets_problem().map_or_else(
        || moli_i18n::tr!("pilotes.verisure.coffre_indisponible"),
        str::to_owned,
    )
}

/// Stores the tokens: the refresh token first, before anything else.
async fn keep(ctx: &DriverCtx, tokens: &Tokens) -> Result<(), Stop> {
    if let Some(refresh) = &tokens.refresh {
        ctx.store_secret(REFRESH, refresh).await?;
    }
    Ok(())
}

/// A session, by the cheapest way that works: the refresh token, then the
/// second factor in progress, then (once) the password.
async fn authenticate(client: &mut Client, ctx: &DriverCtx) -> Result<(), Stop> {
    if let Some(token) = ctx.secret(REFRESH) {
        match client.refresh(&token).await {
            Ok(Some(tokens)) => {
                keep(ctx, &tokens).await?;
                return Ok(());
            }
            // No longer accepted: forgotten, the password takes over.
            Ok(None) => {
                ctx.forget_secret(REFRESH).await?;
            }
            Err(e) => return Err(passing(e)),
        }
    }

    // A second factor under way: the code, or the wait for it.
    if let Some(hash) = ctx.secret(OTP_HASH) {
        let Some(code) = ctx.secret(OTP_CODE) else {
            return Err(Stop::Park(moli_i18n::tr!(
                "pilotes.verisure.attente_code",
                instance = ctx.instance()
            )));
        };
        // Spent whatever happens, before the call: a code is good for one
        // try, and a hash with no code would wait for an SMS nobody sent.
        ctx.forget_secret(OTP_CODE).await?;
        ctx.forget_secret(OTP_HASH).await?;
        match client
            .validate_device(Some((&hash, &code)))
            .await
            .map_err(passing)?
        {
            DeviceStep::Tokens(tokens) => return keep(ctx, &tokens).await,
            // The device is trusted: the login below gets the tokens.
            DeviceStep::Validated => {}
            DeviceStep::Challenge { .. } => {
                return Err(Stop::Park(moli_i18n::tr!(
                    "pilotes.verisure.code_refuse",
                    instance = ctx.instance()
                )));
            }
        }
    }

    if ctx.secret(LOCKED).is_some() {
        return Err(Stop::Park(moli_i18n::tr!(
            "pilotes.verisure.connexion_suspendue",
            instance = ctx.instance()
        )));
    }
    let Some(password) = ctx.secret(PASSWORD) else {
        return Err(Stop::Park(missing(ctx)));
    };
    // Recorded before the attempt: a crash during it must not repeat it.
    ctx.store_secret(LOCKED, "1").await?;
    match client.login(&password).await {
        Ok(LoginOutcome::Tokens(tokens)) => {
            ctx.forget_secret(LOCKED).await?;
            keep(ctx, &tokens).await
        }
        Ok(LoginOutcome::NeedsDevice) => {
            ctx.forget_secret(LOCKED).await?;
            second_factor(client, ctx).await
        }
        Err(ApiError::Blocked) => Err(Stop::Park(moli_i18n::tr!(
            "pilotes.verisure.compte_bloque",
            instance = ctx.instance()
        ))),
        Err(ApiError::Refused(why)) => Err(Stop::Park(moli_i18n::tr!(
            "pilotes.verisure.connexion_refusee",
            why = why,
            instance = ctx.instance()
        ))),
        // The network, the firewall: Verisure never judged the password.
        Err(e) => {
            ctx.forget_secret(LOCKED).await?;
            Err(passing(e))
        }
    }
}

fn missing(ctx: &DriverCtx) -> String {
    moli_i18n::tr!("pilotes.verisure.identifiants_manquants", instance = ctx.instance())
}

/// A new device: asks for the SMS, then waits for a person to type it.
async fn second_factor(client: &mut Client, ctx: &DriverCtx) -> Result<(), Stop> {
    match client.validate_device(None).await.map_err(passing)? {
        DeviceStep::Tokens(tokens) => keep(ctx, &tokens).await,
        // Already trusted: the next start logs in again, now without a code.
        DeviceStep::Validated => Err(Stop::Fail(anyhow::anyhow!(
            "verisure device already trusted, logging in again"
        ))),
        DeviceStep::Challenge { hash, phones } => {
            let chosen = ctx
                .secret(OTP_PHONE)
                .and_then(|p| p.trim().parse::<i64>().ok())
                .or_else(|| (phones.len() == 1).then(|| phones[0].id));
            let Some(phone) = chosen else {
                let list = phones
                    .iter()
                    .map(|p| format!("{} = {}", p.id, p.number))
                    .collect::<Vec<_>>()
                    .join(", ");
                return Err(Stop::Park(moli_i18n::tr!(
                    "pilotes.verisure.choisir_telephone",
                    phones = list,
                    instance = ctx.instance()
                )));
            };
            client.send_otp(phone, &hash).await.map_err(passing)?;
            ctx.store_secret(OTP_HASH, &hash).await?;
            tracing::info!(instance = %ctx.instance(), "verisure: a code was sent by SMS");
            Err(Stop::Park(moli_i18n::tr!(
                "pilotes.verisure.attente_code",
                instance = ctx.instance()
            )))
        }
    }
}

fn spec(key: &str, label: String, kind: Kind, write: bool, semantic: Semantic) -> PointSpec {
    PointSpec {
        key: key.into(),
        label: label.into(),
        semantic,
        kind,
        access: Access { read: true, write },
        unit: None,
    }
}

fn alarm_device(ctx: &DriverCtx, installation: &Installation) -> Device {
    // Never the installation's alias: Verisure fills it with the postal
    // address. A person renames the device in Moli if they want to.
    let name = moli_i18n::tr!("pilotes.verisure.alarme");
    Device {
        id: ctx.device_id(&installation.number),
        instance: ctx.instance().clone(),
        native_name: name.into(),
        manufacturer: Some("Verisure".into()),
        model: Some(moli_i18n::tr!("pilotes.verisure.alarme").into()),
        description: None,
        native_room: None,
        members: Vec::new(),
        points: vec![
            spec(
                "state",
                moli_i18n::tr!("pilotes.verisure.etat"),
                Kind::Text,
                false,
                Semantic::Other,
            ),
            // `control`: the guard holds an order here for a human, whoever
            // gives it (an agent, a script, the dashboard without the code).
            spec(
                "mode",
                moli_i18n::tr!("pilotes.verisure.mode"),
                Kind::Enum {
                    values: model::mode_values(),
                },
                true,
                Semantic::Control,
            ),
            spec(
                "last_order",
                moli_i18n::tr!("pilotes.verisure.dernier_ordre"),
                Kind::Text,
                false,
                Semantic::Other,
            ),
        ],
    }
}

/// Publishes what the panel's code says.
fn publish(ctx: &DriverCtx, id: &moli_core::DeviceId, code: &str) {
    let state = model::state_of(code).unwrap_or("unknown");
    ctx.set_state(id, "state", Value::Text(state.into()));
    if let Some(mode) = model::mode_of(state) {
        ctx.set_state(id, "mode", Value::Text(mode.into()));
    }
}

/// A session for a call, renewed first when it is about to end.
async fn session(client: &mut Client, ctx: &DriverCtx) -> Result<(), Stop> {
    if client.has_session() {
        return Ok(());
    }
    authenticate(client, ctx).await
}

/// How long to wait after a failure, by what it was.
fn backoff_for(e: &anyhow::Error) -> Duration {
    match e.downcast_ref::<ApiError>() {
        Some(ApiError::Waf) => WAF_BACKOFF,
        Some(ApiError::RateLimited) => RATE_BACKOFF,
        _ => ERROR_BACKOFF,
    }
}

/// Everything before the first reading: a session, the installation.
async fn startup(
    client: &mut Client,
    ctx: &DriverCtx,
    config: &Config,
) -> Result<Installation, Stop> {
    session(client, ctx).await?;
    let installations = client.installations().await.map_err(passing)?;
    let chosen = match &config.installation {
        Some(number) => installations.iter().find(|i| &i.number == number),
        None => installations.first(),
    };
    chosen
        .cloned()
        .ok_or_else(|| Stop::Park(moli_i18n::tr!("pilotes.verisure.aucune_installation")))
}

async fn run(config: &Config, ctx: &mut DriverCtx) -> anyhow::Result<()> {
    if !ctx.can_store_secrets() {
        let reason = moli_i18n::tr!(
            "pilotes.verisure.coffre_requis",
            problem = secrets_problem(ctx)
        );
        return park(ctx, reason).await;
    }
    let Some(username) = ctx.secret(USERNAME) else {
        return park(ctx, missing(ctx)).await;
    };
    let device = match ctx.secret(DEVICE) {
        Some(device) => device,
        None => {
            let fresh = api::random_hex(16);
            ctx.store_secret(DEVICE, &fresh).await?;
            fresh
        }
    };
    let mut client = Client::new(&config.country, &username, &device);

    // A failure that is nobody's fault waits here, inside the driver: a
    // restart would log in again, and a firewall block must be left alone.
    let installation = loop {
        match startup(&mut client, ctx, config).await {
            Ok(installation) => break installation,
            Err(Stop::Park(reason)) => return park(ctx, reason).await,
            Err(Stop::Fail(e)) => {
                let wait = backoff_for(&e);
                tracing::warn!(instance = %ctx.instance(), error = %format!("{e:#}"), wait_s = wait.as_secs(), "verisure start failed");
                tokio::select! {
                    () = ctx.cancelled() => return Ok(()),
                    () = tokio::time::sleep(wait) => {}
                }
            }
        }
    };
    // What this panel says it can do: the modes it really takes.
    if client.ensure_capabilities(&installation).await.is_ok() {
        let (caps, partitions) = client.panel_facts();
        tracing::info!(instance = %ctx.instance(), capabilities = ?caps, partitions = %partitions, "verisure panel");
    }
    let id = ctx.device_id(&installation.number);
    ctx.upsert_device(alarm_device(ctx, &installation));
    tracing::info!(instance = %ctx.instance(), "verisure installation found");
    ctx.ready();

    let poll = Duration::from_secs(config.poll_secs.max(MIN_POLL));
    let mut next = tokio::time::Instant::now();
    let mut failures = 0u32;
    // The panel's last code: an order says it back (the app does the same).
    let mut current = String::new();

    loop {
        tokio::select! {
            command = ctx.next_command() => {
                let Some(command) = command else { return Ok(()) };
                match order(&mut client, ctx, &installation, &id, &mut current, command).await {
                    Ok(()) => next = tokio::time::Instant::now() + AFTER_ORDER,
                    Err(Stop::Park(reason)) => return park(ctx, reason).await,
                    Err(Stop::Fail(e)) => return Err(e),
                }
            }
            () = tokio::time::sleep_until(next) => {
                match read(&mut client, ctx, &installation).await {
                    Ok(code) => {
                        failures = 0;
                        publish(ctx, &id, &code);
                        ctx.set_availability(&id, true);
                        current = code;
                        next = tokio::time::Instant::now() + poll;
                    }
                    Err(Stop::Park(reason)) => return park(ctx, reason).await,
                    Err(Stop::Fail(e)) => {
                        failures += 1;
                        let wait = backoff_for(&e);
                        tracing::warn!(instance = %ctx.instance(), error = %format!("{e:#}"), wait_s = wait.as_secs(), "verisure reading failed");
                        if failures >= 3 {
                            ctx.set_availability(&id, false);
                        }
                        next = tokio::time::Instant::now() + wait;
                    }
                }
            }
        }
    }
}

/// One reading of the panel, with a renewed session if the old one ended.
async fn read(
    client: &mut Client,
    ctx: &DriverCtx,
    installation: &Installation,
) -> Result<String, Stop> {
    session(client, ctx).await?;
    match client.check_alarm(installation).await {
        Err(ApiError::SessionExpired) => {
            authenticate(client, ctx).await?;
            client.check_alarm(installation).await.map_err(passing)
        }
        other => other.map_err(passing),
    }
}

/// One order. The answer to the hub goes as soon as Verisure has taken the
/// order (the hub waits a few seconds); the panel's verdict follows, and
/// shows in `last_order` and `state`. A person's problem (a lock, a refused
/// password) comes back as `Stop::Park`; anything passing is told to the
/// caller of the order and the driver goes on.
async fn order(
    client: &mut Client,
    ctx: &DriverCtx,
    installation: &Installation,
    id: &moli_core::DeviceId,
    current: &mut String,
    command: CommandRequest,
) -> Result<(), Stop> {
    let mode = match &command.value {
        Value::Text(mode) => mode.to_string(),
        _ => String::new(),
    };
    if &*command.key != "mode" {
        command.reply(Err(moli_i18n::tr!("pilotes.verisure.point_non_ecrivable")));
        return Ok(());
    }
    let Some(verb) = model::command_for(&mode, current) else {
        command.reply(Err(moli_i18n::tr!("pilotes.verisure.mode_inconnu", mode = mode)));
        return Ok(());
    };
    // A warm session: the hub's answer cannot wait for a login.
    if let Err(stop) = session(client, ctx).await {
        return match stop {
            Stop::Park(reason) => {
                command.reply(Err(reason.clone()));
                Err(Stop::Park(reason))
            }
            Stop::Fail(e) => {
                command.reply(Err(format!("{e:#}")));
                Ok(())
            }
        };
    }
    let reference = match client.begin_command(installation, verb, current).await {
        Ok(reference) => reference,
        Err(e) => {
            // Said where a person looks (the card), not only to the caller.
            let said = moli_i18n::tr!("pilotes.verisure.ordre_refuse", why = e.to_string());
            ctx.set_state(id, "last_order", Value::Text(said.clone().into()));
            command.reply(Err(said));
            return Ok(());
        }
    };
    tracing::info!(instance = %ctx.instance(), order = verb, "verisure order taken");
    ctx.set_state(id, "last_order", Value::Text(moli_i18n::tr!("pilotes.verisure.ordre_envoye").into()));
    command.reply(Ok(()));

    let text = match client.finish_command(installation, verb, current, &reference).await {
        Ok(result) => {
            if let Some(code) = result.code {
                publish(ctx, id, &code);
                *current = code;
            }
            match result.refusal {
                Some(why) => moli_i18n::tr!("pilotes.verisure.ordre_refuse", why = why),
                None => moli_i18n::tr!("pilotes.verisure.ordre_execute"),
            }
        }
        Err(e) => moli_i18n::tr!("pilotes.verisure.ordre_incertain", why = e.to_string()),
    };
    ctx.set_state(id, "last_order", Value::Text(text.into()));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options_are_checked() {
        let bad: Result<Config, _> = serde_json::from_str(r#"{"password":"nope"}"#);
        assert!(bad.is_err(), "a password never goes in moli.toml");
        let good: Config = serde_json::from_str("{}").unwrap();
        assert_eq!(good.country, "FR");
        assert_eq!(good.poll_secs, 120);
        assert!(good.installation.is_none());
    }
}
