//! The RFXtrx serial protocol: length-prefixed frames, 38 400 bauds, 8N1.
//!
//! Every frame starts with its length (the bytes that follow), then its
//! packet type, subtype and a sequence number. Only what Moli reads is
//! decoded: the transceiver's status, and the sensors that measure the air
//! (temperature, humidity, pressure). The rest is named by its type, so a
//! module heard but not understood shows in the logs.

/// Commands to the transceiver (packet type 0x00, 14 bytes).
const fn command(seq: u8, cmd: u8) -> [u8; 14] {
    [0x0D, 0x00, 0x00, seq, cmd, 0, 0, 0, 0, 0, 0, 0, 0, 0]
}

/// Forget everything, stop receiving (then wait, then flush what came).
pub const RESET: [u8; 14] = command(0, 0x00);
/// « What are you, what do you listen to? »
pub const GET_STATUS: [u8; 14] = command(1, 0x02);
/// Firmware 1001 and later receive only after this.
pub const START_RECEIVER: [u8; 14] = command(2, 0x07);

/// Packet types Moli decodes.
pub const TEMP: u8 = 0x50;
pub const HUM: u8 = 0x51;
pub const TEMP_HUM: u8 = 0x52;
pub const TEMP_HUM_BARO: u8 = 0x54;

/// What the transceiver says about itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Status {
    /// 433.92 MHz for an RFXtrx433.
    pub frequency: &'static str,
    pub firmware: u16,
    pub firmware_type: &'static str,
    /// The enabled protocols, as the four bytes of the status (msg3 to msg6).
    pub protocols: [u8; 4],
}

/// One measure from a sensor.
#[derive(Clone, Debug, PartialEq)]
pub struct Reading {
    pub packet: u8,
    pub subtype: u8,
    /// The sensor's code on the air (Oregon: a new one after each battery
    /// change).
    pub id: u16,
    pub temperature: Option<f64>,
    pub humidity: Option<u8>,
    pub pressure: Option<u16>,
    /// 0 to 9; 0 is a battery to change.
    pub battery: u8,
    /// 0 to 15.
    pub rssi: u8,
}

impl Reading {
    /// The sensor's code: `<packet>-<subtype>-<id>`, `52-01-1a2b`.
    pub fn code(&self) -> String {
        code(self.packet, self.subtype, self.id)
    }
}

pub fn code(packet: u8, subtype: u8, id: u16) -> String {
    format!("{packet:02x}-{subtype:02x}-{id:04x}")
}

/// `52-01-1a2b` → its parts, for a decoded packet type only.
pub fn parse_code(code: &str) -> Option<(u8, u8, u16)> {
    let mut parts = code.trim().split('-');
    let packet = u8::from_str_radix(parts.next()?, 16).ok()?;
    let subtype = u8::from_str_radix(parts.next()?, 16).ok()?;
    let id = u16::from_str_radix(parts.next()?, 16).ok()?;
    (parts.next().is_none() && is_sensor(packet)).then_some((packet, subtype, id))
}

pub fn is_sensor(packet: u8) -> bool {
    matches!(packet, TEMP | HUM | TEMP_HUM | TEMP_HUM_BARO)
}

#[derive(Clone, Debug, PartialEq)]
pub enum Frame {
    Status(Status),
    Reading(Reading),
    /// Anything else: its packet type and subtype.
    Other(u8, u8),
}

/// One whole frame (length byte included) → what it says. `None` when the
/// frame is too short for its type.
pub fn decode(frame: &[u8]) -> Option<Frame> {
    let (&len, rest) = frame.split_first()?;
    if usize::from(len) != rest.len() || rest.len() < 3 {
        return None;
    }
    let (packet, subtype) = (frame[1], frame[2]);
    let at = |i: usize| frame.get(i).copied();
    let reading = |id: u16| Reading {
        packet,
        subtype,
        id,
        temperature: None,
        humidity: None,
        pressure: None,
        battery: 0,
        rssi: 0,
    };
    let id = || Some(u16::from(at(4)?) << 8 | u16::from(at(5)?));
    let temperature = |hi: u8, lo: u8| {
        let tenths = f64::from(u16::from(hi & 0x7F) << 8 | u16::from(lo)) / 10.0;
        if hi & 0x80 == 0 { tenths } else { -tenths }
    };
    // Battery in the low nibble, signal in the high one.
    let power = |r: &mut Reading, b: u8| {
        r.battery = b & 0x0F;
        r.rssi = b >> 4;
    };
    Some(match packet {
        0x01 if subtype == 0x00 && at(4)? == 0x02 => Frame::Status(status(frame)?),
        TEMP if len >= 0x08 => {
            let mut r = reading(id()?);
            r.temperature = Some(temperature(frame[6], frame[7]));
            power(&mut r, frame[8]);
            Frame::Reading(r)
        }
        HUM if len >= 0x08 => {
            let mut r = reading(id()?);
            r.humidity = Some(frame[6]);
            power(&mut r, frame[8]);
            Frame::Reading(r)
        }
        TEMP_HUM if len >= 0x0A => {
            let mut r = reading(id()?);
            r.temperature = Some(temperature(frame[6], frame[7]));
            r.humidity = Some(frame[8]);
            power(&mut r, frame[10]);
            Frame::Reading(r)
        }
        TEMP_HUM_BARO if len >= 0x0D => {
            let mut r = reading(id()?);
            r.temperature = Some(temperature(frame[6], frame[7]));
            r.humidity = Some(frame[8]);
            r.pressure = Some(u16::from(frame[10]) << 8 | u16::from(frame[11]));
            power(&mut r, frame[13]);
            Frame::Reading(r)
        }
        TEMP | HUM | TEMP_HUM | TEMP_HUM_BARO => return None,
        _ => Frame::Other(packet, subtype),
    })
}

fn status(frame: &[u8]) -> Option<Status> {
    let frequency = match frame.get(5)? {
        0x50 | 0x51 => "310/315 MHz",
        0x52..=0x54 => "433.92 MHz",
        0x55..=0x5F => "868 MHz",
        _ => "?",
    };
    let raw = *frame.get(6)?;
    let protocols = [
        *frame.get(7)?,
        *frame.get(8)?,
        *frame.get(9)?,
        *frame.get(10)?,
    ];
    // A long status (20 bytes) carries the firmware type; its version is
    // then counted from 1000.
    let (firmware, firmware_type) = match frame.get(14) {
        Some(&t) if frame[0] >= 0x14 => (
            1000 + u16::from(raw),
            match t {
                0x00 => "Type1 RFXrec",
                0x01 => "Type1",
                0x02 => "Type2",
                0x03 => "Ext",
                0x04 => "Ext2",
                0x05 => "Pro1",
                0x06 => "Pro2",
                0x10 => "ProXL1",
                _ => "?",
            },
        ),
        _ => (u16::from(raw), "?"),
    };
    Some(Status {
        frequency,
        firmware,
        firmware_type,
        protocols,
    })
}

/// Brand and model a packet type and subtype stand for.
// One line per (type, subtype), as the SDK lists them: a model sold under
// two packet types appears twice.
#[allow(clippy::match_same_arms)]
pub fn model(packet: u8, subtype: u8) -> (&'static str, &'static str) {
    const OREGON: &str = "Oregon Scientific";
    const LA_CROSSE: &str = "La Crosse";
    match (packet, subtype) {
        (TEMP, 0x01) => (OREGON, "THR128/138, THC138"),
        (TEMP, 0x02) => (
            OREGON,
            "THC238/268, THN132, THWR288, THRN122, THN122, AW129/131",
        ),
        (TEMP, 0x03) => (OREGON, "THWR800"),
        (TEMP, 0x04) => (OREGON, "RTHN318"),
        (TEMP, 0x05) => (LA_CROSSE, "TX2, TX3, TX4, TX17"),
        (TEMP, 0x06) => ("TFA", "TS15C"),
        (TEMP, 0x07) => ("Viking", "02811, 02813"),
        (TEMP, 0x08) => (LA_CROSSE, "WS2300"),
        (TEMP, 0x09) => ("Rubicson", "Rubicson"),
        (TEMP, 0x0A) => ("TFA", "30.3133"),
        (TEMP, 0x0B) => ("WT", "WT0122"),
        (HUM, 0x01) => (LA_CROSSE, "TX3"),
        (HUM, 0x02) => (LA_CROSSE, "WS2300"),
        (HUM, 0x03) => ("Inovalley", "S80"),
        (TEMP_HUM, 0x01) => (OREGON, "THGN122/123, THGN132, THGR122/228/238/268"),
        (TEMP_HUM, 0x02) => (OREGON, "THGR810, THGN800"),
        (TEMP_HUM, 0x03) => (OREGON, "RTGR328"),
        (TEMP_HUM, 0x04) => (OREGON, "THGR328"),
        (TEMP_HUM, 0x05) => (OREGON, "WTGR800"),
        (TEMP_HUM, 0x06) => (OREGON, "THGR918/928, THGRN228, THGN500"),
        (TEMP_HUM, 0x07) => ("TFA", "TS34C, Cresta"),
        (TEMP_HUM, 0x08) => (LA_CROSSE, "WT260, WT260H, WT440H, WT450, WT450H"),
        (TEMP_HUM, 0x09) => ("Viking", "02035, 02038"),
        (TEMP_HUM, 0x0A) => ("Rubicson", "Rubicson"),
        (TEMP_HUM_BARO, 0x01) => (OREGON, "BTHR918, BTHGN129"),
        (TEMP_HUM_BARO, 0x02) => (OREGON, "BTHR918N, BTHR968"),
        _ => ("RFXCOM", "433 MHz"),
    }
}

/// Cuts the byte stream into frames. A length byte that cannot start a
/// frame is dropped (the stream resynchronizes on the next one).
#[derive(Debug, Default)]
pub struct Framer {
    buf: Vec<u8>,
}

impl Framer {
    pub fn push(&mut self, bytes: &[u8]) -> Vec<Vec<u8>> {
        self.buf.extend_from_slice(bytes);
        let mut frames = Vec::new();
        while let Some(&len) = self.buf.first() {
            // No frame is shorter than 4 bytes nor longer than 64.
            if !(4..=0x40).contains(&len) {
                self.buf.remove(0);
            } else if self.buf.len() > usize::from(len) {
                frames.push(self.buf.drain(..=usize::from(len)).collect());
            } else {
                break;
            }
        }
        frames
    }

    /// Drops a half frame (one whose end never came).
    pub fn clear(&mut self) {
        self.buf.clear();
    }

    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Real frames heard by an RFXtrx433E (sensor codes changed), one per
    /// line: `<hex>` after the time.
    fn fixture() -> Vec<Vec<u8>> {
        include_str!("../../../integrations/rfxtrx/fixtures/frames.txt")
            .lines()
            .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
            .map(|l| {
                l.split_whitespace()
                    .skip(1)
                    .map(|b| u8::from_str_radix(b, 16).unwrap())
                    .collect()
            })
            .collect()
    }

    #[test]
    fn reads_a_real_capture() {
        let frames = fixture();
        assert_eq!(
            decode(&frames[0]),
            Some(Frame::Status(Status {
                frequency: "433.92 MHz",
                firmware: 1025,
                firmware_type: "Ext",
                protocols: [0x00, 0x0C, 0x2F, 0x00],
            }))
        );
        assert_eq!(decode(&frames[1]), Some(Frame::Other(0x01, 0x07)));
        let Some(Frame::Reading(r)) = decode(&frames[2]) else {
            panic!("a reading");
        };
        assert_eq!(r.code(), "52-01-1a2b");
        assert_eq!(r.temperature, Some(22.6));
        assert_eq!(r.humidity, Some(50));
        assert_eq!((r.battery, r.rssi), (9, 5));
        assert_eq!(model(r.packet, r.subtype).0, "Oregon Scientific");
        let Some(Frame::Reading(r)) = decode(&frames[3]) else {
            panic!("a reading");
        };
        assert_eq!(r.code(), "52-01-3c4d");
        assert_eq!(r.temperature, Some(21.5));
        assert_eq!(r.humidity, Some(53));
    }

    #[test]
    fn decodes_every_air_sensor() {
        // Below zero: the sign is the high bit.
        let t = decode(&[0x08, TEMP, 0x02, 0, 0xAB, 0xCD, 0x80, 0x2D, 0x70]);
        let Some(Frame::Reading(t)) = t else { panic!() };
        assert_eq!(t.temperature, Some(-4.5));
        assert_eq!((t.battery, t.rssi), (0, 7));
        assert_eq!(t.humidity, None);

        let h = decode(&[0x08, HUM, 0x01, 0, 0x00, 0x01, 61, 0x02, 0x89]);
        let Some(Frame::Reading(h)) = h else { panic!() };
        assert_eq!((h.humidity, h.temperature), (Some(61), None));

        let b = decode(&[
            0x0D,
            TEMP_HUM_BARO,
            0x01,
            0,
            0x12,
            0x34,
            0x00,
            0xC8,
            45,
            0x01,
            0x03,
            0xF5,
            0x01,
            0x69,
        ]);
        let Some(Frame::Reading(b)) = b else { panic!() };
        assert_eq!(b.temperature, Some(20.0));
        assert_eq!(b.pressure, Some(1013));
        assert_eq!(b.code(), "54-01-1234");

        // Too short for its type, or for its own length: nothing.
        assert_eq!(decode(&[0x05, TEMP_HUM, 0x01, 0, 0x12, 0x34]), None);
        assert_eq!(decode(&[0x0A, TEMP_HUM, 0x01]), None);
        // A light switch: named, not read.
        assert_eq!(
            decode(&[0x07, 0x10, 0x01, 0, 0x41, 0x01, 0x01, 0x70]),
            Some(Frame::Other(0x10, 0x01))
        );
    }

    #[test]
    fn cuts_the_stream_into_frames() {
        let mut f = Framer::default();
        let frames = fixture();
        let stream: Vec<u8> = frames.iter().flatten().copied().collect();
        // Byte by byte, then all at once: the same frames.
        let mut one: Vec<Vec<u8>> = Vec::new();
        for b in &stream {
            one.extend(f.push(std::slice::from_ref(b)));
        }
        assert_eq!(one, frames);
        assert!(f.is_empty());
        assert_eq!(f.push(&stream), frames);
        // Noise before a frame is skipped.
        let mut noisy = vec![0x00, 0xFF, 0x02];
        noisy.extend_from_slice(&frames[2]);
        let got = f.push(&noisy);
        assert_eq!(got.last(), Some(&frames[2]));
        // Half a frame waits for the rest.
        assert!(f.push(&frames[3][..5]).is_empty());
        assert!(!f.is_empty());
        f.clear();
        assert!(f.is_empty());
    }

    #[test]
    fn codes_round_trip() {
        assert_eq!(parse_code("52-01-1a2b"), Some((0x52, 0x01, 0x1A2B)));
        assert_eq!(parse_code(" 50-02-00ff "), Some((0x50, 0x02, 0x00FF)));
        assert_eq!(parse_code("10-01-1a2b"), None, "not a sensor Moli reads");
        assert_eq!(parse_code("52-01"), None);
        assert_eq!(parse_code("52-01-1a2b-0"), None);
        assert_eq!(code(0x52, 1, 0x1A2B), "52-01-1a2b");
        assert_eq!(START_RECEIVER.len(), usize::from(START_RECEIVER[0]) + 1);
        assert_eq!(GET_STATUS[4], 0x02);
        assert_eq!(RESET[4], 0x00);
    }
}
