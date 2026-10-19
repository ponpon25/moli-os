//! SRTP and SRTCP (RFC 3711) for the one suite HomeKit uses here,
//! `AES_CM_128_HMAC_SHA1_80`: AES in counter mode and a 10-byte HMAC-SHA1 tag.
//! Sender side only (the bridge protects what it sends).

use aes::Aes128;
use aes::cipher::generic_array::GenericArray;
use aes::cipher::{BlockEncrypt as _, KeyInit as _};
use ring::hmac;

pub const KEY_LEN: usize = 16;
pub const SALT_LEN: usize = 14;
const TAG_LEN: usize = 10;

/// The session keys derived from a master key and salt (§4.3).
struct Keys {
    cipher: Aes128,
    auth: hmac::Key,
    salt: [u8; SALT_LEN],
}

/// AES-CM keystream block `n` for `iv` (the counter is the low 16 bits).
fn keystream(cipher: &Aes128, iv: &[u8; 16], block: u16) -> [u8; 16] {
    let mut counter = *iv;
    let low = u16::from_be_bytes([counter[14], counter[15]]).wrapping_add(block);
    counter[14..].copy_from_slice(&low.to_be_bytes());
    let mut out = GenericArray::from(counter);
    cipher.encrypt_block(&mut out);
    out.into()
}

/// XORs `data` with the keystream of `iv`, in place.
fn apply(cipher: &Aes128, iv: &[u8; 16], data: &mut [u8]) {
    for (n, chunk) in data.chunks_mut(16).enumerate() {
        let block = keystream(cipher, iv, n as u16);
        for (byte, key) in chunk.iter_mut().zip(block) {
            *byte ^= key;
        }
    }
}

/// `n` bytes of the key derivation function for `label` (§4.3.1, with a key
/// derivation rate of zero).
fn derive(
    master_key: &[u8; KEY_LEN],
    master_salt: &[u8; SALT_LEN],
    label: u8,
    n: usize,
) -> Vec<u8> {
    let cipher = Aes128::new(GenericArray::from_slice(master_key));
    let mut x = *master_salt;
    x[7] ^= label;
    let mut iv = [0u8; 16];
    iv[..SALT_LEN].copy_from_slice(&x);
    let mut out = vec![0u8; n];
    apply(&cipher, &iv, &mut out);
    out
}

impl Keys {
    /// `labels`: cipher key, auth key, salt (0, 1, 2 for RTP; 3, 4, 5 for RTCP).
    fn new(master_key: &[u8; KEY_LEN], master_salt: &[u8; SALT_LEN], labels: [u8; 3]) -> Self {
        let key = derive(master_key, master_salt, labels[0], KEY_LEN);
        let auth = derive(master_key, master_salt, labels[1], 20);
        let mut salt = [0u8; SALT_LEN];
        salt.copy_from_slice(&derive(master_key, master_salt, labels[2], SALT_LEN));
        Self {
            cipher: Aes128::new(GenericArray::from_slice(&key)),
            auth: hmac::Key::new(hmac::HMAC_SHA1_FOR_LEGACY_USE_ONLY, &auth),
            salt,
        }
    }

    /// The IV of a packet: `(salt << 16) ^ (ssrc << 64) ^ (index << 16)`.
    fn iv(&self, ssrc: u32, index: u64) -> [u8; 16] {
        let mut iv = [0u8; 16];
        iv[..SALT_LEN].copy_from_slice(&self.salt);
        for (i, b) in ssrc.to_be_bytes().iter().enumerate() {
            iv[4 + i] ^= b;
        }
        for (i, b) in index.to_be_bytes()[2..].iter().enumerate() {
            iv[8 + i] ^= b;
        }
        iv
    }

    fn tag(&self, authenticated: &[u8], trailer: &[u8]) -> [u8; TAG_LEN] {
        let mut context = hmac::Context::with_key(&self.auth);
        context.update(authenticated);
        context.update(trailer);
        let full = context.sign();
        let mut tag = [0u8; TAG_LEN];
        tag.copy_from_slice(&full.as_ref()[..TAG_LEN]);
        tag
    }
}

/// Protects the RTP and RTCP packets of one stream.
pub struct Sender {
    rtp: Keys,
    rtcp: Keys,
    roc: u32,
    last_seq: Option<u16>,
    rtcp_index: u32,
}

impl std::fmt::Debug for Sender {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Sender").finish_non_exhaustive()
    }
}

impl Sender {
    #[must_use]
    pub fn new(master_key: &[u8; KEY_LEN], master_salt: &[u8; SALT_LEN]) -> Self {
        Self {
            rtp: Keys::new(master_key, master_salt, [0, 1, 2]),
            rtcp: Keys::new(master_key, master_salt, [3, 4, 5]),
            roc: 0,
            last_seq: None,
            // libsrtp numbers its first report 1 as well.
            rtcp_index: 1,
        }
    }

    /// An RTP packet (12-byte header, no extension or CSRC) encrypted and
    /// tagged. Sequence numbers must go up by one; the rollover counter
    /// follows them.
    pub fn protect_rtp(&mut self, packet: &[u8]) -> Vec<u8> {
        let seq = u16::from_be_bytes([packet[2], packet[3]]);
        if self
            .last_seq
            .is_some_and(|last| seq < last && last - seq > 0x8000)
        {
            self.roc = self.roc.wrapping_add(1);
        }
        self.last_seq = Some(seq);
        let ssrc = u32::from_be_bytes([packet[8], packet[9], packet[10], packet[11]]);
        let index = (u64::from(self.roc) << 16) | u64::from(seq);
        let mut out = packet.to_vec();
        apply(&self.rtp.cipher, &self.rtp.iv(ssrc, index), &mut out[12..]);
        let tag = self.rtp.tag(&out, &self.roc.to_be_bytes());
        out.extend_from_slice(&tag);
        out
    }

    /// Like [`Sender::protect_rtp`] for a header longer than 12 bytes
    /// (tests: a phone's packets may carry an extension).
    #[cfg(test)]
    pub fn protect_rtp_with_header(&mut self, packet: &[u8], header: usize) -> Vec<u8> {
        let seq = u16::from_be_bytes([packet[2], packet[3]]);
        let ssrc = u32::from_be_bytes([packet[8], packet[9], packet[10], packet[11]]);
        let index = (u64::from(self.roc) << 16) | u64::from(seq);
        let mut out = packet.to_vec();
        apply(
            &self.rtp.cipher,
            &self.rtp.iv(ssrc, index),
            &mut out[header..],
        );
        let tag = self.rtp.tag(&out, &self.roc.to_be_bytes());
        out.extend_from_slice(&tag);
        out
    }

    /// What a receiver does with a packet of this sender (tests): checks the
    /// tag against the rollover counter and returns the clear packet.
    #[cfg(test)]
    pub fn open_rtp(&self, packet: &[u8], roc: u32) -> Option<Vec<u8>> {
        let (body, tag) = packet.split_at(packet.len().checked_sub(TAG_LEN)?);
        if tag != self.rtp.tag(body, &roc.to_be_bytes()) {
            return None;
        }
        let seq = u16::from_be_bytes([body[2], body[3]]);
        let ssrc = u32::from_be_bytes([body[8], body[9], body[10], body[11]]);
        let index = (u64::from(roc) << 16) | u64::from(seq);
        let mut out = body.to_vec();
        apply(&self.rtp.cipher, &self.rtp.iv(ssrc, index), &mut out[12..]);
        Some(out)
    }

    /// Opens an SRTCP packet received from the other side with the same
    /// keys: checks the tag, decrypts, returns the clear compound packet.
    /// The error says what failed (for the journal).
    pub fn open_rtcp(&self, packet: &[u8]) -> Result<Vec<u8>, &'static str> {
        if packet.len() < 8 + 4 + TAG_LEN {
            return Err("too short");
        }
        let (body, tag) = packet.split_at(packet.len() - TAG_LEN);
        if tag != self.rtcp.tag(body, &[]) {
            return Err("tag does not match");
        }
        let (payload, word) = body.split_at(body.len() - 4);
        let word = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        let mut out = payload.to_vec();
        if word >> 31 == 1 {
            let ssrc = u32::from_be_bytes([out[4], out[5], out[6], out[7]]);
            apply(
                &self.rtcp.cipher,
                &self.rtcp.iv(ssrc, u64::from(word & 0x7FFF_FFFF)),
                &mut out[8..],
            );
        }
        Ok(out)
    }

    /// An RTCP compound packet encrypted, numbered and tagged (§3.4).
    pub fn protect_rtcp(&mut self, packet: &[u8]) -> Vec<u8> {
        let ssrc = u32::from_be_bytes([packet[4], packet[5], packet[6], packet[7]]);
        let index = self.rtcp_index & 0x7FFF_FFFF;
        self.rtcp_index = self.rtcp_index.wrapping_add(1);
        let mut out = packet.to_vec();
        apply(
            &self.rtcp.cipher,
            &self.rtcp.iv(ssrc, u64::from(index)),
            &mut out[8..],
        );
        out.extend_from_slice(&(0x8000_0000 | index).to_be_bytes());
        let tag = self.rtcp.tag(&out, &[]);
        out.extend_from_slice(&tag);
        out
    }
}

/// Opens the RTP packets the other side sends (the phone's voice), keeping
/// the rollover counter by the estimate of RFC 3711 appendix A.
pub struct Receiver {
    rtp: Keys,
    roc: u32,
    last_seq: Option<u16>,
}

impl std::fmt::Debug for Receiver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Receiver").finish_non_exhaustive()
    }
}

/// Bytes of an RTP header: fixed part, CSRCs, extension.
pub fn header_len(packet: &[u8]) -> Option<usize> {
    if packet.len() < 12 || packet[0] >> 6 != 2 {
        return None;
    }
    let mut len = 12 + 4 * usize::from(packet[0] & 0x0F);
    if packet[0] & 0x10 != 0 {
        let words = packet.get(len + 2..len + 4)?;
        len += 4 + 4 * usize::from(u16::from_be_bytes([words[0], words[1]]));
    }
    (len <= packet.len()).then_some(len)
}

impl Receiver {
    #[must_use]
    pub fn new(master_key: &[u8; KEY_LEN], master_salt: &[u8; SALT_LEN]) -> Self {
        Self {
            rtp: Keys::new(master_key, master_salt, [0, 1, 2]),
            roc: 0,
            last_seq: None,
        }
    }

    /// The clear RTP packet, or `None` when the tag does not match.
    pub fn unprotect(&mut self, packet: &[u8]) -> Option<Vec<u8>> {
        let header = header_len(packet.get(..packet.len().checked_sub(TAG_LEN)?)?)?;
        let (body, tag) = packet.split_at(packet.len() - TAG_LEN);
        let seq = u16::from_be_bytes([body[2], body[3]]);
        let roc = match self.last_seq {
            None => self.roc,
            Some(last) if last < 0x8000 => {
                if seq > last + 0x8000 {
                    self.roc.wrapping_sub(1)
                } else {
                    self.roc
                }
            }
            Some(last) => {
                if last - 0x8000 > seq {
                    self.roc.wrapping_add(1)
                } else {
                    self.roc
                }
            }
        };
        if tag != self.rtp.tag(body, &roc.to_be_bytes()) {
            return None;
        }
        if roc == self.roc.wrapping_add(1) {
            self.roc = roc;
            self.last_seq = Some(seq);
        } else if roc == self.roc && self.last_seq.is_none_or(|last| seq > last) {
            self.last_seq = Some(seq);
        }
        let ssrc = u32::from_be_bytes([body[8], body[9], body[10], body[11]]);
        let index = (u64::from(roc) << 16) | u64::from(seq);
        let mut out = body.to_vec();
        apply(
            &self.rtp.cipher,
            &self.rtp.iv(ssrc, index),
            &mut out[header..],
        );
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(text: &str) -> Vec<u8> {
        (0..text.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
            .collect()
    }

    /// RFC 3711, appendix B.2: AES-CM keystream.
    #[test]
    fn the_aes_counter_keystream_matches_the_rfc() {
        let key = hex("2B7E151628AED2A6ABF7158809CF4F3C");
        let cipher = Aes128::new(GenericArray::from_slice(&key));
        let iv: [u8; 16] = hex("F0F1F2F3F4F5F6F7F8F9FAFBFCFD0000").try_into().unwrap();
        assert_eq!(
            keystream(&cipher, &iv, 0).to_vec(),
            hex("E03EAD0935C95E80E166B16DD92B4EB4")
        );
        assert_eq!(
            keystream(&cipher, &iv, 1).to_vec(),
            hex("D23513162B02D0F72A43A2FE4A5F97AB")
        );
        assert_eq!(
            keystream(&cipher, &iv, 2).to_vec(),
            hex("41E95B3BB0A2E8DD477901E4FCA894C0")
        );
    }

    /// RFC 3711, appendix B.3: session key derivation.
    #[test]
    fn the_session_keys_match_the_rfc() {
        let key: [u8; 16] = hex("E1F97A0D3E018BE0D64FA32C06DE4139").try_into().unwrap();
        let salt: [u8; 14] = hex("0EC675AD498AFEEBB6960B3AABE6").try_into().unwrap();
        assert_eq!(
            derive(&key, &salt, 0, 16),
            hex("C61E7A93744F39EE10734AFE3FF7A087")
        );
        assert_eq!(
            derive(&key, &salt, 2, 14),
            hex("30CBBC08863D8C85D49DB34A9AE1")
        );
        assert_eq!(
            derive(&key, &salt, 1, 20),
            hex("CEBE321F6FF7716B6FD4AB49AF256A156D38BAA4")
        );
    }

    /// HMAC-SHA1 as RFC 2202 gives it (test case 2), the primitive the tag rests on.
    #[test]
    fn the_hmac_sha1_primitive_matches_rfc_2202() {
        let key = hmac::Key::new(hmac::HMAC_SHA1_FOR_LEGACY_USE_ONLY, b"Jefe");
        let tag = hmac::sign(&key, b"what do ya want for nothing?");
        assert_eq!(
            tag.as_ref(),
            hex("effcdf6ae5eb2fa2d27416d5f184df9c259a7c79")
        );
    }

    fn rtp(seq: u16, ssrc: u32, payload: &[u8]) -> Vec<u8> {
        let mut p = vec![0x80, 99];
        p.extend_from_slice(&seq.to_be_bytes());
        p.extend_from_slice(&1234u32.to_be_bytes());
        p.extend_from_slice(&ssrc.to_be_bytes());
        p.extend_from_slice(payload);
        p
    }

    /// Decrypts the way a receiver does, from the RFC's definitions, to
    /// check the sender's output end to end (header clear, payload
    /// recoverable, tag valid over header, ciphertext and ROC).
    #[test]
    fn a_protected_packet_can_be_opened_and_its_tag_checks() {
        let key = [7u8; 16];
        let salt = [9u8; 14];
        let mut sender = Sender::new(&key, &salt);
        let payload: Vec<u8> = (0..100u8).collect();
        let sealed = sender.protect_rtp(&rtp(65535, 0xDEAD_BEEF, &payload));
        assert_eq!(sealed.len(), 12 + payload.len() + TAG_LEN);
        assert_eq!(&sealed[..12], &rtp(65535, 0xDEAD_BEEF, &[])[..]);
        assert_ne!(&sealed[12..112], &payload[..]);
        let keys = Keys::new(&key, &salt, [0, 1, 2]);
        let (body, tag) = sealed.split_at(sealed.len() - TAG_LEN);
        assert_eq!(tag, keys.tag(body, &0u32.to_be_bytes()));
        let mut opened = body[12..].to_vec();
        apply(&keys.cipher, &keys.iv(0xDEAD_BEEF, 65535), &mut opened);
        assert_eq!(opened, payload);
        // The next packet wraps the sequence: the rollover counter goes up
        // and the tag covers it.
        let next = sender.protect_rtp(&rtp(0, 0xDEAD_BEEF, &payload));
        let (body, tag) = next.split_at(next.len() - TAG_LEN);
        assert_eq!(tag, keys.tag(body, &1u32.to_be_bytes()));
        let mut opened = body[12..].to_vec();
        apply(&keys.cipher, &keys.iv(0xDEAD_BEEF, 1 << 16), &mut opened);
        assert_eq!(opened, payload);
    }

    #[test]
    fn a_protected_sender_report_carries_its_index_and_tag() {
        let key = [1u8; 16];
        let salt = [2u8; 14];
        let mut sender = Sender::new(&key, &salt);
        let mut report = vec![0x80, 200, 0, 6];
        report.extend_from_slice(&0xCAFEu32.to_be_bytes());
        report.extend_from_slice(&[0u8; 20]);
        let first = sender.protect_rtcp(&report);
        assert_eq!(first.len(), 28 + 4 + TAG_LEN);
        assert_eq!(&first[..8], &report[..8]);
        assert_eq!(&first[28..32], &0x8000_0001u32.to_be_bytes());
        let second = sender.protect_rtcp(&report);
        assert_eq!(&second[28..32], &0x8000_0002u32.to_be_bytes());
        assert_ne!(first[8..28], second[8..28]);
        let keys = Keys::new(&key, &salt, [3, 4, 5]);
        assert_eq!(&first[32..], keys.tag(&first[..32], &[]));
        let mut opened = first[8..28].to_vec();
        apply(&keys.cipher, &keys.iv(0xCAFE, 1), &mut opened);
        assert_eq!(opened, report[8..]);
    }

    /// Packets protected by libsrtp (via pylibsrtp) with the same key and
    /// salt: the sender must produce exactly these bytes, the rollover of
    /// the sequence number and the second report included.
    #[test]
    fn the_output_equals_libsrtps() {
        let key: [u8; 16] = std::array::from_fn(|i| i as u8);
        let salt: [u8; 14] = std::array::from_fn(|i| 0xA0 + i as u8);
        let payload =
            hex("030a11181f262d343b424950575e656c737a81888f969da4abb2b9c0c7ced5dce3eaf1f8ff060d14");
        let mut sender = Sender::new(&key, &salt);
        let a = sender.protect_rtp(&rtp_with(65535, 90000, true, &payload));
        assert_eq!(
            a,
            hex(
                "80e3ffff00015f9001020304fb0d3f9c7dc4e240cce5fdace0883c8b7604863b340408ea1b997d5a673465b407f90c2d1897bf628af3223d0886427134bf"
            )
        );
        let b = sender.protect_rtp(&rtp_with(0, 93000, false, &payload));
        assert_eq!(
            b,
            hex(
                "8063000000016b48010203043e050e853bfd795ea5ff288fb491fe8f702f10116347e709a2c342c2fc455de955bd4047127bab4b899405fdc52707bccf13"
            )
        );
        let sr = hex("80c80006010203040102030405060708090a0b0c0d0e0f1011121314");
        let mut sender = Sender::new(&key, &salt);
        assert_eq!(
            sender.protect_rtcp(&sr),
            hex(
                "80c8000601020304e3a7f42e1af14a323444a36fa5aa1422cbaa83f2800000018904bf992a453681ded6"
            )
        );
        assert_eq!(
            sender.protect_rtcp(&sr),
            hex(
                "80c80006010203046f906cacf5b5e1a4642dd48d6257f59699ce9498800000024517fb9e6d7dbce62130"
            )
        );
    }

    fn rtp_with(seq: u16, ts: u32, marker: bool, payload: &[u8]) -> Vec<u8> {
        let mut p = vec![0x80, 0x63 | if marker { 0x80 } else { 0 }];
        p.extend_from_slice(&seq.to_be_bytes());
        p.extend_from_slice(&ts.to_be_bytes());
        p.extend_from_slice(&0x0102_0304u32.to_be_bytes());
        p.extend_from_slice(payload);
        p
    }

    #[test]
    fn a_receiver_opens_what_a_sender_sealed_across_the_sequence_wrap() {
        let (key, salt) = ([5u8; 16], [6u8; 14]);
        let mut sender = Sender::new(&key, &salt);
        let mut receiver = Receiver::new(&key, &salt);
        let payload = [9u8; 60];
        for seq in [65_533u16, 65_534, 65_535, 0, 1, 2] {
            let sealed = sender.protect_rtp(&rtp_with(seq, u32::from(seq), false, &payload));
            let clear = receiver.unprotect(&sealed).expect("opens");
            assert_eq!(&clear[12..], &payload[..], "seq {seq}");
        }
        // A late packet from before the wrap still opens (previous rollover).
        let late = Sender::new(&key, &salt).protect_rtp(&rtp_with(65_535, 7, false, &payload));
        assert!(receiver.unprotect(&late).is_some());
        // Tampering and other keys are refused.
        let mut sealed = Sender::new(&key, &salt).protect_rtp(&rtp_with(3, 7, false, &payload));
        sealed[20] ^= 1;
        assert!(receiver.unprotect(&sealed).is_none());
        assert!(Receiver::new(&[1; 16], &salt).unprotect(&late).is_none());
        // A header extension is skipped over, left in the clear.
        let mut ext = vec![
            0x90, 111, 0, 1, 0, 0, 0, 1, 0, 0, 0, 2, 0xBE, 0xDE, 0, 1, 1, 2, 3, 4,
        ];
        ext.extend_from_slice(&payload);
        let sealed = Sender::new(&key, &salt).protect_rtp_with_header(&ext, 20);
        let clear = Receiver::new(&key, &salt).unprotect(&sealed).unwrap();
        assert_eq!(&clear[20..], &payload[..]);
    }
}
