//! RTP packetization of H.264 (RFC 6184, mode 1): what the Home app takes.
//! SPS and PPS travel together in one STAP-A, big NAL units are cut in
//! FU-A fragments, the marker bit closes each frame.

use moli_net::h264::{NAL_PPS, NAL_SPS, nal_type};

/// Builds the RTP packets of one stream.
#[derive(Debug)]
pub struct Packetizer {
    payload_type: u8,
    ssrc: u32,
    seq: u16,
    /// Most bytes of payload in a packet.
    max_payload: usize,
}

impl Packetizer {
    /// `mtu`: the largest packet the controller asked for; room is left for
    /// the SRTP tag and the UDP and IP headers.
    #[must_use]
    pub fn new(payload_type: u8, ssrc: u32, first_seq: u16, mtu: usize) -> Self {
        Self {
            payload_type,
            ssrc,
            seq: first_seq,
            max_payload: mtu.saturating_sub(12 + 10 + 28).clamp(200, 1400),
        }
    }

    fn header(&mut self, timestamp: u32, marker: bool) -> Vec<u8> {
        let mut p = Vec::with_capacity(self.max_payload + 12);
        p.push(0x80);
        p.push(self.payload_type | if marker { 0x80 } else { 0 });
        p.extend_from_slice(&self.seq.to_be_bytes());
        p.extend_from_slice(&timestamp.to_be_bytes());
        p.extend_from_slice(&self.ssrc.to_be_bytes());
        self.seq = self.seq.wrapping_add(1);
        p
    }

    /// The packets of one frame (the last one carries the marker).
    pub fn frame(&mut self, timestamp: u32, nals: &[Vec<u8>]) -> Vec<Vec<u8>> {
        // Access unit delimiters and filler carry nothing a viewer needs.
        let nals: Vec<&Vec<u8>> = nals
            .iter()
            .filter(|n| !n.is_empty() && !matches!(nal_type(n), 9 | 12))
            .collect();
        let mut payloads: Vec<Vec<u8>> = Vec::new();
        let mut i = 0;
        while i < nals.len() {
            let nal = nals[i];
            // Parameter sets of one picture go together.
            if matches!(nal_type(nal), NAL_SPS | NAL_PPS) {
                let mut j = i;
                let mut size = 1;
                while j < nals.len()
                    && matches!(nal_type(nals[j]), NAL_SPS | NAL_PPS)
                    && size + 2 + nals[j].len() <= self.max_payload
                {
                    size += 2 + nals[j].len();
                    j += 1;
                }
                if j - i > 1 {
                    let nri = nals[i..j].iter().map(|n| n[0] & 0x60).max().unwrap_or(0);
                    let mut p = vec![nri | 0x18];
                    for n in &nals[i..j] {
                        p.extend_from_slice(&(n.len() as u16).to_be_bytes());
                        p.extend_from_slice(n);
                    }
                    payloads.push(p);
                    i = j;
                    continue;
                }
            }
            if nal.len() <= self.max_payload {
                payloads.push(nal.clone());
            } else {
                let indicator = (nal[0] & 0xE0) | 0x1C;
                let kind = nal[0] & 0x1F;
                let body = &nal[1..];
                let chunks: Vec<&[u8]> = body.chunks(self.max_payload - 2).collect();
                for (n, chunk) in chunks.iter().enumerate() {
                    let flags = (if n == 0 { 0x80 } else { 0 })
                        | (if n == chunks.len() - 1 { 0x40 } else { 0 });
                    let mut p = vec![indicator, flags | kind];
                    p.extend_from_slice(chunk);
                    payloads.push(p);
                }
            }
            i += 1;
        }
        let last = payloads.len().saturating_sub(1);
        payloads
            .into_iter()
            .enumerate()
            .map(|(n, payload)| {
                let mut packet = self.header(timestamp, n == last);
                packet.extend_from_slice(&payload);
                packet
            })
            .collect()
    }
}

/// An RTCP sender report (no report blocks).
#[must_use]
pub fn sender_report(
    ssrc: u32,
    ntp: u64,
    rtp_timestamp: u32,
    packets: u32,
    octets: u32,
) -> Vec<u8> {
    let mut p = vec![0x80, 200, 0, 6];
    p.extend_from_slice(&ssrc.to_be_bytes());
    p.extend_from_slice(&ntp.to_be_bytes());
    p.extend_from_slice(&rtp_timestamp.to_be_bytes());
    p.extend_from_slice(&packets.to_be_bytes());
    p.extend_from_slice(&octets.to_be_bytes());
    p
}

/// The wall clock as NTP time (seconds since 1900 in the high half).
#[must_use]
pub fn ntp_now() -> u64 {
    let since_unix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let seconds = since_unix.as_secs() + 2_208_988_800;
    let fraction = (u64::from(since_unix.subsec_nanos()) << 32) / 1_000_000_000;
    (seconds << 32) | fraction
}

#[cfg(test)]
mod tests {
    use super::*;
    use moli_net::h264::Depacketizer;

    #[test]
    fn what_is_packetized_comes_back_the_same_through_the_depacketizer() {
        let sps = vec![0x67, 1, 2, 3];
        let pps = vec![0x68, 4];
        let big: Vec<u8> = std::iter::once(0x65)
            .chain((0..5000).map(|i| (i % 251) as u8))
            .collect();
        let small = vec![0x41, 9, 9];
        for nals in [
            vec![sps.clone(), pps.clone(), big.clone()],
            vec![small.clone()],
            vec![small.clone(), big.clone()],
        ] {
            let mut packetizer = Packetizer::new(99, 0xAABB, 65530, 1378);
            let packets = packetizer.frame(4000, &nals);
            assert!(
                packets.iter().all(|p| p.len() + 10 + 28 <= 1378),
                "within the MTU"
            );
            assert_eq!(packets.last().unwrap()[1], 0x63 | 0x80);
            assert!(packets[..packets.len() - 1].iter().all(|p| p[1] == 99));
            let mut d = Depacketizer::new();
            let mut got = Vec::new();
            for p in &packets {
                got.extend(d.push(p));
            }
            assert_eq!(got.len(), 1);
            assert_eq!(got[0].nals, nals);
            assert_eq!(got[0].timestamp, 4000);
        }
    }

    #[test]
    fn parameter_sets_share_a_stap_a_and_sequence_numbers_wrap() {
        let mut packetizer = Packetizer::new(99, 1, 65535, 1378);
        let packets = packetizer.frame(0, &[vec![0x67, 1], vec![0x68, 2], vec![0x65, 3]]);
        assert_eq!(packets.len(), 2);
        assert_eq!(packets[0][12] & 0x1F, 24);
        assert_eq!(u16::from_be_bytes([packets[0][2], packets[0][3]]), 65535);
        assert_eq!(u16::from_be_bytes([packets[1][2], packets[1][3]]), 0);
    }

    #[test]
    fn a_sender_report_is_28_bytes_with_its_fields_in_place() {
        let sr = sender_report(7, 0x0102_0304_0506_0708, 9, 10, 11);
        assert_eq!(sr.len(), 28);
        assert_eq!(&sr[..4], &[0x80, 200, 0, 6]);
        assert_eq!(&sr[8..16], &0x0102_0304_0506_0708u64.to_be_bytes());
        assert_eq!(&sr[24..], &11u32.to_be_bytes());
    }
}
