//! RFXtrx: the 433 MHz sensors of the house through an RFXCOM RFXtrx433
//! transceiver on USB. Local, read-only, no cloud.
//!
//! The transceiver speaks length-prefixed frames on a serial port (38 400
//! bauds): Moli resets it, asks its status, starts its receiver, then reads.
//! Each sensor is known by its code on the air, `<packet>-<subtype>-<id>`
//! (`52-01-1a2b`). With `auto_add`, a sensor heard becomes a device under
//! that code; a `[[driver.options.sensor]]` gives it a stable id instead,
//! which survives an Oregon battery change (the sensor then picks a new
//! code: only `code` changes, the device and its history stay).
//!
//! The protocols the transceiver listens to are kept in its memory and left
//! as they are (RFXmngr changes them); the status in the logs says which.
//! The transceiver is a device too (`<instance>:transceiver`): its `listen`
//! switch has it hear every protocol for five minutes (never saved in its
//! memory), and `heard` lists who spoke: a remote to try, a neighbour's
//! probe. Its saved protocols come back at the end.

mod frame;
mod learn;

use std::collections::{HashMap, HashSet};
use std::os::fd::OwnedFd;
use std::time::{Duration, Instant};

use anyhow::{Context as _, bail, ensure};
use frame::{Frame, Framer, Reading, Status};
use learn::{Learn, Standing};
use moli_core::{Access, Device, DeviceId, Kind, PointSpec, Semantic, Unit, Value};
use moli_runtime::{BoxFuture, Driver, DriverCtx};
use rustix::termios::{self, ControlModes, OptionalActions, QueueSelector, SpecialCodeIndex};
use serde::Deserialize;
use tokio::io::unix::AsyncFd;

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// The transceiver's serial port (default `/dev/rfxtrx`, the name the
    /// compose file gives it).
    #[serde(default = "default_port")]
    pub port: String,
    /// A sensor heard becomes a device on its own (default true).
    #[serde(default = "default_true")]
    pub auto_add: bool,
    /// Codes never added (a neighbour's weather station).
    #[serde(default)]
    pub ignore: Vec<String>,
    /// Minutes without a word before a sensor counts as offline (default 30:
    /// an Oregon sensor speaks every minute or so).
    #[serde(default = "default_offline")]
    pub offline_after: u64,
    #[serde(default, rename = "sensor")]
    pub sensors: Vec<Sensor>,
}

/// A sensor given a stable id.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Sensor {
    /// The device is `<instance>:<id>`.
    pub id: String,
    /// Its code on the air, `52-01-1a2b` (in the device's description and
    /// in the logs when it is first heard).
    pub code: String,
    #[serde(default)]
    pub name: Option<String>,
}

/// The transceiver's own device.
const TRANSCEIVER: &str = "transceiver";

/// How long « listen to everything » lasts.
const LISTENING: Duration = Duration::from_secs(5 * 60);

fn default_port() -> String {
    "/dev/rfxtrx".into()
}

fn default_true() -> bool {
    true
}

fn default_offline() -> u64 {
    30
}

#[derive(Debug)]
pub struct Rfxtrx {
    config: Config,
    /// Declared code → its sensor.
    declared: HashMap<(u8, u8, u16), Sensor>,
    ignored: HashSet<(u8, u8, u16)>,
}

impl Rfxtrx {
    pub fn new(config: Config) -> anyhow::Result<Self> {
        ensure!(!config.port.trim().is_empty(), "port: the serial port");
        ensure!(
            config.offline_after >= 2,
            "offline_after: 2 minutes at least"
        );
        let code = |c: &str| {
            frame::parse_code(c).with_context(|| {
                format!("{c:?}: a sensor code like 52-01-1a2b (temperature, humidity, pressure)")
            })
        };
        let mut declared = HashMap::new();
        let mut ids = HashSet::new();
        for s in &config.sensors {
            ensure!(
                !s.id.is_empty() && !s.id.contains([':', '/']),
                "sensor id {:?}: non-empty, without ':' nor '/'",
                s.id
            );
            // An id shaped like a code would collide with an added sensor.
            ensure!(
                frame::parse_code(&s.id).is_none(),
                "sensor id {:?}: a name, not a code",
                s.id
            );
            ensure!(
                s.id != TRANSCEIVER,
                "sensor id {TRANSCEIVER:?} is the transceiver's"
            );
            ensure!(ids.insert(s.id.clone()), "sensor id {:?} twice", s.id);
            ensure!(
                declared.insert(code(&s.code)?, s.clone()).is_none(),
                "sensor code {:?} twice",
                s.code
            );
        }
        let ignored = config
            .ignore
            .iter()
            .map(|c| code(c))
            .collect::<anyhow::Result<_>>()?;
        Ok(Self {
            config,
            declared,
            ignored,
        })
    }
}

impl Rfxtrx {
    /// Where a sensor's code stands, for the « listen » list.
    fn standing(&self, code: &str, known: &HashSet<(u8, u8, u16)>) -> Standing {
        let Some(code) = frame::parse_code(code) else {
            return Standing::Unread;
        };
        if self.ignored.contains(&code) {
            Standing::Ignored
        } else if self.declared.contains_key(&code)
            || (self.config.auto_add && known.contains(&code))
        {
            Standing::Known
        } else {
            Standing::New
        }
    }

    /// A device of an earlier run this configuration no longer shows.
    fn stale(&self, native: &str) -> bool {
        if native == TRANSCEIVER || self.declared.values().any(|s| s.id == native) {
            return false;
        }
        match frame::parse_code(native) {
            Some(code) => {
                !self.config.auto_add
                    || self.ignored.contains(&code)
                    || self.declared.contains_key(&code)
            }
            None => true,
        }
    }
}

impl Driver for Rfxtrx {
    fn kind(&self) -> &'static str {
        "rfxtrx"
    }

    fn run<'a>(&'a self, ctx: &'a mut DriverCtx) -> BoxFuture<'a, anyhow::Result<()>> {
        Box::pin(run(self, ctx))
    }
}

/// The serial port, raw, 38 400 bauds, non-blocking.
fn open(path: &str) -> std::io::Result<AsyncFd<OwnedFd>> {
    use rustix::fs::{Mode, OFlags};
    let fd = rustix::fs::open(
        path,
        OFlags::RDWR | OFlags::NOCTTY | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    )?;
    let mut t = termios::tcgetattr(&fd)?;
    t.make_raw();
    t.set_speed(38_400)?;
    t.control_modes
        .insert(ControlModes::CLOCAL | ControlModes::CREAD);
    t.control_modes.remove(ControlModes::CSTOPB);
    // VMIN 1: an empty port then answers « would block » (O_NONBLOCK); with
    // 0 it answers 0 bytes, which is also what a port gone away answers.
    t.special_codes[SpecialCodeIndex::VMIN] = 1;
    t.special_codes[SpecialCodeIndex::VTIME] = 0;
    termios::tcsetattr(&fd, OptionalActions::Now, &t)?;
    AsyncFd::new(fd)
}

async fn send(port: &AsyncFd<OwnedFd>, bytes: &[u8]) -> anyhow::Result<()> {
    let mut sent = 0;
    while sent < bytes.len() {
        let mut guard = port.writable().await?;
        match guard.try_io(|fd| Ok(rustix::io::write(fd.get_ref(), &bytes[sent..])?)) {
            Ok(n) => sent += n.context("write to the transceiver")?,
            Err(_would_block) => {}
        }
    }
    Ok(())
}

/// Some bytes, as soon as there are; an error once the port is gone.
async fn receive(port: &AsyncFd<OwnedFd>, buf: &mut [u8]) -> anyhow::Result<usize> {
    loop {
        let mut guard = port.readable().await?;
        match guard.try_io(|fd| Ok(rustix::io::read(fd.get_ref(), &mut *buf)?)) {
            Ok(Ok(0)) => bail!("the transceiver is gone (unplugged?)"),
            Ok(n) => return n.context("read from the transceiver"),
            Err(_would_block) => {}
        }
    }
}

/// The first frame `want` accepts, within `limit`.
async fn answer(
    port: &AsyncFd<OwnedFd>,
    framer: &mut Framer,
    limit: Duration,
    want: impl Fn(&Frame) -> bool,
) -> anyhow::Result<Option<Frame>> {
    let deadline = tokio::time::Instant::now() + limit;
    let mut buf = [0u8; 256];
    loop {
        let n = match tokio::time::timeout_at(deadline, receive(port, &mut buf)).await {
            Ok(n) => n?,
            Err(_) => return Ok(None),
        };
        for f in framer.push(&buf[..n]) {
            if let Some(f) = frame::decode(&f).filter(&want) {
                return Ok(Some(f));
            }
        }
    }
}

/// Reset, status (in the logs), receiver on.
async fn start(port: &AsyncFd<OwnedFd>, framer: &mut Framer, path: &str) -> anyhow::Result<Status> {
    send(port, &frame::RESET).await?;
    tokio::time::sleep(Duration::from_secs(1)).await;
    termios::tcflush(port.get_ref(), QueueSelector::IFlush)?;
    send(port, &frame::GET_STATUS).await?;
    let Some(Frame::Status(status)) = answer(port, framer, Duration::from_secs(5), |f| {
        matches!(f, Frame::Status(_))
    })
    .await?
    else {
        bail!("no answer from the transceiver on {path}");
    };
    let protocols: Vec<String> = status
        .protocols
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    tracing::info!(
        "rfxtrx: {} firmware {} ({}), protocols {}",
        status.frequency,
        status.firmware,
        status.firmware_type,
        protocols.join(" "),
    );
    send(port, &frame::START_RECEIVER).await?;
    Ok(status)
}

/// Every protocol for a while (`on`), or the transceiver's own again.
async fn listen(port: &AsyncFd<OwnedFd>, status: &Status, on: bool) -> anyhow::Result<()> {
    let protocols = if on {
        frame::EVERYTHING
    } else {
        status.protocols
    };
    send(port, &frame::set_mode(status.frequency_code, protocols)).await?;
    tracing::info!(
        "rfxtrx: {}",
        if on {
            "listening to every protocol for 5 minutes"
        } else {
            "back to the saved protocols"
        }
    );
    Ok(())
}

/// Resolves at `when`, never without one.
async fn until(when: Option<tokio::time::Instant>) {
    match when {
        Some(when) => tokio::time::sleep_until(when).await,
        None => std::future::pending().await,
    }
}

/// The transceiver's device, with what its status said.
fn publish_transceiver(ctx: &DriverCtx, status: &Status) -> DeviceId {
    let transceiver = ctx.device_id(TRANSCEIVER);
    ctx.upsert_device(transceiver_device(ctx, &transceiver, status));
    ctx.set_availability(&transceiver, true);
    ctx.set_state(
        &transceiver,
        "firmware",
        Value::Text(
            format!(
                "{} ({}), {}",
                status.firmware, status.firmware_type, status.frequency
            )
            .into(),
        ),
    );
    ctx.set_state(
        &transceiver,
        "protocols",
        Value::Text(frame::protocol_names(status.protocols).join(", ").into()),
    );
    ctx.set_state(&transceiver, "listen", Value::Bool(false));
    transceiver
}

/// When each sensor last spoke, and whether it was said online since.
type Seen = HashMap<DeviceId, (Instant, bool)>;

/// The declared sensors (unknown until they speak), and the devices of
/// earlier runs this configuration no longer shows, removed.
fn publish_sensors(this: &Rfxtrx, ctx: &DriverCtx) -> (Seen, HashSet<(u8, u8, u16)>) {
    // When each sensor last spoke, and whether it was said online since.
    let mut seen: Seen = HashMap::new();
    let mut known: HashSet<(u8, u8, u16)> = HashSet::new();
    for (&code, s) in &this.declared {
        let id = ctx.device_id(&s.id);
        ctx.upsert_device(device(ctx, &id, code, s.name.as_deref()));
        // Unknown until it speaks (some speak every few minutes): silent
        // only after a whole `offline_after`.
        seen.insert(id, (Instant::now(), false));
        known.insert(code);
    }
    // What earlier runs published and this one will not: a sensor no
    // longer declared, a code now ignored or given a stable id. Gone from
    // Moli, from the HomeKit bridge, from the state cache.
    for id in ctx.known_devices() {
        let native = id.to_string();
        let native = native.split_once(':').map_or("", |(_, n)| n);
        if this.stale(native) {
            tracing::debug!("rfxtrx: {id} removed (no longer declared nor heard)");
            ctx.remove_device(&id);
        }
    }
    (seen, known)
}

async fn run(this: &Rfxtrx, ctx: &mut DriverCtx) -> anyhow::Result<()> {
    // Not plugged (or not passed to the container): say so, look again.
    let port = loop {
        match open(&this.config.port) {
            Ok(port) => break port,
            Err(e) => {
                tracing::debug!(port = %this.config.port, "rfxtrx: {e}");
                ctx.wait_for(moli_i18n::tr!(
                    "pilotes.rfxtrx.absent",
                    port = this.config.port,
                    cause = e
                ));
                tokio::select! {
                    () = ctx.cancelled() => return Ok(()),
                    () = tokio::time::sleep(Duration::from_secs(30)) => {}
                }
            }
        }
    };
    let mut framer = Framer::default();
    let status = start(&port, &mut framer, &this.config.port).await?;
    let transceiver = publish_transceiver(ctx, &status);
    let mut listening: Option<tokio::time::Instant> = None;
    let mut learn = Learn::default();

    let offline_after = Duration::from_secs(this.config.offline_after.saturating_mul(60));
    let (mut seen, mut known) = publish_sensors(this, ctx);
    // Each kind of module heard but not read, said once.
    let mut unread: HashSet<(u8, u8)> = HashSet::new();
    ctx.ready();

    let mut buf = [0u8; 256];
    let mut last_byte = Instant::now();
    let mut tick = tokio::time::interval(Duration::from_secs(60));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            command = ctx.next_command() => match command {
                Some(command) if command.device.id == transceiver && &*command.key == "listen" => {
                    let on = command.value == Value::Bool(true);
                    if let Err(e) = listen(&port, &status, on).await {
                        command.reply(Err(e.to_string()));
                        return Err(e);
                    }
                    command.reply(Ok(()));
                    if on {
                        listening = Some(tokio::time::Instant::now() + LISTENING);
                        learn = Learn::default();
                        let end = moli_core::now_ms() + u64::try_from(LISTENING.as_millis()).unwrap_or(0);
                        ctx.set_state(&transceiver, "listen_until", Value::Int(i64::try_from(end).unwrap_or(0)));
                        ctx.set_state(&transceiver, "heard", Value::Text("[]".into()));
                    } else {
                        listening = None;
                    }
                    ctx.set_state(&transceiver, "listen", Value::Bool(on));
                }
                Some(command) => command.reply(Err(moli_i18n::tr!("pilotes.rfxtrx.lecture_seule"))),
                None => return Ok(()),
            },
            () = until(listening) => {
                listen(&port, &status, false).await?;
                listening = None;
                ctx.set_state(&transceiver, "listen", Value::Bool(false));
            }
            n = receive(&port, &mut buf) => {
                let n = n?;
                // The end of a frame never comes half a second later.
                if !framer.is_empty() && last_byte.elapsed() > Duration::from_millis(500) {
                    framer.clear();
                }
                last_byte = Instant::now();
                for f in framer.push(&buf[..n]) {
                    let decoded = frame::decode(&f);
                    if listening.is_some()
                        && learn.note(&f, decoded.as_ref(), moli_core::now_ms(), |code| this.standing(code, &known))
                    {
                        ctx.set_state(&transceiver, "heard", Value::Text(learn.json().into()));
                    }
                    match decoded {
                        Some(Frame::Reading(r)) => {
                            heard(this, ctx, &r, &mut known, &mut seen);
                        }
                        // 01/07: the receiver's answer to START_RECEIVER.
                        Some(Frame::Other(0x01, 0x07) | Frame::Status(_)) | None => {}
                        Some(Frame::Other(packet, subtype)) => {
                            if unread.insert((packet, subtype)) {
                                tracing::info!("rfxtrx: heard packet type {packet:02x}/{subtype:02x}, not read by Moli");
                            }
                        }
                    }
                }
            }
            _ = tick.tick() => {
                let now = Instant::now();
                seen.retain(|id, (at, _)| {
                    let fresh = now.duration_since(*at) < offline_after;
                    if !fresh {
                        ctx.set_availability(id, false);
                    }
                    fresh
                });
            }
        }
    }
}

/// One reading: its device (added if new), its values.
fn heard(
    this: &Rfxtrx,
    ctx: &DriverCtx,
    r: &Reading,
    known: &mut HashSet<(u8, u8, u16)>,
    seen: &mut Seen,
) {
    let code = (r.packet, r.subtype, r.id);
    let id = match this.declared.get(&code) {
        Some(s) => ctx.device_id(&s.id),
        None if this.ignored.contains(&code) => return,
        None if !this.config.auto_add => {
            if known.insert(code) {
                tracing::info!(
                    "rfxtrx: sensor {} heard, not added (auto_add = false)",
                    r.code()
                );
            }
            return;
        }
        None => ctx.device_id(&r.code()),
    };
    if known.insert(code) {
        let (brand, model) = frame::model(r.packet, r.subtype);
        let measured: Vec<String> = [
            r.temperature.map(|t| format!("{t} °C")),
            r.humidity.map(|h| format!("{h} %")),
            r.pressure.map(|p| format!("{p} hPa")),
        ]
        .into_iter()
        .flatten()
        .collect();
        tracing::info!(
            "rfxtrx: new sensor {} ({}), {}",
            r.code(),
            format!("{brand} {model}").trim(),
            measured.join(", ")
        );
        ctx.upsert_device(device(ctx, &id, code, None));
    }
    if !matches!(seen.get(&id), Some((_, true))) {
        ctx.set_availability(&id, true);
    }
    seen.insert(id.clone(), (Instant::now(), true));
    if let Some(t) = r.temperature {
        ctx.set_state(&id, "temperature", Value::Float(t));
    }
    if let Some(h) = r.humidity {
        ctx.set_state(&id, "humidity", Value::Int(i64::from(h)));
    }
    if let Some(p) = r.pressure {
        ctx.set_state(&id, "pressure", Value::Int(i64::from(p)));
    }
    ctx.set_state(&id, "battery_low", Value::Bool(r.battery == 0));
    ctx.set_state(&id, "rssi", Value::Int(i64::from(r.rssi)));
}

fn device(ctx: &DriverCtx, id: &DeviceId, code: (u8, u8, u16), name: Option<&str>) -> Device {
    let (packet, subtype, raw) = code;
    let (brand, model) = frame::model(packet, subtype);
    let code = frame::code(packet, subtype, raw);
    Device {
        id: id.clone(),
        instance: ctx.instance().clone(),
        native_name: name
            .map_or_else(
                || {
                    moli_i18n::tr!(
                        "pilotes.rfxtrx.sonde",
                        brand = if brand.is_empty() { model } else { brand },
                        id = format!("{raw:04x}")
                    )
                },
                str::to_owned,
            )
            .into(),
        manufacturer: (!brand.is_empty()).then(|| brand.into()),
        model: Some(model.into()),
        // The code: what to write in `code` or `ignore`.
        description: Some(code.into()),
        native_room: None,
        members: Vec::new(),
        points: points(packet),
    }
}

/// The transceiver: what it is, what it listens to, and its « listen to
/// everything » switch.
fn transceiver_device(ctx: &DriverCtx, id: &DeviceId, status: &Status) -> Device {
    let point = |key: &str, label: String, kind: Kind, write: bool| PointSpec {
        key: key.into(),
        label: label.into(),
        kind,
        access: Access { read: true, write },
        unit: None,
        semantic: Semantic::Config,
    };
    Device {
        id: id.clone(),
        instance: ctx.instance().clone(),
        native_name: moli_i18n::tr!("pilotes.rfxtrx.cle").into(),
        manufacturer: Some("RFXCOM".into()),
        model: Some(format!("RFXtrx ({})", status.frequency).into()),
        description: None,
        native_room: None,
        members: Vec::new(),
        points: vec![
            point(
                "listen",
                moli_i18n::tr!("pilotes.rfxtrx.ecoute"),
                Kind::Binary,
                true,
            ),
            point(
                "listen_until",
                moli_i18n::tr!("pilotes.rfxtrx.ecoute_jusqua"),
                Kind::Numeric {
                    min: None,
                    max: None,
                    step: None,
                },
                false,
            ),
            point(
                "heard",
                moli_i18n::tr!("pilotes.rfxtrx.entendu"),
                Kind::Text,
                false,
            ),
            point(
                "firmware",
                moli_i18n::tr!("pilotes.rfxtrx.firmware"),
                Kind::Text,
                false,
            ),
            point(
                "protocols",
                moli_i18n::tr!("pilotes.rfxtrx.protocoles"),
                Kind::Text,
                false,
            ),
        ],
    }
}

fn points(packet: u8) -> Vec<PointSpec> {
    let read = Access {
        read: true,
        write: false,
    };
    let numeric = |min, max, step| Kind::Numeric { min, max, step };
    let mut points = Vec::new();
    if matches!(packet, frame::TEMP | frame::TEMP_HUM | frame::TEMP_HUM_BARO) {
        points.push(PointSpec {
            key: "temperature".into(),
            label: moli_i18n::tr!("pilotes.rfxtrx.temperature").into(),
            kind: numeric(None, None, Some(0.1)),
            access: read,
            unit: Some(Unit::Celsius),
            semantic: Semantic::Temperature,
        });
    }
    if matches!(packet, frame::HUM | frame::TEMP_HUM | frame::TEMP_HUM_BARO) {
        points.push(PointSpec {
            key: "humidity".into(),
            label: moli_i18n::tr!("pilotes.rfxtrx.humidite").into(),
            kind: numeric(Some(0.0), Some(100.0), Some(1.0)),
            access: read,
            unit: Some(Unit::Percent),
            semantic: Semantic::Humidity,
        });
    }
    if packet == frame::TEMP_HUM_BARO {
        points.push(PointSpec {
            key: "pressure".into(),
            label: moli_i18n::tr!("pilotes.rfxtrx.pression").into(),
            kind: numeric(None, None, Some(1.0)),
            access: read,
            unit: Some(Unit::HectoPascal),
            semantic: Semantic::Pressure,
        });
    }
    points.push(PointSpec {
        key: "battery_low".into(),
        label: moli_i18n::tr!("pilotes.rfxtrx.pile_faible").into(),
        kind: Kind::Binary,
        access: read,
        unit: None,
        semantic: Semantic::BatteryLow,
    });
    points.push(PointSpec {
        key: "rssi".into(),
        label: moli_i18n::tr!("pilotes.rfxtrx.signal").into(),
        kind: numeric(Some(0.0), Some(15.0), Some(1.0)),
        access: read,
        unit: None,
        semantic: Semantic::SignalStrength,
    });
    points
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(toml: &str) -> anyhow::Result<Rfxtrx> {
        Rfxtrx::new(toml::from_str(toml)?)
    }

    #[test]
    fn validates_the_options() {
        let r = config("").unwrap();
        assert_eq!(r.config.port, "/dev/rfxtrx");
        assert!(r.config.auto_add);
        let r = config(
            "ignore = [\"52-01-9999\"]\n\
             [[sensor]]\nid = \"salon\"\ncode = \"52-01-1a2b\"\nname = \"Salon\"\n",
        )
        .unwrap();
        assert_eq!(r.declared[&(0x52, 0x01, 0x1A2B)].id, "salon");
        assert!(r.ignored.contains(&(0x52, 0x01, 0x9999)));
        assert!(config("[[sensor]]\nid = \"a/b\"\ncode = \"52-01-1a2b\"\n").is_err());
        assert!(config("[[sensor]]\nid = \"52-01-0000\"\ncode = \"52-01-1a2b\"\n").is_err());
        assert!(config("[[sensor]]\nid = \"salon\"\ncode = \"salon\"\n").is_err());
        assert!(
            config(
                "[[sensor]]\nid = \"a\"\ncode = \"52-01-1a2b\"\n\
                 [[sensor]]\nid = \"b\"\ncode = \"52-01-1A2B\"\n"
            )
            .is_err(),
            "one code, one sensor"
        );
        assert!(config("ignore = [\"oups\"]").is_err());
        assert!(config("offline_after = 1").is_err());
        assert!(config("unknown = 1").is_err());
    }

    #[test]
    fn forgets_what_it_no_longer_shows() {
        let r = config(
            "ignore = [\"50-06-3c0a\"]\n\
             [[sensor]]\nid = \"salon\"\ncode = \"52-01-1a2b\"\n",
        )
        .unwrap();
        assert!(!r.stale("salon"));
        assert!(r.stale("52-01-1a2b"), "now under its stable id");
        assert!(r.stale("50-06-3c0a"), "ignored");
        assert!(r.stale("garage"), "no longer declared");
        assert!(!r.stale("52-01-9999"), "added when heard");
        assert!(!r.stale("transceiver"));
        assert_eq!(r.standing("52-01-1a2b", &HashSet::new()), Standing::Known);
        assert_eq!(r.standing("50-06-3c0a", &HashSet::new()), Standing::Ignored);
        assert_eq!(r.standing("52-01-7777", &HashSet::new()), Standing::New);
        assert_eq!(
            r.standing("13-00-abcdef", &HashSet::new()),
            Standing::Unread
        );
        assert!(config("[[sensor]]\nid = \"transceiver\"\ncode = \"52-01-1a2b\"\n").is_err());
        let r = config("auto_add = false").unwrap();
        assert!(r.stale("52-01-9999"));
    }

    #[test]
    fn points_follow_what_the_sensor_measures() {
        let keys = |p: u8| -> Vec<String> { points(p).iter().map(|p| p.key.to_string()).collect() };
        assert_eq!(
            keys(frame::TEMP_HUM),
            ["temperature", "humidity", "battery_low", "rssi"]
        );
        assert_eq!(keys(frame::TEMP), ["temperature", "battery_low", "rssi"]);
        assert_eq!(keys(frame::HUM), ["humidity", "battery_low", "rssi"]);
        assert!(keys(frame::TEMP_HUM_BARO).contains(&"pressure".to_owned()));
        assert!(points(frame::TEMP_HUM).iter().all(|p| !p.access.write));
    }
}
