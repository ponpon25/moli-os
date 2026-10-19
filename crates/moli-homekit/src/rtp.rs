//! RTP packetization of H.264 (RFC 6184, mode 1): what the Home app takes.
//! SPS and PPS travel together in one STAP-A, big NAL units are cut in
//! FU-A fragments, the marker bit closes each frame.

use std::time::{Duration, Instant};

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

/// Maps the camera's RTP clock to the one the phone sees, in real time.
///
/// The phone plays by RTP timestamps against its own arrival clock. A source
/// that stalls and then bursts (a console serving a camera over RTSP) would
/// make every frame of the burst look late, and the phone gives up. So frames
/// that arrive early are held back until their time (never long), and when
/// the stream falls behind the wall clock it is *re-anchored*: the timeline
/// goes on from the last timestamp sent plus the real time elapsed, always
/// moving forward.
#[derive(Debug)]
pub struct Retimer {
    anchor_ts: u32,
    anchor_at: Instant,
    out_base: u32,
    last_out: u32,
    last_at: Instant,
    started: bool,
    /// Times the stream was put back in step with the clock.
    pub reanchors: u32,
}

/// A frame earlier than this is held until its time; further ahead, the
/// source's clock is not trusted and the stream is re-anchored.
const MAX_HOLD: Duration = Duration::from_millis(300);
/// A frame later than this behind its time puts the stream back in step.
const MAX_LATE: Duration = Duration::from_millis(250);
const CLOCK: f64 = 90_000.0;

impl Retimer {
    #[must_use]
    pub fn new(first_out: u32, now: Instant) -> Self {
        Self {
            anchor_ts: 0,
            anchor_at: now,
            out_base: first_out,
            last_out: first_out,
            last_at: now,
            started: false,
            reanchors: 0,
        }
    }

    /// For a frame of source timestamp `ts` arriving at `now`: how long to
    /// hold it, and the timestamp to send it with.
    pub fn frame(&mut self, ts: u32, now: Instant) -> (Duration, u32) {
        if !self.started {
            self.started = true;
            self.anchor(ts, now, now);
            return (Duration::ZERO, self.out_base);
        }
        let ticks = ts.wrapping_sub(self.anchor_ts).cast_signed();
        let due = if ticks >= 0 {
            Some(self.anchor_at + Duration::from_secs_f64(f64::from(ticks) / CLOCK))
        } else {
            None
        };
        let hold = match due {
            Some(due) if due > now => due - now,
            _ => Duration::ZERO,
        };
        let late = due.is_none_or(|due| now > due + MAX_LATE);
        if late || hold > MAX_HOLD {
            self.reanchors += 1;
            let elapsed = now.saturating_duration_since(self.last_at).as_secs_f64();
            let step = ((elapsed * CLOCK) as u32).max(1);
            let base = self.last_out.wrapping_add(step);
            self.anchor_ts = ts;
            self.anchor_at = now;
            self.out_base = base;
            self.last_out = base;
            self.last_at = now;
            return (Duration::ZERO, base);
        }
        let out = self.out_base.wrapping_add(ticks.cast_unsigned());
        // Never backwards, never equal.
        let out = if out.wrapping_sub(self.last_out).cast_signed() > 0 {
            out
        } else {
            self.last_out.wrapping_add(1)
        };
        self.last_out = out;
        self.last_at = now + hold;
        (hold, out)
    }

    fn anchor(&mut self, ts: u32, at: Instant, now: Instant) {
        self.anchor_ts = ts;
        self.anchor_at = at;
        self.last_at = now;
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

    #[test]
    fn a_steady_source_keeps_its_own_clock() {
        let t0 = Instant::now();
        let mut r = Retimer::new(1000, t0);
        assert_eq!(r.frame(500_000, t0), (Duration::ZERO, 1000));
        // 30 fps, arriving on time: the source's step, no wait.
        let (wait, out) = r.frame(503_000, t0 + Duration::from_millis(34));
        assert_eq!(out, 4000);
        assert!(wait.is_zero());
        assert_eq!(r.reanchors, 0);
    }

    #[test]
    fn a_burst_is_spread_over_its_timestamps() {
        let t0 = Instant::now();
        let mut r = Retimer::new(0, t0);
        r.frame(0, t0);
        // Ten frames of 33 ms arriving at once: each waits for its time.
        let mut last = Duration::ZERO;
        for i in 1..=5u32 {
            let (wait, out) = r.frame(i * 3000, t0);
            assert_eq!(out, i * 3000);
            assert!(wait > last);
            last = wait;
        }
        assert!(last < Duration::from_millis(200));
    }

    #[test]
    fn after_a_stall_the_stream_goes_on_from_now_and_never_backwards() {
        let t0 = Instant::now();
        let mut r = Retimer::new(0, t0);
        r.frame(0, t0);
        r.frame(3000, t0 + Duration::from_millis(33));
        // Two seconds without a frame, then a burst of old timestamps.
        let later = t0 + Duration::from_millis(2033);
        let (wait, out) = r.frame(6000, later);
        assert!(wait.is_zero());
        assert_eq!(r.reanchors, 1);
        // About two seconds of the 90 kHz clock went by.
        assert!((170_000..190_000).contains(&out), "{out}");
        let (_, next) = r.frame(9000, later);
        assert_eq!(next, out + 3000);
        // A source clock that jumps back is re-anchored, still forward.
        let (_, back) = r.frame(100, later);
        assert!(back.wrapping_sub(next).cast_signed() > 0);
    }

    #[test]
    fn timestamps_wrap_around_without_a_hitch() {
        let t0 = Instant::now();
        let mut r = Retimer::new(u32::MAX - 1000, t0);
        r.frame(u32::MAX - 100, t0);
        let (_, out) = r.frame(2900, t0 + Duration::from_millis(33));
        // 2900 is 3001 ticks after u32::MAX - 100.
        assert_eq!(out, (u32::MAX - 1000).wrapping_add(3001));
    }
}
