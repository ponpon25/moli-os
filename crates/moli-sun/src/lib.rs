//! The sun over the house, as points automations can read: its height and
//! direction, which walls it reaches, and how much cooler outside is than
//! inside. Computed here (NOAA's approximation), nothing asked of the
//! internet. It only reads: no order goes to any device from here.
//!
//! ```toml
//! [sun]
//! indoor = ["rfxtrx:salon/temperature"]
//! outdoor = "rfxtrx:dehors/temperature"
//! [[sun.facade]]
//! id = "sud"
//! name = "Façade sud"
//! azimuth = 180.0
//! ```

use std::time::Duration;

use jiff::Timestamp;
use jiff::tz::TimeZone;
use moli_core::{Access, Device, Kind, PointId, PointSpec, Semantic, Value};
use moli_runtime::{BoxFuture, Driver, DriverCtx, Hub};
use serde::Deserialize;

/// The instance Moli runs it as: the device is `soleil:maison`.
pub const INSTANCE: &str = "soleil";
const DEVICE: &str = "maison";
const EVERY: Duration = Duration::from_secs(60);
/// How much cooler outside is than inside.
const COOLER: &str = "outdoor_cooler_by";

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Below this height (degrees) the sun does not count on a wall.
    #[serde(default = "default_min_elevation")]
    pub min_elevation: f64,
    /// How far from straight in front of a wall (degrees) the sun still counts.
    #[serde(default = "default_half_width")]
    pub half_width: f64,
    #[serde(default, rename = "facade")]
    pub facades: Vec<Facade>,
    /// Inside temperatures: the warmest is compared with `outdoor`.
    #[serde(default)]
    pub indoor: Vec<String>,
    #[serde(default)]
    pub outdoor: Option<String>,
}

fn default_min_elevation() -> f64 {
    15.0
}

fn default_half_width() -> f64 {
    60.0
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Facade {
    /// Stable id: the point is `facade_<id>`.
    pub id: String,
    pub name: String,
    /// The way the wall looks, in degrees from north, clockwise.
    pub azimuth: f64,
}

/// What is wrong with a configuration, if anything.
pub fn check(config: &Config) -> Result<(), String> {
    if !(0.0..=90.0).contains(&config.min_elevation) {
        return Err("min_elevation from 0 to 90".to_owned());
    }
    if !(1.0..=90.0).contains(&config.half_width) {
        return Err("half_width from 1 to 90".to_owned());
    }
    let mut seen = std::collections::HashSet::new();
    for f in &config.facades {
        let plain = !f.id.is_empty()
            && f.id
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
        if !plain {
            return Err(format!("facade {:?}: id in a-z, 0-9 and _", f.id));
        }
        if !seen.insert(&f.id) {
            return Err(format!("facade {:?} twice", f.id));
        }
        if !(0.0..360.0).contains(&f.azimuth) {
            return Err(format!("facade {}: azimuth from 0 to 360", f.id));
        }
    }
    if config.indoor.is_empty() != config.outdoor.is_none() {
        return Err("indoor and outdoor go together".to_owned());
    }
    Ok(())
}

/// Where the sun is, in degrees.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Position {
    /// Above the horizon (negative: below).
    pub elevation: f64,
    /// From north, clockwise.
    pub azimuth: f64,
}

/// Where the sun is at a moment, seen from a place.
#[must_use]
#[allow(clippy::many_single_char_names)]
pub fn position(at: Timestamp, latitude: f64, longitude: f64) -> Position {
    let utc = at.to_zoned(TimeZone::UTC);
    let hour =
        f64::from(utc.hour()) + f64::from(utc.minute()) / 60.0 + f64::from(utc.second()) / 3600.0;
    let g = 2.0 * std::f64::consts::PI / 365.0
        * (f64::from(utc.day_of_year()) - 1.0 + (hour - 12.0) / 24.0);
    let eqtime = 229.18
        * (0.000_075 + 0.001_868 * g.cos()
            - 0.032_077 * g.sin()
            - 0.014_615 * (2.0 * g).cos()
            - 0.040_849 * (2.0 * g).sin());
    let decl = 0.006_918 - 0.399_912 * g.cos() + 0.070_257 * g.sin() - 0.006_758 * (2.0 * g).cos()
        + 0.000_907 * (2.0 * g).sin()
        - 0.002_697 * (3.0 * g).cos()
        + 0.001_48 * (3.0 * g).sin();
    let solar_minutes = hour * 60.0 + eqtime + 4.0 * longitude;
    let ha = (solar_minutes / 4.0 - 180.0).to_radians();
    let lat = latitude.to_radians();
    let height = (lat.sin() * decl.sin() + lat.cos() * decl.cos() * ha.cos()).asin();
    let azimuth = ha
        .sin()
        .atan2(ha.cos() * lat.sin() - decl.tan() * lat.cos())
        .to_degrees()
        + 180.0;
    Position {
        elevation: height.to_degrees(),
        azimuth: azimuth.rem_euclid(360.0),
    }
}

/// Does the sun reach a wall looking that way? (Trees and neighbours unknown.)
#[must_use]
pub fn reaches(sun: Position, wall: f64, half_width: f64, min_elevation: f64) -> bool {
    let apart = (sun.azimuth - wall + 180.0).rem_euclid(360.0) - 180.0;
    sun.elevation >= min_elevation && apart.abs() <= half_width
}

/// How much cooler outside is than the warmest room, once something is known
/// of both.
#[must_use]
pub fn cooler_by(indoor: &[f64], outdoor: Option<f64>) -> Option<f64> {
    let warmest = indoor.iter().copied().reduce(f64::max)?;
    Some(warmest - outdoor?)
}

fn tenth(x: f64) -> f64 {
    (x * 10.0).round() / 10.0
}

fn read_only(key: &str, label: &str, kind: Kind) -> PointSpec {
    PointSpec {
        key: key.into(),
        label: label.into(),
        kind,
        access: Access {
            read: true,
            write: false,
        },
        unit: None,
        semantic: Semantic::Other,
    }
}

fn number() -> Kind {
    Kind::Numeric {
        min: None,
        max: None,
        step: None,
    }
}

#[derive(Debug)]
pub struct Sun {
    hub: Hub,
    config: Config,
    latitude: f64,
    longitude: f64,
}

impl Sun {
    #[must_use]
    pub fn new(hub: Hub, config: Config, latitude: f64, longitude: f64) -> Self {
        Self {
            hub,
            config,
            latitude,
            longitude,
        }
    }

    fn compares(&self) -> bool {
        self.config.outdoor.is_some()
    }

    fn device(&self, ctx: &DriverCtx) -> Device {
        let mut points = vec![
            read_only(
                "elevation",
                &moli_i18n::tr!("pilotes.soleil.hauteur"),
                number(),
            ),
            read_only(
                "azimuth",
                &moli_i18n::tr!("pilotes.soleil.azimut"),
                number(),
            ),
        ];
        for f in &self.config.facades {
            points.push(read_only(
                &format!("facade_{}", f.id),
                &moli_i18n::tr!("pilotes.soleil.facade", name = f.name),
                Kind::Binary,
            ));
        }
        if self.compares() {
            points.push(read_only(
                COOLER,
                &moli_i18n::tr!("pilotes.soleil.dehors_plus_frais"),
                number(),
            ));
        }
        Device {
            id: ctx.device_id(DEVICE),
            instance: ctx.instance().clone(),
            native_name: moli_i18n::tr!("pilotes.soleil.nom").as_str().into(),
            manufacturer: Some("Moli".into()),
            model: None,
            description: None,
            native_room: None,
            members: Vec::new(),
            points,
        }
    }

    fn temperature(&self, point: &str) -> Option<f64> {
        self.hub
            .state(&PointId::from(point))
            .and_then(|sample| sample.value.as_f64())
    }

    fn publish(&self, ctx: &DriverCtx) {
        let id = ctx.device_id(DEVICE);
        let sun = position(Timestamp::now(), self.latitude, self.longitude);
        ctx.set_state(&id, "elevation", Value::Float(tenth(sun.elevation)));
        ctx.set_state(&id, "azimuth", Value::Float(tenth(sun.azimuth)));
        for f in &self.config.facades {
            ctx.set_state(
                &id,
                &format!("facade_{}", f.id),
                Value::Bool(reaches(
                    sun,
                    f.azimuth,
                    self.config.half_width,
                    self.config.min_elevation,
                )),
            );
        }
        if self.compares() {
            let indoor: Vec<f64> = self
                .config
                .indoor
                .iter()
                .filter_map(|p| self.temperature(p))
                .collect();
            let outdoor = self
                .config
                .outdoor
                .as_deref()
                .and_then(|p| self.temperature(p));
            if let Some(cooler) = cooler_by(&indoor, outdoor) {
                ctx.set_state(&id, COOLER, Value::Float(tenth(cooler)));
            }
        }
    }
}

impl Driver for Sun {
    fn kind(&self) -> &'static str {
        "sun"
    }

    fn run<'a>(&'a self, ctx: &'a mut DriverCtx) -> BoxFuture<'a, anyhow::Result<()>> {
        Box::pin(async move {
            ctx.upsert_device(self.device(ctx));
            ctx.set_availability(&ctx.device_id(DEVICE), true);
            self.publish(ctx);
            ctx.ready();
            let mut tick = tokio::time::interval(EVERY);
            loop {
                tokio::select! {
                    _ = tick.tick() => self.publish(ctx),
                    command = ctx.next_command() => match command {
                        Some(command) => command.reply(Err(moli_i18n::tr!("pilotes.soleil.lecture_seule"))),
                        None => return Ok(()),
                    },
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use moli_core::InstanceId;
    use moli_runtime::{HubOptions, spawn_driver};
    use tokio_util::sync::CancellationToken;

    // Paris, far from any real house this runs in.
    const LAT: f64 = 48.85;
    const LON: f64 = 2.35;

    fn at(s: &str) -> Position {
        position(s.parse().unwrap(), LAT, LON)
    }

    fn near(x: f64, low: f64, high: f64) {
        assert!((low..=high).contains(&x), "{x} not in {low}..{high}");
    }

    #[test]
    fn the_sun_is_where_the_almanac_says() {
        // Near solar noon: due south, 90° - latitude ± the season's 23.4°.
        let summer = at("2026-06-21T11:52:00Z");
        near(summer.elevation, 64.0, 65.2);
        near(summer.azimuth, 176.0, 184.0);
        let winter = at("2026-12-21T11:50:00Z");
        near(winter.elevation, 17.0, 18.3);
        near(winter.azimuth, 177.0, 183.0);
        // Just after sunrise at the equinox: due east.
        let morning = at("2026-03-20T06:10:00Z");
        near(morning.elevation, 0.5, 5.0);
        near(morning.azimuth, 88.0, 100.0);
        // Evening: west, under the horizon at midnight.
        near(at("2026-03-20T17:00:00Z").azimuth, 255.0, 275.0);
        assert!(at("2026-06-21T23:50:00Z").elevation < 0.0);
    }

    #[test]
    fn a_wall_sees_the_sun_in_front_of_it_and_high_enough() {
        let sun = |azimuth, elevation| Position { elevation, azimuth };
        assert!(reaches(sun(95.0, 30.0), 95.0, 60.0, 15.0));
        assert!(reaches(sun(155.0, 30.0), 95.0, 60.0, 15.0));
        assert!(!reaches(sun(156.0, 30.0), 95.0, 60.0, 15.0));
        assert!(!reaches(sun(95.0, 14.0), 95.0, 60.0, 15.0));
        // Across north, the turn of the circle counts for nothing.
        assert!(reaches(sun(350.0, 30.0), 5.0, 60.0, 15.0));
        assert!(reaches(sun(20.0, 30.0), 350.0, 60.0, 15.0));
    }

    #[test]
    fn the_warmest_room_is_compared_once_both_are_known() {
        assert_eq!(cooler_by(&[21.0, 24.5], Some(20.0)), Some(4.5));
        assert_eq!(cooler_by(&[], Some(20.0)), None);
        assert_eq!(cooler_by(&[21.0], None), None);
    }

    fn config(text: &str) -> Config {
        toml::from_str(text).unwrap()
    }

    #[test]
    fn bad_configurations_are_refused() {
        let wall = "[[facade]]\nid = \"sud\"\nname = \"Sud\"\nazimuth = 180.0\n";
        assert!(check(&config(wall)).is_ok());
        assert!(check(&config(&format!("{wall}{wall}"))).is_err(), "twice");
        assert!(
            check(&config(
                "[[facade]]\nid = \"a:b\"\nname = \"x\"\nazimuth = 1.0"
            ))
            .is_err()
        );
        assert!(
            check(&config(
                "[[facade]]\nid = \"a\"\nname = \"x\"\nazimuth = 360.0"
            ))
            .is_err()
        );
        assert!(check(&config("half_width = 0.0")).is_err());
        assert!(
            check(&config("indoor = [\"a:b/c\"]")).is_err(),
            "no outdoor"
        );
    }

    #[tokio::test]
    async fn it_shows_where_the_sun_is_and_takes_no_orders() {
        let hub = Hub::new(HubOptions::default()).unwrap();
        let cancel = CancellationToken::new();
        let wall = config("[[facade]]\nid = \"sud\"\nname = \"Sud\"\nazimuth = 180.0\n");
        spawn_driver(
            &hub,
            InstanceId::from(INSTANCE),
            Arc::new(Sun::new(hub.clone(), wall, LAT, LON)),
            cancel.clone(),
        );
        let elevation = PointId::from("soleil:maison/elevation".to_owned());
        let wall = PointId::from("soleil:maison/facade_sud".to_owned());
        for _ in 0..100 {
            if hub.state(&wall).is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(hub.state(&elevation).is_some());
        assert!(matches!(hub.state(&wall).unwrap().value, Value::Bool(_)));
        cancel.cancel();
    }
}
