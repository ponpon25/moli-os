//! Olvid: private messages to the household, through the official Olvid bot
//! daemon (a container Moli talks to over gRPC). End-to-end encrypted, no
//! phone number, no third-party server holding a token.
//!
//! One device, « Olvid », whose `notify` point sends its text to every paired
//! person: a command like any other, so it is journaled, guarded and usable by
//! automations (their written-message channel) and by Moli itself.
//!
//! - **Setup**: the daemon's administrator key (a secret named `admin_key`,
//!   entered with `moli-os secrets set`) lets Moli create its own bot
//!   identity and a client key for it, once. Both are kept in the vault; the
//!   administrator key is never used again.
//! - **Pairing** is opened by a person (the `pairing` point, which asks for
//!   the household code), for 15 minutes: Moli shows the bot's link, accepts
//!   the invitation that comes from it, then both sides compare a short code
//!   (the bot's is shown by Moli, the person's is typed in the `sas` point).
//!   Outside that window no invitation is accepted.

mod daemon;
mod grpc;
mod proto;

use std::time::{Duration, Instant};

use anyhow::Context as _;
use moli_core::{Access, Device, DeviceId, Kind, PointSpec, Semantic, Value};
use moli_runtime::{BoxFuture, Driver, DriverCtx};
use serde::{Deserialize, Serialize};

use crate::daemon::Invitation;
use crate::grpc::Daemon;

/// Olvid messages are cut at this many characters.
const CHUNK: usize = 2000;
const MAX_PEOPLE: usize = 8;
const PAIRING_WINDOW: Duration = Duration::from_secs(15 * 60);
const WATCH: Duration = Duration::from_secs(3);
const PING_EVERY: Duration = Duration::from_secs(60);

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// The daemon, `host:port` (`127.0.0.1:50051`).
    #[serde(default = "default_daemon")]
    pub daemon: String,
    /// The first name of the bot, as people see it in their Olvid.
    #[serde(default = "default_bot")]
    pub bot_name: String,
}

fn default_daemon() -> String {
    "127.0.0.1:50051".into()
}

fn default_bot() -> String {
    "Moli".into()
}

#[derive(Debug)]
pub struct Olvid {
    config: Config,
}

impl Olvid {
    pub fn new(config: Config) -> anyhow::Result<Self> {
        anyhow::ensure!(
            config.daemon.contains(':') && !config.daemon.contains('/'),
            "daemon: expected host:port"
        );
        anyhow::ensure!(
            !config.bot_name.trim().is_empty() && config.bot_name.chars().count() <= 40,
            "bot_name: 1 to 40 characters"
        );
        Ok(Self { config })
    }
}

impl Driver for Olvid {
    fn kind(&self) -> &'static str {
        "olvid"
    }

    fn run<'a>(&'a self, ctx: &'a mut DriverCtx) -> BoxFuture<'a, anyhow::Result<()>> {
        Box::pin(run(&self.config, ctx))
    }
}

/// Someone paired: their contact and discussion in the daemon.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Person {
    contact: u64,
    discussion: u64,
    name: String,
}

const SECRET_ADMIN: &str = "admin_key";
const SECRET_KEY: &str = "client_key";
const SECRET_IDENTITY: &str = "identity";
const SECRET_PEOPLE: &str = "people";

fn load_people(ctx: &DriverCtx) -> Vec<Person> {
    ctx.secret(SECRET_PEOPLE)
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

async fn save_people(ctx: &DriverCtx, people: &[Person]) -> anyhow::Result<()> {
    let raw = serde_json::to_string(people)?;
    ctx.store_secret(SECRET_PEOPLE, &raw)
        .await
        .context("keeping the paired people")
}

/// The bot's identity and its client key, once (administrator's key).
async fn setup(ctx: &DriverCtx, admin: &Daemon, bot_name: &str) -> anyhow::Result<String> {
    // The identity first and on its own: a failure after it must not make a
    // second bot on the next try.
    let identity = if let Some(id) = ctx.secret(SECRET_IDENTITY).and_then(|s| s.parse().ok()) {
        id
    } else {
        let id = daemon::identity_new(admin, bot_name).await?;
        ctx.store_secret(SECRET_IDENTITY, &id.to_string()).await?;
        id
    };
    let key = daemon::client_key_new(admin, "moli-os", identity).await?;
    ctx.store_secret(SECRET_KEY, &key).await?;
    tracing::info!(instance = %ctx.instance(), identity, "olvid: bot identity created");
    Ok(key)
}

fn denied(error: &anyhow::Error) -> bool {
    let text = error.to_string();
    text.contains("unauthenticated") || text.contains("permission denied")
}

/// What the dashboard reads.
#[derive(Debug, Default, PartialEq, Eq)]
struct View {
    status: String,
    link: String,
    invitation: String,
    code: String,
    contacts: String,
}

fn status_text(pairing: bool, invitation: Option<&Invitation>, people: &[Person]) -> String {
    if let Some(inv) = invitation.filter(|_| pairing) {
        return moli_i18n::tr!("pilotes.olvid.statut_code", name = inv.name);
    }
    if pairing {
        return moli_i18n::tr!("pilotes.olvid.statut_appairage");
    }
    if people.is_empty() {
        return moli_i18n::tr!("pilotes.olvid.statut_vide");
    }
    moli_i18n::tr!("pilotes.olvid.statut_relie", names = names(people))
}

fn names(people: &[Person]) -> String {
    people
        .iter()
        .map(|p| p.name.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

fn publish(ctx: &DriverCtx, id: &DeviceId, view: &View) {
    let text = |key: &str, value: &str| ctx.set_state(id, key, Value::Text(value.into()));
    text("status", &view.status);
    text("link", &view.link);
    text("invitation", &view.invitation);
    text("code", &view.code);
    text("contacts", &view.contacts);
}

fn point(
    key: &str,
    label: String,
    kind: Kind,
    read: bool,
    write: bool,
    semantic: Semantic,
) -> PointSpec {
    PointSpec {
        key: key.into(),
        label: label.into(),
        kind,
        access: Access { read, write },
        unit: None,
        semantic,
    }
}

fn device(ctx: &DriverCtx, id: &DeviceId, bot: &str) -> Device {
    Device {
        id: id.clone(),
        instance: ctx.instance().clone(),
        native_name: "Olvid".into(),
        manufacturer: Some("Olvid".into()),
        model: Some(bot.into()),
        description: Some(moli_i18n::tr!("pilotes.olvid.description").into()),
        native_room: None,
        members: Vec::new(),
        points: vec![
            point(
                "notify",
                moli_i18n::tr!("pilotes.olvid.envoyer"),
                Kind::Text,
                false,
                true,
                Semantic::Other,
            ),
            point(
                "status",
                moli_i18n::tr!("pilotes.olvid.etat"),
                Kind::Text,
                true,
                false,
                Semantic::Other,
            ),
            point(
                "pairing",
                moli_i18n::tr!("pilotes.olvid.appairer"),
                Kind::Binary,
                true,
                true,
                Semantic::Control,
            ),
            point(
                "link",
                moli_i18n::tr!("pilotes.olvid.lien"),
                Kind::Text,
                true,
                false,
                Semantic::Other,
            ),
            point(
                "invitation",
                moli_i18n::tr!("pilotes.olvid.invitation"),
                Kind::Text,
                true,
                false,
                Semantic::Other,
            ),
            point(
                "code",
                moli_i18n::tr!("pilotes.olvid.code_bot"),
                Kind::Text,
                true,
                false,
                Semantic::Other,
            ),
            point(
                "sas",
                moli_i18n::tr!("pilotes.olvid.code_saisi"),
                Kind::Text,
                false,
                true,
                Semantic::Control,
            ),
            point(
                "contacts",
                moli_i18n::tr!("pilotes.olvid.personnes"),
                Kind::Text,
                true,
                false,
                Semantic::Other,
            ),
            point(
                "forget",
                moli_i18n::tr!("pilotes.olvid.oublier"),
                Kind::Text,
                false,
                true,
                Semantic::Control,
            ),
        ],
    }
}

/// A text cut into pieces Olvid takes, on line breaks when it can.
fn pieces(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    for line in text.trim().lines() {
        let mut line = line.to_owned();
        while line.chars().count() > CHUNK {
            let head: String = line.chars().take(CHUNK).collect();
            line = line.chars().skip(CHUNK).collect();
            if !current.is_empty() {
                out.push(std::mem::take(&mut current));
            }
            out.push(head);
        }
        if current.chars().count() + line.chars().count() + 1 > CHUNK && !current.is_empty() {
            out.push(std::mem::take(&mut current));
        }
        if !current.is_empty() {
            current.push('\n');
        }
        current.push_str(&line);
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

async fn notify(daemon: &Daemon, people: &[Person], text: &str) -> Result<(), String> {
    if people.is_empty() {
        return Err(moli_i18n::tr!("pilotes.olvid.personne"));
    }
    let mut failures = Vec::new();
    for person in people {
        for piece in pieces(text) {
            if let Err(e) = daemon::send(daemon, person.discussion, &piece).await {
                failures.push(format!("{} : {e:#}", person.name));
                break;
            }
        }
    }
    if failures.len() == people.len() {
        return Err(failures.join(" ; "));
    }
    if !failures.is_empty() {
        tracing::warn!(failures = ?failures, "olvid: some people were not reached");
    }
    Ok(())
}

/// Pairing in progress: the link, the invitation being exchanged.
#[derive(Debug, Default)]
struct Pairing {
    until: Option<Instant>,
    link: String,
    current: Option<Invitation>,
}

impl Pairing {
    fn open(&self) -> bool {
        self.until.is_some_and(|t| Instant::now() < t)
    }
}

/// The bot's client key: the one in the vault, else made now with the
/// administrator's. `None` when a person has something to do first (said on
/// every surface through `wait_for`).
async fn client_key(config: &Config, ctx: &DriverCtx) -> anyhow::Result<Option<String>> {
    if let Some(key) = ctx.secret(SECRET_KEY) {
        return Ok(Some(key));
    }
    let Some(admin) = ctx.secret(SECRET_ADMIN) else {
        ctx.wait_for(moli_i18n::tr!("pilotes.olvid.pas_de_cle"));
        return Ok(None);
    };
    if !ctx.can_store_secrets() {
        ctx.wait_for(moli_i18n::tr!(
            "pilotes.olvid.coffre",
            why = ctx.secrets_problem().unwrap_or("?")
        ));
        return Ok(None);
    }
    let admin = Daemon::new(&config.daemon, &admin);
    match setup(ctx, &admin, &config.bot_name).await {
        Ok(key) => Ok(Some(key)),
        Err(e) if denied(&e) => {
            ctx.wait_for(moli_i18n::tr!(
                "pilotes.olvid.cle_refusee",
                error = format!("{e:#}")
            ));
            Ok(None)
        }
        Err(e) => Err(e),
    }
}

async fn run(config: &Config, ctx: &mut DriverCtx) -> anyhow::Result<()> {
    let Some(key) = client_key(config, ctx).await? else {
        ctx.cancelled().await;
        return Ok(());
    };
    let daemon = Daemon::new(&config.daemon, &key);
    if let Err(e) = daemon::ping(&daemon).await {
        if denied(&e) {
            ctx.wait_for(moli_i18n::tr!(
                "pilotes.olvid.cle_refusee",
                error = format!("{e:#}")
            ));
            ctx.cancelled().await;
            return Ok(());
        }
        return Err(e.context(moli_i18n::tr!(
            "pilotes.olvid.injoignable",
            daemon = config.daemon
        )));
    }

    let id = ctx.device_id("bot");
    ctx.upsert_device(device(ctx, &id, &config.bot_name));
    ctx.set_availability(&id, true);
    ctx.set_state(&id, "pairing", Value::Bool(false));
    let mut people = load_people(ctx);
    let mut pairing = Pairing::default();
    publish(ctx, &id, &view(&pairing, &people));
    ctx.ready();
    tracing::info!(instance = %ctx.instance(), people = people.len(), "olvid ready");

    let mut watch = tokio::time::interval(WATCH);
    let mut last_ping = Instant::now();
    loop {
        tokio::select! {
            command = ctx.next_command() => {
                let Some(command) = command else { return Ok(()) };
                let result = match (&*command.key, &command.value) {
                    ("notify", Value::Text(text)) if !text.trim().is_empty() => notify(&daemon, &people, text).await,
                    ("notify", Value::Text(_)) => Err(moli_i18n::tr!("pilotes.olvid.message_vide")),
                    ("pairing", Value::Bool(true)) => start_pairing(&daemon, &people, &mut pairing).await,
                    ("pairing", Value::Bool(false)) => { pairing = Pairing::default(); Ok(()) }
                    ("sas", Value::Text(code)) => enter_code(&daemon, &pairing, code).await,
                    ("forget", Value::Text(who)) => forget(ctx, &mut people, who).await,
                    ("notify" | "sas" | "forget", _) => Err(moli_i18n::tr!("pilotes.olvid.texte_attendu")),
                    ("pairing", _) => Err(moli_i18n::tr!("pilotes.olvid.oui_non")),
                    _ => Err(moli_i18n::tr!("pilotes.olvid.ne_s_ecrit_pas", point = command.key)),
                };
                command.reply(result);
                ctx.set_state(&id, "pairing", Value::Bool(pairing.open()));
                publish(ctx, &id, &view(&pairing, &people));
            }
            _ = watch.tick() => {
                if pairing.until.is_some() && !pairing.open() {
                    pairing = Pairing::default();
                }
                if pairing.open() {
                    if let Err(e) = follow(ctx, &daemon, &mut pairing, &mut people, &config.bot_name).await {
                        tracing::warn!(error = %format!("{e:#}"), "olvid: pairing");
                    }
                    last_ping = Instant::now();
                } else if last_ping.elapsed() >= PING_EVERY {
                    last_ping = Instant::now();
                    let up = daemon::ping(&daemon).await.is_ok();
                    ctx.set_availability(&id, up);
                }
                ctx.set_state(&id, "pairing", Value::Bool(pairing.open()));
                publish(ctx, &id, &view(&pairing, &people));
            }
        }
    }
}

fn view(pairing: &Pairing, people: &[Person]) -> View {
    let open = pairing.open();
    let inv = pairing.current.as_ref();
    View {
        status: status_text(open, inv, people),
        link: if open {
            pairing.link.clone()
        } else {
            String::new()
        },
        invitation: inv
            .filter(|_| open)
            .map(|i| i.name.clone())
            .unwrap_or_default(),
        code: inv
            .filter(|_| open)
            .map(|i| i.sas.clone())
            .unwrap_or_default(),
        contacts: names(people),
    }
}

async fn start_pairing(
    daemon: &Daemon,
    people: &[Person],
    pairing: &mut Pairing,
) -> Result<(), String> {
    if people.len() >= MAX_PEOPLE {
        return Err(moli_i18n::tr!(
            "pilotes.olvid.trop_de_monde",
            max = MAX_PEOPLE
        ));
    }
    let link = daemon::invitation_link(daemon)
        .await
        .map_err(|e| format!("{e:#}"))?;
    *pairing = Pairing {
        until: Some(Instant::now() + PAIRING_WINDOW),
        link,
        current: None,
    };
    Ok(())
}

async fn enter_code(daemon: &Daemon, pairing: &Pairing, code: &str) -> Result<(), String> {
    let code: String = code.chars().filter(char::is_ascii_digit).collect();
    if code.is_empty() {
        return Err(moli_i18n::tr!("pilotes.olvid.code_vide"));
    }
    let Some(inv) = pairing
        .current
        .as_ref()
        .filter(|i| pairing.open() && i.status == daemon::WAIT_YOU_FOR_SAS)
    else {
        return Err(moli_i18n::tr!("pilotes.olvid.pas_d_invitation"));
    };
    daemon::invitation_sas(daemon, inv.id, &code)
        .await
        .map_err(|e| format!("{e:#}"))
}

async fn forget(ctx: &DriverCtx, people: &mut Vec<Person>, who: &str) -> Result<(), String> {
    let who = who.trim().to_lowercase();
    let before = people.len();
    if who == "*" {
        people.clear();
    } else {
        people.retain(|p| p.name.to_lowercase() != who);
    }
    if people.len() == before {
        return Err(moli_i18n::tr!("pilotes.olvid.inconnu", name = who));
    }
    save_people(ctx, people).await.map_err(|e| format!("{e:#}"))
}

/// One look at the invitations while pairing is open: the daemon's side of
/// the dance, then the new person when it is done.
async fn follow(
    ctx: &DriverCtx,
    daemon: &Daemon,
    pairing: &mut Pairing,
    people: &mut Vec<Person>,
    bot: &str,
) -> anyhow::Result<()> {
    let mut current = None;
    for inv in daemon::invitations(daemon).await? {
        match inv.status {
            daemon::WAIT_YOU_TO_ACCEPT => {
                tracing::info!(name = %inv.name, "olvid: invitation accepted");
                daemon::invitation_accept(daemon, inv.id).await?;
            }
            daemon::WAIT_YOU_FOR_SAS | daemon::WAIT_IT_FOR_SAS => current = Some(inv),
            _ => {}
        }
    }
    pairing.current = current;
    for contact in daemon::contacts(daemon).await? {
        if people.iter().any(|p| p.contact == contact.id) || people.len() >= MAX_PEOPLE {
            continue;
        }
        let discussion = daemon::discussion_with(daemon, contact.id).await?;
        people.push(Person {
            contact: contact.id,
            discussion,
            name: contact.name.clone(),
        });
        save_people(ctx, people).await?;
        tracing::info!(name = %contact.name, "olvid: person paired");
        let hello = moli_i18n::tr!("pilotes.olvid.bienvenue", name = contact.name, bot = bot);
        let _ = daemon::send(daemon, discussion, &hello).await;
        // Paired: the window closes by itself.
        *pairing = Pairing::default();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn person(name: &str) -> Person {
        Person {
            contact: 1,
            discussion: 2,
            name: name.into(),
        }
    }

    #[test]
    fn short_texts_stay_whole_and_long_ones_are_cut_on_lines() {
        assert_eq!(pieces("  bonjour\nà tous "), vec!["bonjour\nà tous"]);
        let long = format!("{}\n{}", "a".repeat(1500), "b".repeat(1500));
        let cut = pieces(&long);
        assert_eq!(cut.len(), 2);
        assert!(cut.iter().all(|p| p.chars().count() <= CHUNK));
        let one_line = "é".repeat(4500);
        let cut = pieces(&one_line);
        assert_eq!(cut.len(), 3);
        assert_eq!(cut.concat(), one_line);
        assert!(pieces("   ").is_empty());
    }

    #[test]
    fn the_status_follows_the_pairing() {
        moli_i18n::set_language("fr");
        let people = vec![person("Lucien"), person("Annabelle")];
        assert!(status_text(false, None, &[]).contains("Aucun"));
        assert!(status_text(false, None, &people).contains("Lucien, Annabelle"));
        assert!(
            status_text(true, None, &people)
                .to_lowercase()
                .contains("appairage")
        );
        let inv = Invitation {
            id: 1,
            status: daemon::WAIT_YOU_FOR_SAS,
            name: "Lucien".into(),
            sas: "1234".into(),
        };
        assert!(status_text(true, Some(&inv), &people).contains("Lucien"));
        // A closed window shows nothing of an old invitation.
        assert!(!status_text(false, Some(&inv), &people).contains("code"));
    }

    #[test]
    fn a_view_of_a_closed_pairing_shows_no_link_or_code() {
        let closed = Pairing {
            until: None,
            link: "https://invitation.olvid.io/#x".into(),
            current: Some(Invitation {
                id: 1,
                status: daemon::WAIT_YOU_FOR_SAS,
                name: "Lucien".into(),
                sas: "1234".into(),
            }),
        };
        let v = view(&closed, &[]);
        assert!(v.link.is_empty() && v.code.is_empty() && v.invitation.is_empty());
        let open = Pairing {
            until: Some(Instant::now() + Duration::from_secs(60)),
            ..closed
        };
        let v = view(&open, &[]);
        assert_eq!(v.code, "1234");
        assert_eq!(v.invitation, "Lucien");
        assert!(v.link.starts_with("https://"));
    }

    #[test]
    fn a_daemon_address_is_host_and_port() {
        let ok = Config {
            daemon: "127.0.0.1:50051".into(),
            bot_name: "Moli".into(),
        };
        assert!(Olvid::new(ok).is_ok());
        let bad = Config {
            daemon: "http://x:1".into(),
            bot_name: "Moli".into(),
        };
        assert!(Olvid::new(bad).is_err());
        let nameless = Config {
            daemon: "x:1".into(),
            bot_name: " ".into(),
        };
        assert!(Olvid::new(nameless).is_err());
    }

    #[test]
    fn a_refused_key_is_told_from_a_down_daemon() {
        assert!(denied(&anyhow::anyhow!("gRPC status 16 (unauthenticated)")));
        assert!(!denied(&anyhow::anyhow!("connecting to x: refused")));
    }

    /// Against a real daemon (`OLVID_PROBE=host:port cargo test -- --ignored`):
    /// a wrong key must come back as a gRPC refusal, not as a transport error.
    #[tokio::test]
    #[ignore = "needs a running Olvid daemon"]
    async fn a_real_daemon_refuses_a_wrong_key() {
        let address = std::env::var("OLVID_PROBE").expect("OLVID_PROBE=host:port");
        let daemon = Daemon::new(&address, "not-a-real-key");
        let error = daemon::ping(&daemon).await.unwrap_err();
        assert!(denied(&error), "unexpected: {error:#}");
    }
}
