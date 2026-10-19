//! H.264 over RTP (RFC 6184): rebuilds the frames of a camera's stream
//! from its packets, and reads the picture size out of a sequence
//! parameter set. No decoding: the NAL units come out as they went in.

use std::collections::VecDeque;

/// One frame: the NAL units sharing an RTP timestamp.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccessUnit {
    pub timestamp: u32,
    /// NAL units without start codes (the first byte is the NAL header).
    pub nals: Vec<Vec<u8>>,
    /// Contains an IDR slice.
    pub keyframe: bool,
}

pub const NAL_IDR: u8 = 5;
pub const NAL_SPS: u8 = 7;
pub const NAL_PPS: u8 = 8;

/// The type of a NAL unit (`nal_unit_type`).
#[must_use]
pub fn nal_type(nal: &[u8]) -> u8 {
    nal.first().map_or(0, |b| b & 0x1F)
}

/// Rebuilds frames from RTP packets (single NAL units, STAP-A, FU-A: what
/// cameras send in packetization mode 1). A packet lost in the middle of a
/// frame drops that frame.
#[derive(Debug, Default)]
pub struct Depacketizer {
    last_seq: Option<u16>,
    timestamp: Option<u32>,
    nals: Vec<Vec<u8>>,
    /// A fragmented NAL being rebuilt.
    fragment: Option<Vec<u8>>,
    damaged: bool,
}

impl Depacketizer {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Takes one RTP packet; returns the frames it completed (usually none
    /// or one).
    pub fn push(&mut self, packet: &[u8]) -> Vec<AccessUnit> {
        let mut out = VecDeque::new();
        let Some(rtp) = Rtp::parse(packet) else {
            return Vec::new();
        };
        if let Some(last) = self.last_seq
            && rtp.seq != last.wrapping_add(1)
        {
            self.damaged = true;
            self.fragment = None;
        }
        self.last_seq = Some(rtp.seq);
        if self.timestamp.is_some_and(|t| t != rtp.timestamp) {
            out.extend(self.finish());
        }
        self.timestamp = Some(rtp.timestamp);
        self.payload(rtp.payload);
        if rtp.marker {
            out.extend(self.finish());
        }
        out.into()
    }

    fn finish(&mut self) -> Option<AccessUnit> {
        let timestamp = self.timestamp.take()?;
        let nals = std::mem::take(&mut self.nals);
        self.fragment = None;
        if std::mem::take(&mut self.damaged) || nals.is_empty() {
            return None;
        }
        let keyframe = nals
            .iter()
            .any(|n| nal_type(n) == NAL_IDR || is_intra_picture(n));
        Some(AccessUnit {
            timestamp,
            nals,
            keyframe,
        })
    }

    fn payload(&mut self, payload: &[u8]) {
        let Some(&header) = payload.first() else {
            return;
        };
        match header & 0x1F {
            1..=23 => self.nals.push(payload.to_vec()),
            // STAP-A: 16-bit sizes, each followed by a NAL unit.
            24 => {
                let mut rest = &payload[1..];
                while rest.len() >= 2 {
                    let size = usize::from(u16::from_be_bytes([rest[0], rest[1]]));
                    rest = &rest[2..];
                    if size == 0 || size > rest.len() {
                        self.damaged = true;
                        return;
                    }
                    self.nals.push(rest[..size].to_vec());
                    rest = &rest[size..];
                }
            }
            // FU-A.
            28 => {
                let Some(&fu) = payload.get(1) else { return };
                let (start, end) = (fu & 0x80 != 0, fu & 0x40 != 0);
                if start {
                    let mut nal = vec![(header & 0xE0) | (fu & 0x1F)];
                    nal.extend_from_slice(&payload[2..]);
                    self.fragment = Some(nal);
                } else if let Some(nal) = self.fragment.as_mut() {
                    nal.extend_from_slice(&payload[2..]);
                } else {
                    self.damaged = true;
                    return;
                }
                if end && let Some(nal) = self.fragment.take() {
                    self.nals.push(nal);
                }
            }
            _ => {}
        }
    }
}

/// The fixed and variable parts of an RTP packet that matter here.
struct Rtp<'a> {
    marker: bool,
    seq: u16,
    timestamp: u32,
    payload: &'a [u8],
}

impl<'a> Rtp<'a> {
    fn parse(packet: &'a [u8]) -> Option<Self> {
        if packet.len() < 12 || packet[0] >> 6 != 2 {
            return None;
        }
        let csrc = usize::from(packet[0] & 0x0F);
        let mut start = 12 + 4 * csrc;
        if packet[0] & 0x10 != 0 {
            let words = packet.get(start + 2..start + 4)?;
            start += 4 + 4 * usize::from(u16::from_be_bytes([words[0], words[1]]));
        }
        let mut end = packet.len();
        if packet[0] & 0x20 != 0 {
            end = end.checked_sub(usize::from(*packet.last()?))?;
        }
        Some(Self {
            marker: packet[1] & 0x80 != 0,
            seq: u16::from_be_bytes([packet[2], packet[3]]),
            timestamp: u32::from_be_bytes([packet[4], packet[5], packet[6], packet[7]]),
            payload: packet.get(start..end)?,
        })
    }
}

/// A non-IDR slice (type 1) that is intra coded (I or SI): some cameras
/// refresh with these instead of IDR pictures, and a viewer can begin on one.
#[must_use]
pub fn is_intra_picture(nal: &[u8]) -> bool {
    if nal_type(nal) != 1 || nal.len() < 3 {
        return false;
    }
    let mut r = Bits::new(&nal[1..]);
    let Some(_first_mb) = r.ue() else {
        return false;
    };
    // slice_type 2, 4, 7, 9: I or SI (the second range says every slice is).
    r.ue().is_some_and(|t| matches!(t % 5, 2 | 4))
}

// ---- the sequence parameter set ------------------------------------------

struct Bits {
    bytes: Vec<u8>,
    pos: usize,
}

impl Bits {
    /// The RBSP of a NAL unit: emulation prevention bytes removed.
    fn new(nal: &[u8]) -> Self {
        let mut bytes = Vec::with_capacity(nal.len());
        let mut zeros = 0;
        for &b in nal {
            if zeros >= 2 && b == 3 {
                zeros = 0;
                continue;
            }
            zeros = if b == 0 { zeros + 1 } else { 0 };
            bytes.push(b);
        }
        Self { bytes, pos: 0 }
    }

    fn bit(&mut self) -> Option<u32> {
        let byte = *self.bytes.get(self.pos / 8)?;
        let bit = (byte >> (7 - self.pos % 8)) & 1;
        self.pos += 1;
        Some(u32::from(bit))
    }

    fn bits(&mut self, n: u32) -> Option<u32> {
        let mut v = 0;
        for _ in 0..n {
            v = (v << 1) | self.bit()?;
        }
        Some(v)
    }

    fn ue(&mut self) -> Option<u32> {
        let mut zeros = 0;
        while self.bit()? == 0 {
            zeros += 1;
            if zeros > 31 {
                return None;
            }
        }
        u32::try_from((1u64 << zeros) - 1 + u64::from(self.bits(zeros)?)).ok()
    }

    fn se(&mut self) -> Option<i32> {
        let k = self.ue()?;
        let magnitude = i32::try_from(k.div_ceil(2)).ok()?;
        Some(if k % 2 == 1 { magnitude } else { -magnitude })
    }

    fn skip_scaling_list(&mut self, size: usize) -> Option<()> {
        let (mut last, mut next) = (8i32, 8i32);
        for _ in 0..size {
            if next != 0 {
                next = (last + self.se()? + 256).rem_euclid(256);
            }
            if next != 0 {
                last = next;
            }
        }
        Some(())
    }
}

/// What a sequence parameter set says of the stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SpsInfo {
    pub profile_idc: u8,
    pub constraint_flags: u8,
    pub level_idc: u8,
    pub width: u32,
    pub height: u32,
    /// 2 means output order is decoding order: no B-frames.
    pub poc_type: u32,
    pub ref_frames: u32,
    pub frame_mbs_only: bool,
}

/// `(width, height)` of the pictures a sequence parameter set describes.
#[must_use]
pub fn picture_size(sps: &[u8]) -> Option<(u32, u32)> {
    sps_info(sps).map(|i| (i.width, i.height))
}

#[must_use]
pub fn sps_info(sps: &[u8]) -> Option<SpsInfo> {
    if nal_type(sps) != NAL_SPS || sps.len() < 4 {
        return None;
    }
    let profile = sps[1];
    let mut r = Bits::new(&sps[4..]);
    r.ue()?; // seq_parameter_set_id
    let mut chroma = 1;
    if matches!(
        profile,
        100 | 110 | 122 | 244 | 44 | 83 | 86 | 118 | 128 | 138 | 139 | 134 | 135
    ) {
        chroma = r.ue()?;
        if chroma == 3 {
            r.bit()?;
        }
        r.ue()?;
        r.ue()?;
        r.bit()?;
        if r.bit()? == 1 {
            for i in 0..(if chroma == 3 { 12 } else { 8 }) {
                if r.bit()? == 1 {
                    r.skip_scaling_list(if i < 6 { 16 } else { 64 })?;
                }
            }
        }
    }
    r.ue()?; // log2_max_frame_num_minus4
    let poc_type = r.ue()?;
    match poc_type {
        0 => {
            r.ue()?;
        }
        1 => {
            r.bit()?;
            r.se()?;
            r.se()?;
            for _ in 0..r.ue()? {
                r.se()?;
            }
        }
        _ => {}
    }
    let ref_frames = r.ue()?;
    r.bit()?;
    let width_mbs = r.ue()? + 1;
    let height_units = r.ue()? + 1;
    let frame_mbs_only = r.bit()?;
    if frame_mbs_only == 0 {
        r.bit()?;
    }
    r.bit()?; // direct_8x8_inference
    let (mut width, mut height) = (width_mbs * 16, (2 - frame_mbs_only) * height_units * 16);
    if r.bit()? == 1 {
        let (left, right, top, bottom) = (r.ue()?, r.ue()?, r.ue()?, r.ue()?);
        let unit_x = if chroma == 1 || chroma == 2 { 2 } else { 1 };
        let unit_y = if chroma == 1 { 2 } else { 1 } * (2 - frame_mbs_only);
        width = width.checked_sub(unit_x * (left + right))?;
        height = height.checked_sub(unit_y * (top + bottom))?;
    }
    Some(SpsInfo {
        profile_idc: profile,
        constraint_flags: sps[2],
        level_idc: sps[3],
        width,
        height,
        poc_type,
        ref_frames,
        frame_mbs_only: frame_mbs_only == 1,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn packet(seq: u16, ts: u32, marker: bool, payload: &[u8]) -> Vec<u8> {
        let mut p = vec![0x80, 0x60 | if marker { 0x80 } else { 0 }];
        p.extend_from_slice(&seq.to_be_bytes());
        p.extend_from_slice(&ts.to_be_bytes());
        p.extend_from_slice(&[0, 0, 0, 1]);
        p.extend_from_slice(payload);
        p
    }

    #[test]
    fn single_nals_stap_a_and_fu_a_make_one_frame() {
        let mut d = Depacketizer::new();
        let sps = [0x67, 1, 2, 3];
        let pps = [0x68, 4, 5];
        let mut stap = vec![24];
        for nal in [&sps[..], &pps[..]] {
            stap.extend_from_slice(&u16::try_from(nal.len()).unwrap().to_be_bytes());
            stap.extend_from_slice(nal);
        }
        assert!(d.push(&packet(10, 1000, false, &stap)).is_empty());
        // An IDR slice in three fragments (NRI 3, type 5).
        let idr = [0x65, 9, 8, 7, 6, 5, 4];
        let indicator = 0x60 | 0x1C;
        assert!(
            d.push(&packet(11, 1000, false, &[indicator, 0x80 | 5, 9, 8]))
                .is_empty()
        );
        assert!(
            d.push(&packet(12, 1000, false, &[indicator, 5, 7, 6]))
                .is_empty()
        );
        let done = d.push(&packet(13, 1000, true, &[indicator, 0x40 | 5, 5, 4]));
        assert_eq!(done.len(), 1);
        let unit = &done[0];
        assert_eq!(unit.timestamp, 1000);
        assert!(unit.keyframe);
        assert_eq!(unit.nals, vec![sps.to_vec(), pps.to_vec(), idr.to_vec()]);
    }

    #[test]
    fn a_new_timestamp_closes_the_previous_frame_and_a_gap_drops_it() {
        let mut d = Depacketizer::new();
        assert!(d.push(&packet(1, 100, false, &[0x41, 1])).is_empty());
        let done = d.push(&packet(2, 200, true, &[0x41, 2]));
        assert_eq!(done.len(), 2);
        assert!(!done[0].keyframe);
        // Sequence 4 missing: the frame at 300 is dropped, the next is fine.
        assert!(d.push(&packet(3, 300, false, &[0x41, 3])).is_empty());
        assert!(d.push(&packet(5, 300, true, &[0x41, 4])).is_empty());
        assert_eq!(d.push(&packet(6, 400, true, &[0x41, 5])).len(), 1);
    }

    #[test]
    fn the_size_of_a_1080p_high_profile_stream_is_read() {
        let sps = [
            0x67, 0x64, 0x00, 0x28, 0xAC, 0xD9, 0x40, 0x78, 0x02, 0x27, 0xE5, 0xC0, 0x44, 0x00,
            0x00, 0x03, 0x00, 0x04, 0x00, 0x00, 0x03, 0x00, 0xF0, 0x3C, 0x60, 0xC6, 0x58,
        ];
        assert_eq!(picture_size(&sps), Some((1920, 1080)));
        assert_eq!(picture_size(&[0x68, 1, 2, 3, 4]), None);
    }

    #[test]
    fn an_intra_slice_that_is_not_an_idr_is_found() {
        // first_mb 0 ('1'), slice_type 7 = I ('0001000'), then padding.
        let intra = [0x41, 0b1000_1000, 0x80];
        assert!(is_intra_picture(&intra));
        // slice_type 5 = P ('00110'): 1 + 00110 -> 0b1001_1000.
        let predicted = [0x41, 0b1001_1000, 0x80];
        assert!(!is_intra_picture(&predicted));
        assert!(
            !is_intra_picture(&[0x65, 0x88, 0x80]),
            "an IDR has its own type"
        );
        let mut d = Depacketizer::new();
        let mut p = vec![0x80, 0xE0, 0, 1, 0, 0, 0, 9, 0, 0, 0, 1];
        p.extend_from_slice(&intra);
        let frames = d.push(&p);
        assert!(frames[0].keyframe);
    }

    #[test]
    fn the_profile_level_and_ordering_of_a_stream_are_read() {
        let sps = [
            0x67, 0x64, 0x00, 0x28, 0xAC, 0xD9, 0x40, 0x78, 0x02, 0x27, 0xE5, 0xC0, 0x44, 0x00,
            0x00, 0x03, 0x00, 0x04, 0x00, 0x00, 0x03, 0x00, 0xF0, 0x3C, 0x60, 0xC6, 0x58,
        ];
        let info = sps_info(&sps).unwrap();
        assert_eq!((info.profile_idc, info.level_idc), (100, 40));
        assert!(info.frame_mbs_only);
        assert_eq!(info.poc_type, 0);
    }
}
