//! Z-Wave through Z-Wave JS: the server of Z-Wave JS UI, on this machine.
//!
//! Z-Wave JS UI drives the stick, includes and configures the nodes (its own
//! interface, linked from Moli as « Appairage et réglages »). Moli reads its
//! Z-Wave JS server (a websocket, `ws://127.0.0.1:3000`): the whole network
//! on `start_listening`, then every change as an event; an order is a
//! `node.set_value`. When a node joins, leaves or finishes its interview,
//! the state is read again (a few seconds after the last such event).

mod model;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context as _, bail};
use model::{DIM_FLOOR, Dim, Mapped, Node, ValueId};
use moli_core::{DeviceId, Value};
use moli_net::ws::WebSocket;
use moli_runtime::{BoxFuture, CommandRequest, Driver, DriverCtx};
use serde::Deserialize;
use serde_json::{Value as Json, json};

/// The API version Moli speaks (the server answers with its own range).
const SCHEMA: u64 = 35;

/// Driver configuration (`[driver.options]` in `moli.toml`).
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// The Z-Wave JS server: on this machine only (it speaks in clear).
    #[serde(default = "default_host")]
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
    /// Z-Wave JS UI's web interface (`http://host:8091`), linked from Moli
    /// for inclusion and device settings.
    #[serde(default)]
    pub frontend: Option<String>,
}

fn default_host() -> String {
    "127.0.0.1".into()
}

fn default_port() -> u16 {
    3000
}

#[derive(Debug)]
pub struct Zwave {
    config: Config,
}

impl Zwave {
    pub fn new(config: Config) -> anyhow::Result<Self> {
        let local = config.host == "localhost"
            || config
                .host
                .parse::<std::net::IpAddr>()
                .is_ok_and(|ip| ip.is_loopback());
        anyhow::ensure!(
            local,
            "host {:?}: the Z-Wave JS server speaks in clear, it runs on this machine (127.0.0.1)",
            config.host
        );
        Ok(Self { config })
    }
}

impl Driver for Zwave {
    fn kind(&self) -> &'static str {
        "zwave"
    }

    fn web(&self) -> Option<Arc<str>> {
        let url = self.config.frontend.as_deref()?.trim_end_matches('/');
        (url.starts_with("http://") || url.starts_with("https://")).then(|| Arc::from(url))
    }

    fn run<'a>(&'a self, ctx: &'a mut DriverCtx) -> BoxFuture<'a, anyhow::Result<()>> {
        Box::pin(run(&self.config, ctx))
    }
}

async fn run(config: &Config, ctx: &mut DriverCtx) -> anyhow::Result<()> {
    loop {
        let mut ws = WebSocket::connect_local(&config.host, config.port, "/")
            .await
            .with_context(|| {
                format!(
                    "Z-Wave JS server unreachable at {}:{} (Z-Wave JS UI, « WS Server » on)",
                    config.host, config.port
                )
            })?;
        match session(&mut ws, ctx).await? {
            Session::Stop => return Ok(()),
            Session::Again => {
                tracing::info!(instance = %ctx.instance(), "z-wave network changed, reading it again");
            }
        }
    }
}

enum Session {
    Stop,
    Again,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Version {
    #[serde(default, rename = "minSchemaVersion")]
    min: u64,
    #[serde(default = "schema", rename = "maxSchemaVersion")]
    max: u64,
    #[serde(default, rename = "driverVersion")]
    driver: Option<String>,
}

fn schema() -> u64 {
    SCHEMA
}

/// Sends a command and waits for its result (the network is quiet until
/// `start_listening`: nothing else comes in between).
async fn call(ws: &mut WebSocket, id: &str, mut command: Json) -> anyhow::Result<Json> {
    command["messageId"] = json!(id);
    ws.send_text(&command.to_string()).await?;
    loop {
        let message: Json = serde_json::from_str(&ws.recv_text(Duration::from_secs(60)).await?)?;
        if message["type"] == "result" && message["messageId"] == id {
            if message["success"] != true {
                bail!(
                    "{}: {} {}",
                    command["command"],
                    message["errorCode"].as_str().unwrap_or("refused"),
                    message["message"].as_str().unwrap_or_default()
                );
            }
            return Ok(message["result"].clone());
        }
    }
}

/// Moli's view of the network: each node, its device, its points.
#[derive(Default)]
struct Network {
    home: u32,
    nodes: HashMap<u16, (Node, Mapped)>,
}

impl Network {
    fn by_device(&self, id: &DeviceId) -> Option<&(Node, Mapped)> {
        self.nodes.values().find(|(_, m)| m.device.id == *id)
    }
}

async fn session(ws: &mut WebSocket, ctx: &mut DriverCtx) -> anyhow::Result<Session> {
    let hello: Json = serde_json::from_str(&ws.recv_text(Duration::from_secs(15)).await?)?;
    let version: Version = serde_json::from_value(hello).context("not a Z-Wave JS server")?;
    let schema = SCHEMA.clamp(version.min, version.max);
    call(
        ws,
        "schema",
        json!({"command": "set_api_schema", "schemaVersion": schema}),
    )
    .await?;
    let state = call(ws, "listen", json!({"command": "start_listening"})).await?;
    let state = &state["state"];
    let mut net = Network {
        home: state["controller"]["homeId"]
            .as_u64()
            .and_then(|h| u32::try_from(h).ok())
            .unwrap_or_default(),
        ..Network::default()
    };
    let nodes: Vec<Node> = serde_json::from_value(state["nodes"].clone()).context("nodes")?;
    for node in nodes {
        publish(ctx, &mut net, node);
    }
    // Nodes gone while Moli was away (excluded, or another stick).
    for id in ctx.known_devices() {
        if net.by_device(&id).is_none() {
            ctx.remove_device(&id);
        }
    }
    tracing::info!(
        instance = %ctx.instance(),
        driver = version.driver.as_deref().unwrap_or("?"),
        schema,
        nodes = net.nodes.len(),
        "z-wave network read"
    );
    ctx.ready();

    let mut pending: HashMap<String, CommandRequest> = HashMap::new();
    let mut sent = 0u64;
    let mut again: Option<tokio::time::Instant> = None;
    let mut revive = Revive::default();
    let mut glides = Glides::default();
    let mut ping = tokio::time::interval(Duration::from_secs(30));
    ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let outcome = loop {
        tokio::select! {
            message = ws.recv_text(Duration::from_secs(90)) => {
                let message: Json = match serde_json::from_str(&message?) {
                    Ok(m) => m,
                    Err(e) => {
                        tracing::debug!(error = %e, "z-wave message not understood");
                        continue;
                    }
                };
                match message["type"].as_str() {
                    Some("event") => {
                        revive.on_event(&message["event"]);
                        if on_event(ctx, &mut net, &message["event"]) {
                            again = Some(tokio::time::Instant::now() + Duration::from_secs(3));
                        }
                    }
                    Some("result") => {
                        if let Some(command) = message["messageId"].as_str().and_then(|id| pending.remove(id)) {
                            command.reply(outcome_of(&message));
                        }
                    }
                    _ => {}
                }
            }
            command = ctx.next_command() => {
                let Some(command) = command else { break Session::Stop };
                match order_for(ctx, &net, &mut glides, &command) {
                    Err(e) => command.reply(Err(e)),
                    // Nothing to send (a stop with nothing moving).
                    Ok(None) => command.reply(Ok(())),
                    Ok(Some(mut order)) => {
                        sent += 1;
                        let id = format!("set-{sent}");
                        order["messageId"] = json!(id);
                        if let Err(e) = ws.send_text(&order.to_string()).await {
                            command.reply(Err(e.to_string()));
                            return Err(e);
                        }
                        pending.insert(id, command);
                    }
                }
            }
            () = until(again) => break Session::Again,
            () = until(revive.next()) => revive.ping_due(ws, &mut sent).await?,
            _ = ping.tick() => ws.ping().await?,
        }
    };
    for (_, command) in pending.drain() {
        command.reply(Err("z-wave network read again".into()));
    }
    Ok(outcome)
}

/// What to send for an order: `None` when there is nothing to send (a stop
/// with nothing moving).
fn order_for(
    ctx: &DriverCtx,
    net: &Network,
    glides: &mut Glides,
    command: &CommandRequest,
) -> Result<Option<Json>, String> {
    let (node, mapped) = net
        .by_device(&command.device.id)
        .ok_or_else(|| "unknown node".to_owned())?;
    match glides.order(ctx, node.id, mapped, command)? {
        Plan::Send(order) => Ok(Some(order)),
        Plan::Nothing => Ok(None),
        Plan::Plain => {
            let (id, value) = mapped.order(&command.key, &command.value)?;
            Ok(Some(
                json!({"command": "node.set_value", "nodeId": node.id, "valueId": id, "value": value}),
            ))
        }
    }
}

/// What the glides make of an order.
enum Plan {
    /// Not theirs: the plain order.
    Plain,
    Send(Json),
    /// Done, nothing to send.
    Nothing,
}

/// A dimmer held down: Moli glides it to [`DIM_FLOOR`] (its own level
/// change would go on to off) and, when let go, pins it where it has got to.
#[derive(Default)]
struct Glides(HashMap<(u16, String), Glide>);

struct Glide {
    from: f64,
    started: Instant,
    secs: f64,
}

impl Glides {
    /// Seconds from the top (99) to the floor.
    const FULL: f64 = 5.0;
    const TOP: f64 = 99.0;

    fn order(
        &mut self,
        ctx: &DriverCtx,
        node: u16,
        mapped: &Mapped,
        command: &CommandRequest,
    ) -> Result<Plan, String> {
        let Some((dim, level)) = mapped.dim(&command.key) else {
            return Ok(Plan::Plain);
        };
        let held = command.value == Value::Bool(true);
        let at = (node, level.to_owned());
        let set = |value: i64, secs: f64| -> Result<Plan, String> {
            let target = mapped.target(level).ok_or("read-only point")?;
            let mut order = json!({"command": "node.set_value", "nodeId": node, "valueId": target, "value": value});
            order["options"] = json!({ "transitionDuration": format!("{secs}s") });
            Ok(Plan::Send(order))
        };
        match (dim, held) {
            (Dim::Down, true) => {
                let now = ctx
                    .current(&command.device.id, level)
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0);
                #[allow(clippy::cast_precision_loss)]
                let floor = DIM_FLOOR as f64;
                // Off stays off; at the floor, nothing lower.
                if now <= floor {
                    self.0.remove(&at);
                    return Ok(Plan::Nothing);
                }
                let secs = ((now - floor) / (Self::TOP - floor) * Self::FULL)
                    .round()
                    .max(1.0);
                self.0.insert(
                    at,
                    Glide {
                        from: now,
                        started: Instant::now(),
                        secs,
                    },
                );
                set(DIM_FLOOR, secs)
            }
            (Dim::Up | Dim::Down, false) => match self.0.remove(&at) {
                Some(glide) => {
                    let done = (glide.started.elapsed().as_secs_f64() / glide.secs).min(1.0);
                    #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
                    let pinned = (glide.from - (glide.from - DIM_FLOOR as f64) * done)
                        .round()
                        .clamp(DIM_FLOOR as f64, Self::TOP) as i64;
                    set(pinned, 0.0)
                }
                None if dim == Dim::Down => Ok(Plan::Nothing),
                None => Ok(Plan::Plain),
            },
            (Dim::Up, true) => {
                self.0.remove(&at);
                Ok(Plan::Plain)
            }
            _ => Ok(Plan::Plain),
        }
    }
}

/// Z-Wave JS says a module dead after one missed reply and keeps it so
/// until it next speaks (an hour for a dimmer at rest): Moli asks it again
/// shortly after.
#[derive(Default)]
struct Revive(HashMap<u16, tokio::time::Instant>);

impl Revive {
    const AFTER: Duration = Duration::from_secs(20);

    fn on_event(&mut self, event: &Json) {
        if event["source"] == "node"
            && event["event"] == "dead"
            && let Some(node) = event["nodeId"].as_u64().and_then(|n| u16::try_from(n).ok())
        {
            self.0
                .entry(node)
                .or_insert_with(|| tokio::time::Instant::now() + Self::AFTER);
        }
    }

    fn next(&self) -> Option<tokio::time::Instant> {
        self.0.values().min().copied()
    }

    /// Pings the modules whose time has come (the answer brings them back
    /// alive, or not: Z-Wave JS says so by event).
    async fn ping_due(&mut self, ws: &mut WebSocket, sent: &mut u64) -> anyhow::Result<()> {
        let now = tokio::time::Instant::now();
        let due: Vec<u16> = self
            .0
            .iter()
            .filter(|(_, when)| **when <= now)
            .map(|(node, _)| *node)
            .collect();
        for node in due {
            self.0.remove(&node);
            *sent += 1;
            let order = json!({"command": "node.ping", "nodeId": node, "messageId": format!("ping-{sent}")});
            ws.send_text(&order.to_string()).await?;
        }
        Ok(())
    }
}

async fn until(when: Option<tokio::time::Instant>) {
    match when {
        Some(when) => tokio::time::sleep_until(when).await,
        None => std::future::pending().await,
    }
}

fn publish(ctx: &DriverCtx, net: &mut Network, node: Node) {
    let Some(mapped) = model::map(ctx.instance(), net.home, &node) else {
        return;
    };
    ctx.upsert_device(mapped.device.clone());
    ctx.set_availability(&mapped.device.id, node.online());
    for (key, value) in mapped.readings(&node) {
        ctx.set_state(&mapped.device.id, key, value);
    }
    net.nodes.insert(node.id, (node, mapped));
}

/// What an event changes.
#[derive(Debug, PartialEq)]
enum Change {
    Nothing,
    /// The network changed: read it again.
    Read,
    Value {
        device: DeviceId,
        key: String,
        value: moli_core::Value,
        /// An occurrence (a scene button): two alike are two events.
        pulse: bool,
    },
    Online(DeviceId, bool),
}

fn change(net: &mut Network, event: &Json) -> Change {
    let name = event["event"].as_str().unwrap_or_default();
    if event["source"] == "controller" {
        return if matches!(name, "node added" | "node removed") {
            Change::Read
        } else {
            Change::Nothing
        };
    }
    if event["source"] != "node" {
        return Change::Nothing;
    }
    let Some(node_id) = event["nodeId"].as_u64().and_then(|n| u16::try_from(n).ok()) else {
        return Change::Nothing;
    };
    let Some((node, mapped)) = net.nodes.get_mut(&node_id) else {
        // A node Moli does not know yet: read the network again.
        return if matches!(name, "statistics updated") {
            Change::Nothing
        } else {
            Change::Read
        };
    };
    let device = mapped.device.id.clone();
    match name {
        "value updated" | "value notification" => {
            let args = &event["args"];
            let Ok(id) = serde_json::from_value::<ValueId>(args.clone()) else {
                return Change::Nothing;
            };
            let raw = if name == "value updated" {
                &args["newValue"]
            } else {
                &args["value"]
            };
            match node.values.iter_mut().find(|v| v.id == id) {
                Some(v) => v.value = Some(raw.clone()),
                // A value never described: its metadata comes with a new read.
                None => return Change::Read,
            }
            match mapped.reading(&id, raw) {
                Some((key, value)) => Change::Value {
                    device,
                    key: key.to_owned(),
                    value,
                    pulse: name == "value notification",
                },
                None => Change::Nothing,
            }
        }
        "dead" => {
            node.status = 3;
            Change::Online(device, false)
        }
        "alive" | "wake up" => {
            node.status = 4;
            Change::Online(device, true)
        }
        // Back to sleep after being heard: still there.
        "sleep" => {
            node.status = 1;
            node.statistics
                .last_seen
                .get_or_insert_with(|| "now".into());
            Change::Online(device, true)
        }
        "value added" | "value removed" | "metadata updated" | "ready" | "interview completed" => {
            Change::Read
        }
        _ => Change::Nothing,
    }
}

/// One event, applied. Returns whether the network is to be read again.
fn on_event(ctx: &DriverCtx, net: &mut Network, event: &Json) -> bool {
    match change(net, event) {
        Change::Nothing => false,
        Change::Read => true,
        Change::Value {
            device,
            key,
            value,
            pulse: true,
        } => {
            ctx.pulse_state(&device, &key, value);
            false
        }
        Change::Value {
            device,
            key,
            value,
            pulse: false,
        } => {
            ctx.set_state(&device, &key, value);
            false
        }
        Change::Online(device, online) => {
            ctx.set_availability(&device, online);
            false
        }
    }
}

/// The answer to a `node.set_value`.
fn outcome_of(message: &Json) -> Result<(), String> {
    if message["success"] != true {
        return Err(format!(
            "{} {}",
            message["errorCode"].as_str().unwrap_or("refused"),
            message["message"].as_str().unwrap_or_default()
        )
        .trim()
        .to_owned());
    }
    // SetValueStatus: 1 working, 254 done without confirmation, 255 done.
    match message["result"]["result"]["status"].as_u64() {
        None | Some(1 | 254 | 255) => Ok(()),
        Some(status) => Err(format!("z-wave refused the value (status {status})")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_on_this_machine() {
        let config = |host: &str| Config {
            host: host.into(),
            port: 3000,
            frontend: None,
        };
        assert!(Zwave::new(config("127.0.0.1")).is_ok());
        assert!(Zwave::new(config("localhost")).is_ok());
        assert!(Zwave::new(config("192.168.1.20")).is_err());
    }

    #[test]
    fn set_value_answers() {
        assert_eq!(
            outcome_of(&json!({"success": true, "result": {"result": {"status": 255}}})),
            Ok(())
        );
        assert_eq!(outcome_of(&json!({"success": true, "result": {}})), Ok(()));
        assert!(
            outcome_of(&json!({"success": true, "result": {"result": {"status": 2}}})).is_err()
        );
        assert_eq!(
            outcome_of(&json!({"success": false, "errorCode": "node_not_found"})),
            Err("node_not_found".into())
        );
    }

    #[test]
    fn events_update_values_or_ask_for_a_new_read() {
        let state: Json = serde_json::from_str(include_str!(
            "../../../integrations/zwave/fixtures/state.json"
        ))
        .unwrap();
        let nodes: Vec<Node> = serde_json::from_value(state["nodes"].clone()).unwrap();
        let instance = moli_core::InstanceId::from("zwave");
        let mut net = Network {
            home: 1,
            ..Network::default()
        };
        for node in nodes {
            if let Some(m) = model::map(&instance, 1, &node) {
                net.nodes.insert(node.id, (node, m));
            }
        }
        let event = |name: &str, node: u64, args: Json| json!({"source": "node", "event": name, "nodeId": node, "args": args});
        let relay = model::device_id(&instance, 1, 2);
        let on = json!({"commandClass": 37, "endpoint": 0, "property": "currentValue", "newValue": true});
        assert_eq!(
            change(&mut net, &event("value updated", 2, on)),
            Change::Value {
                device: relay.clone(),
                key: "switch".into(),
                value: moli_core::Value::Bool(true),
                pulse: false
            }
        );
        // Kept: a later read of the node says « on ».
        assert!(
            net.nodes[&2]
                .0
                .values
                .iter()
                .any(|v| v.value == Some(json!(true)))
        );
        // A hidden value (a setting) changes nothing Moli shows.
        let setting = json!({"commandClass": 112, "endpoint": 0, "property": 11, "newValue": 10});
        assert_eq!(
            change(&mut net, &event("value updated", 2, setting)),
            Change::Nothing
        );
        // A value never described, a new node, an interview: read again.
        let new =
            json!({"commandClass": 49, "endpoint": 0, "property": "Humidity", "newValue": 50});
        assert_eq!(
            change(&mut net, &event("value updated", 2, new)),
            Change::Read
        );
        assert_eq!(
            change(&mut net, &event("value updated", 9, json!({}))),
            Change::Read
        );
        assert_eq!(
            change(&mut net, &event("interview completed", 3, json!({}))),
            Change::Read
        );
        assert_eq!(
            change(
                &mut net,
                &json!({"source": "controller", "event": "node added"})
            ),
            Change::Read
        );
        // Dead and back.
        assert_eq!(
            change(&mut net, &event("dead", 2, json!({}))),
            Change::Online(relay.clone(), false)
        );
        assert!(!net.nodes[&2].0.online());
        assert_eq!(
            change(&mut net, &event("alive", 2, json!({}))),
            Change::Online(relay, true)
        );
    }

    #[tokio::test]
    async fn a_dead_module_is_asked_again_once() {
        let mut revive = Revive::default();
        revive.on_event(&json!({"source": "node", "event": "alive", "nodeId": 30}));
        assert_eq!(revive.next(), None);
        let before = tokio::time::Instant::now();
        revive.on_event(&json!({"source": "node", "event": "dead", "nodeId": 40}));
        let first = revive.next().expect("asked again");
        assert!(first >= before + Revive::AFTER);
        // Said dead again meanwhile: still one ping, at the first time.
        revive.on_event(&json!({"source": "node", "event": "dead", "nodeId": 40}));
        assert_eq!(revive.next(), Some(first));
        assert_eq!(revive.0.len(), 1);
    }
}
