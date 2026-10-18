//! The setup code people type (or scan) in the Home app, and the little
//! values around it: the bridge's pairing identifier, the setup id, the
//! `X-HM://` address a QR code carries.

use ring::digest::{self, SHA512};
use ring::rand::{SecureRandom, SystemRandom};

/// HAP accessory category « Bridge ».
pub const CATEGORY_BRIDGE: u64 = 2;

/// Codes the Home app refuses (and that any attacker would try first).
const TRIVIAL: [&str; 12] = [
    "00000000", "11111111", "22222222", "33333333", "44444444", "55555555", "66666666", "77777777",
    "88888888", "99999999", "12345678", "87654321",
];

fn random(bytes: &mut [u8]) {
    SystemRandom::new().fill(bytes).expect("system randomness");
}

/// A new random setup code, `123-45-678`.
pub fn new_code() -> String {
    loop {
        let mut bytes = [0u8; 8];
        random(&mut bytes);
        let n = u64::from_le_bytes(bytes) % 100_000_000;
        let digits = format!("{n:08}");
        if !TRIVIAL.contains(&digits.as_str()) {
            return format!("{}-{}-{}", &digits[..3], &digits[3..5], &digits[5..]);
        }
    }
}

/// Whether a stored value is a well-formed `123-45-678` code.
pub fn is_code(code: &str) -> bool {
    let b = code.as_bytes();
    b.len() == 10
        && b[3] == b'-'
        && b[6] == b'-'
        && b.iter()
            .enumerate()
            .all(|(i, c)| i == 3 || i == 6 || c.is_ascii_digit())
        && !TRIVIAL.contains(&code.replace('-', "").as_str())
}

/// The bridge's pairing identifier, shaped like a MAC address
/// (`AA:BB:CC:DD:EE:FF`, locally administered), unique to this install.
pub fn new_device_id() -> String {
    let mut b = [0u8; 6];
    random(&mut b);
    b[0] = (b[0] | 0x02) & 0xfe;
    b.iter()
        .map(|x| format!("{x:02X}"))
        .collect::<Vec<_>>()
        .join(":")
}

/// Four characters of `0-9A-Z`, the QR code's setup id.
pub fn new_setup_id() -> String {
    const ALPHABET: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ";
    let mut b = [0u8; 4];
    random(&mut b);
    b.iter()
        .map(|x| char::from(ALPHABET[usize::from(*x) % ALPHABET.len()]))
        .collect()
}

/// `X-HM://` + 9 base-36 characters + the setup id: what the QR code holds.
pub fn uri(code: &str, setup_id: &str, category: u64) -> String {
    let digits: u64 = code.replace('-', "").parse().unwrap_or(0);
    // version 0 · reserved 0 · category · flags (2 = IP) · setup code
    let payload = digits | (2 << 27) | (category << 31);
    format!("X-HM://{}{setup_id}", base36(payload, 9))
}

fn base36(mut n: u64, width: usize) -> String {
    const DIGITS: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ";
    let mut out = Vec::new();
    while n > 0 {
        out.push(DIGITS[(n % 36) as usize]);
        n /= 36;
    }
    while out.len() < width {
        out.push(b'0');
    }
    out.reverse();
    String::from_utf8(out).expect("ascii")
}

/// `sh` of the advertisement: base64 of the first 4 bytes of
/// SHA-512(setup id + device id).
pub fn setup_hash(setup_id: &str, device_id: &str) -> String {
    let hash = digest::digest(&SHA512, format!("{setup_id}{device_id}").as_bytes());
    base64(&hash.as_ref()[..4])
}

fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |acc, (i, b)| acc | (u32::from(*b) << (16 - 8 * i)));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(char::from(ALPHABET[((n >> (18 - 6 * i)) & 63) as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

pub fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::new(), |mut out, b| {
        let _ = write!(out, "{b:02x}");
        out
    })
}

pub fn unhex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) || !text.is_ascii() {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_have_the_expected_shape() {
        for _ in 0..50 {
            let code = new_code();
            assert!(is_code(&code), "{code}");
        }
        assert!(!is_code("111-11-111"));
        assert!(!is_code("123-45-678"));
        assert!(!is_code("123456789"));
        assert!(!is_code("12a-45-678"));
        assert!(is_code("031-45-154"));
    }

    #[test]
    fn identifiers_look_right() {
        let id = new_device_id();
        assert_eq!(id.len(), 17);
        assert_eq!(id.matches(':').count(), 5);
        // Locally administered, unicast.
        let first = u8::from_str_radix(&id[..2], 16).unwrap();
        assert_eq!(first & 0x03, 0x02);
        let setup = new_setup_id();
        assert_eq!(setup.len(), 4);
        assert!(
            setup
                .chars()
                .all(|c| c.is_ascii_digit() || c.is_ascii_uppercase())
        );
    }

    // Reference values from HAP-python (`Accessory.xhm_uri`, `util.base36`)
    // and Python's hashlib/base64.
    #[test]
    fn uri_and_hash_match_a_reference() {
        let expected = include_str!("../testdata/setup_reference.txt");
        let mut lines = expected.lines();
        assert_eq!(
            uri("031-45-154", "ABCD", CATEGORY_BRIDGE),
            lines.next().unwrap()
        );
        assert_eq!(
            setup_hash("ABCD", "AA:BB:CC:DD:EE:FF"),
            lines.next().unwrap()
        );
    }

    #[test]
    fn base64_pads() {
        assert_eq!(base64(&[0xfb, 0xff, 0xbf, 0x00]), "+/+/AA==");
        assert_eq!(base64(&[1, 2, 3, 4]), "AQIDBA==");
    }

    #[test]
    fn hex_round_trips() {
        assert_eq!(hex(&[0, 255, 16]), "00ff10");
        assert_eq!(unhex("00ff10"), Some(vec![0, 255, 16]));
        assert_eq!(unhex("0"), None);
        assert_eq!(unhex("zz"), None);
    }
}
