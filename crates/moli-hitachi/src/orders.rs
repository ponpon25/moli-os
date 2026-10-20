//! The orders waiting to go, and what the driver remembers of the ones that
//! went. Hitachi's cloud takes several seconds over an order and takes the
//! unit's whole state in each, so:
//! - orders for one unit are merged while the person is still tapping (the
//!   last value wins) and leave once nothing changed for a moment;
//! - the next order starts from what was asked, not from a picture the cloud
//!   has not yet caught up with: during the wait it may still show the old
//!   values, or an intermediate state (an order built on it would undo the
//!   previous one, or switch the unit off).

use std::collections::HashMap;
use std::time::Duration;

use tokio::time::Instant;

use crate::model::{Change, Mode, Unit};

/// A setpoint is given this long to settle (taps on − and + add up).
const SETTLE_TARGET: Duration = Duration::from_millis(900);
/// Anything else, a little: a mode and a speed chosen in a row leave together.
const SETTLE_OTHER: Duration = Duration::from_millis(250);
/// However long someone taps, an order leaves within this.
const MAX_WAIT: Duration = Duration::from_secs(4);
/// What was asked is trusted over the cloud's picture for this long.
const TRUST: Duration = Duration::from_secs(30);

#[derive(Debug)]
struct Pending {
    unit: Unit,
    first: Instant,
    due: Instant,
}

/// What an order set, until the cloud says the same.
#[derive(Debug)]
struct Asked {
    at: Instant,
    power: Option<bool>,
    mode: Option<Mode>,
    target: Option<f64>,
    fan: Option<String>,
}

#[derive(Debug, Default)]
pub struct Orders {
    queue: HashMap<String, Pending>,
    asked: HashMap<String, Asked>,
}

impl Orders {
    /// The unit as the next order must start from.
    #[must_use]
    pub fn effective(&self, native: &str, cloud: &Unit, now: Instant) -> Unit {
        let mut unit = cloud.clone();
        if let Some(a) = self
            .asked
            .get(native)
            .filter(|a| now.duration_since(a.at) < TRUST)
        {
            if let Some(p) = a.power {
                unit.power = p;
            }
            if a.mode.is_some() {
                unit.mode = a.mode;
            }
            if a.target.is_some() {
                unit.target = a.target;
            }
            if let Some(f) = &a.fan {
                unit.fan.clone_from(f);
            }
        }
        unit
    }

    /// Where a new change starts from: what waits to go, else the above.
    #[must_use]
    pub fn projected(&self, native: &str, cloud: &Unit, now: Instant) -> Unit {
        self.queue
            .get(native)
            .map_or_else(|| self.effective(native, cloud, now), |p| p.unit.clone())
    }

    /// Queues the unit as it should be, merged with what waits already.
    pub fn enqueue(&mut self, native: &str, unit: Unit, change: &Change, now: Instant) {
        let settle = if matches!(change, Change::Target(_)) {
            SETTLE_TARGET
        } else {
            SETTLE_OTHER
        };
        let first = self.queue.get(native).map_or(now, |p| p.first);
        let due = (now + settle).min(first + MAX_WAIT);
        self.queue
            .insert(native.to_owned(), Pending { unit, first, due });
    }

    /// Puts an order back, to go at once (the session had ended).
    pub fn requeue(&mut self, native: &str, unit: Unit, now: Instant) {
        self.queue.entry(native.to_owned()).or_insert(Pending {
            unit,
            first: now,
            due: now,
        });
    }

    /// When the first order is due.
    #[must_use]
    pub fn next_due(&self) -> Option<Instant> {
        self.queue.values().map(|p| p.due).min()
    }

    /// The order that is due, if any (the oldest first).
    pub fn take_due(&mut self, now: Instant) -> Option<(String, Unit)> {
        let native = self
            .queue
            .iter()
            .filter(|(_, p)| p.due <= now)
            .min_by_key(|(_, p)| p.due)
            .map(|(n, _)| n.clone())?;
        self.queue.remove(&native).map(|p| (native, p.unit))
    }

    /// An order went: what it set is trusted until the cloud says it too.
    pub fn sent(&mut self, native: &str, unit: &Unit, now: Instant) {
        self.asked.insert(
            native.to_owned(),
            Asked {
                at: now,
                power: Some(unit.power),
                mode: unit.mode,
                target: unit.target,
                fan: Some(unit.fan.clone()),
            },
        );
    }

    /// The cloud's picture arrived: what it agrees with is no longer held.
    pub fn observe(&mut self, native: &str, cloud: &Unit, now: Instant) {
        let Some(a) = self.asked.get_mut(native) else {
            return;
        };
        if now.duration_since(a.at) >= TRUST {
            self.asked.remove(native);
            return;
        }
        if a.power == Some(cloud.power) {
            a.power = None;
        }
        if a.mode == cloud.mode {
            a.mode = None;
        }
        if a.target == cloud.target {
            a.target = None;
        }
        if a.fan.as_deref() == Some(cloud.fan.as_str()) {
            a.fan = None;
        }
        if a.power.is_none() && a.mode.is_none() && a.target.is_none() && a.fan.is_none() {
            self.asked.remove(native);
        }
    }

    /// A unit that left the account.
    pub fn forget(&mut self, native: &str) {
        self.queue.remove(native);
        self.asked.remove(native);
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn unit(power: &str, target: f64) -> Unit {
        Unit::from_json(&json!({
            "id": 1, "vendorThingId": "JCH-1", "power": power, "mode": "COOLING",
            "iduTemperature": target, "fanSpeed": "AUTO", "online": true
        }))
        .unwrap()
    }

    #[test]
    fn taps_are_merged_into_one_order() {
        let mut o = Orders::default();
        let t0 = Instant::now();
        let mut u = unit("ON", 25.0);
        for (i, t) in [24.5, 24.0, 23.5].into_iter().enumerate() {
            u.target = Some(t);
            let at = t0 + Duration::from_millis(300 * i as u64);
            o.enqueue("a", u.clone(), &Change::Target(t), at);
            assert!(o.take_due(at).is_none(), "nothing leaves while taps go on");
        }
        let (_, sent) = o.take_due(t0 + Duration::from_millis(600 + 900)).unwrap();
        assert_eq!(sent.target, Some(23.5));
        assert!(o.is_empty());
    }

    #[test]
    fn an_order_leaves_within_the_maximum_wait() {
        let mut o = Orders::default();
        let t0 = Instant::now();
        let u = unit("ON", 24.0);
        for i in 0..20u64 {
            o.enqueue(
                "a",
                u.clone(),
                &Change::Target(24.0),
                t0 + Duration::from_millis(500 * i),
            );
        }
        assert!(o.next_due().unwrap() <= t0 + MAX_WAIT);
    }

    #[test]
    fn a_stale_cloud_picture_does_not_undo_an_order() {
        let mut o = Orders::default();
        let t0 = Instant::now();
        let on = unit("ON", 24.0);
        o.sent("a", &on, t0);
        // The cloud still shows the unit off, at the old setpoint.
        let stale = unit("OFF", 25.0);
        o.observe("a", &stale, t0 + Duration::from_secs(3));
        let base = o.effective("a", &stale, t0 + Duration::from_secs(3));
        assert!(
            base.power,
            "the unit is not switched off by a stale picture"
        );
        assert_eq!(base.target, Some(24.0));
        // Once the cloud agrees, it is trusted again.
        o.observe("a", &on, t0 + Duration::from_secs(6));
        let later = unit("OFF", 22.0);
        assert!(!o.effective("a", &later, t0 + Duration::from_secs(7)).power);
    }

    #[test]
    fn what_was_asked_is_trusted_only_for_a_while() {
        let mut o = Orders::default();
        let t0 = Instant::now();
        o.sent("a", &unit("ON", 24.0), t0);
        let cloud = unit("OFF", 25.0);
        assert!(
            !o.effective("a", &cloud, t0 + TRUST + Duration::from_secs(1))
                .power
        );
    }

    #[test]
    fn a_new_change_starts_from_what_waits() {
        let mut o = Orders::default();
        let t0 = Instant::now();
        let cloud = unit("OFF", 25.0);
        let mut on = cloud.clone();
        on.power = true;
        o.enqueue("a", on, &Change::Power(true), t0);
        assert!(o.projected("a", &cloud, t0).power);
    }
}
