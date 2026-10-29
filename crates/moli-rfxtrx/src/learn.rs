//! Listening to everything a few minutes: what a remote, a doorbell or a
//! sensor sends, so a human can tell whether (and how) the transceiver
//! hears it. Each sender is one line, however many times it was heard.

use serde::Serialize;

use crate::frame::{self, Frame};

/// Senders kept at most (the oldest go first).
const KEPT: usize = 40;

/// Where a sender stands for Moli.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Standing {
    /// A sensor Moli shows.
    Known,
    /// A sensor in `ignore`.
    Ignored,
    /// A sensor Moli would add (or would, with `auto_add`).
    New,
    /// Heard, but not a sensor Moli reads (a remote, a doorbell…).
    Unread,
}

/// One sender.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Heard {
    /// Packet type and subtype, `13-00`.
    pub packet: String,
    /// `lighting4`, `undecoded`, `sensor`… (the dashboard says it in words).
    pub kind: &'static str,
    /// A sensor's code (`52-01-1a2b`), else the bytes that set it apart.
    pub code: String,
    /// Brand and model, for a sensor.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// What it said last, for a sensor (« 21,5 °C · 48 % »).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reading: Option<String>,
    pub standing: Standing,
    pub count: u32,
    /// Last heard, ms since the epoch.
    pub last: u64,
    /// Signal, 0 to 15.
    pub rssi: u8,
}

#[derive(Debug, Default)]
pub struct Learn {
    pub heard: Vec<Heard>,
}

impl Learn {
    /// One frame heard while listening. `standing` says where a sensor's
    /// code stands. Returns whether the list changed.
    pub fn note(
        &mut self,
        raw: &[u8],
        decoded: Option<&Frame>,
        now: u64,
        standing: impl Fn(&str) -> Standing,
    ) -> bool {
        let (Some(&packet), Some(&subtype)) = (raw.get(1), raw.get(2)) else {
            return false;
        };
        // The transceiver's own answers are not senders.
        if packet <= 0x02 {
            return false;
        }
        let rssi = if packet == 0x03 {
            0
        } else {
            raw.last().map_or(0, |b| b >> 4)
        };
        let (code, model, reading, standing) = match decoded {
            Some(Frame::Reading(r)) => {
                let (brand, model) = frame::model(r.packet, r.subtype);
                let reading = [
                    r.temperature.map(|t| format!("{t} °C")),
                    r.humidity.map(|h| format!("{h} %")),
                    r.pressure.map(|p| format!("{p} hPa")),
                ]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join(" · ");
                let code = r.code();
                let standing = standing(&code);
                (
                    code,
                    Some(format!("{brand} {model}").trim().to_owned()),
                    Some(reading),
                    standing,
                )
            }
            _ => (frame::signature(raw), None, None, Standing::Unread),
        };
        let packet_name = format!("{packet:02x}-{subtype:02x}");
        if let Some(h) = self
            .heard
            .iter_mut()
            .find(|h| h.packet == packet_name && h.code == code)
        {
            h.count = h.count.saturating_add(1);
            h.last = now;
            h.rssi = rssi;
            h.reading = reading.or(h.reading.take());
            h.standing = standing;
            return true;
        }
        if self.heard.len() >= KEPT
            && let Some(oldest) = self
                .heard
                .iter()
                .enumerate()
                .min_by_key(|(_, h)| h.last)
                .map(|(i, _)| i)
        {
            self.heard.remove(oldest);
        }
        self.heard.push(Heard {
            packet: packet_name,
            kind: frame::kind_of(packet),
            code,
            model,
            reading,
            standing,
            count: 1,
            last: now,
            rssi,
        });
        true
    }

    /// The list, newest first, as the `heard` point holds it.
    pub fn json(&self) -> String {
        let mut list: Vec<&Heard> = self.heard.iter().collect();
        list.sort_by_key(|h| std::cmp::Reverse(h.last));
        serde_json::to_string(&list).unwrap_or_else(|_| "[]".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_line_per_sender() {
        let mut l = Learn::default();
        let press = [0x09, 0x13, 0x00, 0x07, 0xAB, 0xCD, 0xEF, 0x01, 0x5E, 0x60];
        let sensor = [
            0x0A, 0x52, 0x01, 0x00, 0x1A, 0x2B, 0x00, 0xE2, 0x32, 0x01, 0x59,
        ];
        let known = |c: &str| {
            if c == "52-01-1a2b" {
                Standing::Known
            } else {
                Standing::New
            }
        };
        assert!(l.note(&press, frame::decode(&press).as_ref(), 1, known));
        let mut again = press;
        again[3] = 0x08;
        assert!(l.note(&again, frame::decode(&again).as_ref(), 2, known));
        assert!(l.note(&sensor, frame::decode(&sensor).as_ref(), 3, known));
        // The transceiver's own status is not a sender.
        let status = [0x04, 0x01, 0x00, 0x00, 0x02];
        assert!(!l.note(&status, None, 4, known));

        assert_eq!(l.heard.len(), 2);
        let remote = &l.heard[0];
        assert_eq!(
            (remote.kind, remote.count, remote.rssi),
            ("lighting4", 2, 6)
        );
        assert_eq!(remote.standing, Standing::Unread);
        let probe = &l.heard[1];
        assert_eq!(probe.code, "52-01-1a2b");
        assert_eq!(probe.standing, Standing::Known);
        assert_eq!(probe.reading.as_deref(), Some("22.6 °C · 50 %"));
        let json = l.json();
        assert!(json.starts_with("[{\"packet\":\"52-01\""), "{json}");
        assert!(json.contains("\"standing\":\"unread\""));
    }

    #[test]
    fn keeps_the_newest_forty() {
        let mut l = Learn::default();
        for i in 0..50u8 {
            let press = [0x09, 0x13, 0x00, 0x00, i, 0x00, 0x00, 0x01, 0x5E, 0x60];
            l.note(&press, None, u64::from(i), |_| Standing::New);
        }
        assert_eq!(l.heard.len(), KEPT);
        assert!(l.heard.iter().all(|h| h.last >= 10));
    }
}
