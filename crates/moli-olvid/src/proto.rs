//! Just enough Protocol Buffers for the daemon's few messages: varints,
//! text, nested messages. No schema, no generated code: the fields used are
//! listed next to the calls (`daemon.rs`).

use anyhow::{bail, ensure};

pub fn varint(out: &mut Vec<u8>, mut value: u64) {
    loop {
        let byte = (value & 0x7f) as u8;
        value >>= 7;
        if value == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

fn tag(out: &mut Vec<u8>, field: u32, wire: u8) {
    varint(out, u64::from(field) << 3 | u64::from(wire));
}

pub fn put_uint(out: &mut Vec<u8>, field: u32, value: u64) {
    tag(out, field, 0);
    varint(out, value);
}

pub fn put_bytes(out: &mut Vec<u8>, field: u32, value: &[u8]) {
    tag(out, field, 2);
    varint(out, value.len() as u64);
    out.extend_from_slice(value);
}

pub fn put_str(out: &mut Vec<u8>, field: u32, value: &str) {
    put_bytes(out, field, value.as_bytes());
}

/// One field read from a message.
#[derive(Debug, PartialEq, Eq)]
pub enum Wire<'a> {
    Varint(u64),
    Bytes(&'a [u8]),
    /// 32 or 64 bits fixed: never needed, skipped.
    Fixed,
}

impl<'a> Wire<'a> {
    pub fn uint(&self) -> u64 {
        match self {
            Self::Varint(v) => *v,
            _ => 0,
        }
    }

    pub fn bytes(&self) -> &'a [u8] {
        match self {
            Self::Bytes(b) => b,
            _ => &[],
        }
    }

    pub fn text(&self) -> String {
        String::from_utf8_lossy(self.bytes()).into_owned()
    }
}

fn read_varint(data: &[u8], at: &mut usize) -> anyhow::Result<u64> {
    let mut value = 0u64;
    for shift in (0..70).step_by(7) {
        let Some(&byte) = data.get(*at) else {
            bail!("protobuf: truncated varint");
        };
        *at += 1;
        if shift < 64 {
            value |= u64::from(byte & 0x7f) << shift;
        }
        if byte & 0x80 == 0 {
            return Ok(value);
        }
    }
    bail!("protobuf: varint too long")
}

/// Every `(field, value)` of a message, in order.
pub fn fields(data: &[u8]) -> anyhow::Result<Vec<(u32, Wire<'_>)>> {
    let mut out = Vec::new();
    let mut at = 0;
    while at < data.len() {
        let key = read_varint(data, &mut at)?;
        let field = u32::try_from(key >> 3)?;
        match key & 7 {
            0 => out.push((field, Wire::Varint(read_varint(data, &mut at)?))),
            1 | 5 => {
                let n = if key & 7 == 1 { 8 } else { 4 };
                ensure!(at + n <= data.len(), "protobuf: truncated fixed field");
                at += n;
                out.push((field, Wire::Fixed));
            }
            2 => {
                let n = usize::try_from(read_varint(data, &mut at)?)?;
                let end = at.checked_add(n).filter(|end| *end <= data.len());
                let Some(end) = end else {
                    bail!("protobuf: truncated field");
                };
                out.push((field, Wire::Bytes(&data[at..end])));
                at = end;
            }
            other => bail!("protobuf: wire type {other} not supported"),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_message_round_trips() {
        let mut m = Vec::new();
        put_uint(&mut m, 1, 300);
        put_str(&mut m, 2, "bonjour é");
        let mut inner = Vec::new();
        put_str(&mut inner, 1, "Moli");
        put_bytes(&mut m, 3, &inner);
        let f = fields(&m).unwrap();
        assert_eq!(f.len(), 3);
        assert_eq!(f[0], (1, Wire::Varint(300)));
        assert_eq!(f[1].1.text(), "bonjour é");
        assert_eq!(fields(f[2].1.bytes()).unwrap()[0].1.text(), "Moli");
    }

    #[test]
    fn known_encodings() {
        // The protobuf documentation's example: field 1 = 150.
        let mut m = Vec::new();
        put_uint(&mut m, 1, 150);
        assert_eq!(m, [0x08, 0x96, 0x01]);
    }

    #[test]
    fn damaged_messages_are_refused_not_panicked_on() {
        assert!(fields(&[0x0a, 0x05, b'a']).is_err());
        assert!(fields(&[0x08]).is_err());
        assert!(fields(&[0x0b]).is_err());
        assert!(fields(&[0x80; 12]).is_err());
        assert!(fields(&[]).unwrap().is_empty());
    }
}
