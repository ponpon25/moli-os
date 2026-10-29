//! Z-Wave nodes as Moli devices.
//!
//! Every value Z-Wave JS knows (`commandClass`, `endpoint`, `property`,
//! `propertyKey`, with its metadata) becomes a point, by meaning, never by
//! brand. A `currentValue` and its `targetValue` (a switch, a dimmer) are
//! one point: read from the first, written to the second. What is about the
//! radio itself (versions, associations, settings) stays in Z-Wave JS UI.

use std::collections::BTreeMap;
use std::sync::Arc;

use moli_core::{Access, Device, DeviceId, InstanceId, Kind, PointSpec, Semantic, Unit, Value};
use serde::Deserialize;
use serde_json::{Value as Json, json};

const BASIC: u16 = 0x20;
const BINARY_SWITCH: u16 = 0x25;
const MULTILEVEL_SWITCH: u16 = 0x26;
const BATTERY: u16 = 0x80;

/// About the network, the radio or the settings: not points.
const HIDDEN: &[u16] = &[
    0x22, // Application Status
    0x55, // Transport Service
    0x56, // CRC-16
    0x59, // Association Group Info
    0x5A, // Device Reset Locally
    0x5E, // Z-Wave Plus Info
    0x6C, // Supervision
    0x70, // Configuration (Z-Wave JS UI sets it)
    0x72, // Manufacturer Specific
    0x73, // Powerlevel
    0x77, // Node Naming
    0x7A, // Firmware Update
    0x84, // Wake Up
    0x85, // Association
    0x86, // Version
    0x87, // Indicator
    0x8E, // Multi Channel Association
    0x98, // Security
    0x9F, // Security 2
];

/// Where a value lives on its node.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ValueId {
    pub command_class: u16,
    #[serde(default)]
    pub endpoint: u16,
    pub property: Prop,
    #[serde(default)]
    pub property_key: Option<Prop>,
}

/// A property or property key: a name or a number.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(untagged)]
pub enum Prop {
    Number(i64),
    Name(String),
}

impl Prop {
    fn json(&self) -> Json {
        match self {
            Self::Number(n) => json!(n),
            Self::Name(s) => json!(s),
        }
    }

    fn slug(&self) -> String {
        match self {
            Self::Number(n) => n.to_string(),
            Self::Name(s) => slug(s),
        }
    }
}

impl ValueId {
    /// As the server wants it in `node.set_value`.
    pub fn json(&self) -> Json {
        let mut id = json!({
            "commandClass": self.command_class,
            "endpoint": self.endpoint,
            "property": self.property.json(),
        });
        if let Some(key) = &self.property_key {
            id["propertyKey"] = key.json();
        }
        id
    }

    fn named(&self, name: &str) -> bool {
        self.property == Prop::Name(name.to_owned())
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Metadata {
    #[serde(default, rename = "type")]
    pub kind: String,
    #[serde(default = "yes")]
    pub readable: bool,
    #[serde(default)]
    pub writeable: bool,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub min: Option<f64>,
    #[serde(default)]
    pub max: Option<f64>,
    #[serde(default)]
    pub steps: Option<f64>,
    #[serde(default)]
    pub unit: Option<String>,
    /// Numbers with names (`{"0": "Off", "255": "On"}`).
    #[serde(default)]
    pub states: BTreeMap<String, String>,
}

fn yes() -> bool {
    true
}

/// One value of a node, as `start_listening` and `value added` give it.
#[derive(Clone, Debug, Deserialize)]
pub struct ZValue {
    #[serde(flatten)]
    pub id: ValueId,
    #[serde(default)]
    pub metadata: Metadata,
    #[serde(default)]
    pub value: Option<Json>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceConfig {
    #[serde(default)]
    pub manufacturer: Option<String>,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
}

/// One node, as the server describes it.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Node {
    #[serde(rename = "nodeId")]
    pub id: u16,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub location: Option<String>,
    /// 0 unknown, 1 asleep, 2 awake, 3 dead, 4 alive.
    #[serde(default)]
    pub status: u8,
    #[serde(default)]
    #[serde(rename = "isControllerNode")]
    pub controller: bool,
    #[serde(default)]
    pub device_config: Option<DeviceConfig>,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub values: Vec<ZValue>,
    #[serde(default)]
    pub statistics: Statistics,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Statistics {
    /// When the node was last heard (absent: never, since the server started).
    #[serde(default)]
    pub last_seen: Option<String>,
}

impl Node {
    /// Dead is offline. Asleep (a battery device between two wake-ups) is
    /// online only once heard: a node the stick remembers but never hears
    /// (an old sensor in a drawer) is not.
    pub fn online(&self) -> bool {
        match self.status {
            3 => false,
            0 | 1 => self.statistics.last_seen.is_some(),
            _ => true,
        }
    }
}

/// How a point reads and writes.
#[derive(Clone, Debug, PartialEq)]
pub struct Binding {
    /// Where it is read.
    pub read: ValueId,
    /// Where it is written, if it is.
    pub write: Option<ValueId>,
    /// Named numbers: the name Moli shows, the number Z-Wave takes.
    pub states: Vec<(Arc<str>, i64)>,
}

/// A node as a Moli device, and how each point maps back.
#[derive(Clone, Debug, PartialEq)]
pub struct Mapped {
    pub device: Device,
    pub points: BTreeMap<String, Binding>,
}

/// A point key from free text: lowercase, `_` between words.
fn slug(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            if c.is_ascii_uppercase() && !out.is_empty() && !out.ends_with('_') {
                // currentValue → current_value
                let prev_lower = out.chars().last().is_some_and(|p| p.is_ascii_lowercase());
                if prev_lower {
                    out.push('_');
                }
            }
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('_') && !out.is_empty() {
            out.push('_');
        }
    }
    out.trim_end_matches('_').to_owned()
}

fn cc_name(cc: u16) -> String {
    match cc {
        BASIC => "basic".into(),
        BINARY_SWITCH => "switch".into(),
        MULTILEVEL_SWITCH => "level".into(),
        0x30 => "binary_sensor".into(),
        0x31 => "sensor".into(),
        0x32 => "meter".into(),
        0x33 => "color".into(),
        0x43 => "setpoint".into(),
        0x40 => "thermostat_mode".into(),
        0x5B => "scene".into(),
        0x62 => "lock".into(),
        0x66 => "barrier".into(),
        0x71 => "notification".into(),
        BATTERY => "battery".into(),
        other => format!("cc{other}"),
    }
}

/// The point key of a value: `<endpoint>.<class>.<property>[.<key>]`, the
/// endpoint only when not the root.
fn key_of(id: &ValueId, merged: bool) -> String {
    let mut key = String::new();
    if id.endpoint > 0 {
        key = format!("ep{}.", id.endpoint);
    }
    key.push_str(&cc_name(id.command_class));
    if !merged {
        key.push('.');
        key.push_str(&id.property.slug());
        if let Some(k) = &id.property_key {
            key.push('.');
            key.push_str(&k.slug());
        }
    }
    key
}

fn semantic(id: &ValueId, key: &str, unit: Option<&Unit>, merged: bool, kind: &Kind) -> Semantic {
    match id.command_class {
        BINARY_SWITCH if merged => return Semantic::OnOff,
        MULTILEVEL_SWITCH if merged => return Semantic::Brightness,
        BATTERY if id.named("level") => return Semantic::Battery,
        BATTERY if id.named("isLow") => return Semantic::BatteryLow,
        _ => {}
    }
    let s = Semantic::infer(key, unit);
    // A name alone says nothing for a number about the radio.
    if s == Semantic::OnOff && *kind != Kind::Binary {
        return Semantic::Other;
    }
    s
}

/// A node's device: `None` for the controller (the stick itself).
#[allow(clippy::too_many_lines)] // one value after the other, each point whole
pub fn map(instance: &InstanceId, home: u32, node: &Node) -> Option<Mapped> {
    if node.controller {
        return None;
    }
    let config = node.device_config.clone().unwrap_or_default();
    let has_switch = node
        .values
        .iter()
        .any(|v| matches!(v.id.command_class, BINARY_SWITCH | MULTILEVEL_SWITCH));
    let mut points = Vec::new();
    let mut bindings = BTreeMap::new();
    for v in &node.values {
        let id = &v.id;
        if HIDDEN.contains(&id.command_class) || (id.command_class == BASIC && has_switch) {
            continue;
        }
        // A target is written through its current value.
        let target_of_current = id.named("targetValue")
            && node.values.iter().any(|o| {
                o.id.named("currentValue")
                    && o.id.command_class == id.command_class
                    && o.id.endpoint == id.endpoint
            });
        if target_of_current || !v.metadata.readable {
            continue;
        }
        let target = id.named("currentValue").then(|| {
            node.values
                .iter()
                .find(|o| {
                    o.id.named("targetValue")
                        && o.id.command_class == id.command_class
                        && o.id.endpoint == id.endpoint
                        && o.metadata.writeable
                })
                .map(|o| o.id.clone())
        });
        let merged = matches!(target, Some(Some(_)));
        let write = match target {
            Some(t) => t,
            None => v.metadata.writeable.then(|| id.clone()),
        };
        let m = &v.metadata;
        let states: Vec<(Arc<str>, i64)> = m
            .states
            .iter()
            .filter_map(|(n, name)| Some((Arc::from(name.as_str()), n.parse().ok()?)))
            .collect();
        let kind = match m.kind.as_str() {
            "boolean" => Kind::Binary,
            "number" if !states.is_empty() => Kind::Enum {
                values: states.iter().map(|(n, _)| Arc::clone(n)).collect(),
            },
            "number" => Kind::Numeric {
                min: m.min,
                max: m.max,
                step: m.steps,
            },
            "string" => Kind::Text,
            // Durations, colours, buffers: not a value a point holds.
            _ => continue,
        };
        let key = key_of(id, merged);
        let unit = m.unit.as_deref().and_then(Unit::parse);
        let label = match (id.command_class, merged) {
            (BINARY_SWITCH, true) => moli_i18n::tr!("pilotes.zwave.marche"),
            (MULTILEVEL_SWITCH, true) => moli_i18n::tr!("pilotes.zwave.niveau"),
            _ => m.label.clone().unwrap_or_else(|| key.clone()),
        };
        let label = if id.endpoint > 0 {
            format!("{label} ({})", id.endpoint)
        } else {
            label
        };
        points.push(PointSpec {
            key: key.as_str().into(),
            label: label.into(),
            semantic: semantic(id, &key, unit.as_ref(), merged, &kind),
            kind,
            access: Access {
                read: true,
                write: write.is_some(),
            },
            unit,
        });
        bindings.insert(
            key,
            Binding {
                read: id.clone(),
                write,
                states,
            },
        );
    }
    let model = config.label.clone().or_else(|| node.label.clone());
    let name = node
        .name
        .clone()
        .filter(|n| !n.trim().is_empty())
        .or_else(|| config.description.clone())
        .or_else(|| model.clone())
        .unwrap_or_else(|| format!("Z-Wave {}", node.id));
    Some(Mapped {
        device: Device {
            id: device_id(instance, home, node.id),
            instance: instance.clone(),
            native_name: name.into(),
            manufacturer: config.manufacturer.map(Into::into),
            model: model.map(Into::into),
            description: config.description.map(Into::into),
            native_room: node
                .location
                .clone()
                .filter(|l| !l.trim().is_empty())
                .map(Into::into),
            members: Vec::new(),
            points,
        },
        points: bindings,
    })
}

/// `<instance>:<home id>-<node>`: a new stick (another home id) never
/// mistakes its nodes for the old ones.
pub fn device_id(instance: &InstanceId, home: u32, node: u16) -> DeviceId {
    DeviceId::new(instance, &format!("{home:08x}-{node}"))
}

impl Mapped {
    /// The point a value feeds, with the value as Moli holds it.
    pub fn reading(&self, id: &ValueId, raw: &Json) -> Option<(&str, Value)> {
        let (key, b) = self.points.iter().find(|(_, b)| b.read == *id)?;
        let value = match raw
            .as_i64()
            .and_then(|n| b.states.iter().find(|(_, v)| *v == n))
        {
            Some((name, _)) => Value::Text(Arc::clone(name)),
            None if raw.is_object() || raw.is_array() => return None,
            None => Value::from_json(raw),
        };
        Some((key, value))
    }

    /// Every value the node holds now, by point.
    pub fn readings<'a>(&'a self, node: &'a Node) -> impl Iterator<Item = (&'a str, Value)> + 'a {
        node.values
            .iter()
            .filter_map(|v| self.reading(&v.id, v.value.as_ref()?))
    }

    /// What to send for an order on `key`: the value id and the value.
    pub fn order(&self, key: &str, value: &Value) -> Result<(Json, Json), String> {
        let b = self.points.get(key).ok_or("unknown point")?;
        let target = b.write.as_ref().ok_or("read-only point")?;
        let wire = match value {
            Value::Text(name) if !b.states.is_empty() => {
                let (_, n) = b
                    .states
                    .iter()
                    .find(|(s, _)| **s == **name)
                    .ok_or("unknown choice")?;
                json!(n)
            }
            Value::Bool(v) => json!(v),
            Value::Int(n) => json!(n),
            // Z-Wave numbers are mostly whole: 50.0 goes as 50.
            #[allow(clippy::cast_possible_truncation)]
            Value::Float(f) if f.fract() == 0.0 && f.abs() < 1e15 => json!(*f as i64),
            Value::Float(f) => json!(f),
            Value::Text(t) => json!(t.as_ref()),
            Value::Null => return Err("no value".into()),
        };
        Ok((target.json(), wire))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> Json {
        serde_json::from_str(include_str!(
            "../../../integrations/zwave/fixtures/state.json"
        ))
        .unwrap()
    }

    fn nodes() -> Vec<Node> {
        serde_json::from_value(state()["nodes"].clone()).unwrap()
    }

    #[test]
    fn slugs_are_point_keys() {
        assert_eq!(slug("currentValue"), "current_value");
        assert_eq!(slug("Air temperature"), "air_temperature");
        assert_eq!(slug("Power Management"), "power_management");
        assert_eq!(slug("isLow"), "is_low");
    }

    #[test]
    fn a_relay_is_one_switch_and_its_sensors() {
        let instance = InstanceId::from("zwave");
        let nodes = nodes();
        assert!(
            map(&instance, 0xABCD_0001, &nodes[0]).is_none(),
            "the stick"
        );
        let relay = map(&instance, 0xABCD_0001, &nodes[1]).unwrap();
        assert_eq!(relay.device.id.to_string(), "zwave:abcd0001-2");
        assert_eq!(&*relay.device.native_name, "Portail");
        assert_eq!(relay.device.model.as_deref(), Some("ZMNHND1"));
        let keys: Vec<&str> = relay.device.points.iter().map(|p| &*p.key).collect();
        assert_eq!(
            keys,
            ["switch", "sensor.air_temperature", "meter.value.66049"]
        );
        let on = &relay.device.points[0];
        assert_eq!(
            (on.semantic, on.access.write, &on.kind),
            (Semantic::OnOff, true, &Kind::Binary)
        );
        assert_eq!(relay.device.points[1].semantic, Semantic::Temperature);
        assert_eq!(relay.device.points[2].semantic, Semantic::Power);
        // Read from the current value, written to the target.
        let (id, value) = relay.order("switch", &Value::Bool(true)).unwrap();
        assert_eq!(
            id,
            json!({"commandClass": 37, "endpoint": 0, "property": "targetValue"})
        );
        assert_eq!(value, json!(true));
        let readings: Vec<(&str, Value)> = relay.readings(&nodes[1]).collect();
        assert!(readings.contains(&("switch", Value::Bool(false))));
        assert!(readings.contains(&("sensor.air_temperature", Value::Float(21.4))));
        assert_eq!(
            relay
                .order("sensor.air_temperature", &Value::Float(1.0))
                .unwrap_err(),
            "read-only point"
        );
    }

    #[test]
    fn named_numbers_are_choices() {
        let instance = InstanceId::from("zwave");
        let nodes = nodes();
        let sensor = map(&instance, 1, &nodes[2]).unwrap();
        let keys: Vec<&str> = sensor.device.points.iter().map(|p| &*p.key).collect();
        assert_eq!(
            keys,
            [
                "battery.level",
                "battery.is_low",
                "notification.access_control.door_state"
            ]
        );
        assert_eq!(sensor.device.points[0].semantic, Semantic::Battery);
        assert_eq!(sensor.device.points[1].semantic, Semantic::BatteryLow);
        let door = &sensor.device.points[2];
        assert_eq!(
            door.kind,
            Kind::Enum {
                values: vec!["Window/door is open".into(), "Window/door is closed".into()]
            }
        );
        let id = ValueId {
            command_class: 0x71,
            endpoint: 0,
            property: Prop::Name("Access Control".into()),
            property_key: Some(Prop::Name("Door state".into())),
        };
        assert_eq!(
            sensor.reading(&id, &json!(23)),
            Some((
                "notification.access_control.door_state",
                Value::Text("Window/door is closed".into())
            ))
        );
        // Asleep but heard (a last-seen date): online; never heard: not.
        assert!(nodes[2].online());
        let mut never = nodes[2].clone();
        never.statistics.last_seen = None;
        assert!(!never.online());
        assert_eq!(&*sensor.device.native_name, "Door sensor");
        assert_eq!(sensor.device.native_room.as_deref(), Some("Garage"));
    }
}
