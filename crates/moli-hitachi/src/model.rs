//! What the cloud says about a unit, what it takes as an order, and how
//! both read in Moli. The names Moli uses (`on`, `mode`, `target_temperature`
//! and the French modes) are the ones the dashboard's climate card knows.

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::{Value as Json, json};

/// The cloud's "unknown" (Kii's `INT32_MAX`).
const UNKNOWN: f64 = 2_147_483_647.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Mode {
    Auto,
    Cooling,
    Heating,
    Dry,
    Fan,
}

pub const MODES: [Mode; 5] = [
    Mode::Auto,
    Mode::Cooling,
    Mode::Heating,
    Mode::Dry,
    Mode::Fan,
];

impl Mode {
    #[must_use]
    pub fn cloud(self) -> &'static str {
        match self {
            Self::Auto => "AUTO",
            Self::Cooling => "COOLING",
            Self::Heating => "HEATING",
            Self::Dry => "DRY",
            Self::Fan => "FAN",
        }
    }

    /// The word Moli's dashboard uses.
    #[must_use]
    pub fn moli(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Cooling => "froid",
            Self::Heating => "chaud",
            Self::Dry => "déshumidification",
            Self::Fan => "ventilation",
        }
    }

    #[must_use]
    pub fn from_cloud(s: &str) -> Option<Self> {
        MODES.into_iter().find(|m| m.cloud() == s)
    }

    #[must_use]
    pub fn from_moli(s: &str) -> Option<Self> {
        MODES.into_iter().find(|m| m.moli() == s)
    }
}

/// Fan speeds, as Moli says them (`auto`, `1`…`5`) and the cloud does
/// (`AUTO`, `LV1`…`LV5`).
#[must_use]
pub fn fan_to_moli(cloud: &str) -> Option<&'static str> {
    Some(match cloud {
        "AUTO" => "auto",
        "LV1" => "1",
        "LV2" => "2",
        "LV3" => "3",
        "LV4" => "4",
        "LV5" => "5",
        _ => return None,
    })
}

#[must_use]
pub fn fan_to_cloud(moli: &str) -> Option<&'static str> {
    Some(match moli {
        "auto" => "AUTO",
        "1" => "LV1",
        "2" => "LV2",
        "3" => "LV3",
        "4" => "LV4",
        "5" => "LV5",
        _ => return None,
    })
}

pub fn fan_values() -> Vec<Arc<str>> {
    ["auto", "1", "2", "3", "4", "5"]
        .into_iter()
        .map(Arc::from)
        .collect()
}

pub fn mode_values() -> Vec<Arc<str>> {
    MODES.into_iter().map(|m| Arc::from(m.moli())).collect()
}

/// One unit, as the notification service describes it.
#[derive(Clone, Debug, PartialEq)]
pub struct Unit {
    /// The cloud's number for the unit (used in orders).
    pub id: i64,
    /// The module's name (`JCH-…`): stable when the unit is paired again.
    pub thing: String,
    pub name: String,
    pub cloud_id: String,
    pub sys_type: i64,
    pub online: bool,
    pub power: bool,
    pub mode: Option<Mode>,
    pub target: Option<f64>,
    pub room: Option<f64>,
    /// Cloud wording (`AUTO`, `LV3`…), kept as it came for the next order.
    pub fan: String,
    pub swing: String,
    pub fault: bool,
}

fn real(v: &Json) -> Option<f64> {
    v.as_f64().filter(|n| (n - UNKNOWN).abs() > 1.0)
}

impl Unit {
    /// `None` when the entry has no number or no module name.
    #[must_use]
    pub fn from_json(j: &Json) -> Option<Self> {
        let id = j["id"].as_i64()?;
        let thing = j["vendorThingId"].as_str().filter(|t| !t.is_empty())?;
        Some(Self {
            id,
            thing: thing.to_owned(),
            name: j["name"].as_str().unwrap_or_default().to_owned(),
            cloud_id: j["cloudId"].as_str().unwrap_or_default().to_owned(),
            sys_type: j["SysType"].as_i64().unwrap_or(1),
            online: j["online"].as_bool().unwrap_or(true),
            power: j["power"].as_str() == Some("ON"),
            mode: j["mode"].as_str().and_then(Mode::from_cloud),
            target: real(&j["iduTemperature"]),
            room: real(&j["roomTemperature"]),
            fan: j["fanSpeed"].as_str().unwrap_or("AUTO").to_owned(),
            swing: j["fanSwing"].as_str().unwrap_or("OFF").to_owned(),
            fault: j["errorStatus"]["active"].as_bool().unwrap_or(false),
        })
    }

    /// The identifier Moli keeps for the unit.
    #[must_use]
    pub fn native_id(&self) -> String {
        self.thing.to_ascii_lowercase()
    }

    /// `RAK-DJ25RHAE` out of `174_RAK-DJ25RHAE`.
    #[must_use]
    pub fn model(&self) -> &str {
        self.cloud_id
            .split_once('_')
            .map_or(&self.cloud_id, |(_, m)| m)
    }
}

/// Every unit in a message of the notification service (`ON_CONNECT`,
/// `REFRESH_ALL`, and whatever else carries a `data` list).
#[must_use]
pub fn units_of(message: &Json) -> Vec<Unit> {
    message["data"]
        .as_array()
        .map(|a| a.iter().filter_map(Unit::from_json).collect())
        .unwrap_or_default()
}

/// What a unit takes in a given mode.
#[derive(Clone, Debug, PartialEq)]
pub struct ModeLimits {
    pub min: f64,
    pub max: f64,
    pub step: f64,
    pub temperature: bool,
    /// Cloud wording of the allowed fan speeds.
    pub fans: Vec<String>,
}

/// The limits of every model of the account (`rac-configuration`).
#[derive(Clone, Debug, Default)]
pub struct Capabilities {
    models: HashMap<String, HashMap<Mode, ModeLimits>>,
}

impl Capabilities {
    #[must_use]
    pub fn from_json(j: &Json) -> Self {
        let mut models = HashMap::new();
        for entry in j.as_array().into_iter().flatten() {
            let Some(cloud_id) = entry["cloudId"].as_str() else {
                continue;
            };
            let mut by_mode = HashMap::new();
            for m in entry["racOperationModes"].as_array().into_iter().flatten() {
                let Some(mode) = m["mode"].as_str().and_then(Mode::from_cloud) else {
                    continue;
                };
                let temperature = m["enableSettings"]["temperature"]
                    .as_bool()
                    .unwrap_or(false);
                let mut fans: Vec<String> = m["enableFanSpeed"]
                    .as_object()
                    .into_iter()
                    .flatten()
                    .filter(|(_, on)| on.as_bool() == Some(true))
                    .map(|(k, _)| k.clone())
                    .collect();
                fans.sort();
                by_mode.insert(
                    mode,
                    ModeLimits {
                        min: m["minTemperature"].as_f64().unwrap_or(16.0),
                        max: m["maxTemperature"].as_f64().unwrap_or(32.0),
                        step: m["temperatureSettingPitchType"]
                            .as_f64()
                            .filter(|s| *s > 0.0)
                            .unwrap_or(0.5),
                        temperature,
                        fans,
                    },
                );
            }
            models.insert(cloud_id.to_owned(), by_mode);
        }
        Self { models }
    }

    /// What the unit takes; when the account's configuration is unknown,
    /// the common ground (16–32 °C by half degrees, every speed).
    #[must_use]
    pub fn limits(&self, unit: &Unit, mode: Mode) -> ModeLimits {
        self.models
            .get(&unit.cloud_id)
            .and_then(|m| m.get(&mode))
            .cloned()
            .unwrap_or_else(|| ModeLimits {
                min: 16.0,
                max: 32.0,
                step: 0.5,
                temperature: mode != Mode::Fan,
                fans: ["AUTO", "LV1", "LV2", "LV3", "LV4", "LV5"]
                    .into_iter()
                    .map(str::to_owned)
                    .collect(),
            })
    }

    /// The widest range over every mode, for the point's declared bounds.
    #[must_use]
    pub fn overall(&self) -> (f64, f64) {
        let mut range: Option<(f64, f64)> = None;
        for l in self
            .models
            .values()
            .flat_map(HashMap::values)
            .filter(|l| l.temperature)
        {
            range = Some(range.map_or((l.min, l.max), |(a, b)| (a.min(l.min), b.max(l.max))));
        }
        range.unwrap_or((16.0, 32.0))
    }
}

/// One change asked of a unit.
#[derive(Clone, Debug, PartialEq)]
pub enum Change {
    Power(bool),
    Mode(Mode),
    Target(f64),
    Fan(String),
}

/// The unit as it will be once the change is made, or why not.
pub fn apply(unit: &Unit, caps: &Capabilities, change: &Change) -> Result<Unit, String> {
    let mut next = unit.clone();
    match change {
        Change::Power(on) => next.power = *on,
        Change::Mode(mode) => next.mode = Some(*mode),
        Change::Target(t) => next.target = Some(*t),
        Change::Fan(f) => next.fan.clone_from(f),
    }
    let Some(mode) = next.mode else {
        return Err("mode unknown".into());
    };
    let limits = caps.limits(unit, mode);
    if matches!(change, Change::Target(_)) && !limits.temperature {
        return Err(format!("no setpoint in mode {}", mode.moli()));
    }
    if let Some(t) = next.target.filter(|_| limits.temperature) {
        // A mode change keeps the setpoint when the new mode takes it, else
        // the nearest bound (the app does the same).
        if matches!(change, Change::Target(_)) {
            if t < limits.min || t > limits.max {
                return Err(format!(
                    "{t} is out of range [{}, {}]",
                    limits.min, limits.max
                ));
            }
            let steps = (t - limits.min) / limits.step;
            if (steps - steps.round()).abs() > 1e-6 {
                return Err(format!("{t} is not a multiple of {}", limits.step));
            }
        } else {
            next.target = Some(t.clamp(limits.min, limits.max));
        }
    }
    if !limits.fans.contains(&next.fan) {
        if matches!(change, Change::Fan(_)) {
            return Err(format!(
                "fan speed {} is not available in mode {}",
                next.fan,
                mode.moli()
            ));
        }
        // A mode change that does not take the speed falls back to the first allowed.
        if let Some(first) = limits.fans.first() {
            next.fan.clone_from(first);
        }
    }
    Ok(next)
}

/// The order's body: the unit's whole state, as the app sends it.
#[must_use]
pub fn order_body(unit: &Unit) -> Json {
    json!({
        "id": unit.id,
        "sysType": unit.sys_type,
        "power": if unit.power { "ON" } else { "OFF" },
        "mode": unit.mode.map_or("AUTO", Mode::cloud),
        "iduTemperature": unit.target.unwrap_or(25.0),
        "fanSpeed": unit.fan,
        "fanSwing": unit.swing,
        "relativeTemperature": 0,
        "humidity": 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    pub fn sample() -> Json {
        serde_json::from_str(include_str!(
            "../../../integrations/hitachi/fixtures/notification.json"
        ))
        .unwrap()
    }

    fn caps() -> Capabilities {
        Capabilities::from_json(
            &serde_json::from_str(include_str!(
                "../../../integrations/hitachi/fixtures/rac-configuration.json"
            ))
            .unwrap(),
        )
    }

    #[test]
    fn units_are_read() {
        let units = units_of(&sample());
        assert_eq!(units.len(), 2);
        let u = &units[0];
        assert_eq!(u.id, 100_001);
        assert_eq!(u.native_id(), "jch-aabbcc01");
        assert_eq!(u.model(), "RAK-DJ25RHAE");
        assert!(!u.power && u.online && !u.fault);
        assert_eq!(u.mode, Some(Mode::Auto));
        assert_eq!(u.target, Some(25.0));
        assert_eq!(u.room, Some(23.0));
        assert!(units[1].power);
        assert_eq!(units[1].mode, Some(Mode::Heating));
    }

    #[test]
    fn unknown_values_are_not_numbers() {
        let u = Unit::from_json(&json!({
            "id": 1, "vendorThingId": "JCH-1", "iduTemperature": 2_147_483_647, "roomTemperature": 2_147_483_647
        }))
        .unwrap();
        assert_eq!((u.target, u.room), (None, None));
        assert!(Unit::from_json(&json!({"id": 1})).is_none());
    }

    #[test]
    fn modes_round_trip() {
        for m in MODES {
            assert_eq!(Mode::from_cloud(m.cloud()), Some(m));
            assert_eq!(Mode::from_moli(m.moli()), Some(m));
        }
        assert_eq!(fan_to_cloud("3"), Some("LV3"));
        assert_eq!(fan_to_moli("LV5"), Some("5"));
        assert_eq!(fan_to_cloud("9"), None);
    }

    #[test]
    fn limits_come_from_the_configuration() {
        let c = caps();
        let u = &units_of(&sample())[0];
        let cool = c.limits(u, Mode::Cooling);
        assert_eq!((cool.min, cool.max, cool.step), (16.0, 32.0, 0.5));
        assert!(cool.fans.contains(&"LV5".to_owned()));
        let dry = c.limits(u, Mode::Dry);
        assert!(
            !dry.fans.contains(&"LV5".to_owned()),
            "dry takes fewer speeds"
        );
        assert!(!c.limits(u, Mode::Fan).temperature);
        assert_eq!(c.overall(), (16.0, 32.0));
    }

    #[test]
    fn orders_are_checked() {
        let c = caps();
        let u = units_of(&sample()).remove(1);
        assert_eq!(
            apply(&u, &c, &Change::Target(22.5)).unwrap().target,
            Some(22.5)
        );
        assert!(apply(&u, &c, &Change::Target(40.0)).is_err());
        assert!(apply(&u, &c, &Change::Target(22.3)).is_err());
        assert!(apply(&u, &c, &Change::Mode(Mode::Fan)).is_ok());
        let fan = apply(&u, &c, &Change::Mode(Mode::Fan)).unwrap();
        assert!(
            apply(&fan, &c, &Change::Target(22.0)).is_err(),
            "no setpoint in fan mode"
        );
        let dry = apply(&u, &c, &Change::Mode(Mode::Dry)).unwrap();
        assert!(apply(&dry, &c, &Change::Fan("LV5".into())).is_err());
    }

    #[test]
    fn the_body_is_the_whole_state() {
        let u = units_of(&sample()).remove(1);
        let body = order_body(&apply(&u, &caps(), &Change::Power(false)).unwrap());
        assert_eq!(
            body,
            json!({
                "id": 100_002, "sysType": 1, "power": "OFF", "mode": "HEATING",
                "iduTemperature": 25.0, "fanSpeed": "AUTO", "fanSwing": "OFF",
                "relativeTemperature": 0, "humidity": 0
            })
        );
    }
}
