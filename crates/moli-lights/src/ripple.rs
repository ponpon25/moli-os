//! Ripple-control re-levelling: the grid's 175 Hz tariff signal (sent at
//! fixed times to switch off-peak relays, water heaters, street lights)
//! upsets phase-cut dimmers. A LED dimmed low brightens on its own and
//! stays there, while the dimmer still reports the old level. Sending the
//! same level does nothing; moving it one step and back does.
//!
//! Moli runs it as instance [`INSTANCE`] whenever `moli.toml` declares:
//!
//! ```toml
//! [ripple]
//! # Power readings of dimmers that measure: a jump with no order is a frame.
//! watch = ["zwave:…/ep1.sensor.power"]
//! # The levels to re-set once the frame is over (dimmers that are on).
//! dimmers = ["zwave:…/ep1.level", "z2m:…/brightness"]
//! # Known frame times (local), for when no watched dimmer is lit.
//! times = ["20:00:25", "21:07:25"]
//! settle_secs = 100   # a frame lasts about 1 min 40
//! ```

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use moli_core::{Event, Origin, PointId, Value};
use moli_runtime::{BoxFuture, Driver, DriverCtx, Hub};
use serde::Deserialize;
use tokio::sync::broadcast::error::RecvError;

/// The instance.
pub const INSTANCE: &str = "trames";

/// No frame is read in a power change this soon after an order to a dimmer.
const QUIET_AFTER_ORDER: u64 = 60_000;
/// Nor this soon after a re-levelling (it moves the power itself).
const QUIET_AFTER_FIX: u64 = 30_000;
/// A second jump inside a frame pushes the re-levelling this far…
const EXTEND: u64 = 20_000;
/// …but never past this, counted from the first jump.
const CAP: u64 = 150_000;
/// A scheduled frame is looked at this long after it settled.
const SCHEDULE_MARGIN: u64 = 10_000;
/// A watched dimmer drawing at least this is lit (it would have seen the frame).
const LIT_WATTS: f64 = 0.5;
/// The step and back: time for the dimmer to take the step.
const STEP: Duration = Duration::from_secs(3);

/// `[ripple]` as `moli.toml` declares it.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub watch: Vec<String>,
    pub dimmers: Vec<String>,
    #[serde(default)]
    pub times: Vec<String>,
    #[serde(default = "default_settle")]
    pub settle_secs: u64,
}

fn default_settle() -> u64 {
    100
}

/// `[ripple]` checked.
pub fn check(config: &Config) -> Result<(), String> {
    if config.dimmers.is_empty() {
        return Err("dimmers is empty".into());
    }
    for p in config.watch.iter().chain(&config.dimmers) {
        if PointId::from(p.as_str()).split().is_none() {
            return Err(format!("{p:?} is not a point (device/key)"));
        }
    }
    for t in &config.times {
        t.parse::<jiff::civil::Time>()
            .map_err(|e| format!("time {t:?}: {e}"))?;
    }
    if !(30..=600).contains(&config.settle_secs) {
        return Err("settle_secs must be between 30 and 600".into());
    }
    Ok(())
}

/// When a frame went through and when to re-level. Pure: times in ms.
#[derive(Debug, Default)]
struct Detector {
    settle: u64,
    power: HashMap<PointId, f64>,
    levels: HashMap<PointId, f64>,
    last_order: u64,
    last_fix: u64,
    /// (first jump, re-level at)
    pending: Option<(u64, u64)>,
}

impl Detector {
    fn new(settle_secs: u64) -> Self {
        Self {
            settle: settle_secs * 1000,
            ..Self::default()
        }
    }

    /// A dimmer's level was reported: an order if it changed.
    fn level(&mut self, point: &PointId, value: f64, now: u64) {
        if self.levels.insert(point.clone(), value) != Some(value) {
            self.last_order = now;
        }
    }

    /// A watched power reading. True when it is a frame's jump.
    fn power(&mut self, point: &PointId, watts: f64, now: u64) -> bool {
        let before = self.power.insert(point.clone(), watts);
        let Some(before) = before else { return false };
        let jump = before >= LIT_WATTS && watts >= before * 1.5 && watts - before >= 2.0;
        if !jump
            || now.saturating_sub(self.last_order) < QUIET_AFTER_ORDER
            || now.saturating_sub(self.last_fix) < QUIET_AFTER_FIX
        {
            return false;
        }
        self.pending = Some(match self.pending {
            None => (now, now + self.settle),
            Some((first, due)) => (first, due.max(now + EXTEND).min(first + CAP)),
        });
        true
    }

    /// True once the frame is over: re-level now.
    fn due(&mut self, now: u64) -> bool {
        match self.pending {
            Some((_, due)) if now >= due => {
                self.pending = None;
                self.last_fix = now;
                true
            }
            _ => false,
        }
    }

    /// A known frame time `frame` (ms): true when nobody saw it and a
    /// re-levelling is still due for it.
    fn scheduled(&mut self, frame: u64, now: u64) -> bool {
        let seen = self.last_fix + QUIET_AFTER_ORDER >= frame || self.pending.is_some();
        let watched_lit = self.power.values().any(|w| *w >= LIT_WATTS);
        if seen || watched_lit {
            return false;
        }
        self.last_fix = now;
        true
    }

    fn fire_at(&self, frame: u64) -> u64 {
        frame + self.settle + SCHEDULE_MARGIN
    }
}

/// One step towards the middle, then back: what a dimmer upset by a frame
/// needs (the same level again is ignored).
fn step(level: f64) -> f64 {
    if level >= 2.0 {
        level - 1.0
    } else {
        level + 1.0
    }
}

/// The value in the point's own kind (integer stays integer).
fn like(model: &Value, v: f64) -> Value {
    match model {
        #[allow(clippy::cast_possible_truncation)]
        Value::Int(_) => Value::Int(v.round() as i64),
        _ => Value::Float(v),
    }
}

/// The level to re-set, if the dimmer is on. A Zigbee light keeps its last
/// brightness while off: its `state` point says whether it is lit, and
/// sending it a brightness would switch it on.
fn lit_level(level: &Value, state: Option<&Value>) -> Option<f64> {
    let off = match state {
        Some(Value::Bool(on)) => !on,
        Some(Value::Int(i)) => *i == 0,
        Some(Value::Text(t)) => t.eq_ignore_ascii_case("off"),
        _ => false,
    };
    level.as_f64().filter(|l| *l > 0.0 && !off)
}

/// The `state` point next to a level point (same device).
fn state_point(point: &PointId) -> Option<PointId> {
    let (device, _) = point.split()?;
    Some(PointId::new(&device, "state"))
}

/// Every dimmer that is on: one step and back.
async fn relevel(hub: Hub, dimmers: Vec<PointId>) {
    let actor = moli_i18n::tr!("pilotes.lights.trame_acteur");
    let runs = dimmers.into_iter().filter_map(|point| {
        let value = hub.state(&point)?.value;
        let state = state_point(&point)
            .and_then(|s| hub.state(&s))
            .map(|s| s.value);
        let level = lit_level(&value, state.as_ref())?;
        let (hub, actor) = (hub.clone(), actor.clone());
        Some(async move {
            let first = hub
                .command(
                    &point,
                    like(&value, step(level)),
                    Origin::System,
                    Some(actor.clone()),
                )
                .await;
            tokio::time::sleep(STEP).await;
            let back = hub
                .command(&point, like(&value, level), Origin::System, Some(actor))
                .await;
            if let Err(e) = first.and(back) {
                tracing::warn!(%point, error = %e, "ripple: re-levelling failed");
            } else {
                tracing::info!(%point, level, "ripple: re-levelled");
            }
        })
    });
    futures::future::join_all(runs).await;
}

/// The driver.
#[derive(Debug)]
pub struct Ripple {
    hub: Hub,
    config: Config,
    tz: jiff::tz::TimeZone,
}

impl Ripple {
    /// `timezone`: the house's (`[automations]`), for `times`.
    #[must_use]
    pub fn new(hub: Hub, config: Config, timezone: &str) -> Self {
        let tz = jiff::tz::TimeZone::get(timezone).unwrap_or(jiff::tz::TimeZone::UTC);
        Self { hub, config, tz }
    }

    /// Today's frame times, in ms.
    fn frames_today(&self) -> Vec<u64> {
        let today = jiff::Timestamp::now().to_zoned(self.tz.clone()).date();
        self.config
            .times
            .iter()
            .filter_map(|t| t.parse::<jiff::civil::Time>().ok())
            .filter_map(|t| today.to_datetime(t).to_zoned(self.tz.clone()).ok())
            .filter_map(|z| u64::try_from(z.timestamp().as_millisecond()).ok())
            .collect()
    }
}

impl Driver for Ripple {
    fn kind(&self) -> &'static str {
        "ripple"
    }

    fn run<'a>(&'a self, ctx: &'a mut DriverCtx) -> BoxFuture<'a, anyhow::Result<()>> {
        Box::pin(async move {
            let mut events = self.hub.subscribe();
            let watch: HashSet<PointId> = self
                .config
                .watch
                .iter()
                .map(|p| PointId::from(p.as_str()))
                .collect();
            let dimmers: Vec<PointId> = self
                .config
                .dimmers
                .iter()
                .map(|p| PointId::from(p.as_str()))
                .collect();
            let mut detector = Detector::new(self.config.settle_secs);
            for p in &dimmers {
                if let Some(level) = self.hub.state(p).and_then(|s| s.value.as_f64()) {
                    detector.levels.insert(p.clone(), level);
                }
            }
            for p in &watch {
                if let Some(w) = self.hub.state(p).and_then(|s| s.value.as_f64()) {
                    detector.power.insert(p.clone(), w);
                }
            }
            let mut fired: HashSet<u64> = HashSet::new();
            let mut tick = tokio::time::interval(Duration::from_secs(1));
            ctx.ready();
            loop {
                tokio::select! {
                    command = ctx.next_command() => match command {
                        None => return Ok(()),
                        Some(c) => c.reply(Err(moli_i18n::tr!("pilotes.lights.ordre_inconnu"))),
                    },
                    event = events.recv() => match event {
                        Err(RecvError::Closed) => return Ok(()),
                        Ok(Event::State { point, value, .. }) => {
                            let now = moli_core::now_ms();
                            let Some(v) = value.as_f64() else { continue };
                            if watch.contains(&point) {
                                if detector.power(&point, v, now) {
                                    tracing::info!(%point, watts = v, "ripple: frame seen");
                                }
                            } else if dimmers.contains(&point) {
                                detector.level(&point, v, now);
                            }
                        }
                        Ok(_) | Err(RecvError::Lagged(_)) => {}
                    },
                    _ = tick.tick() => {
                        let now = moli_core::now_ms();
                        let mut fix = detector.due(now);
                        for frame in self.frames_today() {
                            let at = detector.fire_at(frame);
                            if now >= at && now < at + 30_000 && fired.insert(at) {
                                fix |= detector.scheduled(frame, now);
                            }
                        }
                        fired.retain(|at| now < at + 86_400_000);
                        if fix {
                            tokio::spawn(relevel(self.hub.clone(), dimmers.clone()));
                        }
                    }
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> PointId {
        PointId::from(s)
    }

    #[test]
    fn a_jump_with_no_order_is_a_frame_relevelled_once_settled() {
        let mut d = Detector::new(100);
        let w = p("zwave:x-1/power");
        assert!(!d.power(&w, 3.0, 100_000));
        assert!(d.power(&w, 9.2, 110_000));
        assert!(!d.due(150_000));
        assert!(d.power(&w, 11.5, 120_000) || d.pending.is_some());
        assert!(d.due(210_000));
        assert!(!d.due(211_000));
    }

    #[test]
    fn a_jump_right_after_an_order_is_not_a_frame() {
        let mut d = Detector::new(100);
        let (w, l) = (p("zwave:x-1/power"), p("zwave:x-1/level"));
        d.level(&l, 0.0, 0);
        d.power(&w, 3.0, 0);
        d.level(&l, 10.0, 100_000);
        assert!(!d.power(&w, 9.0, 105_000));
        // The same level reported again is not an order.
        d.level(&l, 10.0, 200_000);
        assert!(d.power(&w, 14.0, 200_000));
    }

    #[test]
    fn small_moves_and_switching_on_are_not_frames() {
        let mut d = Detector::new(100);
        let w = p("zwave:x-1/power");
        d.power(&w, 0.0, 0);
        assert!(!d.power(&w, 30.0, 100_000));
        d.power(&w, 3.0, 200_000);
        assert!(!d.power(&w, 4.4, 210_000));
    }

    #[test]
    fn a_second_jump_pushes_the_fix_within_the_cap() {
        let mut d = Detector::new(100);
        let w = p("zwave:x-1/power");
        d.power(&w, 3.0, 100_000);
        d.power(&w, 9.0, 100_000 + 61_000);
        d.power(&w, 4.0, 161_000 + 80_000);
        d.power(&w, 9.0, 161_000 + 95_000);
        assert_eq!(d.pending, Some((161_000, 161_000 + 115_000)));
    }

    #[test]
    fn a_scheduled_frame_is_fixed_only_when_nobody_watched_it() {
        let mut d = Detector::new(100);
        assert!(d.scheduled(1_000_000, 1_110_000));
        let mut d = Detector::new(100);
        d.power.insert(p("zwave:x-1/power"), 3.0);
        assert!(!d.scheduled(1_000_000, 1_110_000));
        let mut d = Detector::new(100);
        d.last_fix = 1_100_000;
        assert!(!d.scheduled(1_000_000, 1_110_000));
    }

    #[test]
    fn a_step_stays_in_range_and_keeps_the_kind() {
        assert!((step(1.0) - 2.0).abs() < f64::EPSILON);
        assert!((step(99.0) - 98.0).abs() < f64::EPSILON);
        assert_eq!(like(&Value::Int(5), 4.0), Value::Int(4));
        assert_eq!(like(&Value::Float(5.0), 4.0), Value::Float(4.0));
    }

    #[test]
    fn a_zigbee_light_that_is_off_is_left_alone() {
        let b = Value::Int(1);
        assert_eq!(lit_level(&b, Some(&Value::Bool(false))), None);
        assert_eq!(lit_level(&b, Some(&Value::Text("OFF".into()))), None);
        assert_eq!(lit_level(&b, Some(&Value::Bool(true))), Some(1.0));
        assert_eq!(lit_level(&b, None), Some(1.0));
        assert_eq!(lit_level(&Value::Int(0), None), None);
        assert_eq!(
            state_point(&p("z2m:0x60/brightness")),
            Some(p("z2m:0x60/state"))
        );
    }

    #[test]
    fn config_is_checked() {
        let c: Config = toml::from_str(
            r#"dimmers = ["zwave:x-1/level"]
               times = ["20:00:25"]"#,
        )
        .unwrap();
        assert!(check(&c).is_ok());
        let c: Config = toml::from_str(
            r#"dimmers = ["zwave:x-1/level"]
            times = ["25:00"]"#,
        )
        .unwrap();
        assert!(check(&c).is_err());
    }
}
