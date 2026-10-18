//! TLV8, the framing of HAP's pairing messages: type (1 byte), length
//! (1 byte), value; a value longer than 255 bytes is split into consecutive
//! fragments of the same type.

pub const METHOD: u8 = 0;
pub const IDENTIFIER: u8 = 1;
pub const SALT: u8 = 2;
pub const PUBLIC_KEY: u8 = 3;
pub const PROOF: u8 = 4;
pub const ENCRYPTED_DATA: u8 = 5;
pub const STATE: u8 = 6;
pub const ERROR: u8 = 7;
pub const SIGNATURE: u8 = 10;
pub const PERMISSIONS: u8 = 11;
pub const SEPARATOR: u8 = 0xFF;

/// HAP's pairing error codes (`kTLVType_Error`).
pub const ERR_UNKNOWN: u8 = 1;
pub const ERR_AUTHENTICATION: u8 = 2;
pub const ERR_MAX_TRIES: u8 = 5;
pub const ERR_UNAVAILABLE: u8 = 6;
pub const ERR_BUSY: u8 = 7;

/// Items in order of appearance (a list of pairings repeats the identifier).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Tlv(Vec<(u8, Vec<u8>)>);

impl Tlv {
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn with(mut self, tag: u8, value: impl AsRef<[u8]>) -> Self {
        self.0.push((tag, value.as_ref().to_vec()));
        self
    }

    /// Reads a message; `None` when a header or a value is cut short.
    pub fn parse(mut bytes: &[u8]) -> Option<Self> {
        let mut items: Vec<(u8, Vec<u8>)> = Vec::new();
        let mut continues = None;
        while !bytes.is_empty() {
            let (&tag, rest) = bytes.split_first()?;
            let (&len, rest) = rest.split_first()?;
            let len = usize::from(len);
            if rest.len() < len {
                return None;
            }
            let (value, rest) = rest.split_at(len);
            bytes = rest;
            // A full 255-byte item continues in the next item of the same tag.
            match (continues, items.last_mut()) {
                (Some(prev), Some(last)) if prev == tag && tag != SEPARATOR => {
                    last.1.extend_from_slice(value);
                }
                _ => items.push((tag, value.to_vec())),
            }
            continues = (len == 255).then_some(tag);
        }
        Some(Self(items))
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        for (tag, value) in &self.0 {
            if value.is_empty() {
                out.extend_from_slice(&[*tag, 0]);
                continue;
            }
            for chunk in value.chunks(255) {
                out.push(*tag);
                out.push(chunk.len() as u8);
                out.extend_from_slice(chunk);
            }
        }
        out
    }

    pub fn get(&self, tag: u8) -> Option<&[u8]> {
        self.0
            .iter()
            .find(|(t, _)| *t == tag)
            .map(|(_, v)| v.as_slice())
    }

    pub fn byte(&self, tag: u8) -> Option<u8> {
        match self.get(tag)? {
            [b] => Some(*b),
            _ => None,
        }
    }

    #[cfg(test)]
    pub fn all(&self, tag: u8) -> impl Iterator<Item = &[u8]> {
        self.0
            .iter()
            .filter(move |(t, _)| *t == tag)
            .map(|(_, v)| v.as_slice())
    }

    #[cfg(test)]
    pub fn items(&self) -> &[(u8, Vec<u8>)] {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_items_round_trip() {
        let tlv = Tlv::new().with(STATE, [1]).with(METHOD, [0]);
        assert_eq!(tlv.encode(), [6, 1, 1, 0, 1, 0]);
        let back = Tlv::parse(&tlv.encode()).unwrap();
        assert_eq!(back.byte(STATE), Some(1));
        assert_eq!(back.byte(METHOD), Some(0));
        assert_eq!(back.get(SALT), None);
    }

    #[test]
    fn long_values_are_fragmented_and_reassembled() {
        let key: Vec<u8> = (0..384u32)
            .map(|i| u8::try_from(i % 251).unwrap())
            .collect();
        let tlv = Tlv::new().with(PUBLIC_KEY, &key).with(STATE, [2]);
        let bytes = tlv.encode();
        // 255 + 129 bytes of key, then the state.
        assert_eq!(bytes[..2], [3, 255]);
        assert_eq!(bytes[257..259], [3, 129]);
        let back = Tlv::parse(&bytes).unwrap();
        assert_eq!(back.get(PUBLIC_KEY), Some(key.as_slice()));
        assert_eq!(back.byte(STATE), Some(2));
    }

    #[test]
    fn exactly_255_bytes_is_one_item() {
        let value = vec![7u8; 255];
        let back = Tlv::parse(&Tlv::new().with(SIGNATURE, &value).encode()).unwrap();
        assert_eq!(back.get(SIGNATURE), Some(value.as_slice()));
    }

    #[test]
    fn repeated_identifiers_with_separators_stay_apart() {
        let tlv = Tlv::new()
            .with(IDENTIFIER, b"a")
            .with(SEPARATOR, [])
            .with(IDENTIFIER, b"b");
        let back = Tlv::parse(&tlv.encode()).unwrap();
        assert_eq!(back.all(IDENTIFIER).collect::<Vec<_>>(), [b"a", b"b"]);
    }

    #[test]
    fn truncated_input_is_refused() {
        assert!(Tlv::parse(&[6]).is_none());
        assert!(Tlv::parse(&[3, 5, 1, 2]).is_none());
        assert!(Tlv::parse(&[]).unwrap().items().is_empty());
    }
}
