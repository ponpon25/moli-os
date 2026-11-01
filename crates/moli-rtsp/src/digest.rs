//! HTTP Digest authentication: as RTSP uses it (RFC 2069 form, MD5, no
//! `qop`), and as HTTP clients answer it (RFC 2617, `qop=auth`, for the
//! ONVIF snapshot). The password never travels.
//!
//! The nonce is the server's, not a connection's: players (VLC) ask for the
//! password on one connection and answer on the next, with the nonce they
//! were given. It changes every hour; the one before stays good, and an
//! answer to an older one is told *stale* (asked again, not counted wrong).

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

/// The challenge header for `nonce`; `stale`: the password was right, the
/// nonce too old (the client answers again without asking anyone).
#[must_use]
pub fn challenge(nonce: &str, stale: bool) -> String {
    let stale = if stale { ", stale=TRUE" } else { "" };
    format!("Digest realm=\"{REALM}\", nonce=\"{nonce}\", algorithm=MD5{stale}")
}

/// The challenge of an HTTP resource (`qop="auth"`, what HTTP clients expect).
#[must_use]
pub fn http_challenge(nonce: &str, stale: bool) -> String {
    let stale = if stale { ", stale=TRUE" } else { "" };
    format!("Digest realm=\"{REALM}\", qop=\"auth\", nonce=\"{nonce}\", algorithm=MD5{stale}")
}

/// The server's nonces: a new one every [`NONCE_LIFE`], the one before still
/// good.
#[derive(Debug)]
pub struct Nonces {
    current: String,
    previous: String,
    since: std::time::Instant,
}

pub const NONCE_LIFE: std::time::Duration = std::time::Duration::from_secs(3600);

impl Default for Nonces {
    fn default() -> Self {
        Self {
            current: nonce(),
            previous: nonce(),
            since: std::time::Instant::now(),
        }
    }
}

impl Nonces {
    /// The nonce to give, and the ones accepted.
    pub fn now(&mut self) -> (String, [String; 2]) {
        if self.since.elapsed() >= NONCE_LIFE {
            self.previous = std::mem::replace(&mut self.current, nonce());
            self.since = std::time::Instant::now();
        }
        (
            self.current.clone(),
            [self.current.clone(), self.previous.clone()],
        )
    }
}

/// What an `Authorization` header proves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Check {
    Proved,
    /// The right password, on a nonce no longer accepted.
    Stale,
    /// Not the password (or not a header Moli reads).
    Wrong,
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

/// What `header` proves of the password for this `method`, given the
/// nonces accepted.
#[must_use]
pub fn check(header: &str, method: &str, nonces: &[String], user: &str, password: &str) -> Check {
    let Some(params) = params(header) else {
        return Check::Wrong;
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
        return Check::Wrong;
    };
    if get("algorithm").is_some_and(|a| !a.eq_ignore_ascii_case("md5")) {
        return Check::Wrong;
    }
    // With `qop=auth`, the client's counter and nonce enter the answer.
    let qop = match get("qop") {
        None => None,
        Some(q) if q.eq_ignore_ascii_case("auth") => match (get("nc"), get("cnonce")) {
            (Some(nc), Some(cnonce)) if nc.len() <= 16 && cnonce.len() <= 128 => Some((nc, cnonce)),
            _ => return Check::Wrong,
        },
        Some(_) => return Check::Wrong,
    };
    if !equal(username, user) || realm != REALM || given_nonce.len() > 64 {
        return Check::Wrong;
    }
    let ha1 = md5_hex(&format!("{user}:{REALM}:{password}"));
    let ha2 = md5_hex(&format!("{method}:{uri}"));
    let expected = match qop {
        None => md5_hex(&format!("{ha1}:{given_nonce}:{ha2}")),
        Some((nc, cnonce)) => md5_hex(&format!("{ha1}:{given_nonce}:{nc}:{cnonce}:auth:{ha2}")),
    };
    if !equal(&response.to_ascii_lowercase(), &expected) {
        Check::Wrong
    } else if nonces.iter().any(|n| equal(n, given_nonce)) {
        Check::Proved
    } else {
        Check::Stale
    }
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
        let ok = [n.clone()];
        let uri = "rtsp://192.168.1.2:8554/door";
        let good = answer("moli", "secret", "DESCRIBE", uri, &n);
        assert_eq!(
            check(&good, "DESCRIBE", &ok, "moli", "secret"),
            Check::Proved
        );
        assert_eq!(check(&good, "DESCRIBE", &ok, "moli", "other"), Check::Wrong);
        assert_eq!(check(&good, "SETUP", &ok, "moli", "secret"), Check::Wrong);
        assert_eq!(
            check(&good, "DESCRIBE", &[nonce()], "moli", "secret"),
            Check::Stale
        );
        assert_eq!(
            check(&good, "DESCRIBE", &ok, "admin", "secret"),
            Check::Wrong
        );
        for bad in ["Basic bW9saTpzZWNyZXQ=", "Digest garbage", ""] {
            assert_eq!(check(bad, "DESCRIBE", &ok, "moli", "secret"), Check::Wrong);
        }
    }

    #[test]
    fn the_nonce_before_stays_good_for_an_hour() {
        let mut nonces = Nonces::default();
        let (first, ok) = nonces.now();
        assert!(ok.contains(&first));
        nonces.since -= NONCE_LIFE;
        let (second, ok) = nonces.now();
        assert_ne!(first, second);
        assert!(ok.contains(&first) && ok.contains(&second));
    }

    #[test]
    fn an_http_client_answer_with_qop_is_read() {
        // RFC 2617 §3.5, with this realm and password.
        let n = "dcd98b7102dd2f0e8b11d0f600bfb0c093";
        let ha1 = md5_hex(&format!("moli:{REALM}:pw"));
        let ha2 = md5_hex("GET:/onvif/snapshot.jpg");
        let r = md5_hex(&format!("{ha1}:{n}:00000001:0a4f113b:auth:{ha2}"));
        let header = format!(
            "Digest username=\"moli\", realm=\"Moli\", nonce=\"{n}\", uri=\"/onvif/snapshot.jpg\", qop=auth, nc=00000001, cnonce=\"0a4f113b\", response=\"{r}\""
        );
        assert_eq!(
            check(&header, "GET", &[n.to_owned()], "moli", "pw"),
            Check::Proved
        );
        assert_eq!(
            check(&header, "GET", &[n.to_owned()], "moli", "no"),
            Check::Wrong
        );
        let bad_qop = header.replace("qop=auth", "qop=auth-int");
        assert_eq!(
            check(&bad_qop, "GET", &[n.to_owned()], "moli", "pw"),
            Check::Wrong
        );
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
        assert_eq!(
            check(&header, "SETUP", &[n.to_owned()], "moli", "pw"),
            Check::Proved
        );
    }
}
