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
//! Each such device shows `control` (open, stop, close) and
//! `percent_control` (0 closed … 100 open) instead of the module's
//! `state`, `position` and `moving`. Pure state machine; time comes in.

use std::time::{Duration, Instant};

use moli_core::{Access, Device, Kind, PointSpec, Semantic, Unit, Value};
use serde::Deserialize;

/// `[driver.options.impulse_covers."0x…"]`: one shutter, by IEEE address.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CoverConfig {
    /// Seconds from fully closed to fully open.
    pub travel_up: f64,
    /// Seconds from fully open to fully closed.
    pub travel_down: f64,
    /// Seconds the module powers the wire for one impulse.
    #[serde(default = "default_pulse")]
    pub pulse: f64,
}

fn default_pulse() -> f64 {
    1.5
}

/// Pause between the end of one impulse and the next: the module ignores
/// an order that comes too soon after the previous one (~0.6 s seen).
const GAP: Duration = Duration::from_millis(800);
/// Module reports seen this long after one of our impulses are its echo.
const ECHO: Duration = Duration::from_secs(1);
/// A requested position this close to the current one is not worth a move.
const NEAR: f64 = 2.0;

pub const CONTROL: &str = "control";
pub const PERCENT: &str = "percent_control";
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

    fn end(self) -> f64 {
        match self {
            Self::Up => 100.0,
            Self::Down => 0.0,
        }
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

#[derive(Clone, Copy, Debug)]
struct Motion {
    dir: Dir,
    since: Instant,
    from: Option<f64>,
}

#[derive(Debug)]
pub struct Cover {
    config: CoverConfig,
    /// Known position while still; `None` until a full travel.
    pos: Option<f64>,
    motion: Option<Motion>,
    /// Where a `percent_control` order wants to stop.
    target: Option<f64>,
    pending: Vec<(Instant, Step)>,
    /// No impulse before this (the module must see a fresh order).
    ready_at: Instant,
    /// Module reports before this are our own impulses.
    echo_until: Instant,
    module_moving: bool,
    /// Last position published.
    shown: Option<i64>,
}

impl Cover {
    #[must_use]
    pub fn new(config: CoverConfig, pos: Option<f64>, now: Instant) -> Self {
        Self {
            config,
            pos: pos.map(|p| p.clamp(0.0, 100.0)),
            motion: None,
            target: None,
            pending: Vec::new(),
            ready_at: now,
            echo_until: now,
            module_moving: false,
            shown: None,
        }
    }

    fn travel(&self, dir: Dir) -> Duration {
        let secs = match dir {
            Dir::Up => self.config.travel_up,
            Dir::Down => self.config.travel_down,
        };
        Duration::from_secs_f64(secs.max(1.0))
    }

    /// Estimated position, `None` when unknown.
    #[must_use]
    pub fn position(&self, now: Instant) -> Option<f64> {
        let Some(m) = self.motion else {
            return self.pos;
        };
        let done = now.saturating_duration_since(m.since).as_secs_f64()
            / self.travel(m.dir).as_secs_f64()
            * 100.0;
        match (m.from, m.dir) {
            (Some(from), Dir::Up) => Some((from + done).min(100.0)),
            (Some(from), Dir::Down) => Some((from - done).max(0.0)),
            // Unknown start: known only once a full travel is behind.
            (None, dir) => (done >= 100.0).then(|| dir.end()),
        }
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
                #[allow(clippy::cast_precision_loss)]
                let target = match v {
                    Value::Int(i) => *i as f64,
                    Value::Float(f) => *f,
                    _ => return Err(moli_i18n::tr!("pilotes.z2m.volet.position_attendue")),
                }
                .clamp(0.0, 100.0);
                if target >= 100.0 - NEAR {
                    self.go(Dir::Up, now);
                } else if target <= NEAR {
                    self.go(Dir::Down, now);
                } else {
                    let Some(pos) = self.position(now) else {
                        return Err(moli_i18n::tr!("pilotes.z2m.volet.position_inconnue"));
                    };
                    if (target - pos).abs() <= NEAR && self.motion.is_none() {
                        return Ok(());
                    }
                    let dir = if target > pos { Dir::Up } else { Dir::Down };
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
        let share = ((target - from).abs() / 100.0).min(1.0);
        Some(since + self.travel(dir).mul_f64(share))
    }

    /// The next moment `fire` has something to do.
    #[must_use]
    pub fn deadline(&self) -> Option<Instant> {
        self.pending.iter().map(|(at, _)| *at).min()
    }

    /// Runs what is due. Returns the module `state` values to publish.
    pub fn fire(&mut self, now: Instant) -> Vec<&'static str> {
        self.settle(now);
        let mut out = Vec::new();
        while let Some(i) = (0..self.pending.len())
            .filter(|&i| self.pending[i].0 <= now)
            .min_by_key(|&i| self.pending[i].0)
        {
            let step = self.pending[i].1;
            // A fresh impulse waits until the module can take it.
            if !matches!(step, Step::Release) && now < self.ready_at {
                self.pending[i].0 = self.ready_at;
                break;
            }
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
            let pulse = Duration::from_secs_f64(self.config.pulse.max(0.3));
            self.pending.push((now + pulse, Step::Release));
            self.ready_at = now + pulse + GAP;
            self.echo_until = now + pulse + ECHO;
            if matches!(step, Step::Start(_))
                && let Some(at) = self.stop_time(now, dir)
            {
                self.pending.push((at, Step::Stop));
            }
        }
        out
    }

    /// The module's own `moving` report: a wall button pressed shows up
    /// as the module starting to power a wire we did not ask for.
    pub fn on_module_moving(&mut self, moving: &str, now: Instant) {
        let dir = match moving {
            "forward" => Some(Dir::Up),
            "reverse" => Some(Dir::Down),
            _ => None,
        };
        let started = dir.is_some() && !self.module_moving;
        self.module_moving = dir.is_some();
        if started && now >= self.echo_until {
            self.pending.clear();
            self.target = None;
            self.impulse(dir.unwrap_or(Dir::Up), now);
            tracing::debug!(moving, "wall button impulse");
        }
    }

    /// The position to publish, when it changed (whole percent).
    pub fn shown_change(&mut self, now: Instant) -> Option<i64> {
        self.settle(now);
        #[allow(clippy::cast_possible_truncation)]
        let now_shown = self.position(now).map(|p| p.round() as i64);
        if now_shown.is_some() && now_shown != self.shown {
            self.shown = now_shown;
            return now_shown;
        }
        None
    }
}

/// Replaces the module's position points with the impulse cover's.
pub fn reshape(device: &mut Device) {
    device.points.retain(|p| !HIDDEN.contains(&&*p.key));
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
            },
            pos,
            t0,
        )
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

    #[test]
    fn open_is_one_impulse_and_the_motor_does_the_rest() {
        let t0 = Instant::now();
        let mut c = cover(Some(0.0), t0);
        c.order(CONTROL, &text("open"), t0).unwrap();
        assert_eq!(run(&mut c, secs(t0, 60.0)), [(0.0, "OPEN"), (1.5, "STOP")]);
        assert_eq!(c.position(secs(t0, 20.0)), Some(50.0));
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
        assert_eq!(c.position(secs(t0, 60.0)), Some(50.0));
    }

    #[test]
    fn a_short_move_waits_for_the_module() {
        let t0 = Instant::now();
        let mut c = cover(Some(50.0), t0);
        c.order(PERCENT, &Value::Int(45), t0).unwrap();
        let sent = run(&mut c, secs(t0, 60.0));
        // 1.6 s of travel asked, but no impulse before 1.5 + 0.8 s.
        assert_eq!(sent[2], (2.3, "CLOSE"));
    }

    #[test]
    fn stop_while_moving_and_nothing_once_at_the_end() {
        let t0 = Instant::now();
        let mut c = cover(Some(0.0), t0);
        c.order(CONTROL, &text("open"), t0).unwrap();
        run(&mut c, secs(t0, 5.0));
        c.order(CONTROL, &text("stop"), secs(t0, 10.0)).unwrap();
        assert_eq!(c.fire(secs(t0, 10.0)), ["OPEN"]);
        assert_eq!(c.position(secs(t0, 30.0)), Some(25.0));
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
        c.on_module_moving("forward", secs(t0, 0.3));
        c.on_module_moving("stop", secs(t0, 1.6));
        assert!(c.moving());
        // A press on "up" 10 s later stops it.
        c.on_module_moving("forward", secs(t0, 10.0));
        assert!(!c.moving());
        assert_eq!(c.position(secs(t0, 20.0)), Some(25.0));
    }

    #[test]
    fn unknown_position_refuses_a_percentage_until_a_full_travel() {
        let t0 = Instant::now();
        let mut c = cover(None, t0);
        assert!(c.order(PERCENT, &Value::Int(40), t0).is_err());
        c.order(PERCENT, &Value::Int(0), t0).unwrap();
        run(&mut c, secs(t0, 2.0));
        assert_eq!(c.position(secs(t0, 10.0)), None);
        assert_eq!(c.position(secs(t0, 33.0)), Some(0.0));
    }
}
