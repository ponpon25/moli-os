//! The small cryptographic building blocks of HAP, over `ring`: HKDF-SHA512
//! key derivation, the ChaCha20-Poly1305 of the pairing messages and of the
//! encrypted session.

use ring::aead::{Aad, CHACHA20_POLY1305, LessSafeKey, Nonce, UnboundKey};
use ring::hkdf;

/// Output length marker for `ring`'s HKDF.
struct Len(usize);

impl hkdf::KeyType for Len {
    fn len(&self) -> usize {
        self.0
    }
}

/// HKDF-SHA512 into 32 bytes.
pub fn hkdf32(ikm: &[u8], salt: &str, info: &str) -> [u8; 32] {
    let prk = hkdf::Salt::new(hkdf::HKDF_SHA512, salt.as_bytes()).extract(ikm);
    let mut out = [0u8; 32];
    prk.expand(&[info.as_bytes()], Len(32))
        .and_then(|okm| okm.fill(&mut out))
        .expect("HKDF output length is valid");
    out
}

/// `tag` of 16 bytes follows the ciphertext.
const TAG: usize = 16;

fn key(bytes: &[u8; 32]) -> LessSafeKey {
    LessSafeKey::new(UnboundKey::new(&CHACHA20_POLY1305, bytes).expect("32-byte key"))
}

/// 4 zero bytes then the 8 bytes (a message tag like `PS-Msg05`, or a
/// little-endian counter).
fn nonce(tail: [u8; 8]) -> Nonce {
    let mut n = [0u8; 12];
    n[4..].copy_from_slice(&tail);
    Nonce::assume_unique_for_key(n)
}

/// Seals `plain` (ciphertext followed by the 16-byte tag).
pub fn seal(key_bytes: &[u8; 32], tail: [u8; 8], aad: &[u8], plain: &[u8]) -> Vec<u8> {
    let mut buf = plain.to_vec();
    key(key_bytes)
        .seal_in_place_append_tag(nonce(tail), Aad::from(aad), &mut buf)
        .expect("sealing does not fail");
    buf
}

/// Opens a ciphertext with its tag; `None` if it does not authenticate.
pub fn open(key_bytes: &[u8; 32], tail: [u8; 8], aad: &[u8], sealed: &[u8]) -> Option<Vec<u8>> {
    if sealed.len() < TAG {
        return None;
    }
    let mut buf = sealed.to_vec();
    let plain = key(key_bytes)
        .open_in_place(nonce(tail), Aad::from(aad), &mut buf)
        .ok()?;
    Some(plain.to_vec())
}

/// The 8-byte nonce tail of a pairing message (`PS-Msg05`…).
pub fn message_tag(name: &str) -> [u8; 8] {
    let mut tail = [0u8; 8];
    tail.copy_from_slice(name.as_bytes());
    tail
}

/// Frames of the encrypted session: `[len: u16 LE][sealed len bytes][tag]`,
/// at most 1024 bytes of payload each, one counter per direction.
#[derive(Debug)]
pub struct Channel {
    /// Controller → accessory.
    read: [u8; 32],
    read_counter: u64,
    /// Accessory → controller.
    write: [u8; 32],
    write_counter: u64,
}

pub const MAX_FRAME: usize = 1024;

impl Channel {
    pub fn new(shared: &[u8]) -> Self {
        Self {
            read: hkdf32(shared, "Control-Salt", "Control-Write-Encryption-Key"),
            read_counter: 0,
            write: hkdf32(shared, "Control-Salt", "Control-Read-Encryption-Key"),
            write_counter: 0,
        }
    }

    /// The controller's side of the same session (tests).
    #[cfg(test)]
    pub fn mirrored(shared: &[u8]) -> Self {
        let accessory = Self::new(shared);
        Self {
            read: accessory.write,
            read_counter: 0,
            write: accessory.read,
            write_counter: 0,
        }
    }

    /// Encrypts a whole message into one or more frames.
    pub fn encrypt(&mut self, plain: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(plain.len() + 18 * (plain.len() / MAX_FRAME + 1));
        for chunk in plain.chunks(MAX_FRAME) {
            let aad = (chunk.len() as u16).to_le_bytes();
            out.extend_from_slice(&aad);
            out.extend(seal(
                &self.write,
                self.write_counter.to_le_bytes(),
                &aad,
                chunk,
            ));
            self.write_counter += 1;
        }
        out
    }

    /// Decrypts the complete frames at the start of `buffer`, removing them;
    /// a partial frame stays. `Err` when a frame does not authenticate.
    pub fn decrypt(&mut self, buffer: &mut Vec<u8>) -> Result<Vec<u8>, ()> {
        let mut plain = Vec::new();
        let mut used = 0;
        while buffer.len() - used >= 2 {
            let len = usize::from(u16::from_le_bytes([buffer[used], buffer[used + 1]]));
            if len > MAX_FRAME {
                return Err(());
            }
            let end = used + 2 + len + TAG;
            if buffer.len() < end {
                break;
            }
            let aad = [buffer[used], buffer[used + 1]];
            let opened = open(
                &self.read,
                self.read_counter.to_le_bytes(),
                &aad,
                &buffer[used + 2..end],
            )
            .ok_or(())?;
            self.read_counter += 1;
            plain.extend(opened);
            used = end;
        }
        buffer.drain(..used);
        Ok(plain)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // RFC 5869 test case 1 is SHA-256; for SHA-512 the vector below comes
    // from RFC 5869 computed with Python hmac/hashlib (SHA-512).
    #[test]
    fn hkdf_sha512_matches_a_reference() {
        let out = hkdf32(
            &[0x0b; 22],
            "Pair-Setup-Encrypt-Salt",
            "Pair-Setup-Encrypt-Info",
        );
        assert_eq!(
            crate::setup::hex(&out),
            include_str!("../testdata/hkdf_reference.txt").trim()
        );
    }

    #[test]
    fn pairing_messages_round_trip_and_reject_tampering() {
        let k = [9u8; 32];
        let sealed = seal(&k, message_tag("PS-Msg05"), &[], b"hello");
        assert_eq!(sealed.len(), 5 + 16);
        assert_eq!(
            open(&k, message_tag("PS-Msg05"), &[], &sealed).unwrap(),
            b"hello"
        );
        assert!(open(&k, message_tag("PS-Msg06"), &[], &sealed).is_none());
        let mut bad = sealed.clone();
        bad[0] ^= 1;
        assert!(open(&k, message_tag("PS-Msg05"), &[], &bad).is_none());
        assert!(open(&k, message_tag("PS-Msg05"), &[], &[1, 2]).is_none());
    }

    #[test]
    fn channels_mirror_each_other_across_frames() {
        let shared = [3u8; 32];
        let mut accessory = Channel::new(&shared);
        // The controller derives the same keys with the directions swapped.
        let mut controller = Channel {
            read: accessory.write,
            read_counter: 0,
            write: accessory.read,
            write_counter: 0,
        };
        let message: Vec<u8> = (0..2500u32)
            .map(|i| u8::try_from(i % 256).unwrap())
            .collect();
        let wire = controller.encrypt(&message);
        // 2500 bytes = 3 frames, each with a 2-byte length and a tag.
        assert_eq!(wire.len(), 2500 + 3 * 18);
        // Delivered in two pieces: the partial frame waits.
        let (first, second) = wire.split_at(1500);
        let mut buffer = first.to_vec();
        let mut got = accessory.decrypt(&mut buffer).unwrap();
        assert!(!buffer.is_empty());
        buffer.extend_from_slice(second);
        got.extend(accessory.decrypt(&mut buffer).unwrap());
        assert_eq!(got, message);
        assert!(buffer.is_empty());
        // And the other way.
        let mut back = controller.encrypt(&[]);
        assert!(back.is_empty());
        back = accessory.encrypt(b"pong");
        let mut buffer = back;
        assert_eq!(controller.decrypt(&mut buffer).unwrap(), b"pong");
    }

    #[test]
    fn a_forged_frame_is_an_error() {
        let mut a = Channel::new(&[1u8; 32]);
        let mut buffer = vec![
            4, 0, 1, 2, 3, 4, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        ];
        assert!(a.decrypt(&mut buffer).is_err());
        let mut huge = vec![0xff, 0xff];
        assert!(a.decrypt(&mut huge).is_err());
    }
}
