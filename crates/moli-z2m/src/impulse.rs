//! Shutters whose motor runs on impulses (Bubendorff and the like).
//!
//! Such a motor starts on a short impulse on its up or down wire, runs to
//! its end stop on its own, and stops on the next impulse, whatever its
//! direction. Cutting the power does nothing. A roller shutter module
//! (MINI-ZBRBS…) drives it fine from the wall buttons, but its own
//! `position` and `STOP` are meaningless: Moli sends the impulses itself
//! (`OPEN`/`CLOSE`, then `STOP` to release) and times the travel to know
//! where the shutter is.
//!
//! A module with two independent relays (`relays`) fits such a motor better:
//! each relay feeds one wire, follows its wall button and opens again by
//! itself after a short time (its own auto-off, set in the module, which
//! `pulse` must match). Moli only closes the relay of the wanted way.
//!
//! Each such device shows `control` (open, stop, close), `percent_control`
//! (how high the bottom of the shutter is: 0 on the sill … 100 open) and
//! `slats_open` (on the sill, the slats pulled apart: light comes through)
//! instead of the module's `state`, `position` and `moving`. Pure state
//! machine; time comes in.

use std::time::{Duration, Instant};

use moli_core::{Access, Device, Kind, PointSpec, Semantic, Unit, Value};
use serde::Deserialize;

/// `[driver.options.impulse_covers."0x…"]`: one shutter, by IEEE address.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CoverConfig {
    /// Seconds from fully closed to fully open. This and the three other
    /// durations are where a new shutter starts: then they are set from the
    /// dashboard (`TIMINGS`) and the value set there is the one kept.
    #[serde(default = "default_travel")]
    pub travel_up: f64,
    /// Seconds from fully open to fully closed.
    #[serde(default = "default_travel")]
    pub travel_down: f64,
    /// Seconds the module powers the wire for one impulse.
    #[serde(default = "default_pulse")]
    pub pulse: f64,
    /// Seconds, at the start of a rise from closed, spent pulling the slats
    /// apart (light shows between them, the bottom has not left the sill).
    #[serde(default)]
    pub slack_up: f64,
    /// Seconds, at the end of a descent, spent pressing the slats together
    /// once the bottom is on the sill.
    #[serde(default)]
    pub slack_down: f64,
    /// The module's relays, for a module with one relay per wire.
    #[serde(default)]
    pub relays: Option<Relays>,
}

/// Which relay feeds which wire: the suffix of the module's `state_…` points
/// (`l1`, `l2`).
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Relays {
    pub up: String,
    pub down: String,
}

impl Relays {
    /// The relays' own points and the wall inputs' settings: closing a relay by
    /// hand could feed both wires, and no motor current goes through them.
    fn hides(&self, key: &str) -> bool {
        key.starts_with("switch_type")
            || key.starts_with("switch_mode")
            || [&self.up, &self.down].iter().any(|relay| {
                key.strip_suffix(relay.as_str())
                    .is_some_and(|k| k.ends_with('_'))
            })
    }

    /// The way of the wall button a module's `action` tells of: input 1 goes
    /// with relay `l1`. Its end (`input_1_off`) is no new press.
    fn pressed(&self, action: &str) -> Option<Dir> {
        let (input, what) = action.strip_prefix("input_")?.split_once('_')?;
        if !matches!(what, "single" | "double" | "triple" | "hold") {
            return None;
        }
        match input {
            n if self.up.strip_prefix('l') == Some(n) => Some(Dir::Up),
            n if self.down.strip_prefix('l') == Some(n) => Some(Dir::Down),
            _ => None,
        }
    }
}

fn default_pulse() -> f64 {
    1.0
}

fn default_travel() -> f64 {
    30.0
}

/// The durations set from the dashboard: key, shortest, longest (seconds).
pub const TIMINGS: [(&str, f64, f64); 4] = [
    ("travel_up", 3.0, 180.0),
    ("travel_down", 3.0, 180.0),
    ("slack_up", 0.0, 30.0),
    ("slack_down", 0.0, 30.0),
];

/// A number, however it came.
#[must_use]
pub fn number(value: &Value) -> Option<f64> {
    #[allow(clippy::cast_precision_loss)]
    match value {
        Value::Int(i) => Some(*i as f64),
        Value::Float(f) => Some(*f),
        _ => None,
    }
}

/// Pause between the end of one impulse and the next: the module must see
/// a fresh order (wall buttons 0.55 s apart were both taken).
const GAP: Duration = Duration::from_millis(600);
/// How long after one of our impulses the module's report of it may come.
const ECHO: Duration = Duration::from_millis(1500);
/// The same with one relay per wire, where a late report is told from a wall
/// button by the input's own event. The radio may hold an order this long.
/// Until then no other impulse leaves: two late orders arriving together would
/// close both relays.
const RELAY_ECHO: Duration = Duration::from_secs(30);
/// How long after the relay closed a wall button's input event may come.
const INPUT_LAG: Duration = Duration::from_secs(3);
/// How long after one of our impulses arrived the radio may hand over its
/// report once more (seen 2.6 s later, after the relay had opened again). A
/// closing of the same relay within this time counts only with the wall
/// button's input event.
const REPEAT: Duration = Duration::from_secs(10);
/// A requested position this close to the current one is not worth a move.
const NEAR: f64 = 2.0;

pub const CONTROL: &str = "control";
pub const PERCENT: &str = "percent_control";
pub const SLATS: &str = "slats_open";
/// Module points that make no sense with such a motor.
const HIDDEN: [&str; 3] = ["state", "position", "moving"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dir {
    Up,
    Down,
}

impl Dir {
    /// The module's `state` value that powers this wire.
    #[must_use]
    pub fn wire(self) -> &'static str {
        match self {
            Self::Up => "OPEN",
            Self::Down => "CLOSE",
        }
    }

    fn end(self) -> Spot {
        match self {
            Self::Up => Spot {
                height: 100.0,
                slack: 1.0,
            },
            Self::Down => Spot {
                height: 0.0,
                slack: 0.0,
            },
        }
    }
}

/// Where the shutter is: how high its bottom is (0 … 100) and how far
/// apart its slats are (0 pressed together … 1 pulled apart; always 1 once
/// the bottom has left the sill).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Spot {
    pub height: f64,
    pub slack: f64,
}

impl Spot {
    /// A height alone (restored after a restart): slats pressed together
    /// on the sill unless said otherwise.
    #[must_use]
    pub fn at(height: f64, slats_open: bool) -> Self {
        let height = height.clamp(0.0, 100.0);
        Self {
            height,
            slack: if height > 0.0 || slats_open { 1.0 } else { 0.0 },
        }
    }

    fn slats_open(self) -> bool {
        self.height < 0.5 && self.slack >= 0.5
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Step {
    /// Start moving this way (skipped if already moving).
    Start(Dir),
    /// Stop (skipped if the motor already stopped on its own).
    Stop,
    /// End of an impulse: the module stops powering the wire.
    Release,
}

/// One of our impulses, until the module says it powers the wire: then the
/// timing is set on that moment, when the motor really got it.
#[derive(Clone, Copy, Debug)]
struct Echo {
    dir: Dir,
    sent: Instant,
    /// The impulse started the motor (else it stopped it).
    started: bool,
}

#[derive(Clone, Copy, Debug)]
struct Motion {
    dir: Dir,
    since: Instant,
    from: Option<Spot>,
}

#[derive(Debug)]
pub struct Cover {
    config: CoverConfig,
    /// Known position while still; `None` until a full travel.
    pos: Option<Spot>,
    motion: Option<Motion>,
    /// Where a `percent_control` order wants to stop.
    target: Option<f64>,
    pending: Vec<(Instant, Step)>,
    /// No impulse before this (the module must see a fresh order).
    ready_at: Instant,
    /// Our last impulse, not yet seen by the module.
    echo: Option<Echo>,
    /// The relay closing last taken for our impulse, and when: the wall
    /// button's input event, should one follow, corrects that.
    ours: Option<(Echo, Instant)>,
    /// A closing of the relay of our last impulse, right after it: the same
    /// report again, unless the wall button's input event follows.
    repeat: Option<(Dir, Instant)>,
    module_moving: bool,
    /// Last position published, and whether the slats were shown open.
    shown: Option<i64>,
    shown_slats: Option<bool>,
}

impl Cover {
    #[must_use]
    pub fn new(config: CoverConfig, pos: Option<Spot>, now: Instant) -> Self {
        Self {
            config,
            pos,
            motion: None,
            target: None,
            pending: Vec::new(),
            ready_at: now,
            echo: None,
            ours: None,
            repeat: None,
            module_moving: false,
            shown: None,
            shown_slats: None,
        }
    }

    /// One of `TIMINGS`, as it is now.
    #[must_use]
    pub fn timing(&self, key: &str) -> Option<f64> {
        match key {
            "travel_up" => Some(self.config.travel_up),
            "travel_down" => Some(self.config.travel_down),
            "slack_up" => Some(self.config.slack_up),
            "slack_down" => Some(self.config.slack_down),
            _ => None,
        }
    }

    /// Sets one of `TIMINGS`. A move under way follows the new duration.
    pub fn set_timing(&mut self, key: &str, value: &Value) -> Result<(), String> {
        let secs = number(value)
            .filter(|secs| {
                TIMINGS
                    .iter()
                    .any(|(k, lo, hi)| *k == key && (*lo..=*hi).contains(secs))
            })
            .ok_or_else(|| moli_i18n::tr!("pilotes.z2m.volet.duree_attendue"))?;
        match key {
            "travel_up" => self.config.travel_up = secs,
            "travel_down" => self.config.travel_down = secs,
            "slack_up" => self.config.slack_up = secs,
            _ => self.config.slack_down = secs,
        }
        Ok(())
    }

    /// Seconds spent on the slats, and on the height, for a full travel.
    fn legs(&self, dir: Dir) -> (f64, f64) {
        let (travel, slack) = match dir {
            Dir::Up => (self.config.travel_up, self.config.slack_up),
            Dir::Down => (self.config.travel_down, self.config.slack_down),
        };
        let travel = travel.max(1.0);
        let slack = slack.clamp(0.0, travel * 0.8);
        (slack, travel - slack)
    }

    /// Where a shutter at `from` is after `secs` of travel. Rising, the
    /// slats pull apart before the bottom leaves the sill; going down, they
    /// press together once it is back on it.
    fn advance(&self, from: Spot, dir: Dir, secs: f64) -> Spot {
        let (slack, height) = self.legs(dir);
        match dir {
            Dir::Up => {
                let apart = (1.0 - from.slack) * slack;
                if secs < apart {
                    Spot {
                        height: from.height,
                        slack: from.slack + secs / slack,
                    }
                } else {
                    let h = from.height + (secs - apart) / height * 100.0;
                    if h >= 100.0 {
                        dir.end()
                    } else {
                        Spot {
                            height: h,
                            slack: 1.0,
                        }
                    }
                }
            }
            Dir::Down => {
                let down = from.height / 100.0 * height;
                if secs < down {
                    Spot {
                        height: from.height - secs / height * 100.0,
                        slack: from.slack,
                    }
                } else {
                    let left = if slack > 0.0 {
                        from.slack - (secs - down) / slack
                    } else {
                        0.0
                    };
                    if left <= 0.0 {
                        dir.end()
                    } else {
                        Spot {
                            height: 0.0,
                            slack: left,
                        }
                    }
                }
            }
        }
    }

    /// Seconds of travel from `from` until the bottom is at `height`.
    fn secs_to(&self, from: Spot, dir: Dir, height: f64) -> f64 {
        let (slack, leg) = self.legs(dir);
        match dir {
            Dir::Up => (1.0 - from.slack) * slack + (height - from.height).max(0.0) / 100.0 * leg,
            Dir::Down => (from.height - height).max(0.0) / 100.0 * leg,
        }
    }

    /// Estimated position, `None` when unknown.
    #[must_use]
    pub fn position(&self, now: Instant) -> Option<Spot> {
        let Some(m) = self.motion else {
            return self.pos;
        };
        let secs = now.saturating_duration_since(m.since).as_secs_f64();
        if let Some(from) = m.from {
            return Some(self.advance(from, m.dir, secs));
        }
        // Unknown start: known only once a full travel is behind.
        let (slack, leg) = self.legs(m.dir);
        (secs >= slack + leg).then(|| m.dir.end())
    }

    #[must_use]
    pub fn moving(&self) -> bool {
        self.motion.is_some()
    }

    /// The motor reaching its end stop on its own.
    fn settle(&mut self, now: Instant) {
        if let Some(m) = self.motion
            && self.position(now) == Some(m.dir.end())
        {
            self.motion = None;
            self.pos = Some(m.dir.end());
            self.target = None;
        }
    }

    /// One impulse reaching the motor: it stops if moving, starts if not.
    fn impulse(&mut self, dir: Dir, now: Instant) {
        self.settle(now);
        if self.motion.is_some() {
            self.pos = self.position(now);
            self.motion = None;
        } else {
            self.motion = Some(Motion {
                dir,
                since: now,
                from: self.pos,
            });
        }
    }

    /// An order from Moli. Returns an error when it cannot be honoured.
    pub fn order(&mut self, key: &str, value: &Value, now: Instant) -> Result<(), String> {
        self.settle(now);
        self.pending.clear();
        self.target = None;
        match (key, value) {
            (CONTROL, Value::Text(t)) if &**t == "open" => self.go(Dir::Up, now),
            (CONTROL, Value::Text(t)) if &**t == "close" => self.go(Dir::Down, now),
            (CONTROL, Value::Text(t)) if &**t == "stop" => {
                if self.motion.is_some() {
                    self.pending.push((now, Step::Stop));
                }
            }
            (PERCENT, v) => {
                let target = number(v)
                    .ok_or_else(|| moli_i18n::tr!("pilotes.z2m.volet.position_attendue"))?
                    .clamp(0.0, 100.0);
                if target >= 100.0 - NEAR {
                    self.go(Dir::Up, now);
                } else if target <= NEAR {
                    self.go(Dir::Down, now);
                } else {
                    let Some(pos) = self.position(now) else {
                        return Err(moli_i18n::tr!("pilotes.z2m.volet.position_inconnue"));
                    };
                    if (target - pos.height).abs() <= NEAR && self.motion.is_none() {
                        return Ok(());
                    }
                    let dir = if target > pos.height {
                        Dir::Up
                    } else {
                        Dir::Down
                    };
                    self.target = Some(target);
                    self.go(dir, now);
                }
            }
            _ => {
                return Err(moli_i18n::tr!(
                    "pilotes.z2m.volet.commande_inconnue",
                    key = key
                ));
            }
        }
        Ok(())
    }

    /// Head for an end (or `self.target`), reversing first if needed.
    fn go(&mut self, dir: Dir, now: Instant) {
        match self.motion {
            Some(m) if m.dir == dir => {
                // Already on its way: only the stop changes.
                if let Some(at) = self.stop_time(m.since, dir) {
                    self.pending.push((at, Step::Stop));
                }
            }
            Some(_) => {
                self.pending.push((now, Step::Stop));
                self.pending.push((now, Step::Start(dir)));
            }
            None => self.pending.push((now, Step::Start(dir))),
        }
    }

    /// When to send the stop impulse for `self.target`, for a move that
    /// started at `since`.
    fn stop_time(&self, since: Instant, dir: Dir) -> Option<Instant> {
        let target = self.target?;
        let from = self.motion.map_or(self.pos, |m| m.from).or(self.pos)?;
        Some(since + Duration::from_secs_f64(self.secs_to(from, dir, target)))
    }

    /// How long a report of our impulse is waited for.
    fn echo_window(&self) -> Duration {
        if self.config.relays.is_some() {
            RELAY_ECHO
        } else {
            ECHO
        }
    }

    /// Seconds the module keeps a wire powered, at the least.
    fn pulse(&self) -> Duration {
        Duration::from_secs_f64(self.config.pulse.max(0.3))
    }

    /// No fresh impulse before this: the module must see a new order, and a
    /// relay's impulse must have arrived and ended before another leaves.
    fn gate(&self) -> Instant {
        match self.echo {
            Some(echo) if self.config.relays.is_some() => self.ready_at.max(echo.sent + RELAY_ECHO),
            _ => self.ready_at,
        }
    }

    /// The next moment `fire` has something to do.
    #[must_use]
    pub fn deadline(&self) -> Option<Instant> {
        let gate = self.gate();
        self.pending
            .iter()
            .map(|(at, step)| {
                if matches!(step, Step::Release) {
                    *at
                } else {
                    (*at).max(gate)
                }
            })
            .min()
    }

    /// Runs what is due. Returns the module `state` values to publish.
    pub fn fire(&mut self, now: Instant) -> Vec<&'static str> {
        self.settle(now);
        let mut out = Vec::new();
        loop {
            // A fresh impulse waits until the module can take it.
            let open = now >= self.gate();
            let Some(i) = (0..self.pending.len())
                .filter(|&i| {
                    self.pending[i].0 <= now && (open || matches!(self.pending[i].1, Step::Release))
                })
                .min_by_key(|&i| self.pending[i].0)
            else {
                break;
            };
            let step = self.pending[i].1;
            self.pending.remove(i);
            let dir = match step {
                Step::Release => {
                    out.push("STOP");
                    self.ready_at = now + GAP;
                    continue;
                }
                Step::Stop => match self.motion {
                    Some(m) => m.dir,
                    None => continue,
                },
                Step::Start(dir) => match self.motion {
                    Some(_) => continue,
                    None => dir,
                },
            };
            out.push(dir.wire());
            self.impulse(dir, now);
            let pulse = self.pulse();
            // A relay module ends the impulse by itself.
            if self.config.relays.is_none() {
                self.pending.push((now + pulse, Step::Release));
            }
            self.ready_at = now + pulse + GAP;
            self.echo = Some(Echo {
                dir,
                sent: now,
                started: self.motion.is_some(),
            });
            if matches!(step, Step::Start(_))
                && let Some(at) = self.stop_time(now, dir)
            {
                self.pending.push((at, Step::Stop));
            }
        }
        out
    }

    /// What to publish for one value returned by `fire`.
    #[must_use]
    pub fn payload(&self, wire: &str) -> serde_json::Value {
        let relay = self.config.relays.as_ref().and_then(|relays| match wire {
            "OPEN" => Some(&relays.up),
            "CLOSE" => Some(&relays.down),
            _ => None,
        });
        match relay {
            Some(relay) => serde_json::json!({ format!("state_{relay}"): "ON" }),
            None => serde_json::json!({ "state": wire }),
        }
    }

    /// A state message from the module.
    pub fn on_report(
        &mut self,
        payload: &serde_json::Map<String, serde_json::Value>,
        now: Instant,
    ) {
        let Some(relays) = &self.config.relays else {
            if let Some(moving) = payload.get("moving").and_then(|m| m.as_str()) {
                self.on_module_moving(moving, now);
            }
            return;
        };
        let pressed = payload
            .get("action")
            .and_then(|a| a.as_str())
            .and_then(|a| relays.pressed(a));
        let state = |relay: &str| payload.get(&format!("state_{relay}"))?.as_str();
        let (up, down) = (state(&relays.up), state(&relays.down));
        let moving = if up == Some("ON") {
            Some("forward")
        } else if down == Some("ON") {
            Some("reverse")
        } else if up.is_some() || down.is_some() {
            Some("stop")
        } else {
            None
        };
        if let Some(moving) = moving {
            self.on_module_moving(moving, now);
        }
        if let Some(dir) = pressed {
            self.on_input(dir, now);
        }
    }

    /// A wall button's own event, a little after its relay closed. Moli's
    /// impulses never cause one. Taken for ours, that relay was a hand: ours
    /// is still on its way, and counts when it comes.
    fn on_input(&mut self, dir: Dir, now: Instant) {
        if let Some((way, closed)) = self.repeat.take()
            && way == dir
            && now.saturating_duration_since(closed) <= INPUT_LAG
        {
            // Not a repeated report after all: a hand on the wall.
            self.pending.clear();
            self.target = None;
            self.impulse(dir, closed);
            tracing::debug!(?dir, "wall button right after our impulse");
            return;
        }
        let Some((echo, closed)) = self.ours.take() else {
            return;
        };
        if echo.dir != dir || now.saturating_duration_since(closed) > INPUT_LAG {
            return;
        }
        self.echo = Some(echo);
        self.pending.clear();
        self.target = None;
        self.impulse(dir, closed);
        tracing::debug!(?dir, "wall button, first taken for our impulse");
    }

    /// The module's own `moving` report. The module starting to power a
    /// wire is either one of our impulses arriving (the timing is moved to
    /// that moment) or a wall button: one button, or both at once (the
    /// module then picks one way), either way one impulse to the motor.
    pub fn on_module_moving(&mut self, moving: &str, now: Instant) {
        let dir = match moving {
            "forward" => Some(Dir::Up),
            "reverse" => Some(Dir::Down),
            _ => None,
        };
        let started = dir.is_some() && !self.module_moving;
        self.module_moving = dir.is_some();
        let Some(dir) = dir.filter(|_| started) else {
            return;
        };
        // The relay is closed from now, however late the order came.
        if self.config.relays.is_some() {
            self.ready_at = self.ready_at.max(now + self.pulse() + GAP);
        }
        let window = self.echo_window();
        let last = self.ours.take();
        self.repeat = None;
        match self.echo.take() {
            Some(echo) if echo.dir == dir && now.saturating_duration_since(echo.sent) <= window => {
                self.arrived(echo, now);
                self.ours = Some((echo, now));
            }
            echo => {
                if echo.is_some_and(|e| now.saturating_duration_since(e.sent) <= window) {
                    self.echo = echo;
                }
                // The relay of our impulse closing again just after: most
                // likely the radio repeating its report. Counted only if the
                // wall button's input event follows.
                if let Some((ours, arrived)) = last
                    && self.config.relays.is_some()
                    && ours.dir == dir
                    && now.saturating_duration_since(arrived) <= REPEAT
                {
                    self.ours = Some((ours, arrived));
                    self.repeat = Some((dir, now));
                    tracing::debug!(?dir, "relay report repeated, wall button awaited");
                    return;
                }
                // Not ours (or ours went missing): a hand on the wall.
                self.pending.clear();
                self.target = None;
                self.impulse(dir, now);
                tracing::debug!(?dir, "wall button impulse");
            }
        }
    }

    /// One of our impulses reached the motor `now`, a little after it was
    /// sent: the move it started began later, the one it stopped ended later.
    fn arrived(&mut self, echo: Echo, now: Instant) {
        let lag = now
            .saturating_duration_since(echo.sent)
            .min(self.echo_window());
        match (echo.started, self.motion.as_mut()) {
            // The stop impulse will be as late as this one was: the travel
            // itself keeps its length, only where it is shown moves.
            (true, Some(m)) if m.since == echo.sent => m.since += lag,
            (false, None) => {
                self.pos = self
                    .pos
                    .map(|pos| self.advance(pos, echo.dir, lag.as_secs_f64()));
            }
            _ => {}
        }
    }

    /// The height to publish, when it changed (whole percent).
    pub fn shown_change(&mut self, now: Instant) -> Option<i64> {
        self.settle(now);
        #[allow(clippy::cast_possible_truncation)]
        let now_shown = self.position(now).map(|p| p.height.round() as i64);
        if now_shown.is_some() && now_shown != self.shown {
            self.shown = now_shown;
            return now_shown;
        }
        None
    }

    /// Whether the slats show open, when that changed.
    pub fn slats_change(&mut self, now: Instant) -> Option<bool> {
        let open = self.position(now).map(Spot::slats_open);
        if open.is_some() && open != self.shown_slats {
            self.shown_slats = open;
            return open;
        }
        None
    }
}

/// Replaces the module's position points with the impulse cover's.
pub fn reshape(device: &mut Device, config: &CoverConfig) {
    device.points.retain(|p| {
        !HIDDEN.contains(&&*p.key) && !config.relays.as_ref().is_some_and(|r| r.hides(&p.key))
    });
    for (key, lo, hi) in TIMINGS {
        device.points.push(PointSpec {
            key: key.into(),
            label: match key {
                "travel_up" => moli_i18n::tr!("pilotes.z2m.volet.duree_montee"),
                "travel_down" => moli_i18n::tr!("pilotes.z2m.volet.duree_descente"),
                "slack_up" => moli_i18n::tr!("pilotes.z2m.volet.lames_montee"),
                _ => moli_i18n::tr!("pilotes.z2m.volet.lames_descente"),
            }
            .into(),
            kind: Kind::Numeric {
                min: Some(lo),
                max: Some(hi),
                step: Some(0.5),
            },
            access: Access {
                read: true,
                write: true,
            },
            unit: Some(Unit::Second),
            semantic: Semantic::Config,
        });
    }
    device.points.push(PointSpec {
        key: CONTROL.into(),
        label: moli_i18n::tr!("pilotes.z2m.volet.commande").into(),
        kind: Kind::Enum {
            values: vec!["open".into(), "stop".into(), "close".into()],
        },
        access: Access {
            read: false,
            write: true,
        },
        unit: None,
        semantic: Semantic::Other,
    });
    device.points.push(PointSpec {
        key: SLATS.into(),
        label: moli_i18n::tr!("pilotes.z2m.volet.ajoure").into(),
        kind: Kind::Binary,
        access: Access {
            read: true,
            write: false,
        },
        unit: None,
        semantic: Semantic::Other,
    });
    device.points.push(PointSpec {
        key: PERCENT.into(),
        label: moli_i18n::tr!("pilotes.z2m.volet.ouverture").into(),
        kind: Kind::Numeric {
            min: Some(0.0),
            max: Some(100.0),
            step: Some(1.0),
        },
        access: Access {
            read: true,
            write: true,
        },
        unit: Some(Unit::Percent),
        semantic: Semantic::Other,
    });
}

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use super::*;

    fn cover(pos: Option<f64>, t0: Instant) -> Cover {
        Cover::new(
            CoverConfig {
                travel_up: 40.0,
                travel_down: 32.0,
                pulse: 1.5,
                slack_up: 0.0,
                slack_down: 0.0,
                relays: None,
            },
            pos.map(|h| Spot::at(h, false)),
            t0,
        )
    }

    fn height(c: &Cover, at: Instant) -> Option<f64> {
        c.position(at).map(|p| p.height)
    }

    fn secs(t0: Instant, s: f64) -> Instant {
        t0 + Duration::from_secs_f64(s)
    }

    fn text(s: &str) -> Value {
        Value::Text(s.into())
    }

    /// Fires everything due up to `until`, step by step, with the times.
    fn run(c: &mut Cover, until: Instant) -> Vec<(f64, &'static str)> {
        let start = c.ready_at;
        let mut out = Vec::new();
        while let Some(at) = c.deadline().filter(|at| *at <= until) {
            for wire in c.fire(at) {
                out.push(((at - start).as_secs_f64(), wire));
            }
        }
        out
    }

    fn relay_cover(pos: Option<f64>, t0: Instant) -> Cover {
        Cover::new(
            CoverConfig {
                travel_up: 40.0,
                travel_down: 32.0,
                pulse: 0.5,
                slack_up: 0.0,
                slack_down: 0.0,
                relays: Some(Relays {
                    up: "l1".into(),
                    down: "l2".into(),
                }),
            },
            pos.map(|h| Spot::at(h, false)),
            t0,
        )
    }

    fn relays_report(l1: &str, l2: &str) -> serde_json::Map<String, serde_json::Value> {
        serde_json::json!({ "state_l1": l1, "state_l2": l2, "power_l1": 0 })
            .as_object()
            .unwrap()
            .clone()
    }

    #[test]
    fn a_relay_module_gets_one_order_and_ends_the_impulse_itself() {
        let t0 = Instant::now();
        let mut c = relay_cover(Some(100.0), t0);
        c.order(PERCENT, &Value::Int(50), t0).unwrap();
        // No release: closing the relay is all it takes, twice the same way.
        assert_eq!(c.fire(t0), ["CLOSE"]);
        c.on_report(&relays_report("OFF", "ON"), t0);
        c.on_report(&relays_report("OFF", "OFF"), secs(t0, 0.5));
        assert_eq!(c.deadline(), Some(secs(t0, 16.0)));
        assert_eq!(c.fire(secs(t0, 16.0)), ["CLOSE"]);
        c.on_report(&relays_report("OFF", "ON"), secs(t0, 16.0));
        assert_eq!(c.payload("CLOSE"), serde_json::json!({"state_l2": "ON"}));
        assert_eq!(c.payload("OPEN"), serde_json::json!({"state_l1": "ON"}));
        assert!(!c.moving());
    }

    #[test]
    fn reversing_never_closes_both_relays_together() {
        let t0 = Instant::now();
        let mut c = relay_cover(Some(0.0), t0);
        c.order(CONTROL, &text("open"), t0).unwrap();
        c.fire(t0);
        c.on_report(&relays_report("ON", "OFF"), t0);
        c.on_report(&relays_report("OFF", "OFF"), secs(t0, 0.5));
        let t1 = secs(t0, 10.0);
        c.order(CONTROL, &text("close"), t1).unwrap();
        // The stop goes on the relay of the way it is moving…
        assert_eq!(c.fire(t1), ["OPEN"]);
        // …and the other relay waits until that one has opened again.
        c.on_report(&relays_report("ON", "OFF"), t1);
        assert!(c.fire(secs(t1, 0.5)).is_empty());
        let at = c.deadline().unwrap();
        assert!(at >= secs(t1, 1.1));
        assert_eq!(c.fire(at), ["CLOSE"]);
    }

    fn action(name: &str) -> serde_json::Map<String, serde_json::Value> {
        serde_json::json!({ "action": name })
            .as_object()
            .unwrap()
            .clone()
    }

    #[test]
    fn a_late_order_is_not_taken_for_a_wall_button() {
        let t0 = Instant::now();
        let mut c = relay_cover(Some(0.0), t0);
        c.order(CONTROL, &text("open"), t0).unwrap();
        c.fire(t0);
        // 1.83 s on a busy network: still ours, and the travel starts then.
        c.on_report(&relays_report("ON", "OFF"), secs(t0, 1.83));
        c.on_report(&relays_report("OFF", "OFF"), secs(t0, 2.33));
        assert!(c.moving());
        assert_eq!(height(&c, secs(t0, 21.83)), Some(50.0));
        // So reversing starts with a stop, not with a lone impulse down.
        let t1 = secs(t0, 15.0);
        c.order(CONTROL, &text("close"), t1).unwrap();
        assert_eq!(c.fire(t1), ["OPEN"]);
    }

    #[test]
    fn a_very_late_stop_holds_back_the_other_relay() {
        let t0 = Instant::now();
        let mut c = relay_cover(Some(0.0), t0);
        c.order(CONTROL, &text("open"), t0).unwrap();
        c.fire(t0);
        c.on_report(&relays_report("ON", "OFF"), t0);
        c.on_report(&relays_report("OFF", "OFF"), secs(t0, 0.5));
        let t1 = secs(t0, 10.0);
        c.order(CONTROL, &text("close"), t1).unwrap();
        assert_eq!(c.fire(t1), ["OPEN"]);
        // Nothing leaves while the stop is on its way.
        assert!(c.fire(secs(t1, 13.0)).is_empty());
        // It arrives 13.8 s late: the other relay waits for its end from then.
        let arrived = secs(t1, 13.8);
        c.on_report(&relays_report("ON", "OFF"), arrived);
        assert!(c.fire(secs(t1, 14.0)).is_empty());
        let at = c.deadline().unwrap();
        assert_eq!(at, arrived + Duration::from_millis(1100));
        c.on_report(&relays_report("OFF", "OFF"), secs(t1, 14.3));
        assert_eq!(c.fire(at), ["CLOSE"]);
        // The travel counted up to the moment the stop really arrived.
        assert_eq!(c.position(at).map(|p| p.height > 50.0), Some(true));
    }

    #[test]
    fn an_impulse_never_reported_holds_back_the_next_only_so_long() {
        let t0 = Instant::now();
        let mut c = relay_cover(Some(0.0), t0);
        c.order(CONTROL, &text("open"), t0).unwrap();
        c.fire(t0);
        c.order(CONTROL, &text("stop"), secs(t0, 1.0)).unwrap();
        assert!(c.fire(secs(t0, 29.0)).is_empty());
        assert_eq!(c.deadline(), Some(t0 + RELAY_ECHO));
        assert_eq!(c.fire(t0 + RELAY_ECHO), ["OPEN"]);
    }

    #[test]
    fn a_wall_press_counts_once_with_its_input_event() {
        let t0 = Instant::now();
        let mut c = relay_cover(Some(0.0), t0);
        c.on_report(&relays_report("ON", "OFF"), t0);
        assert!(c.moving());
        c.on_report(&relays_report("OFF", "OFF"), secs(t0, 0.5));
        c.on_report(&action("input_1_single"), secs(t0, 0.8));
        c.on_report(&action("input_1_off"), secs(t0, 0.8));
        assert!(c.moving());
    }

    #[test]
    fn a_wall_press_taken_for_our_impulse_is_corrected_by_its_input_event() {
        let t0 = Instant::now();
        let mut c = relay_cover(Some(0.0), t0);
        c.order(CONTROL, &text("open"), t0).unwrap();
        c.fire(t0);
        // The relay closes, ours is expected: the event says it was a hand.
        c.on_report(&relays_report("ON", "OFF"), secs(t0, 1.0));
        c.on_report(&relays_report("OFF", "OFF"), secs(t0, 1.5));
        c.on_report(&action("input_1_single"), secs(t0, 1.8));
        assert!(!c.moving());
        // Ours then arrives, and is taken as ours, not as one more press.
        c.on_report(&relays_report("ON", "OFF"), secs(t0, 2.5));
        assert!(!c.moving());
    }

    #[test]
    fn a_repeated_report_of_our_impulse_is_no_stop() {
        let t0 = Instant::now();
        let mut c = relay_cover(Some(0.0), t0);
        c.order(CONTROL, &text("open"), t0).unwrap();
        c.fire(t0);
        c.on_report(&relays_report("ON", "OFF"), secs(t0, 0.04));
        c.on_report(&relays_report("OFF", "OFF"), secs(t0, 0.36));
        // The same closing handed over again 2.6 s later, with no input event.
        c.on_report(&relays_report("OFF", "OFF"), secs(t0, 2.62));
        c.on_report(&relays_report("ON", "OFF"), secs(t0, 2.62));
        assert!(c.moving());
        assert_eq!(c.shown_change(secs(t0, 45.0)), Some(100));
    }

    #[test]
    fn a_wall_press_right_after_our_impulse_counts_with_its_input_event() {
        let t0 = Instant::now();
        let mut c = relay_cover(Some(0.0), t0);
        c.order(CONTROL, &text("open"), t0).unwrap();
        c.fire(t0);
        c.on_report(&relays_report("ON", "OFF"), secs(t0, 0.1));
        c.on_report(&relays_report("OFF", "OFF"), secs(t0, 0.6));
        // « Up » pressed on the wall 4 s later: it stops, from the closing on.
        c.on_report(&relays_report("ON", "OFF"), secs(t0, 4.1));
        c.on_report(&relays_report("OFF", "OFF"), secs(t0, 4.6));
        c.on_report(&action("input_1_single"), secs(t0, 4.9));
        assert!(!c.moving());
        assert_eq!(height(&c, secs(t0, 30.0)), Some(10.0));
        // Later than that, a closing is a press at once.
        c.on_report(&relays_report("ON", "OFF"), secs(t0, 20.0));
        assert!(c.moving());
    }

    #[test]
    fn an_input_event_for_the_other_way_or_too_late_changes_nothing() {
        let t0 = Instant::now();
        let mut c = relay_cover(Some(0.0), t0);
        c.order(CONTROL, &text("open"), t0).unwrap();
        c.fire(t0);
        c.on_report(&relays_report("ON", "OFF"), secs(t0, 1.0));
        c.on_report(&action("input_2_single"), secs(t0, 1.8));
        assert!(c.moving());
        c.on_report(&action("input_1_single"), secs(t0, 9.0));
        assert!(c.moving());
    }

    #[test]
    fn inputs_follow_the_relays_they_go_with() {
        let swapped = Relays {
            up: "l2".into(),
            down: "l1".into(),
        };
        assert_eq!(swapped.pressed("input_2_single"), Some(Dir::Up));
        assert_eq!(swapped.pressed("input_1_hold"), Some(Dir::Down));
        assert_eq!(swapped.pressed("input_1_off"), None);
        assert_eq!(swapped.pressed("input_3_single"), None);
        assert_eq!(swapped.pressed("single"), None);
    }

    #[test]
    fn relay_reports_follow_the_wall_and_keep_our_timing() {
        let t0 = Instant::now();
        let mut c = relay_cover(Some(0.0), t0);
        c.order(CONTROL, &text("open"), t0).unwrap();
        c.fire(t0);
        // Our own relay, seen 0.4 s later: the travel starts then. The same
        // state repeated is not a second press.
        c.on_report(&relays_report("ON", "OFF"), secs(t0, 0.4));
        c.on_report(&relays_report("ON", "OFF"), secs(t0, 0.5));
        c.on_report(&relays_report("OFF", "OFF"), secs(t0, 0.9));
        assert!(c.moving());
        assert_eq!(height(&c, secs(t0, 20.4)), Some(50.0));
        // « Down » pressed on the wall: one impulse, it stops.
        c.on_report(&relays_report("OFF", "ON"), secs(t0, 10.4));
        c.on_report(&relays_report("OFF", "OFF"), secs(t0, 10.9));
        assert!(!c.moving());
        assert_eq!(height(&c, secs(t0, 30.0)), Some(25.0));
        // And down again: it starts on its own.
        c.on_report(&relays_report("OFF", "ON"), secs(t0, 40.0));
        assert!(c.moving());
        // A message about something else changes nothing.
        c.on_report(&serde_json::Map::new(), secs(t0, 41.0));
        assert!(c.moving());
    }

    #[test]
    fn durations_are_set_while_it_runs_and_refused_out_of_range() {
        let t0 = Instant::now();
        let mut c = cover(Some(0.0), t0);
        assert_eq!(c.timing("travel_up"), Some(40.0));
        c.set_timing("travel_up", &Value::Float(20.5)).unwrap();
        c.set_timing("slack_down", &Value::Int(2)).unwrap();
        assert_eq!(c.timing("travel_up"), Some(20.5));
        assert_eq!(c.timing("slack_down"), Some(2.0));
        assert!(c.set_timing("travel_up", &Value::Int(2)).is_err());
        assert!(c.set_timing("slack_up", &Value::Int(31)).is_err());
        assert!(c.set_timing("travel_down", &text("long")).is_err());
        assert!(c.set_timing("pulse", &Value::Int(5)).is_err());
        assert_eq!(c.timing("travel_up"), Some(20.5));
        // The chronometer follows at once.
        c.order(CONTROL, &text("open"), t0).unwrap();
        c.fire(t0);
        assert_eq!(height(&c, secs(t0, 10.25)), Some(50.0));
    }

    #[test]
    fn a_new_shutter_needs_only_its_relays() {
        let config: CoverConfig =
            serde_json::from_str(r#"{"relays":{"up":"l1","down":"l2"},"pulse":0.5}"#).unwrap();
        assert_eq!(config.travel_up, 30.0);
        assert_eq!(config.travel_down, 30.0);
        assert_eq!(config.slack_up, 0.0);
    }

    #[test]
    fn a_relay_cover_hides_what_could_feed_both_wires() {
        let relays = Relays {
            up: "l1".into(),
            down: "l2".into(),
        };
        for key in [
            "state_l1",
            "state_l2",
            "power_l1",
            "energy_l2",
            "switch_type_sw1",
            "switch_mode_sw2",
        ] {
            assert!(relays.hides(key), "{key}");
        }
        for key in ["action", "linkquality", "state", "l1"] {
            assert!(!relays.hides(key), "{key}");
        }
    }

    #[test]
    fn open_is_one_impulse_and_the_motor_does_the_rest() {
        let t0 = Instant::now();
        let mut c = cover(Some(0.0), t0);
        c.order(CONTROL, &text("open"), t0).unwrap();
        assert_eq!(run(&mut c, secs(t0, 60.0)), [(0.0, "OPEN"), (1.5, "STOP")]);
        assert_eq!(height(&c, secs(t0, 20.0)), Some(50.0));
        assert_eq!(c.shown_change(secs(t0, 41.0)), Some(100));
        assert!(!c.moving());
    }

    #[test]
    fn a_position_is_reached_by_a_second_impulse_the_same_way() {
        let t0 = Instant::now();
        let mut c = cover(Some(100.0), t0);
        c.order(PERCENT, &Value::Int(50), t0).unwrap();
        let sent = run(&mut c, secs(t0, 60.0));
        assert_eq!(
            sent,
            [
                (0.0, "CLOSE"),
                (1.5, "STOP"),
                (16.0, "CLOSE"),
                (17.5, "STOP")
            ]
        );
        assert!(!c.moving());
        assert_eq!(height(&c, secs(t0, 60.0)), Some(50.0));
    }

    #[test]
    fn a_short_move_waits_for_the_module() {
        let t0 = Instant::now();
        let mut c = cover(Some(50.0), t0);
        c.order(PERCENT, &Value::Int(45), t0).unwrap();
        let sent = run(&mut c, secs(t0, 60.0));
        // 1.6 s of travel asked, but no impulse before 1.5 + 0.6 s.
        assert_eq!(sent[2], (2.1, "CLOSE"));
    }

    #[test]
    fn stop_while_moving_and_nothing_once_at_the_end() {
        let t0 = Instant::now();
        let mut c = cover(Some(0.0), t0);
        c.order(CONTROL, &text("open"), t0).unwrap();
        run(&mut c, secs(t0, 5.0));
        c.order(CONTROL, &text("stop"), secs(t0, 10.0)).unwrap();
        assert_eq!(c.fire(secs(t0, 10.0)), ["OPEN"]);
        assert_eq!(height(&c, secs(t0, 30.0)), Some(25.0));
        // Already at the end: a stop sends nothing.
        let mut c = cover(Some(0.0), t0);
        c.order(CONTROL, &text("open"), t0).unwrap();
        run(&mut c, secs(t0, 5.0));
        c.order(CONTROL, &text("stop"), secs(t0, 50.0)).unwrap();
        assert!(c.fire(secs(t0, 50.0)).is_empty());
    }

    #[test]
    fn reversing_stops_then_starts() {
        let t0 = Instant::now();
        let mut c = cover(Some(0.0), t0);
        c.order(CONTROL, &text("open"), t0).unwrap();
        run(&mut c, secs(t0, 5.0));
        c.order(CONTROL, &text("close"), secs(t0, 10.0)).unwrap();
        let mut sent = Vec::new();
        while let Some(at) = c.deadline().filter(|at| *at <= secs(t0, 20.0)) {
            sent.extend(c.fire(at));
        }
        assert_eq!(sent, ["OPEN", "STOP", "CLOSE", "STOP"]);
        assert!(c.moving());
    }

    #[test]
    fn wall_buttons_are_followed_but_not_our_own_echo() {
        let t0 = Instant::now();
        let mut c = cover(Some(0.0), t0);
        c.order(CONTROL, &text("open"), t0).unwrap();
        c.fire(t0);
        // Our impulse reaches the motor 0.4 s later: the travel starts then.
        c.on_module_moving("forward", secs(t0, 0.4));
        c.on_module_moving("stop", secs(t0, 1.5));
        assert!(c.moving());
        // A press on "up" 10.4 s later stops it, 10 s of travel.
        c.on_module_moving("forward", secs(t0, 10.4));
        assert!(!c.moving());
        assert_eq!(height(&c, secs(t0, 20.0)), Some(25.0));
    }

    #[test]
    fn both_wall_buttons_stop_a_moving_shutter() {
        let t0 = Instant::now();
        let mut c = cover(Some(0.0), t0);
        // Up by hand, then both buttons: the module powers « down » briefly.
        c.on_module_moving("forward", t0);
        c.on_module_moving("stop", secs(t0, 0.95));
        c.on_module_moving("reverse", secs(t0, 6.0));
        c.on_module_moving("stop", secs(t0, 6.5));
        assert!(!c.moving());
        assert_eq!(height(&c, secs(t0, 30.0)), Some(15.0));
    }

    #[test]
    fn a_wall_press_right_after_ours_is_not_taken_for_its_echo() {
        let t0 = Instant::now();
        let mut c = cover(Some(0.0), t0);
        c.order(CONTROL, &text("open"), t0).unwrap();
        c.fire(t0);
        c.on_module_moving("forward", secs(t0, 0.2));
        c.on_module_moving("stop", secs(t0, 1.0));
        // « Down » pressed on the wall half a second later: it stops.
        c.on_module_moving("reverse", secs(t0, 1.5));
        assert!(!c.moving());
    }

    #[test]
    fn a_late_stop_impulse_counts_the_travel_until_it_arrived() {
        let t0 = Instant::now();
        let mut c = cover(Some(0.0), t0);
        c.order(CONTROL, &text("open"), t0).unwrap();
        c.fire(t0);
        c.on_module_moving("forward", secs(t0, 0.5));
        c.on_module_moving("stop", secs(t0, 1.5));
        c.order(CONTROL, &text("stop"), secs(t0, 10.5)).unwrap();
        assert_eq!(c.fire(secs(t0, 10.5)), ["OPEN"]);
        c.on_module_moving("forward", secs(t0, 11.0));
        // Started at 0.5, stopped at 11.0: 10.5 s of a 40 s travel.
        assert_eq!(height(&c, secs(t0, 20.0)), Some(26.25));
    }

    #[test]
    fn unknown_position_refuses_a_percentage_until_a_full_travel() {
        let t0 = Instant::now();
        let mut c = cover(None, t0);
        assert!(c.order(PERCENT, &Value::Int(40), t0).is_err());
        c.order(PERCENT, &Value::Int(0), t0).unwrap();
        run(&mut c, secs(t0, 2.0));
        assert_eq!(height(&c, secs(t0, 10.0)), None);
        assert_eq!(height(&c, secs(t0, 33.0)), Some(0.0));
    }

    #[test]
    fn slats_pull_apart_before_the_bottom_rises_and_press_together_last() {
        let t0 = Instant::now();
        let mut c = Cover::new(
            CoverConfig {
                travel_up: 29.0,
                travel_down: 27.0,
                pulse: 1.0,
                slack_up: 3.0,
                slack_down: 3.0,
                relays: None,
            },
            Some(Spot::at(0.0, false)),
            t0,
        );
        c.order(CONTROL, &text("open"), t0).unwrap();
        c.fire(t0);
        // 2 s in: still on the sill, the slats opening.
        assert_eq!(height(&c, secs(t0, 2.0)), Some(0.0));
        assert_eq!(c.slats_change(secs(t0, 2.0)), Some(true));
        // 3 s + half of the 26 s left: half way up.
        assert_eq!(height(&c, secs(t0, 16.0)), Some(50.0));
        // Half way up from closed, by height, takes 16 s, not 14.5.
        let mut c = Cover::new(c.config.clone(), Some(Spot::at(0.0, false)), t0);
        c.order(PERCENT, &Value::Int(50), t0).unwrap();
        let sent = run(&mut c, secs(t0, 60.0));
        assert_eq!(sent[2], (16.0, "OPEN"));
        // Down from 50: 12 s to the sill, then 3 s pressing the slats.
        let t1 = secs(t0, 100.0);
        c.order(CONTROL, &text("close"), t1).unwrap();
        c.fire(t1);
        assert_eq!(height(&c, secs(t1, 12.0)), Some(0.0));
        assert!(c.moving());
        assert_eq!(c.position(secs(t1, 13.5)).map(|p| p.slack), Some(0.5));
        assert_eq!(c.position(secs(t1, 15.5)), Some(Dir::Down.end()));
    }
}
