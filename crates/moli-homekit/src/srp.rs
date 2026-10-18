//! SRP-6a as HomeKit uses it for `pair-setup` (HAP spec, "SRP"): the
//! 3072-bit group of RFC 5054, SHA-512, user name `Pair-Setup`, the setup
//! code as password. Only the accessory's (server) side. The shape of the
//! hashed values (public keys padded to 384 bytes, the shared secret too)
//! follows what real controllers do; the test vector comes from aiohomekit's
//! implementation.

use ring::digest::{self, SHA512};
use ring::rand::{SecureRandom, SystemRandom};

use crate::bigint::Uint;

/// The 3072-bit safe prime of RFC 5054 / RFC 3526, big-endian hex.
pub const MODULUS_HEX: &str = include_str!("../testdata/modulus.hex");

/// Length of the public values on the wire.
pub const KEY_LEN: usize = 384;
const USER: &str = "Pair-Setup";

fn sha512(parts: &[&[u8]]) -> [u8; 64] {
    let mut ctx = digest::Context::new(&SHA512);
    for part in parts {
        ctx.update(part);
    }
    let mut out = [0u8; 64];
    out.copy_from_slice(ctx.finish().as_ref());
    out
}

pub(crate) fn modulus() -> Uint {
    let hex = MODULUS_HEX.trim();
    let bytes: Vec<u8> = (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).expect("modulus is hex"))
        .collect();
    Uint::from_be_bytes(&bytes)
}

fn pad(u: &Uint) -> Vec<u8> {
    u.to_be_bytes_padded(KEY_LEN)
}

/// One pairing attempt, from the challenge to the proofs.
#[derive(Debug)]
pub struct Server {
    salt: [u8; 16],
    verifier: Uint,
    b: Uint,
    public: Vec<u8>,
    key: Option<[u8; 64]>,
}

impl Server {
    /// A fresh challenge for a setup code (`123-45-678`).
    pub fn new(code: &str) -> Self {
        let rng = SystemRandom::new();
        let mut salt = [0u8; 16];
        let mut b = [0u8; 32];
        rng.fill(&mut salt).expect("system randomness");
        rng.fill(&mut b).expect("system randomness");
        Self::with_secrets(code, salt, &b)
    }

    pub fn with_secrets(code: &str, salt: [u8; 16], b: &[u8]) -> Self {
        let n = modulus();
        let g = Uint::from_u64(5);
        let inner = sha512(&[format!("{USER}:{code}").as_bytes()]);
        let x = Uint::from_be_bytes(&sha512(&[&salt, &inner]));
        let verifier = g.pow_mod(&x, &n);
        let k = Uint::from_be_bytes(&sha512(&[&pad(&n), &pad(&g)]));
        let b = Uint::from_be_bytes(b);
        let public = k.mul_mod(&verifier, &n).add_mod(&g.pow_mod(&b, &n), &n);
        Self {
            salt,
            verifier,
            b,
            public: pad(&public),
            key: None,
        }
    }

    pub fn salt(&self) -> &[u8; 16] {
        &self.salt
    }

    /// `B`, 384 bytes.
    pub fn public_key(&self) -> &[u8] {
        &self.public
    }

    /// Checks the controller's proof `M1` for its public key `A`; on
    /// success the session key is set and the accessory's proof `M2` is
    /// returned.
    pub fn verify(&mut self, a_bytes: &[u8], m1: &[u8]) -> Option<[u8; 64]> {
        if a_bytes.len() != KEY_LEN {
            return None;
        }
        let n = modulus();
        let a = Uint::from_be_bytes(a_bytes);
        if a.rem(&n).is_zero() {
            return None;
        }
        let u = Uint::from_be_bytes(&sha512(&[a_bytes, &self.public]));
        let s = a
            .mul_mod(&self.verifier.pow_mod(&u, &n), &n)
            .pow_mod(&self.b, &n);
        let key = sha512(&[&pad(&s)]);
        let h_n = sha512(&[&n.to_be_bytes()]);
        let h_g = sha512(&[&[5u8]]);
        let group: Vec<u8> = h_n.iter().zip(h_g).map(|(x, y)| x ^ y).collect();
        let expected = sha512(&[
            &group,
            &sha512(&[USER.as_bytes()]),
            &self.salt,
            a_bytes,
            &self.public,
            &key,
        ]);
        // Both are public-length hashes of a one-shot secret: compared
        // without an early exit anyway.
        let same = m1.len() == expected.len()
            && m1
                .iter()
                .zip(expected)
                .fold(0u8, |acc, (x, y)| acc | (x ^ y))
                == 0;
        if !same {
            return None;
        }
        self.key = Some(key);
        Some(sha512(&[a_bytes, m1, &key]))
    }

    /// The 64-byte session key, once the controller proved the code.
    pub fn session_key(&self) -> Option<&[u8; 64]> {
        self.key.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vector(name: &str) -> Vec<u8> {
        let line = include_str!("../testdata/srp_vector.txt")
            .lines()
            .find_map(|l| l.strip_prefix(&format!("{name}=")))
            .unwrap();
        (0..line.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&line[i..i + 2], 16).unwrap())
            .collect()
    }

    fn server() -> Server {
        let salt: [u8; 16] = vector("salt").try_into().unwrap();
        Server::with_secrets("123-45-678", salt, &vector("b"))
    }

    #[test]
    fn the_modulus_is_the_3072_bit_group() {
        let n = modulus();
        assert_eq!(n.bits(), 3072);
        assert_eq!(n.to_be_bytes()[..8], [0xff; 8]);
    }

    #[test]
    fn matches_a_real_implementation() {
        let mut srv = server();
        assert_eq!(srv.public_key(), vector("B"));
        let m2 = srv
            .verify(&vector("A"), &vector("M1"))
            .expect("proof accepted");
        assert_eq!(m2.as_slice(), vector("M2"));
        assert_eq!(srv.session_key().unwrap().as_slice(), vector("K"));
    }

    #[test]
    fn a_wrong_code_or_proof_is_refused() {
        let salt: [u8; 16] = vector("salt").try_into().unwrap();
        let mut wrong = Server::with_secrets("000-00-000", salt, &vector("b"));
        assert!(wrong.verify(&vector("A"), &vector("M1")).is_none());
        assert!(wrong.session_key().is_none());
        let mut srv = server();
        let mut bad = vector("M1");
        bad[0] ^= 1;
        assert!(srv.verify(&vector("A"), &bad).is_none());
        assert!(srv.verify(&vector("A"), &[]).is_none());
        // A = 0 and A = N would make the secret predictable.
        assert!(srv.verify(&[0u8; KEY_LEN], &vector("M1")).is_none());
        assert!(srv.verify(&pad(&modulus()), &vector("M1")).is_none());
        assert!(srv.verify(&[1, 2, 3], &vector("M1")).is_none());
    }

    #[test]
    fn each_attempt_has_its_own_challenge() {
        let (a, b) = (Server::new("123-45-678"), Server::new("123-45-678"));
        assert_ne!(a.salt(), b.salt());
        assert_ne!(a.public_key(), b.public_key());
        assert_eq!(a.public_key().len(), KEY_LEN);
    }
}
