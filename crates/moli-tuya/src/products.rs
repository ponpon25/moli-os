//! Products whose data points the Tuya cloud does not describe (empty
//! specification: the app draws them with its own panel). Their DPs are
//! filled here, by product name, when the device file has none.
//!
//! dé EV Charger (Wi-Fi wallbox, protocol 3.5), read on a real one and
//! cross-checked with evcc's and sowel's dé drivers:
//! - 102 is JSON: per phase `[0.1 V, 0.1 A, 0.1 kW]`, `p` total power in
//!   0.1 kW, `e` session energy in 0.1 kWh, `t` temperature in 0.1 °C;
//!   pushed about once a minute, and not at all while idle.
//! - 109 says the state in words (`SLEEP` free, `IDLEINS` plugged, `PAUSE`…).
//! - 140 starts (`true`) or pauses (`false`) a charge; never reported back.
//! - 150 is the charge current in A, one of the steps listed in 107.
//! - 152 is the installation maximum, 106 `cp` the pilot signal in V.

use crate::model::{Dp, TuyaDevice};

/// The device with its DPs filled in when the file has none and the
/// product is known here.
#[must_use]
pub fn fill(mut dev: TuyaDevice) -> TuyaDevice {
    if dev.dps.is_empty()
        && let Some(dps) = known_dps(dev.product.as_deref())
    {
        dev.dps = dps;
    }
    dev
}

/// The DPs of a product the cloud leaves undescribed, if it is known here.
#[must_use]
pub fn known_dps(product: Option<&str>) -> Option<Vec<Dp>> {
    let product = product?.trim().to_lowercase();
    let dps = match product.as_str() {
        "dé ev charger" => serde_json::json!([
            { "id": 102, "code": "metrics", "type": "Json", "fields": [
                { "code": "cur_voltage", "path": "L1.0", "unit": "V", "scale": 1 },
                { "code": "cur_current", "path": "L1.1", "unit": "A", "scale": 1 },
                { "code": "cur_power", "path": "p", "unit": "kW", "scale": 1 },
                { "code": "session_energy", "path": "e", "unit": "kWh", "scale": 1 },
                { "code": "device_temp", "path": "t", "unit": "°C", "scale": 1 },
            ] },
            { "id": 106, "code": "info", "type": "Json", "fields": [
                { "code": "cp_voltage", "path": "cp", "unit": "V" },
            ] },
            { "id": 109, "code": "work_state", "type": "String" },
            { "id": 140, "code": "charge_switch", "type": "Boolean", "writable": true },
            { "id": 150, "code": "charge_current", "type": "Integer", "unit": "A",
              "writable": true, "min": 6, "max": 16, "step": 1 },
            { "id": 152, "code": "max_current", "type": "Integer", "unit": "A" },
        ]),
        _ => return None,
    };
    serde_json::from_value(dps).ok()
}

#[cfg(test)]
mod tests {
    use moli_core::{InstanceId, Semantic, Unit, Value};

    use super::*;

    fn charger() -> TuyaDevice {
        fill(
            serde_json::from_value(serde_json::json!({
                "id": "bf1", "name": "Borne", "category": "dj", "product": "dé EV Charger "
            }))
            .unwrap(),
        )
    }

    #[test]
    fn unknown_products_stay_empty() {
        assert!(known_dps(Some("Smart Plug")).is_none());
        assert!(known_dps(None).is_none());
    }

    #[test]
    fn the_charger_reads_its_json_metrics() {
        let dev = charger();
        let raw = serde_json::json!({
            "102": "{\"L1\":[2330,144,33],\"L2\":[0,0,0],\"L3\":[0,0,0],\"t\":215,\"p\":33,\"d\":0,\"e\":111}",
            "106": "{\"r\":\"Type B\",\"fv\":\"1.9.1\",\"cp\":\"9.1\",\"t\":\"2850\",\"e\":\"37013\"}",
            "109": "IDLEINS",
            "150": 16,
            "189": 34,
        });
        let values = dev.decode_dps(raw.as_object().unwrap());
        let get = |k: &str| values.iter().find(|(c, _)| c == k).map(|(_, v)| v.clone());
        assert_eq!(get("cur_voltage"), Some(Value::Float(233.0)));
        assert_eq!(get("cur_current"), Some(Value::Float(14.4)));
        assert_eq!(get("cur_power"), Some(Value::Float(3300.0)));
        assert_eq!(get("session_energy"), Some(Value::Float(11.1)));
        assert_eq!(get("device_temp"), Some(Value::Float(21.5)));
        assert_eq!(get("cp_voltage"), Some(Value::Float(9.1)));
        assert_eq!(get("work_state"), Some(Value::Text("IDLEINS".into())));
        assert_eq!(get("charge_current"), Some(Value::Int(16)));
        assert_eq!(values.len(), 8, "unknown DPs and fields are skipped");
    }

    #[test]
    fn the_charger_points_say_what_they_are() {
        let d = charger().device(&InstanceId::from("tuya"));
        let power = d.point("cur_power").unwrap();
        assert_eq!(power.unit, Some(Unit::Watt));
        assert_eq!(power.semantic, Semantic::Power);
        assert!(!power.access.write);
        assert_eq!(d.point("cp_voltage").unwrap().semantic, Semantic::Other);
        assert_eq!(
            d.point("device_temp").unwrap().semantic,
            Semantic::Other,
            "the charger's own heat, not the room's"
        );
        assert!(d.point("charge_current").unwrap().access.write);
        assert!(d.point("charge_switch").unwrap().access.write);
        assert!(d.point("metrics").is_none(), "the JSON itself is no point");
        let dev = charger();
        let current = dev.dp_by_code("charge_current").unwrap();
        assert_eq!(current.encode(&Value::Int(10)), Some(serde_json::json!(10)));
    }
}
