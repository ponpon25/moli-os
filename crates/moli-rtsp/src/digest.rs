//! HTTP Digest authentication as RTSP uses it (RFC 2069 form, MD5, no
//! `qop`): what recorders and players send. The password never travels;
//! the nonce is new for each connection.

use md5::{Digest as _, Md5};
use ring::rand::{SecureRandom as _, SystemRandom};

pub const REALM: &str = "Moli";

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut s, b| {
            let _ = write!(s, "{b:02x}");
            s
        })
}

fn md5_hex(input: &str) -> String {
    hex(&Md5::digest(input.as_bytes()))
}

/// A fresh nonce: 16 random bytes, in hex.
#[must_use]
pub fn nonce() -> String {
    let mut bytes = [0u8; 16];
    SystemRandom::new()
        .fill(&mut bytes)
        .expect("the system's random source");
    hex(&bytes)
}

/// The challenge header for `nonce`.
#[must_use]
pub fn challenge(nonce: &str) -> String {
    format!("Digest realm=\"{REALM}\", nonce=\"{nonce}\", algorithm=MD5")
}

/// The parameters of an `Authorization: Digest …` header.
fn params(header: &str) -> Option<Vec<(String, String)>> {
    let rest = header.trim();
    let (scheme, rest) = rest.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("digest") {
        return None;
    }
    let mut out = Vec::new();
    let mut rest = rest.trim_start();
    while !rest.is_empty() {
        let (name, after) = rest.split_once('=')?;
        let name = name.trim().to_ascii_lowercase();
        let after = after.trim_start();
        let (value, after) = if let Some(quoted) = after.strip_prefix('"') {
            let end = quoted.find('"')?;
            (&quoted[..end], &quoted[end + 1..])
        } else {
            let end = after.find(',').unwrap_or(after.len());
            (after[..end].trim(), &after[end..])
        };
        out.push((name, value.to_owned()));
        rest = after.trim_start().trim_start_matches(',').trim_start();
    }
    Some(out)
}

fn equal(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .fold(0u8, |acc, (x, y)| acc | (x ^ y))
            == 0
}

/// Whether `header` proves the password for this `method`, with this
/// connection's `nonce`.
#[must_use]
pub fn verify(header: &str, method: &str, nonce: &str, user: &str, password: &str) -> bool {
    let Some(params) = params(header) else {
        return false;
    };
    let get = |name: &str| {
        params
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    };
    let (Some(username), Some(realm), Some(given_nonce), Some(uri), Some(response)) = (
        get("username"),
        get("realm"),
        get("nonce"),
        get("uri"),
        get("response"),
    ) else {
        return false;
    };
    if get("qop").is_some() || get("algorithm").is_some_and(|a| !a.eq_ignore_ascii_case("md5")) {
        return false;
    }
    if !equal(username, user) || realm != REALM || !equal(given_nonce, nonce) {
        return false;
    }
    let ha1 = md5_hex(&format!("{user}:{REALM}:{password}"));
    let ha2 = md5_hex(&format!("{method}:{uri}"));
    let expected = md5_hex(&format!("{ha1}:{nonce}:{ha2}"));
    equal(&response.to_ascii_lowercase(), &expected)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn answer(user: &str, password: &str, method: &str, uri: &str, nonce: &str) -> String {
        let ha1 = md5_hex(&format!("{user}:{REALM}:{password}"));
        let ha2 = md5_hex(&format!("{method}:{uri}"));
        let response = md5_hex(&format!("{ha1}:{nonce}:{ha2}"));
        format!(
            "Digest username=\"{user}\", realm=\"{REALM}\", nonce=\"{nonce}\", uri=\"{uri}\", response=\"{response}\""
        )
    }

    #[test]
    fn md5_is_the_reference() {
        assert_eq!(md5_hex(""), "d41d8cd98f00b204e9800998ecf8427e");
    }

    #[test]
    fn the_right_password_passes_and_nothing_else() {
        let n = nonce();
        let uri = "rtsp://192.168.1.2:8554/door";
        let good = answer("moli", "secret", "DESCRIBE", uri, &n);
        assert!(verify(&good, "DESCRIBE", &n, "moli", "secret"));
        assert!(!verify(&good, "DESCRIBE", &n, "moli", "other"));
        assert!(!verify(&good, "SETUP", &n, "moli", "secret"));
        assert!(!verify(&good, "DESCRIBE", &nonce(), "moli", "secret"));
        assert!(!verify(&good, "DESCRIBE", &n, "admin", "secret"));
        assert!(!verify(
            "Basic bW9saTpzZWNyZXQ=",
            "DESCRIBE",
            &n,
            "moli",
            "secret"
        ));
        assert!(!verify("Digest garbage", "DESCRIBE", &n, "moli", "secret"));
        assert!(!verify("", "DESCRIBE", &n, "moli", "secret"));
    }

    #[test]
    fn the_header_is_read_as_players_write_it() {
        // ffmpeg's order, unquoted algorithm, extra spaces.
        let n = "0123456789abcdef0123456789abcdef";
        let uri = "rtsp://h/door";
        let ha1 = md5_hex(&format!("moli:{REALM}:pw"));
        let ha2 = md5_hex(&format!("SETUP:{uri}"));
        let r = md5_hex(&format!("{ha1}:{n}:{ha2}"));
        let header = format!(
            "Digest username=\"moli\",realm=\"Moli\" , nonce=\"{n}\", uri=\"{uri}\", response=\"{}\", algorithm=MD5",
            r.to_ascii_uppercase()
        );
        assert!(verify(&header, "SETUP", n, "moli", "pw"));
    }
}
