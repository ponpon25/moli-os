//! Apple's binary property lists (`bplist00`), the bodies of AirPlay's
//! RTSP requests and of its data stream: a header, the objects, an offset
//! table, a trailer. Just what AirPlay uses: booleans, integers, reals,
//! strings, data, arrays and dictionaries.

/// One value.
#[derive(Clone, Debug, PartialEq)]
pub enum Plist {
    Bool(bool),
    Int(i64),
    Real(f64),
    Str(String),
    Data(Vec<u8>),
    Array(Vec<Plist>),
    Dict(Vec<(String, Plist)>),
}

impl Plist {
    pub fn dict<const N: usize>(entries: [(&str, Plist); N]) -> Self {
        Self::Dict(
            entries
                .into_iter()
                .map(|(k, v)| (k.to_owned(), v))
                .collect(),
        )
    }

    pub fn str(text: &str) -> Self {
        Self::Str(text.to_owned())
    }

    pub fn get(&self, key: &str) -> Option<&Plist> {
        match self {
            Self::Dict(entries) => entries.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn as_int(&self) -> Option<i64> {
        match self {
            Self::Int(n) => Some(*n),
            _ => None,
        }
    }

    pub fn as_data(&self) -> Option<&[u8]> {
        match self {
            Self::Data(d) => Some(d),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[Plist]> {
        match self {
            Self::Array(items) => Some(items),
            _ => None,
        }
    }

    /// The whole file.
    #[allow(clippy::cast_possible_truncation)]
    pub fn encode(&self) -> Vec<u8> {
        // Objects flattened, the root first; containers refer by index.
        let mut objects: Vec<Vec<u8>> = Vec::new();
        let count = self.count();
        let ref_size = size_for(count as u64);
        flatten(self, &mut objects, ref_size);
        let mut out = b"bplist00".to_vec();
        let mut offsets = Vec::with_capacity(objects.len());
        for object in &objects {
            offsets.push(out.len() as u64);
            out.extend_from_slice(object);
        }
        let table = out.len() as u64;
        let offset_size = size_for(table);
        for offset in offsets {
            out.extend_from_slice(&offset.to_be_bytes()[8 - offset_size..]);
        }
        out.extend_from_slice(&[0; 6]);
        out.push(offset_size as u8);
        out.push(ref_size as u8);
        out.extend_from_slice(&(objects.len() as u64).to_be_bytes());
        out.extend_from_slice(&0u64.to_be_bytes());
        out.extend_from_slice(&table.to_be_bytes());
        out
    }

    fn count(&self) -> usize {
        1 + match self {
            Self::Array(items) => items.iter().map(Self::count).sum(),
            // Each key is an object too.
            Self::Dict(entries) => entries.iter().map(|(_, v)| 1 + v.count()).sum(),
            _ => 0,
        }
    }

    /// Reads a whole file; `None` for anything malformed.
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 40 || !bytes.starts_with(b"bplist00") {
            return None;
        }
        let trailer = &bytes[bytes.len() - 32..];
        let offset_size = usize::from(trailer[6]);
        let ref_size = usize::from(trailer[7]);
        let count = usize::try_from(be(&trailer[8..16])).ok()?;
        let root = usize::try_from(be(&trailer[16..24])).ok()?;
        let table = usize::try_from(be(&trailer[24..32])).ok()?;
        if !(1..=8).contains(&offset_size) || !(1..=8).contains(&ref_size) || count > 1 << 20 {
            return None;
        }
        let mut offsets = Vec::with_capacity(count);
        for i in 0..count {
            let at = table.checked_add(i * offset_size)?;
            offsets.push(usize::try_from(be(bytes.get(at..at + offset_size)?)).ok()?);
        }
        let reader = Reader {
            bytes,
            offsets,
            ref_size,
        };
        reader.object(root, 0)
    }
}

fn be(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0, |n, b| (n << 8) | u64::from(*b))
}

fn size_for(n: u64) -> usize {
    match n {
        0..=0xFF => 1,
        0x100..=0xFFFF => 2,
        0x1_0000..=0xFFFF_FFFF => 4,
        _ => 8,
    }
}

/// A marker and its length: inline under 15, else an integer after.
#[allow(clippy::cast_possible_truncation)]
fn marker(out: &mut Vec<u8>, kind: u8, len: usize) {
    if len < 15 {
        out.push(kind | len as u8);
    } else {
        out.push(kind | 0x0F);
        int(out, i64::try_from(len).unwrap_or(i64::MAX));
    }
}

#[allow(clippy::cast_sign_loss)]
fn int(out: &mut Vec<u8>, n: i64) {
    // Negative numbers take the 8-byte form.
    let size = if n < 0 { 8 } else { size_for(n as u64) };
    out.push(0x10 | size.trailing_zeros() as u8);
    out.extend_from_slice(&n.to_be_bytes()[8 - size..]);
}

/// Writes `value` and what it contains; returns its index.
#[allow(clippy::cast_possible_truncation)]
fn flatten(value: &Plist, objects: &mut Vec<Vec<u8>>, ref_size: usize) -> usize {
    let index = objects.len();
    objects.push(Vec::new());
    let mut out = Vec::new();
    match value {
        Plist::Bool(b) => out.push(if *b { 0x09 } else { 0x08 }),
        Plist::Int(n) => int(&mut out, *n),
        Plist::Real(f) => {
            out.push(0x23);
            out.extend_from_slice(&f.to_be_bytes());
        }
        Plist::Str(s) if s.is_ascii() => {
            marker(&mut out, 0x50, s.len());
            out.extend_from_slice(s.as_bytes());
        }
        Plist::Str(s) => {
            let units: Vec<u16> = s.encode_utf16().collect();
            marker(&mut out, 0x60, units.len());
            for u in units {
                out.extend_from_slice(&u.to_be_bytes());
            }
        }
        Plist::Data(d) => {
            marker(&mut out, 0x40, d.len());
            out.extend_from_slice(d);
        }
        Plist::Array(items) => {
            let refs: Vec<usize> = items
                .iter()
                .map(|i| flatten(i, objects, ref_size))
                .collect();
            marker(&mut out, 0xA0, refs.len());
            for r in refs {
                out.extend_from_slice(&(r as u64).to_be_bytes()[8 - ref_size..]);
            }
        }
        Plist::Dict(entries) => {
            let keys: Vec<usize> = entries
                .iter()
                .map(|(k, _)| flatten(&Plist::Str(k.clone()), objects, ref_size))
                .collect();
            let values: Vec<usize> = entries
                .iter()
                .map(|(_, v)| flatten(v, objects, ref_size))
                .collect();
            marker(&mut out, 0xD0, entries.len());
            for r in keys.into_iter().chain(values) {
                out.extend_from_slice(&(r as u64).to_be_bytes()[8 - ref_size..]);
            }
        }
    }
    objects[index] = out;
    index
}

struct Reader<'a> {
    bytes: &'a [u8],
    offsets: Vec<usize>,
    ref_size: usize,
}

impl Reader<'_> {
    /// The length after a marker, and where the content starts.
    fn length(&self, at: usize) -> Option<(usize, usize)> {
        let low = self.bytes.get(at)? & 0x0F;
        if low < 0x0F {
            return Some((usize::from(low), at + 1));
        }
        let size = 1usize << (self.bytes.get(at + 1)? & 0x0F);
        let n = be(self.bytes.get(at + 2..at + 2 + size)?);
        Some((usize::try_from(n).ok()?, at + 2 + size))
    }

    fn refs(&self, at: usize, n: usize) -> Option<Vec<usize>> {
        (0..n)
            .map(|i| {
                let start = at + i * self.ref_size;
                usize::try_from(be(self.bytes.get(start..start + self.ref_size)?)).ok()
            })
            .collect()
    }

    #[allow(clippy::cast_possible_wrap)]
    fn object(&self, index: usize, depth: usize) -> Option<Plist> {
        if depth > 32 {
            return None;
        }
        let at = *self.offsets.get(index)?;
        let tag = *self.bytes.get(at)?;
        Some(match tag >> 4 {
            0x0 => match tag {
                0x08 => Plist::Bool(false),
                0x09 => Plist::Bool(true),
                _ => return None,
            },
            0x1 => {
                let size = 1usize << (tag & 0x0F);
                let raw = self.bytes.get(at + 1..at + 1 + size)?;
                // A 16-byte integer: its low 8 bytes.
                Plist::Int(be(&raw[raw.len().saturating_sub(8)..]) as i64)
            }
            0x2 => match tag & 0x0F {
                2 => Plist::Real(f64::from(f32::from_be_bytes(
                    self.bytes.get(at + 1..at + 5)?.try_into().ok()?,
                ))),
                3 => Plist::Real(f64::from_be_bytes(
                    self.bytes.get(at + 1..at + 9)?.try_into().ok()?,
                )),
                _ => return None,
            },
            // A date: seconds since 2001, as a real.
            0x3 => Plist::Real(f64::from_be_bytes(
                self.bytes.get(at + 1..at + 9)?.try_into().ok()?,
            )),
            0x4 => {
                let (n, start) = self.length(at)?;
                Plist::Data(self.bytes.get(start..start + n)?.to_vec())
            }
            0x5 => {
                let (n, start) = self.length(at)?;
                Plist::Str(String::from_utf8_lossy(self.bytes.get(start..start + n)?).into_owned())
            }
            0x6 => {
                let (n, start) = self.length(at)?;
                let raw = self.bytes.get(start..start + 2 * n)?;
                let units: Vec<u16> = raw
                    .chunks(2)
                    .map(|c| u16::from_be_bytes([c[0], c[1]]))
                    .collect();
                Plist::Str(String::from_utf16_lossy(&units))
            }
            // A UID (keyed archives): kept as its number.
            0x8 => {
                let size = usize::from(tag & 0x0F) + 1;
                Plist::Int(be(self.bytes.get(at + 1..at + 1 + size)?) as i64)
            }
            0xA => {
                let (n, start) = self.length(at)?;
                let refs = self.refs(start, n)?;
                Plist::Array(
                    refs.into_iter()
                        .map(|r| self.object(r, depth + 1))
                        .collect::<Option<_>>()?,
                )
            }
            0xD => {
                let (n, start) = self.length(at)?;
                let keys = self.refs(start, n)?;
                let values = self.refs(start + n * self.ref_size, n)?;
                let mut entries = Vec::with_capacity(n);
                for (k, v) in keys.into_iter().zip(values) {
                    let Plist::Str(key) = self.object(k, depth + 1)? else {
                        return None;
                    };
                    entries.push((key, self.object(v, depth + 1)?));
                }
                Plist::Dict(entries)
            }
            _ => return None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_round_trip() {
        let value = Plist::dict([
            ("isRemoteControlOnly", Plist::Bool(true)),
            ("seed", Plist::Int(1_234_567_890_123)),
            ("type", Plist::Int(130)),
            ("neg", Plist::Int(-2)),
            ("name", Plist::str("Moli")),
            ("accent", Plist::str("Télé")),
            ("blob", Plist::Data(vec![0; 40])),
            ("x", Plist::Real(1.5)),
            (
                "streams",
                Plist::Array(vec![Plist::dict([("controlType", Plist::Int(2))])]),
            ),
        ]);
        assert_eq!(Plist::decode(&value.encode()), Some(value));
    }

    #[test]
    fn a_python_plist_is_read() {
        // plistlib.dumps({"eventPort": 50000}, fmt=FMT_BINARY)
        let bytes = [
            0x62, 0x70, 0x6c, 0x69, 0x73, 0x74, 0x30, 0x30, 0xd1, 0x01, 0x02, 0x59, 0x65, 0x76,
            0x65, 0x6e, 0x74, 0x50, 0x6f, 0x72, 0x74, 0x11, 0xc3, 0x50, 0x08, 0x0b, 0x15, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x03, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x18,
        ];
        let value = Plist::decode(&bytes).unwrap();
        assert_eq!(value.get("eventPort"), Some(&Plist::Int(50_000)));
    }

    #[test]
    fn junk_is_refused() {
        assert_eq!(Plist::decode(b"bplist00"), None);
        assert_eq!(Plist::decode(&[0; 64]), None);
    }
}
