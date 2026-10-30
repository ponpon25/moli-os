//! WiZ lights and LED strips, 100 % local: JSON over UDP (port 38899), with
//! no account and no key (the WiZ app's « local communication » left on):
//! power, brightness, white temperature, colour.
//!
//! The lamp says what it is (`getSystemConfig`: its MAC, its module) and
//! where it stands (`getPilot`), and takes orders (`setPilot`). It keeps no
//! name of its own on the network: the instance may give one.

use std::time::Duration;

use anyhow::{Context as _, bail};
use moli_core::{Access, Device, DeviceId, Kind, PointSpec, Semantic, Unit, Value};
use moli_runtime::{BoxFuture, CommandRequest, Driver, DriverCtx};
use serde::Deserialize;
use serde_json::{Value as Json, json};
use tokio::net::UdpSocket;

const PORT: u16 = 38899;
const POLL: Duration = Duration::from_secs(5);
/// How long one try waits for the answer (UDP: a lost packet is retried).
const WAIT: Duration = Duration::from_millis(800);
const TRIES: usize = 3;
/// WiZ's own bounds.
const DIM: (i64, i64) = (10, 100);
const TEMP: (i64, i64) = (2200, 6500);
/// How close (squared, channels 0–1) a colour must be to a white to be one.
const NEAR: f64 = 0.01;

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
    /// The lamp's name (it keeps none on the network).
    #[serde(default)]
    pub name: Option<String>,
}

fn default_port() -> u16 {
    PORT
}

#[derive(Debug)]
pub struct Wiz {
    config: Config,
}

impl Wiz {
    #[must_use]
    pub fn new(config: Config) -> Self {
        Self { config }
    }
}

impl Driver for Wiz {
    fn kind(&self) -> &'static str {
        "wiz"
    }

    fn run<'a>(&'a self, ctx: &'a mut DriverCtx) -> BoxFuture<'a, anyhow::Result<()>> {
        Box::pin(run(&self.config, ctx))
    }
}

/// One request, its `result`: retried while the lamp does not answer.
async fn call(config: &Config, method: &str, params: &Json) -> anyhow::Result<Json> {
    let socket = UdpSocket::bind(("0.0.0.0", 0)).await?;
    socket
        .connect((config.host.as_str(), config.port))
        .await
        .with_context(|| config.host.clone())?;
    let request = serde_json::to_vec(&json!({ "method": method, "params": params }))?;
    let mut buf = vec![0u8; 2048];
    for _ in 0..TRIES {
        socket.send(&request).await?;
        let Ok(received) = tokio::time::timeout(WAIT, socket.recv(&mut buf)).await else {
            continue;
        };
        let reply: Json = serde_json::from_slice(&buf[..received?])?;
        if reply["method"].as_str().is_some_and(|m| m != method) {
            continue;
        }
        if let Some(error) = reply.get("error") {
            bail!(moli_i18n::tr!(
                "pilotes.wiz.refus",
                error = error["message"].as_str().unwrap_or("?")
            ));
        }
        return Ok(reply["result"].clone());
    }
    bail!(moli_i18n::tr!("pilotes.wiz.pas_de_reponse"))
}

/// What the module can do, from its name (`ESP03_SHRGB1W_01`: colour and
/// whites; `…SHTW…`: whites; else brightness only).
fn abilities(module: &str) -> (bool, bool) {
    let color = module.contains("RGB");
    (color || module.contains("TW"), color)
}

fn spec(key: &str, label: String, kind: Kind, unit: Option<Unit>, semantic: Semantic) -> PointSpec {
    PointSpec {
        key: key.into(),
        label: label.into(),
        kind,
        access: Access {
            read: true,
            write: true,
        },
        unit,
        semantic,
    }
}

#[allow(clippy::cast_precision_loss)]
fn range((min, max): (i64, i64)) -> Kind {
    Kind::Numeric {
        min: Some(min as f64),
        max: Some(max as f64),
        step: Some(1.0),
    }
}

fn device(ctx: &DriverCtx, id: &DeviceId, name: Option<&str>, module: &str) -> Device {
    let (white, color) = abilities(module);
    let mut points = vec![
        spec(
            "on",
            moli_i18n::tr!("pilotes.wiz.allumee"),
            Kind::Binary,
            None,
            Semantic::OnOff,
        ),
        spec(
            "brightness",
            moli_i18n::tr!("pilotes.wiz.luminosite"),
            range(DIM),
            Some(Unit::Percent),
            Semantic::Brightness,
        ),
    ];
    if white {
        points.push(spec(
            "color_temp",
            moli_i18n::tr!("pilotes.wiz.temperature_couleur"),
            range(TEMP),
            Unit::parse("K"),
            Semantic::ColorTemp,
        ));
    }
    if color {
        points.push(spec(
            "color",
            moli_i18n::tr!("pilotes.wiz.couleur"),
            Kind::Text,
            None,
            Semantic::Color,
        ));
    }
    Device {
        id: id.clone(),
        instance: ctx.instance().clone(),
        native_name: name
            .map_or_else(|| moli_i18n::tr!("pilotes.wiz.lampe"), str::to_owned)
            .into(),
        manufacturer: Some("WiZ".into()),
        model: Some(module.into()),
        description: None,
        native_room: None,
        members: Vec::new(),
        points,
    }
}

fn hex(r: i64, g: i64, b: i64) -> String {
    let byte = |v: i64| v.clamp(0, 255);
    format!("#{:02x}{:02x}{:02x}", byte(r), byte(g), byte(b))
}

fn rgb(text: &str) -> Option<(u8, u8, u8)> {
    let digits = text.strip_prefix('#').filter(|d| d.len() == 6)?;
    let byte = |i: usize| u8::from_str_radix(digits.get(i..i + 2)?, 16).ok();
    Some((byte(0)?, byte(2)?, byte(4)?))
}

/// A white as a lamp of `kelvin` shows it (Tanner Helland's fit), each
/// channel 0–1, the strongest at 1.
#[allow(clippy::cast_precision_loss)]
fn white_rgb(kelvin: i64) -> [f64; 3] {
    let t = kelvin as f64 / 100.0;
    let (r, g) = if t <= 66.0 {
        (255.0, 99.470_802_586_1 * t.ln() - 161.119_568_166_1)
    } else {
        (
            329.698_727_446 * (t - 60.0).powf(-0.133_204_759_2),
            288.122_169_528_3 * (t - 60.0).powf(-0.075_514_849_2),
        )
    };
    let b = if t >= 66.0 {
        255.0
    } else if t <= 19.0 {
        0.0
    } else {
        138.517_731_223_1 * (t - 10.0).ln() - 305.044_792_730_7
    };
    let rgb = [r, g, b].map(|c: f64| c.clamp(0.0, 255.0));
    let max = rgb.iter().copied().fold(1.0, f64::max);
    rgb.map(|c| c / max)
}

/// The white a colour stands for, if it is one: HomeKit's presets are
/// whites given as pale colours, which the colour LEDs alone render badly.
fn white_of(r: u8, g: u8, b: u8) -> Option<i64> {
    let max = f64::from(r.max(g).max(b));
    if max == 0.0 {
        return None;
    }
    let colour = [r, g, b].map(|v| f64::from(v) / max);
    let distance = |kelvin| {
        white_rgb(kelvin)
            .iter()
            .zip(colour)
            .map(|(w, c)| (w - c).powi(2))
            .sum::<f64>()
    };
    (TEMP.0..=TEMP.1)
        .step_by(100)
        .map(|kelvin| (kelvin, distance(kelvin)))
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .filter(|(_, d)| *d < NEAR)
        .map(|(kelvin, _)| kelvin)
}

/// A colour for the strip: its white part on the white LEDs (cold `c` for
/// a cool tint, warm `w` otherwise), the rest on the colour ones.
fn colour_params(r: u8, g: u8, b: u8) -> Json {
    let white = r.min(g).min(b);
    let (warm, cold) = if r >= b { (white, 0) } else { (0, white) };
    json!({ "r": r - white, "g": g - white, "b": b - white, "c": cold, "w": warm })
}

/// `getPilot`'s result as points: a lamp in white mode says `temp`, in
/// colour mode `r`, `g`, `b` (and its white LEDs `c`, `w`).
fn values(pilot: &Json) -> Vec<(&'static str, Value)> {
    let mut out = Vec::new();
    if let Some(on) = pilot["state"].as_bool() {
        out.push(("on", Value::Bool(on)));
    }
    if let Some(dimming) = pilot["dimming"].as_i64() {
        out.push(("brightness", Value::Int(dimming)));
    }
    match pilot["temp"].as_i64() {
        Some(temp) if temp > 0 => out.push(("color_temp", Value::Int(temp))),
        _ => {
            let channel = |c: &str| pilot[c].as_i64();
            if let (Some(r), Some(g), Some(b)) = (channel("r"), channel("g"), channel("b")) {
                let white = channel("c").unwrap_or(0).max(channel("w").unwrap_or(0));
                let colour = hex(r + white, g + white, b + white);
                out.push(("color", Value::from(colour.as_str())));
            }
        }
    }
    out
}

/// The `setPilot` parameters for an order, and the points it changes.
fn order(key: &str, value: &Value) -> anyhow::Result<(Json, Vec<(&'static str, Value)>)> {
    #[allow(clippy::cast_possible_truncation)]
    let number = |v: &Value| v.as_f64().map(|n| n.round() as i64).context("number");
    Ok(match (key, value) {
        ("on", Value::Bool(on)) => (json!({ "state": on }), vec![("on", Value::Bool(*on))]),
        // A level asked of a lamp that is off lights it.
        ("brightness", v) => {
            let level = number(v)?.clamp(DIM.0, DIM.1);
            (
                json!({ "state": true, "dimming": level }),
                vec![("on", Value::Bool(true)), ("brightness", Value::Int(level))],
            )
        }
        ("color_temp", v) => {
            let temp = number(v)?.clamp(TEMP.0, TEMP.1);
            (
                json!({ "temp": temp }),
                vec![("color_temp", Value::Int(temp))],
            )
        }
        ("color", Value::Text(text)) => {
            let (r, g, b) =
                rgb(text).with_context(|| moli_i18n::tr!("pilotes.wiz.couleur_invalide"))?;
            let asked = ("color", Value::from(text.to_lowercase().as_str()));
            match white_of(r, g, b) {
                Some(temp) => (
                    json!({ "temp": temp }),
                    vec![asked, ("color_temp", Value::Int(temp))],
                ),
                None => (colour_params(r, g, b), vec![asked]),
            }
        }
        (key, _) => bail!(moli_i18n::tr!("pilotes.wiz.ne_se_regle_pas", point = key)),
    })
}

async fn apply(config: &Config, ctx: &DriverCtx, id: &DeviceId, command: CommandRequest) {
    let result = async {
        let (params, changed) = order(&command.key[..], &command.value)?;
        call(config, "setPilot", &params).await?;
        for (key, value) in changed {
            ctx.set_state(id, key, value);
        }
        anyhow::Ok(())
    }
    .await
    .map_err(|e| format!("{e:#}"));
    command.reply(result);
}

/// Waits a minute for a lamp off at the wall, refusing orders. `false`: the
/// driver is stopping.
async fn wait_a_minute(ctx: &mut DriverCtx, error: &str) -> bool {
    ctx.wait_for(moli_i18n::tr!(
        "pilotes.wiz.injoignable_nouvel_essai",
        error = error
    ));
    let wake = tokio::time::sleep(Duration::from_secs(60));
    tokio::pin!(wake);
    loop {
        tokio::select! {
            () = &mut wake => return true,
            command = ctx.next_command() => match command {
                Some(command) => command.reply(Err(moli_i18n::tr!("pilotes.wiz.injoignable"))),
                None => return false,
            },
        }
    }
}

async fn run(config: &Config, ctx: &mut DriverCtx) -> anyhow::Result<()> {
    let system = loop {
        match call(config, "getSystemConfig", &json!({})).await {
            Ok(system) => break system,
            Err(e) => {
                if !wait_a_minute(ctx, &format!("{e:#}")).await {
                    return Ok(());
                }
            }
        }
    };
    let native = system["mac"].as_str().unwrap_or(&config.host).to_owned();
    let module = system["moduleName"].as_str().unwrap_or_default();
    let id = ctx.device_id(&native);
    ctx.upsert_device(device(ctx, &id, config.name.as_deref(), module));
    ctx.ready();
    let mut tick = tokio::time::interval(POLL);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut online = None;
    loop {
        tokio::select! {
            command = ctx.next_command() => {
                let Some(command) = command else { return Ok(()) };
                apply(config, ctx, &id, command).await;
            }
            _ = tick.tick() => match call(config, "getPilot", &json!({})).await {
                Ok(pilot) => {
                    if pilot["mac"].as_str().is_some_and(|mac| mac != native) {
                        // Another lamp answers at this address now.
                        bail!(moli_i18n::tr!("pilotes.wiz.une_autre"));
                    }
                    if online != Some(true) {
                        online = Some(true);
                        ctx.set_availability(&id, true);
                    }
                    for (key, value) in values(&pilot) {
                        ctx.set_state(&id, key, value);
                    }
                }
                Err(e) => {
                    if online != Some(false) {
                        online = Some(false);
                        ctx.set_availability(&id, false);
                        tracing::warn!(instance = %ctx.instance(), error = %format!("{e:#}"), "wiz unreachable");
                    }
                }
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pilot_reads_as_points() {
        // A strip in colour mode, then in white mode (as `getPilot` answers).
        let colour = json!({"mac": "aabbccddeeff", "rssi": -60, "state": true, "sceneId": 0,
                            "r": 255, "g": 122, "b": 0, "c": 0, "w": 0, "dimming": 40});
        let v: std::collections::HashMap<_, _> = values(&colour).into_iter().collect();
        assert_eq!(v["on"], Value::Bool(true));
        assert_eq!(v["brightness"], Value::Int(40));
        assert_eq!(v["color"], Value::from("#ff7a00"));
        assert!(!v.contains_key("color_temp"));

        let white = json!({"state": false, "sceneId": 0, "temp": 2700, "dimming": 100});
        let v: std::collections::HashMap<_, _> = values(&white).into_iter().collect();
        assert_eq!(v["on"], Value::Bool(false));
        assert_eq!(v["color_temp"], Value::Int(2700));
        assert!(!v.contains_key("color"));
    }

    #[test]
    fn orders_encode() {
        let (params, changed) = order("brightness", &Value::Int(3)).unwrap();
        assert_eq!(
            params,
            json!({"state": true, "dimming": 10}),
            "lit, within WiZ's bounds"
        );
        assert_eq!(changed[0], ("on", Value::Bool(true)));
        let (params, _) = order("color", &Value::from("#FF7A00")).unwrap();
        assert_eq!(params, json!({"r": 255, "g": 122, "b": 0, "c": 0, "w": 0}));
        // HomeKit's presets: whites as pale colours (hue °, saturation %).
        let preset = |hex: &str| order("color", &Value::from(hex)).unwrap().0;
        assert_eq!(preset("#ffe0c4"), json!({"temp": 4700}), "28°, 23 %");
        assert_eq!(preset("#ffc285"), json!({"temp": 3400}), "30°, 48 %");
        assert_eq!(preset("#ffaa54"), json!({"temp": 2700}), "30°, 67 %");
        assert_eq!(preset("#f5f2ff"), json!({"temp": 6400}), "251°, 5 %");
        // A pale blue that is no white: its white part on the cold LEDs.
        assert_eq!(
            preset("#ccdcff"),
            json!({"r": 0, "g": 16, "b": 51, "c": 204, "w": 0})
        );
        // Read back, the white LEDs count again.
        let v: std::collections::HashMap<_, _> =
            values(&json!({"r": 0, "g": 16, "b": 51, "c": 204, "w": 0}))
                .into_iter()
                .collect();
        assert_eq!(v["color"], Value::from("#ccdcff"));
        let (params, _) = order("color_temp", &Value::Int(9000)).unwrap();
        assert_eq!(params, json!({"temp": 6500}));
        assert!(order("color", &Value::from("orange")).is_err());
        assert!(order("speed", &Value::Int(1)).is_err());
    }

    #[test]
    fn module_names_say_what_it_can() {
        assert_eq!(abilities("ESP03_SHRGB1W_01"), (true, true));
        assert_eq!(abilities("ESP06_SHTW1C_01"), (true, false));
        assert_eq!(abilities("ESP05_SHDW_01"), (false, false));
    }
}
