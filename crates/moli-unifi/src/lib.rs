//! UniFi Protect cameras, through the console's Integration API
//! (`/proxy/protect/integration/v1`, an API key in `X-API-KEY`): the cameras
//! the console knows, their latest image on demand, and motion from its
//! event websocket. Read-only: nothing here changes a camera.
//!
//! The console's certificate is self-signed: it is pinned at first contact,
//! before the key is sent. The key lives in the encrypted secret store
//! (`moli-os secrets set <instance> api_key`), never in `moli.toml`.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context as _, bail};
use moli_core::{Access, Device, Kind, PointSpec, Semantic, Value};
use moli_net::ws::WebSocket;
use moli_net::{Method, PIN_MISMATCH, PinnedHttps, PinnedTls};
use moli_runtime::media::{Image, SnapshotSource};
use moli_runtime::{BoxFuture, Driver, DriverCtx};
use serde::Deserialize;
use serde_json::Value as Json;

const BASE: &str = "/proxy/protect/integration/v1";
const CERT: &str = "cert_sha256";
const API_KEY: &str = "api_key";
/// The camera list is read again this often (new cameras, connection state).
const POLL: Duration = Duration::from_secs(60);
/// A camera that never reported the end of its motion is still after this.
const MOTION_HOLD: Duration = Duration::from_secs(120);
/// A camera waking up for an image may take a while.
const IMAGE_BUDGET: Duration = Duration::from_secs(15);
/// No message for this long: reopen the websocket.
const STREAM_IDLE: Duration = Duration::from_secs(3600);

/// Driver configuration (`[driver.options]` in `moli.toml`).
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// The console's IP or host name.
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
    /// Optional: take the API key from this environment variable instead of
    /// the secret store.
    #[serde(default)]
    pub api_key_env: Option<String>,
    /// Optional: the console certificate's SHA-256 (hex), when it cannot be
    /// learned and stored at first contact (no secret store).
    #[serde(default)]
    pub cert_sha256: Option<String>,
}

fn default_port() -> u16 {
    443
}

#[derive(Debug)]
pub struct Unifi {
    config: Config,
}

impl Unifi {
    #[must_use]
    pub fn new(config: Config) -> Self {
        Self { config }
    }
}

impl Driver for Unifi {
    fn kind(&self) -> &'static str {
        "unifi"
    }

    fn run<'a>(&'a self, ctx: &'a mut DriverCtx) -> BoxFuture<'a, anyhow::Result<()>> {
        Box::pin(run(&self.config, ctx))
    }
}

/// One console and its API key.
struct Console {
    http: PinnedHttps,
    key: String,
}

impl std::fmt::Debug for Console {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never the key: it reads every camera of the house.
        f.debug_struct("Console")
            .field("http", &self.http)
            .finish_non_exhaustive()
    }
}

/// The console refused the API key.
#[derive(Debug)]
struct KeyRejected;

impl std::fmt::Display for KeyRejected {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("unifi API key rejected")
    }
}

impl std::error::Error for KeyRejected {}

impl Console {
    fn new(host: &str, port: u16, pin: String, key: String) -> anyhow::Result<Self> {
        Ok(Self {
            http: PinnedHttps::new(host, port, Some(pin))?,
            key,
        })
    }

    /// A bare handshake, nothing sent: the fingerprint of the certificate
    /// the console presents, to store before any key leaves.
    async fn first_contact(host: &str, port: u16) -> anyhow::Result<String> {
        let probe = PinnedHttps::new(host, port, None)?;
        probe.handshake().await.context("first contact")?;
        probe.pinned_fingerprint().context("no certificate seen")
    }

    fn request(&self, path: &str) -> anyhow::Result<http::Request<http_body_util::Full<moli_net::Body>>> {
        moli_net::empty(
            self.http
                .request(Method::GET, &format!("{BASE}{path}"))
                .header("x-api-key", &self.key),
        )
    }

    /// A `GET` answered with JSON.
    async fn json(&self, path: &str) -> anyhow::Result<Vec<u8>> {
        let (status, body) = self.http.send(self.request(path)?).await?;
        if status == http::StatusCode::UNAUTHORIZED || status == http::StatusCode::FORBIDDEN {
            return Err(anyhow::Error::new(KeyRejected));
        }
        if !status.is_success() {
            bail!("unifi {path}: {status}");
        }
        Ok(body.to_vec())
    }

    /// A `GET` that may take its time (an image), on its own connection.
    async fn slow(&self, path: &str) -> anyhow::Result<(http::StatusCode, moli_net::Body)> {
        self.http.send_slow(self.request(path)?, IMAGE_BUDGET).await
    }
}

/// A camera, as the console lists it.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
struct Camera {
    id: String,
    #[serde(default)]
    name: String,
    /// The model (« UVC G3 Pro », « C1 »…).
    #[serde(default, rename = "type")]
    model: String,
    #[serde(default)]
    state: String,
}

impl Camera {
    fn online(&self) -> bool {
        self.state == "CONNECTED"
    }

    /// Ids go into URL paths: only what the console really uses.
    fn sane(&self) -> bool {
        !self.id.is_empty() && self.id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
    }
}

fn parse_cameras(body: &[u8]) -> anyhow::Result<Vec<Camera>> {
    let all: Vec<Camera> = serde_json::from_slice(body).context("unifi camera list")?;
    Ok(all.into_iter().filter(Camera::sane).collect())
}

fn camera_device(ctx: &DriverCtx, camera: &Camera) -> Device {
    let name = if camera.name.is_empty() {
        camera.id.clone()
    } else {
        camera.name.clone()
    };
    Device {
        id: ctx.device_id(&camera.id),
        instance: ctx.instance().clone(),
        native_name: name.into(),
        manufacturer: Some("Ubiquiti".into()),
        model: Some(if camera.model.is_empty() {
            moli_i18n::tr!("pilotes.unifi.camera")
        } else {
            camera.model.clone()
        }
        .into()),
        description: None,
        native_room: None,
        members: Vec::new(),
        points: vec![PointSpec {
            key: "motion".into(),
            label: moli_i18n::tr!("pilotes.unifi.mouvement").into(),
            semantic: Semantic::infer("motion", None),
            kind: Kind::Binary,
            access: Access {
                read: true,
                write: false,
            },
            unit: None,
        }],
    }
}

/// The latest image of one camera.
#[derive(Debug)]
struct Snapshots {
    console: Arc<Console>,
    camera: String,
}

impl SnapshotSource for Snapshots {
    fn snapshot(&self) -> BoxFuture<'_, anyhow::Result<Image>> {
        Box::pin(async move {
            let (status, bytes) = self
                .console
                .slow(&format!("/cameras/{}/snapshot", self.camera))
                .await?;
            if !status.is_success() || !bytes.starts_with(&[0xFF, 0xD8]) {
                bail!(moli_i18n::tr!("pilotes.unifi.aucune_image", status = status));
            }
            Ok(Image {
                content_type: "image/jpeg".into(),
                bytes: bytes.to_vec(),
            })
        })
    }
}

/// What an event message says about a camera's motion.
#[derive(Debug, PartialEq, Eq)]
enum Motion {
    Start(String),
    End(String),
}

/// Reads one websocket message: `{"type":"add|update|remove","item":{…}}`,
/// the item being an event (`modelKey` « event ») with its `type`, the
/// camera in `device`, and an `end` once it is over. Anything else is not
/// about motion.
fn parse_event(text: &str) -> Option<Motion> {
    let message: Json = serde_json::from_str(text).ok()?;
    let item = message.get("item").unwrap_or(&message);
    if item["modelKey"].as_str().is_some_and(|k| k != "event") {
        return None;
    }
    if !matches!(
        item["type"].as_str()?,
        "motion" | "smartDetectZone" | "smartDetectLine"
    ) {
        return None;
    }
    let camera = ["device", "deviceId", "camera"]
        .iter()
        .find_map(|k| item[*k].as_str())?
        .to_owned();
    let over = item["end"].as_u64().is_some_and(|t| t > 0) || message["type"] == "remove";
    Some(if over {
        Motion::End(camera)
    } else {
        Motion::Start(camera)
    })
}

/// Shows `reason` on every surface and idles until shutdown.
async fn park(ctx: &DriverCtx, reason: String) -> anyhow::Result<()> {
    ctx.wait_for(reason);
    ctx.cancelled().await;
    Ok(())
}

fn secrets_problem(ctx: &DriverCtx) -> String {
    ctx.secrets_problem().map_or_else(
        || moli_i18n::tr!("pilotes.unifi.coffre_indisponible"),
        str::to_owned,
    )
}

/// What the driver knows of the cameras it published.
#[derive(Default)]
struct Cameras {
    online: HashMap<String, bool>,
}

/// Reads the camera list and brings the hub in line with it.
async fn refresh(
    console: &Arc<Console>,
    ctx: &DriverCtx,
    known: &mut Cameras,
) -> anyhow::Result<()> {
    let cameras = parse_cameras(&console.json("/cameras").await?)?;
    for id in ctx.devices() {
        if !cameras.iter().any(|c| ctx.device_id(&c.id) == id) {
            ctx.remove_device(&id);
        }
    }
    known
        .online
        .retain(|id, _| cameras.iter().any(|c| &c.id == id));
    for camera in &cameras {
        let device = ctx.device_id(&camera.id);
        if !known.online.contains_key(&camera.id) {
            ctx.upsert_device(camera_device(ctx, camera));
            ctx.provide_snapshots(
                &device,
                Arc::new(Snapshots {
                    console: Arc::clone(console),
                    camera: camera.id.clone(),
                }),
            );
            ctx.set_state(&device, "motion", Value::Bool(false));
        }
        if known.online.get(&camera.id) != Some(&camera.online()) {
            ctx.set_availability(&device, camera.online());
        }
        known.online.insert(camera.id.clone(), camera.online());
    }
    Ok(())
}

async fn run(config: &Config, ctx: &mut DriverCtx) -> anyhow::Result<()> {
    let pin = match config.cert_sha256.clone().or_else(|| ctx.secret(CERT)) {
        Some(pin) => pin,
        None if !ctx.can_store_secrets() => {
            let reason = moli_i18n::tr!(
                "pilotes.unifi.epinglage_impossible",
                problem = secrets_problem(ctx)
            );
            return park(ctx, reason).await;
        }
        // Pinned and stored before the key leaves.
        None => {
            let seen = Console::first_contact(&config.host, config.port)
                .await
                .context("unifi")?;
            ctx.store_secret(CERT, &seen).await?;
            tracing::info!(instance = %ctx.instance(), "unifi console certificate pinned");
            seen
        }
    };

    let key = config
        .api_key_env
        .as_ref()
        .and_then(|var| std::env::var(var).ok())
        .or_else(|| ctx.secret(API_KEY));
    let Some(key) = key else {
        let reason = moli_i18n::tr!("pilotes.unifi.cle_manquante", instance = ctx.instance());
        return park(ctx, reason).await;
    };

    let console = Arc::new(Console::new(&config.host, config.port, pin.clone(), key.clone())?);
    let tls = PinnedTls::new(Some(pin))?;
    let mut known = Cameras::default();
    match refresh(&console, ctx, &mut known).await {
        Ok(()) => {}
        Err(e) if e.is::<KeyRejected>() => {
            let reason = moli_i18n::tr!(
                "pilotes.unifi.cle_refusee",
                host = config.host,
                instance = ctx.instance()
            );
            return park(ctx, reason).await;
        }
        Err(e) if format!("{e:#}").contains(PIN_MISMATCH) => {
            let reason = moli_i18n::tr!(
                "pilotes.unifi.certificat_change",
                host = config.host,
                instance = ctx.instance()
            );
            return park(ctx, reason).await;
        }
        Err(e) => return Err(e),
    }
    tracing::info!(instance = %ctx.instance(), cameras = known.online.len(), "unifi console loaded");
    ctx.ready();

    let events_path = format!("{BASE}/subscribe/events");
    let mut socket: Option<WebSocket> = None;
    // Motion in progress, and when to call it over if nobody says so.
    let mut moving: HashMap<String, tokio::time::Instant> = HashMap::new();
    let mut poll = tokio::time::interval(POLL);
    let mut hold = tokio::time::interval(Duration::from_secs(10));
    loop {
        if socket.is_none() {
            match WebSocket::connect(
                &config.host,
                config.port,
                &events_path,
                &[("X-API-KEY", key.as_str())],
                &tls,
            )
            .await
            {
                Ok(s) => {
                    tracing::info!(instance = %ctx.instance(), "unifi event stream connected");
                    socket = Some(s);
                }
                Err(e) => {
                    tracing::warn!(instance = %ctx.instance(), error = %format!("{e:#}"), "unifi event stream unavailable, retrying");
                }
            }
        }
        tokio::select! {
            command = ctx.next_command() => match command {
                Some(command) => command.reply(Err(moli_i18n::tr!("pilotes.unifi.lecture_seule"))),
                None => return Ok(()),
            },
            message = async {
                match socket.as_mut() {
                    Some(s) => s.recv_text(STREAM_IDLE).await,
                    None => std::future::pending().await,
                }
            } => match message {
                Ok(text) => {
                    tracing::debug!(instance = %ctx.instance(), message = %text.chars().take(300).collect::<String>(), "unifi event");
                    match parse_event(&text) {
                        Some(Motion::Start(camera)) => {
                            moving.insert(camera.clone(), tokio::time::Instant::now() + MOTION_HOLD);
                            ctx.set_state(&ctx.device_id(&camera), "motion", Value::Bool(true));
                        }
                        Some(Motion::End(camera)) => {
                            moving.remove(&camera);
                            ctx.set_state(&ctx.device_id(&camera), "motion", Value::Bool(false));
                        }
                        None => {}
                    }
                }
                Err(e) => {
                    tracing::warn!(instance = %ctx.instance(), error = %format!("{e:#}"), "unifi event stream dropped, reopening");
                    socket = None;
                }
            },
            _ = poll.tick() => {
                if let Err(e) = refresh(&console, ctx, &mut known).await {
                    if e.is::<KeyRejected>() {
                        let reason = moli_i18n::tr!("pilotes.unifi.cle_refusee", host = config.host, instance = ctx.instance());
                        return park(ctx, reason).await;
                    }
                    return Err(e);
                }
                if let Some(s) = socket.as_mut() {
                    if s.ping().await.is_err() {
                        socket = None;
                    }
                }
            }
            _ = hold.tick() => {
                let now = tokio::time::Instant::now();
                moving.retain(|camera, until| {
                    let keep = *until > now;
                    if !keep {
                        ctx.set_state(&ctx.device_id(camera), "motion", Value::Bool(false));
                    }
                    keep
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A real console's camera list (addresses made up).
    const CAMERAS: &str = include_str!("../../../integrations/unifi/fixtures/cameras.json");

    #[test]
    fn the_cameras_of_a_real_console_are_read() {
        let cameras = parse_cameras(CAMERAS.as_bytes()).unwrap();
        assert_eq!(cameras.len(), 3);
        assert_eq!(cameras[0].name, "Caméra extérieure");
        assert_eq!(cameras[0].model, "UVC G3 Pro");
        assert!(cameras.iter().all(Camera::online));
    }

    #[test]
    fn a_disconnected_camera_is_offline() {
        let body = r#"[{"id":"abc123","name":"Porche","type":"G4","state":"DISCONNECTED"}]"#;
        assert!(!parse_cameras(body.as_bytes()).unwrap()[0].online());
    }

    #[test]
    fn an_id_that_is_not_a_path_segment_is_dropped() {
        let body = r#"[{"id":"../x","name":"a","type":"","state":"CONNECTED"},{"id":"ok1","name":"b","type":"","state":"CONNECTED"}]"#;
        let cameras = parse_cameras(body.as_bytes()).unwrap();
        assert_eq!(cameras.len(), 1);
        assert_eq!(cameras[0].id, "ok1");
    }

    #[test]
    fn motion_starts_and_ends() {
        let start = r#"{"type":"add","item":{"id":"e1","modelKey":"event","type":"motion","start":1700000000000,"device":"cam1"}}"#;
        assert_eq!(parse_event(start), Some(Motion::Start("cam1".into())));
        let end = r#"{"type":"update","item":{"id":"e1","modelKey":"event","type":"motion","start":1700000000000,"end":1700000009000,"device":"cam1"}}"#;
        assert_eq!(parse_event(end), Some(Motion::End("cam1".into())));
        let gone = r#"{"type":"remove","item":{"id":"e1","modelKey":"event","type":"motion","device":"cam1"}}"#;
        assert_eq!(parse_event(gone), Some(Motion::End("cam1".into())));
    }

    #[test]
    fn other_messages_are_not_motion() {
        for text in [
            "not json",
            r#"{"type":"update","item":{"modelKey":"camera","id":"cam1","state":"CONNECTED"}}"#,
            r#"{"type":"add","item":{"modelKey":"event","type":"ring","device":"cam1"}}"#,
            r#"{"type":"add","item":{"modelKey":"event","type":"motion"}}"#,
        ] {
            assert_eq!(parse_event(text), None, "{text}");
        }
    }

    #[test]
    fn unknown_options_are_refused() {
        let bad: Result<Config, _> = toml_like(r#"{"host":"h","nope":1}"#);
        assert!(bad.is_err());
        let good = toml_like(r#"{"host":"h"}"#).unwrap();
        assert_eq!(good.port, 443);
    }

    fn toml_like(json: &str) -> Result<Config, serde_json::Error> {
        serde_json::from_str(json)
    }
}
