//! ES256 tokens (JWT): what some services (Apple Music) want to see to
//! believe a request comes from the holder of a private key. The key is a
//! PEM file (PKCS#8, P-256), as the service gives it.

use std::sync::Mutex;

use anyhow::Context as _;
use ring::rand::SystemRandom;
use ring::signature::{ECDSA_P256_SHA256_FIXED_SIGNING, EcdsaKeyPair};

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// Base64, URL-safe, without padding (what a JWT is made of).
#[must_use]
pub fn base64url(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, b)| n | (u32::from(*b) << (16 - 8 * i)));
        for i in 0..=chunk.len() {
            out.push(char::from(ALPHABET[((n >> (18 - 6 * i)) & 63) as usize]));
        }
    }
    out
}

/// The bytes of a PEM file's body (base64 between the `-----` lines).
#[must_use]
pub fn pem_body(pem: &str) -> Option<Vec<u8>> {
    let body: String = pem
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with("-----"))
        .collect();
    let mut out = Vec::with_capacity(body.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0u32);
    for c in body.bytes().take_while(|c| *c != b'=') {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            _ => return None,
        };
        acc = (acc << 6) | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(u8::try_from((acc >> bits) & 0xFF).ok()?);
        }
    }
    (!out.is_empty()).then_some(out)
}

/// A private key that signs tokens.
pub struct Signer {
    key: EcdsaKeyPair,
    rng: Mutex<SystemRandom>,
}

impl std::fmt::Debug for Signer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Signer(..)")
    }
}

impl Signer {
    /// From the text of a `.p8` file.
    pub fn from_pem(pem: &str) -> anyhow::Result<Self> {
        let rng = SystemRandom::new();
        let der = pem_body(pem).context("the key is not a PEM file")?;
        let key = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &der, &rng)
            .map_err(|_| anyhow::anyhow!("the key is not a P-256 key"))?;
        Ok(Self {
            key,
            rng: Mutex::new(rng),
        })
    }

    /// A token: which key signs (`kid`), who the holder is (`iss`), from
    /// `now` (seconds since the epoch) for `life` seconds.
    pub fn jwt(&self, key_id: &str, issuer: &str, now: u64, life: u64) -> anyhow::Result<String> {
        let header = serde_json::json!({ "alg": "ES256", "kid": key_id });
        let claims = serde_json::json!({ "iss": issuer, "iat": now, "exp": now + life });
        let input = format!(
            "{}.{}",
            base64url(header.to_string().as_bytes()),
            base64url(claims.to_string().as_bytes())
        );
        let rng = self
            .rng
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let signature = self
            .key
            .sign(&*rng, input.as_bytes())
            .map_err(|_| anyhow::anyhow!("the token could not be signed"))?;
        Ok(format!("{input}.{}", base64url(signature.as_ref())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ring::signature::{ECDSA_P256_SHA256_FIXED, KeyPair as _, UnparsedPublicKey};

    /// Standard base64, for the tests' PEM.
    fn base64_standard(bytes: &[u8]) -> String {
        let table = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::new();
        for chunk in bytes.chunks(3) {
            let n = chunk
                .iter()
                .enumerate()
                .fold(0u32, |n, (i, b)| n | (u32::from(*b) << (16 - 8 * i)));
            for i in 0..4 {
                out.push(if i <= chunk.len() {
                    char::from(table[((n >> (18 - 6 * i)) & 63) as usize])
                } else {
                    '='
                });
            }
        }
        out
    }

    fn generated() -> (String, Vec<u8>) {
        let rng = SystemRandom::new();
        let pkcs8 = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &rng).unwrap();
        // (the marker lines are put together: no key is written here)
        let (begin, end) = (
            ["-----BEGIN", "PRIVATE", "KEY-----"],
            ["-----END", "PRIVATE", "KEY-----"],
        );
        let pem = format!(
            "{}\n{}\n{}\n",
            begin.join(" "),
            base64_standard(pkcs8.as_ref()),
            end.join(" ")
        );
        let key = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, pkcs8.as_ref(), &rng)
            .unwrap();
        (pem, key.public_key().as_ref().to_vec())
    }

    #[test]
    fn base64_goes_both_ways() {
        assert_eq!(base64url(b""), "");
        assert_eq!(base64url(b"f"), "Zg");
        assert_eq!(base64url(b"fo"), "Zm8");
        assert_eq!(base64url(b"foo"), "Zm9v");
        assert_eq!(base64url(&[0xFB, 0xFF]), "-_8");
        assert_eq!(
            pem_body("-----BEGIN X-----\nZm9v\nYmFy\n-----END X-----\n"),
            Some(b"foobar".to_vec())
        );
        assert_eq!(pem_body("-----BEGIN X-----\n-----END X-----"), None);
    }

    #[test]
    fn a_signed_token_checks_out() {
        let (pem, public) = generated();
        let signer = Signer::from_pem(&pem).unwrap();
        let jwt = signer
            .jwt("KEYID12345", "TEAMID6789", 1_700_000_000, 43_200)
            .unwrap();
        let parts: Vec<&str> = jwt.split('.').collect();
        assert_eq!(parts.len(), 3);
        assert_eq!(
            base64url(br#"{"alg":"ES256","kid":"KEYID12345"}"#),
            parts[0],
            "the header says who signs"
        );
        let signature = pem_body(&format!("-----B-----\n{}\n-----E-----", parts[2])).unwrap();
        assert_eq!(signature.len(), 64);
        UnparsedPublicKey::new(&ECDSA_P256_SHA256_FIXED, public)
            .verify(format!("{}.{}", parts[0], parts[1]).as_bytes(), &signature)
            .unwrap();
    }

    #[test]
    fn a_bad_key_is_refused() {
        assert!(Signer::from_pem("not a key").is_err());
        assert!(Signer::from_pem("-----BEGIN X-----\nZm9v\n-----END X-----").is_err());
    }
}
