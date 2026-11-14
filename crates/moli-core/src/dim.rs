//! A dimmer's level as people see it. Lamps on a dimmer that only knows a
//! level (a Z-Wave multilevel switch) look far brighter at its first steps
//! than at its last: the percent people set goes through a square curve,
//! fine at the bottom, wide at the top (as Hue lamps dim).

/// Percent (0–100) → the level, within `lo..=hi`. Above 0 % it is never
/// the minimum (off).
#[must_use]
pub fn level_of(pct: f64, lo: f64, hi: f64) -> f64 {
    let p = (pct / 100.0).clamp(0.0, 1.0);
    if p == 0.0 {
        return lo;
    }
    let level = lo + (hi - lo) * p * p;
    if hi - lo >= 1.0 {
        level.max(lo + 1.0)
    } else {
        level
    }
}

/// A brightness that is a dimmer's raw level rather than a percent: a
/// Zigbee dimmer's 0–254. Seen through the same curve as a Z-Wave level, so
/// every dimmer in the house answers alike. A brightness already in percent
/// (Hue, which dims along the eye by itself) stays straight.
#[must_use]
pub fn perceived(spec: &crate::spec::PointSpec) -> bool {
    use crate::spec::{Kind, Semantic, Unit};
    spec.semantic == Semantic::Brightness
        && spec.unit != Some(Unit::Percent)
        && matches!(spec.kind, Kind::Numeric { max: Some(max), .. } if (max - 100.0).abs() > f64::EPSILON)
}

/// The level → percent (0–100).
#[must_use]
pub fn percent_of(level: f64, lo: f64, hi: f64) -> f64 {
    let span = (hi - lo).max(f64::EPSILON);
    100.0 * ((level - lo) / span).clamp(0.0, 1.0).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_raw_brightness_is_seen_like_a_dimmer() {
        use crate::spec::{Access, Kind, PointSpec, Semantic, Unit};
        let spec = |max: f64, unit: Option<Unit>| PointSpec {
            key: "brightness".into(),
            label: "Brightness".into(),
            kind: Kind::Numeric {
                min: Some(0.0),
                max: Some(max),
                step: None,
            },
            access: Access {
                read: true,
                write: true,
            },
            unit,
            semantic: Semantic::Brightness,
        };
        assert!(perceived(&spec(254.0, None)), "a Zigbee dimmer, 0–254");
        assert!(
            !perceived(&spec(100.0, Some(Unit::Percent))),
            "a Hue, in percent"
        );
        assert!(!perceived(&spec(100.0, None)));
        assert_eq!(level_of(50.0, 0.0, 254.0).round(), 64.0);
    }

    #[test]
    fn fine_at_the_bottom_wide_at_the_top() {
        // A Z-Wave level, 0–99.
        assert!(level_of(0.0, 0.0, 99.0).abs() < f64::EPSILON);
        assert!(
            (level_of(1.0, 0.0, 99.0) - 1.0).abs() < f64::EPSILON,
            "never off"
        );
        assert_eq!(level_of(30.0, 0.0, 99.0).round(), 9.0);
        assert_eq!(level_of(50.0, 0.0, 99.0).round(), 25.0);
        assert_eq!(level_of(80.0, 0.0, 99.0).round(), 63.0);
        assert!((level_of(100.0, 0.0, 99.0) - 99.0).abs() < f64::EPSILON);
        // (Below 10 %, the level is raised to 1: read back, 10 %.)
        for pct in [30.0, 50.0, 80.0, 100.0] {
            let back = percent_of(level_of(pct, 0.0, 99.0), 0.0, 99.0);
            assert!((back - pct).abs() < 0.01, "{pct} → {back}");
        }
    }
}
