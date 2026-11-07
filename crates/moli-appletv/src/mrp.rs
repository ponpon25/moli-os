//! The few MRP (Media Remote Protocol) messages Moli needs, as protobuf by
//! hand: who Moli is, « connected », which updates it wants, and changing
//! the Apple TV's audio outputs. A `ProtocolMessage` carries its type
//! (field 1) and the message itself as an extension field.

/// Message types and the field each one's body lives in.
pub const DEVICE_INFO: (u64, u32) = (15, 20);
pub const CLIENT_UPDATES_CONFIG: (u64, u32) = (16, 21);
pub const SET_CONNECTION_STATE: (u64, u32) = (38, 42);
pub const MODIFY_OUTPUT_CONTEXT: (u64, u32) = (48, 52);
pub const UPDATE_OUTPUT_DEVICE: (u64, u32) = (65, 69);

/// A protobuf message being written.
#[derive(Debug, Default)]
pub struct Writer(Vec<u8>);

fn varint(out: &mut Vec<u8>, mut n: u64) {
    while n >= 0x80 {
        out.push((n as u8) | 0x80);
        n >>= 7;
    }
    out.push(n as u8);
}

impl Writer {
    #[must_use]
    pub fn uint(mut self, field: u32, n: u64) -> Self {
        varint(&mut self.0, u64::from(field) << 3);
        varint(&mut self.0, n);
        self
    }

    #[must_use]
    pub fn bool(self, field: u32, b: bool) -> Self {
        self.uint(field, u64::from(b))
    }

    #[must_use]
    pub fn bytes(mut self, field: u32, data: &[u8]) -> Self {
        varint(&mut self.0, (u64::from(field) << 3) | 2);
        varint(&mut self.0, data.len() as u64);
        self.0.extend_from_slice(data);
        self
    }

    #[must_use]
    pub fn str(self, field: u32, text: &str) -> Self {
        self.bytes(field, text.as_bytes())
    }

    pub fn finish(self) -> Vec<u8> {
        self.0
    }
}

/// One field read: its number and its value.
#[derive(Debug, PartialEq, Eq)]
pub enum Field<'a> {
    Varint(u32, u64),
    Bytes(u32, &'a [u8]),
}

fn read_varint(bytes: &[u8], at: &mut usize) -> Option<u64> {
    let mut n = 0u64;
    for shift in (0..64).step_by(7) {
        let b = *bytes.get(*at)?;
        *at += 1;
        n |= u64::from(b & 0x7F) << shift;
        if b & 0x80 == 0 {
            return Some(n);
        }
    }
    None
}

/// The fields of a message (fixed-width ones skipped); `None` if cut short.
pub fn fields(bytes: &[u8]) -> Option<Vec<Field<'_>>> {
    let mut at = 0;
    let mut out = Vec::new();
    while at < bytes.len() {
        let key = read_varint(bytes, &mut at)?;
        let field = u32::try_from(key >> 3).ok()?;
        match key & 7 {
            0 => out.push(Field::Varint(field, read_varint(bytes, &mut at)?)),
            1 => at += 8,
            2 => {
                let len = usize::try_from(read_varint(bytes, &mut at)?).ok()?;
                out.push(Field::Bytes(field, bytes.get(at..at + len)?));
                at += len;
            }
            5 => at += 4,
            _ => return None,
        }
    }
    (at == bytes.len()).then_some(out)
}

/// A whole `ProtocolMessage` of `kind`, its body in its field.
pub fn message((kind, field): (u64, u32), body: Writer, id: &str) -> Vec<u8> {
    Writer::default()
        .uint(1, kind)
        .str(2, id)
        .uint(4, 0)
        .str(85, id)
        .bytes(field, &body.finish())
        .finish()
}

/// The first message, without which the Apple TV says nothing.
pub fn device_info(id: &str, client: &str, name: &str) -> Vec<u8> {
    let body = Writer::default()
        .str(1, client)
        .str(2, name)
        .str(3, "iPhone")
        .str(4, "21A329")
        .str(5, "com.apple.TVRemote")
        .str(6, "344.28")
        .uint(7, 1)
        .uint(8, 108)
        .bool(9, true)
        .bool(10, true)
        .str(12, "com.apple.TVMusic")
        .bool(13, true)
        .bool(14, true)
        .bool(15, true)
        .uint(17, 2);
    message(DEVICE_INFO, body, id)
}

pub fn set_connection_state(id: &str) -> Vec<u8> {
    // Connected.
    message(SET_CONNECTION_STATE, Writer::default().uint(1, 2), id)
}

/// Volume and output device updates only.
pub fn client_updates_config(id: &str) -> Vec<u8> {
    let body = Writer::default()
        .bool(1, false)
        .bool(2, false)
        .bool(3, true)
        .bool(4, false)
        .bool(5, true);
    message(CLIENT_UPDATES_CONFIG, body, id)
}

/// How the outputs change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Change {
    Add,
    Remove,
    /// Unused by the driver (the Apple TV refused it in tests), kept for
    /// completeness of the message.
    #[allow(dead_code)]
    Set,
}

pub fn modify_outputs(id: &str, change: Change, uids: &[String]) -> Vec<u8> {
    let field = match change {
        Change::Add => 2,
        Change::Remove => 3,
        Change::Set => 4,
    };
    // Type 1: shared audio presentation.
    let mut body = Writer::default().uint(1, 1);
    // The « cluster aware » twin list too (fields 5 to 7), as an iPhone.
    for uid in uids {
        body = body.str(field, uid);
    }
    for uid in uids {
        body = body.str(field + 3, uid);
    }
    message(MODIFY_OUTPUT_CONTEXT, body, id)
}

/// An audio output of the Apple TV.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Output {
    pub name: String,
    pub uid: String,
}

/// The type of a `ProtocolMessage`, and its body (the first extension).
pub fn open(message: &[u8]) -> Option<(u64, Option<&[u8]>)> {
    let fields = fields(message)?;
    let kind = fields.iter().find_map(|f| match f {
        Field::Varint(1, k) => Some(*k),
        _ => None,
    })?;
    let body = fields.iter().find_map(|f| match f {
        Field::Bytes(n, b) if (6..=84).contains(n) || *n >= 86 => Some(*b),
        _ => None,
    });
    Some((kind, body))
}

/// The outputs an `UpdateOutputDeviceMessage` lists (field 1, or the
/// « cluster aware » list of field 3 that recent systems fill instead).
pub fn outputs_of(body: &[u8]) -> Vec<Output> {
    let Some(fields) = fields(body) else {
        return Vec::new();
    };
    fields
        .iter()
        .filter_map(|f| match f {
            Field::Bytes(1 | 3, descriptor) => {
                let inner = self::fields(descriptor)?;
                let text = |n: u32| {
                    inner.iter().find_map(|f| match f {
                        Field::Bytes(m, b) if *m == n => {
                            Some(String::from_utf8_lossy(b).into_owned())
                        }
                        _ => None,
                    })
                };
                Some(Output {
                    name: text(1).unwrap_or_default(),
                    uid: text(2)?,
                })
            }
            _ => None,
        })
        .fold(Vec::new(), |mut all: Vec<Output>, o| {
            // The same device may be in both lists.
            if all.iter().all(|a| a.uid != o.uid) {
                all.push(o);
            }
            all
        })
}

/// The messages of a data stream payload: one bare message (it starts
/// with field 1's key), or several, each after its length.
pub fn split(data: &[u8]) -> Vec<&[u8]> {
    if data.first() == Some(&0x08) {
        return vec![data];
    }
    let mut out = Vec::new();
    let mut at = 0;
    while at < data.len() {
        let Some(len) = read_varint(data, &mut at).and_then(|n| usize::try_from(n).ok()) else {
            break;
        };
        let Some(message) = data.get(at..at + len) else {
            break;
        };
        out.push(message);
        at += len;
    }
    out
}

/// Messages after their length, for the data stream.
pub fn join(messages: &[Vec<u8>]) -> Vec<u8> {
    let mut out = Vec::new();
    for m in messages {
        varint(&mut out, m.len() as u64);
        out.extend_from_slice(m);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_carry_their_type_and_body() {
        let wire = modify_outputs("x", Change::Add, &["AA:BB".into()]);
        let (kind, body) = open(&wire).unwrap();
        assert_eq!(kind, MODIFY_OUTPUT_CONTEXT.0);
        assert_eq!(
            fields(body.unwrap()).unwrap(),
            vec![
                Field::Varint(1, 1),
                Field::Bytes(2, b"AA:BB"),
                Field::Bytes(5, b"AA:BB")
            ]
        );
    }

    #[test]
    fn output_lists_are_read() {
        let descriptor = |name: &str, uid: &str| {
            Writer::default()
                .str(1, name)
                .str(2, uid)
                .bool(14, true)
                .finish()
        };
        let body = Writer::default()
            .bytes(1, &descriptor("Salon", "AA:BB"))
            .bytes(1, &descriptor("Sonos", "CC:DD"))
            .str(2, "endpoint");
        let wire = message(UPDATE_OUTPUT_DEVICE, body, "id");
        let (kind, body) = open(&wire).unwrap();
        assert_eq!(kind, UPDATE_OUTPUT_DEVICE.0);
        let outputs = outputs_of(body.unwrap());
        assert_eq!(outputs.len(), 2);
        assert_eq!(outputs[1].uid, "CC:DD");
    }

    #[test]
    fn payloads_split_into_messages() {
        let a = set_connection_state("a");
        let b = client_updates_config("b");
        let joined = join(&[a.clone(), b.clone()]);
        assert_eq!(split(&joined), vec![&a[..], &b[..]]);
        assert_eq!(split(&a), vec![&a[..]]);
    }
}
