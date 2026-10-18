//! What the panel says and what it is asked: the alarm's states, the arm
//! and disarm commands, and the shapes of the answers that matter. Pure
//! (no network), so every rule is tested.

use std::sync::Arc;

use serde_json::Value as Json;

/// The modes a person can choose (the point `mode`). « partial » is the
/// panel's partial arming: Verisure's `ARMDAY1`, the one this kind of panel
/// (the Central Unit, no perimeter) takes; it refuses `ARMNIGHT1`. Perimeter
/// and annex variants are reported by `state` but not offered.
pub const MODES: [&str; 3] = ["disarmed", "partial", "total"];

#[must_use]
pub fn mode_values() -> Vec<Arc<str>> {
    MODES.iter().map(|m| Arc::from(*m)).collect()
}

/// The state a panel code stands for (`protomResponse`).
#[must_use]
pub fn state_of(code: &str) -> Option<&'static str> {
    Some(match code.trim() {
        "D" => "disarmed",
        "E" => "perimeter",
        // The panel's partial arming, under its day or its night code.
        "P" | "Q" => "partial",
        "B" => "partial_day_perimeter",
        "C" => "partial_night_perimeter",
        "T" => "total",
        "A" => "total_perimeter",
        "X" => "annex",
        "R" => "partial_day_annex",
        "S" => "partial_night_annex",
        "O" => "total_annex",
        _ => return None,
    })
}

/// Whether the perimeter is armed in this state code.
fn perimeter_armed(code: &str) -> bool {
    matches!(code.trim(), "E" | "B" | "C" | "A")
}

/// The command for a mode, given the panel's current code (a disarm also
/// lifts the perimeter when it is armed).
#[must_use]
pub fn command_for(mode: &str, current: &str) -> Option<&'static str> {
    Some(match mode {
        "disarmed" if perimeter_armed(current) => "DARM1DARMPERI",
        "disarmed" => "DARM1",
        "partial" => "ARMDAY1",
        "total" => "ARM1",
        _ => return None,
    })
}

#[must_use]
pub fn is_disarm(command: &str) -> bool {
    command.starts_with("DARM")
}

/// The mode a state counts as for the writable point (`None`: a state the
/// point cannot say, such as a perimeter alone).
#[must_use]
pub fn mode_of(state: &str) -> Option<&'static str> {
    MODES.iter().copied().find(|m| *m == state)
}

/// The capabilities a panel announces for one installation, read from the
/// capabilities token (`installations[].cap`, matched on `ins`).
#[must_use]
pub fn capability_list(token: &str, number: &str) -> Vec<String> {
    let Some(bytes) = token.split('.').nth(1).and_then(base64url) else {
        return Vec::new();
    };
    let Ok(claims) = serde_json::from_slice::<Json>(&bytes) else {
        return Vec::new();
    };
    claims["installations"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|i| {
            i["ins"].as_str().is_some_and(|n| n == number)
                || i["ins"].as_u64().is_some_and(|n| n.to_string() == number)
        })
        .and_then(|i| i["cap"].as_array())
        .into_iter()
        .flatten()
        .filter_map(|c| c.as_str().map(str::to_owned))
        .collect()
}

/// An installation of the account.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Installation {
    pub number: String,
    pub alias: String,
    pub panel: String,
}

/// The installations in an `xSInstallations` answer.
#[must_use]
pub fn installations(data: &Json) -> Vec<Installation> {
    data["data"]["xSInstallations"]["installations"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|i| {
            let number = i["numinst"].as_str()?.to_owned();
            // Ids go into headers and variables: only what Verisure uses.
            if number.is_empty() || !number.chars().all(|c| c.is_ascii_alphanumeric()) {
                return None;
            }
            Some(Installation {
                number,
                alias: i["alias"].as_str().unwrap_or_default().to_owned(),
                panel: i["panel"].as_str().unwrap_or_default().to_owned(),
            })
        })
        .collect()
}

/// Whether an answer to a device validation says the second factor was
/// accepted, with or without tokens: Verisure then trusts this device, and
/// a normal login gets the tokens.
#[must_use]
pub fn mfa_accepted(answer: &Json) -> bool {
    let data = &answer["data"]["xSValidateDevice"];
    let says = |text: Option<&str>| text.is_some_and(|t| t.contains("mfa.success"));
    says(data["msg"].as_str())
        || says(answer["errors"][0]["message"].as_str())
        || (data["res"].as_str() == Some("OK") && !data["msg"].as_str().is_some_and(|m| m.contains("error")))
}

/// The phones a second factor can go to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Phone {
    pub id: i64,
    pub number: String,
}

/// The second-factor challenge in an error's `data`: its hash and phones.
#[must_use]
pub fn challenge(data: &Json) -> Option<(String, Vec<Phone>)> {
    let hash = data["auth-otp-hash"].as_str()?.to_owned();
    let phones = data["auth-phones"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|p| {
            Some(Phone {
                id: p["id"].as_i64()?,
                number: p["phone"].as_str()?.to_owned(),
            })
        })
        .collect();
    Some((hash, phones))
}

/// The expiry (`exp`, seconds) of a JWT, read without checking it: the
/// token comes from Verisure over TLS, this only tells when to renew it.
#[must_use]
pub fn jwt_exp(token: &str) -> Option<u64> {
    let payload = token.split('.').nth(1)?;
    let bytes = base64url(payload)?;
    let claims: Json = serde_json::from_slice(&bytes).ok()?;
    claims["exp"].as_u64()
}

fn base64url(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0u32);
    for c in text.trim_end_matches('=').bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'-' | b'+' => 62,
            b'_' | b'/' => 63,
            _ => return None,
        };
        acc = (acc << 6) | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(u8::try_from((acc >> bits) & 0xFF).ok()?);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn panel_codes_become_states() {
        for (code, state) in [
            ("D", "disarmed"),
            ("P", "partial"),
            ("Q", "partial"),
            ("T", "total"),
            ("A", "total_perimeter"),
            ("E", "perimeter"),
        ] {
            assert_eq!(state_of(code), Some(state));
        }
        assert_eq!(state_of("Z"), None);
        assert_eq!(state_of(""), None);
    }

    #[test]
    fn modes_become_commands() {
        assert_eq!(command_for("total", "D"), Some("ARM1"));
        // The panel's one partial arming: only the command it takes.
        assert_eq!(command_for("partial", "D"), Some("ARMDAY1"));
        assert_eq!(command_for("disarmed", "T"), Some("DARM1"));
        // A disarm also lifts the perimeter when it is armed.
        assert_eq!(command_for("disarmed", "A"), Some("DARM1DARMPERI"));
        assert_eq!(command_for("explode", "D"), None);
        assert_eq!(command_for("partial_night", "D"), None);
        assert!(is_disarm("DARM1"));
        assert!(is_disarm("DARM1DARMPERI"));
        assert!(!is_disarm("ARM1"));
    }

    #[test]
    fn only_choosable_states_are_modes() {
        assert_eq!(mode_of("partial"), Some("partial"));
        assert_eq!(mode_of("partial_day_perimeter"), None);
        assert_eq!(mode_of("total"), Some("total"));
        assert_eq!(mode_of("perimeter"), None);
        assert_eq!(mode_of("total_perimeter"), None);
    }

    #[test]
    fn installations_are_read_and_checked() {
        let answer = json!({"data": {"xSInstallations": {"installations": [
            {"numinst": "123456", "alias": "Maison", "panel": "SDVFAST"},
            {"numinst": "../x", "alias": "bad", "panel": "P"},
            {"alias": "no number"}
        ]}}});
        let all = installations(&answer);
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].number, "123456");
        assert_eq!(all[0].panel, "SDVFAST");
    }

    /// A hand-written example of the installation list's shape.
    const INSTALLATIONS: &str = include_str!("../../../integrations/verisure/fixtures/installations.json");

    #[test]
    fn the_installation_list_example_is_read() {
        let answer: Json = serde_json::from_str(INSTALLATIONS).unwrap();
        let all = installations(&answer);
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].number, "1234567");
        assert_eq!(all[0].alias, "Maison");
        assert_eq!(all[0].panel, "SDVFAST");
    }

    #[test]
    fn an_accepted_second_factor_is_recognised() {
        let in_data = json!({"data": {"xSValidateDevice": {"res": "OK", "msg": "auth-api.mfa.success", "hash": null}}});
        assert!(mfa_accepted(&in_data));
        let in_errors = json!({"errors": [{"message": "auth-api.mfa.success"}], "data": null});
        assert!(mfa_accepted(&in_errors));
        let refused = json!({"data": {"xSValidateDevice": {"res": "ERROR", "msg": "auth-api.mfa.error"}}});
        assert!(!mfa_accepted(&refused));
        assert!(!mfa_accepted(&json!({})));
    }

    #[test]
    fn a_challenge_lists_its_phones() {
        let data = json!({"auth-otp-hash": "h4sh", "auth-phones": [
            {"id": 0, "phone": "XXXXX1234"}, {"id": 1, "phone": "XXXXX9876"}, {"phone": "no id"}
        ]});
        let (hash, phones) = challenge(&data).unwrap();
        assert_eq!(hash, "h4sh");
        assert_eq!(phones.len(), 2);
        assert_eq!(phones[1], Phone { id: 1, number: "XXXXX9876".into() });
        assert!(challenge(&json!({"other": 1})).is_none());
    }

    #[test]
    fn the_capabilities_of_an_installation_are_read() {
        // payload: {"installations":[{"ins":"111","cap":["A"]},{"ins":"222","cap":["ARMDAY","ARMNIGHT"]}]}
        let payload = "eyJpbnN0YWxsYXRpb25zIjpbeyJpbnMiOiIxMTEiLCJjYXAiOlsiQSJdfSx7Imlucy\
                       I6IjIyMiIsImNhcCI6WyJBUk1EQVkiLCJBUk1OSUdIVCJdfV19";
        let token = format!("h.{payload}.s");
        assert_eq!(capability_list(&token, "222"), vec!["ARMDAY", "ARMNIGHT"]);
        assert_eq!(capability_list(&token, "111"), vec!["A"]);
        assert!(capability_list(&token, "333").is_empty());
        assert!(capability_list("not a token", "222").is_empty());
    }

    #[test]
    fn a_token_expiry_is_read() {
        // {"alg":"none"} . {"exp":1700000000} . (no signature)
        let token = "eyJhbGciOiJub25lIn0.eyJleHAiOjE3MDAwMDAwMDB9.";
        assert_eq!(jwt_exp(token), Some(1_700_000_000));
        assert_eq!(jwt_exp("not a token"), None);
        assert_eq!(jwt_exp("a.%%%.c"), None);
    }
}
