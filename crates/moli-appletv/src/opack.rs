//! OPACK, the compact binary encoding of Apple's Companion protocol (a
//! cousin of binary plists): one tag byte, then the value. Small integers,
//! short strings and short data fit their length in the tag; a decoder
//! keeps the values it read so a later one can point back to them.
//!
//! Moli writes without back-references (a reader accepts both) and reads
//! everything an Apple TV sends.

/// One value.
#[derive(Clone, Debug, PartialEq)]
pub enum Opack {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    Bytes(Vec<u8>),
    Uuid([u8; 16]),
    Array(Vec<Opack>),
    /// Keys in order of appearance (almost always strings).
    Dict(Vec<(Opack, Opack)>),
}

const END: u8 = 0x03;

impl Opack {
    /// A dictionary of string keys.
    pub fn dict<const N: usize>(entries: [(&str, Opack); N]) -> Self {
        Self::Dict(
            entries
                .into_iter()
                .map(|(k, v)| (Self::Str(k.to_owned()), v))
                .collect(),
        )
    }

    pub fn str(text: &str) -> Self {
        Self::Str(text.to_owned())
    }

    /// The value under a string key of a dictionary.
    pub fn get(&self, key: &str) -> Option<&Opack> {
        match self {
            Self::Dict(entries) => entries
                .iter()
                .find(|(k, _)| matches!(k, Self::Str(s) if s == key))
                .map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_int(&self) -> Option<i64> {
        match self {
            Self::Int(n) => Some(*n),
            #[allow(clippy::cast_possible_truncation)]
            Self::Float(f) if f.fract() == 0.0 => Some(*f as i64),
            Self::Bool(b) => Some(i64::from(*b)),
            _ => None,
        }
    }

    pub fn as_float(&self) -> Option<f64> {
        match self {
            Self::Float(f) => Some(*f),
            #[allow(clippy::cast_precision_loss)]
            Self::Int(n) => Some(*n as f64),
            _ => None,
        }
    }

    pub fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            Self::Bytes(b) => Some(b),
            _ => None,
        }
    }

    /// The entries of a dictionary (empty for anything else).
    pub fn entries(&self) -> &[(Opack, Opack)] {
        match self {
            Self::Dict(entries) => entries,
            _ => &[],
        }
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        self.write(&mut out);
        out
    }

    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    fn write(&self, out: &mut Vec<u8>) {
        match self {
            Self::Null => out.push(0x04),
            Self::Bool(true) => out.push(0x01),
            Self::Bool(false) => out.push(0x02),
            Self::Int(n) if (0..0x28).contains(n) => out.push(0x08 + *n as u8),
            Self::Int(n) => {
                // Negative numbers as their 8-byte two's complement.
                let bytes = n.to_le_bytes();
                let (tag, len) = match *n {
                    0..=0xFF => (0x30, 1),
                    0x100..=0xFFFF => (0x31, 2),
                    0x1_0000..=0xFFFF_FFFF => (0x32, 4),
                    _ => (0x33, 8),
                };
                out.push(tag);
                out.extend_from_slice(&bytes[..len]);
            }
            Self::Float(f) => {
                out.push(0x36);
                out.extend_from_slice(&f.to_le_bytes());
            }
            Self::Str(s) => {
                write_sized(out, s.as_bytes(), 0x40, 0x61, &[1, 2, 3, 4]);
            }
            Self::Bytes(b) => write_sized(out, b, 0x70, 0x91, &[1, 2, 4, 8]),
            Self::Uuid(u) => {
                out.push(0x05);
                out.extend_from_slice(u);
            }
            Self::Array(items) => {
                let short = items.len() < 0xF;
                out.push(if short {
                    0xD0 + items.len() as u8
                } else {
                    0xDF
                });
                for item in items {
                    item.write(out);
                }
                if !short {
                    out.push(END);
                }
            }
            Self::Dict(entries) => {
                let short = entries.len() < 0xF;
                out.push(if short {
                    0xE0 + entries.len() as u8
                } else {
                    0xEF
                });
                for (k, v) in entries {
                    k.write(out);
                    v.write(out);
                }
                if !short {
                    out.push(END);
                }
            }
        }
    }

    /// Reads one whole value; `None` for anything malformed or left over.
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        let mut reader = Reader {
            bytes,
            at: 0,
            seen: Vec::new(),
            depth: 0,
        };
        let value = reader.value()?;
        (reader.at == bytes.len()).then_some(value)
    }

    /// A readable form, for the log and tests.
    pub fn to_json(&self) -> serde_json::Value {
        use serde_json::Value as J;
        match self {
            Self::Null => J::Null,
            Self::Bool(b) => J::Bool(*b),
            Self::Int(n) => J::from(*n),
            Self::Float(f) => J::from(*f),
            Self::Str(s) => J::String(s.clone()),
            Self::Bytes(b) => J::String(format!("0x{}", hex(b))),
            Self::Uuid(u) => J::String(hex(u)),
            Self::Array(items) => J::Array(items.iter().map(Self::to_json).collect()),
            Self::Dict(entries) => J::Object(
                entries
                    .iter()
                    .map(|(k, v)| {
                        let key = match k {
                            Self::Str(s) => s.clone(),
                            other => other.to_json().to_string(),
                        };
                        (key, v.to_json())
                    })
                    .collect(),
            ),
        }
    }
}

/// A string or data: the length in the tag when short (up to 0x20), else
/// after one of four longer tags (`sizes` bytes, little endian).
#[allow(clippy::cast_possible_truncation)]
fn write_sized(out: &mut Vec<u8>, data: &[u8], short: u8, long: u8, sizes: &[usize; 4]) {
    let len = data.len();
    if len <= 0x20 {
        out.push(short + len as u8);
    } else {
        let (i, size) = sizes
            .iter()
            .enumerate()
            .find(|(_, s)| **s >= 8 || len < 1usize << (8 * **s))
            .map_or((3, sizes[3]), |(i, s)| (i, *s));
        out.push(long + i as u8);
        out.extend_from_slice(&(len as u64).to_le_bytes()[..size]);
    }
    out.extend_from_slice(data);
}

/// Lowercase hexadecimal.
pub fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::new(), |mut out, b| {
        let _ = write!(out, "{b:02x}");
        out
    })
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
    /// Values a back-reference may point to, in the order first read.
    seen: Vec<Opack>,
    depth: usize,
}

impl Reader<'_> {
    fn take(&mut self, n: usize) -> Option<&[u8]> {
        let slice = self.bytes.get(self.at..self.at.checked_add(n)?)?;
        self.at += n;
        Some(slice)
    }

    fn uint(&mut self, n: usize) -> Option<u64> {
        let mut buf = [0u8; 8];
        buf[..n].copy_from_slice(self.take(n)?);
        Some(u64::from_le_bytes(buf))
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.at).copied()
    }

    #[allow(clippy::cast_possible_wrap, clippy::cast_possible_truncation)]
    fn value(&mut self) -> Option<Opack> {
        self.depth += 1;
        if self.depth > 32 {
            return None;
        }
        let tag = *self.take(1)?.first()?;
        // Whether a back-reference may point here later.
        let mut keep = true;
        let value = match tag {
            0x01 => {
                keep = false;
                Opack::Bool(true)
            }
            0x02 => {
                keep = false;
                Opack::Bool(false)
            }
            0x04 => {
                keep = false;
                Opack::Null
            }
            0x05 => Opack::Uuid(self.take(16)?.try_into().ok()?),
            // 0x06 is an absolute time (seconds since 2001): kept as a number.
            0x08..=0x2F => {
                keep = false;
                Opack::Int(i64::from(tag - 0x08))
            }
            0x30..=0x33 => {
                let n = 1usize << (tag - 0x30);
                // Unsigned, but the 8-byte form is two's complement.
                Opack::Int(self.uint(n)? as i64)
            }
            0x35 => Opack::Float(f64::from(f32::from_le_bytes(
                self.take(4)?.try_into().ok()?,
            ))),
            0x06 | 0x36 => Opack::Float(f64::from_le_bytes(self.take(8)?.try_into().ok()?)),
            0x40..=0x60 => self.text(usize::from(tag - 0x40))?,
            0x61..=0x64 => {
                let n = self.uint(usize::from(tag - 0x60))?;
                self.text(usize::try_from(n).ok()?)?
            }
            0x70..=0x90 => Opack::Bytes(self.take(usize::from(tag - 0x70))?.to_vec()),
            0x91..=0x94 => {
                let n = self.uint(1 << (tag - 0x91))?;
                Opack::Bytes(self.take(usize::try_from(n).ok()?)?.to_vec())
            }
            0xA0..=0xC0 => {
                keep = false;
                self.seen.get(usize::from(tag - 0xA0))?.clone()
            }
            0xC1..=0xC4 => {
                keep = false;
                let index = self.uint(usize::from(tag - 0xC0))?;
                self.seen.get(usize::try_from(index).ok()?)?.clone()
            }
            0xD0..=0xDF => {
                keep = false;
                let count = tag & 0x0F;
                let mut items = Vec::new();
                if count == 0x0F {
                    while self.peek()? != END {
                        items.push(self.value()?);
                    }
                    self.at += 1;
                } else {
                    for _ in 0..count {
                        items.push(self.value()?);
                    }
                }
                Opack::Array(items)
            }
            0xE0..=0xEF => {
                keep = false;
                let count = tag & 0x0F;
                let mut entries = Vec::new();
                if count == 0x0F {
                    while self.peek()? != END {
                        let k = self.value()?;
                        entries.push((k, self.value()?));
                    }
                    self.at += 1;
                } else {
                    for _ in 0..count {
                        let k = self.value()?;
                        entries.push((k, self.value()?));
                    }
                }
                Opack::Dict(entries)
            }
            _ => return None,
        };
        if keep && !self.seen.contains(&value) {
            self.seen.push(value.clone());
        }
        self.depth -= 1;
        Some(value)
    }

    fn text(&mut self, n: usize) -> Option<Opack> {
        Some(Opack::Str(String::from_utf8(self.take(n)?.to_vec()).ok()?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_round_trip() {
        let long: String = "x".repeat(300);
        let value = Opack::dict([
            ("_i", Opack::str("_systemInfo")),
            ("_x", Opack::Int(41_234)),
            ("_t", Opack::Int(2)),
            ("small", Opack::Int(7)),
            ("big", Opack::Int(5_000_000_000)),
            ("neg", Opack::Int(-3)),
            ("f", Opack::Float(0.5)),
            ("yes", Opack::Bool(true)),
            ("none", Opack::Null),
            ("data", Opack::Bytes(vec![1, 2, 3])),
            ("long", Opack::Str(long)),
            (
                "list",
                Opack::Array((0..20).map(Opack::Int).collect::<Vec<_>>()),
            ),
        ]);
        assert_eq!(Opack::decode(&value.encode()), Some(value));
    }

    #[test]
    fn the_short_forms_match_apple_s() {
        // pyatv's reference encodings.
        assert_eq!(Opack::Int(0).encode(), [0x08]);
        assert_eq!(Opack::Int(0x27).encode(), [0x2F]);
        assert_eq!(Opack::Int(0x28).encode(), [0x30, 0x28]);
        assert_eq!(Opack::Int(0x1FF).encode(), [0x31, 0xFF, 0x01]);
        assert_eq!(Opack::str("a").encode(), [0x41, b'a']);
        assert_eq!(Opack::Bytes(vec![0xAC]).encode(), [0x71, 0xAC]);
        assert_eq!(
            Opack::dict([("a", Opack::Bool(false))]).encode(),
            [0xE1, 0x41, b'a', 0x02]
        );
        assert_eq!(Opack::Array(vec![]).encode(), [0xD0]);
    }

    #[test]
    fn back_references_point_to_values_already_read() {
        // {"a": "xy", "b": <ref 1>}: "a" is 0, "xy" is 1, "b" is 2.
        let bytes = [0xE2, 0x41, b'a', 0x42, b'x', b'y', 0x41, b'b', 0xA1];
        let value = Opack::decode(&bytes).unwrap();
        assert_eq!(value.get("b"), Some(&Opack::str("xy")));
        // An endless dictionary, closed by 0x03.
        let bytes = [0xEF, 0x41, b'k', 0x09, 0x03];
        assert_eq!(
            Opack::decode(&bytes).unwrap().get("k"),
            Some(&Opack::Int(1))
        );
    }

    #[test]
    fn junk_is_refused() {
        assert_eq!(Opack::decode(&[]), None);
        assert_eq!(Opack::decode(&[0x45, b'a']), None);
        assert_eq!(Opack::decode(&[0xA5]), None);
        assert_eq!(Opack::decode(&[0x08, 0x08]), None);
        assert_eq!(Opack::decode(&[0xFF]), None);
    }
}
