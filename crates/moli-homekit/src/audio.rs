//! The camera's sound for the Home app: AAC decoded by the driver arrives as
//! 16-bit samples at the camera's rate; here it is brought to 16 kHz and cut
//! in 20 ms Opus packets (pure Rust, `opus-rs`), HomeKit's audio codec that
//! needs no C library.

// Sample indices and rates are far below f64's exact integers.
#![allow(clippy::cast_precision_loss, clippy::cast_possible_wrap)]

use moli_runtime::media::AudioChunk;
use opus_rs::{Application, OpusEncoder};

/// The one rate Moli offers HomeKit.
pub const RATE: u32 = 16_000;
/// Samples in a 20 ms packet at `RATE`.
pub const FRAME: usize = 320;
/// Bit rate of the Opus stream when the phone does not say (speech, mono).
const BITRATE: i32 = 24_000;
/// Taps on each side of the interpolation point.
const HALF: usize = 24;
/// Positions between two input samples the kernel is tabulated at.
const PHASES: usize = 64;

/// A streaming windowed-sinc resampler: the same output whatever the size
/// of the chunks it is fed, no click at their edges.
pub struct Resampler {
    step: f64,
    kernel: Vec<[f32; 2 * HALF]>,
    buf: Vec<f32>,
    /// Where the next output sample sits in `buf`.
    pos: f64,
}

impl std::fmt::Debug for Resampler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Resampler")
            .field("step", &self.step)
            .finish_non_exhaustive()
    }
}

impl Resampler {
    #[must_use]
    pub fn new(from: u32, to: u32) -> Self {
        let step = f64::from(from) / f64::from(to);
        // Cut off under the lower of the two Nyquist frequencies.
        let cutoff = 0.45 * (1.0 / step).min(1.0);
        let kernel = (0..PHASES)
            .map(|p| {
                let frac = p as f64 / PHASES as f64;
                let mut taps = [0f32; 2 * HALF];
                for (i, tap) in taps.iter_mut().enumerate() {
                    // The input sample `i - HALF + 1` away from the floor.
                    let x = frac - (i as f64 - (HALF as f64 - 1.0));
                    let sinc = if x == 0.0 {
                        2.0 * cutoff
                    } else {
                        (2.0 * std::f64::consts::PI * cutoff * x).sin() / (std::f64::consts::PI * x)
                    };
                    let window = 0.54 + 0.46 * (std::f64::consts::PI * x / HALF as f64).cos();
                    *tap = (sinc * window) as f32;
                }
                taps
            })
            .collect();
        Self {
            step,
            kernel,
            buf: vec![0.0; 2 * HALF],
            pos: HALF as f64,
        }
    }

    pub fn push(&mut self, samples: &[i16]) -> Vec<i16> {
        self.buf.extend(samples.iter().map(|s| f32::from(*s)));
        let mut out = Vec::with_capacity((samples.len() as f64 / self.step) as usize + 2);
        loop {
            let floor = self.pos.floor() as usize;
            if floor + HALF >= self.buf.len() {
                break;
            }
            let phase =
                (((self.pos - floor as f64) * PHASES as f64).round() as usize).min(PHASES - 1);
            let taps = &self.kernel[phase];
            let window = &self.buf[floor + 1 - HALF..floor + 1 + HALF];
            let sum: f32 = window.iter().zip(taps).map(|(s, h)| s * h).sum();
            out.push(sum.round().clamp(-32768.0, 32767.0) as i16);
            self.pos += self.step;
        }
        // Forget what no output will look at again.
        let keep_from = (self.pos.floor() as usize).saturating_sub(HALF);
        if keep_from > 0 {
            self.buf.drain(..keep_from);
            self.pos -= keep_from as f64;
        }
        out
    }
}

/// Camera samples in, Opus packets out.
pub struct OpusPipe {
    from: u32,
    resampler: Option<Resampler>,
    encoder: OpusEncoder,
    pcm: Vec<i16>,
}

impl std::fmt::Debug for OpusPipe {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpusPipe")
            .field("from", &self.from)
            .finish_non_exhaustive()
    }
}

impl OpusPipe {
    /// `kbps`: the most the phone accepted (0: not said).
    pub fn new(from: u32, kbps: u16) -> Result<Self, String> {
        let mut encoder =
            OpusEncoder::new(RATE as i32, 1, Application::Voip).map_err(|e| format!("{e:?}"))?;
        // Constant: left variable the encoder ignores its target (three times
        // the bits on noisy speech), and the phone drops what it did not ask for.
        encoder.bitrate_bps = if kbps == 0 {
            BITRATE
        } else {
            i32::from(kbps).clamp(12, 32) * 1000
        };
        encoder.use_cbr = true;
        Ok(Self {
            from,
            resampler: (from != RATE).then(|| Resampler::new(from, RATE)),
            encoder,
            pcm: Vec::new(),
        })
    }

    /// The packets (20 ms each) that `chunk` completes.
    pub fn push(&mut self, chunk: &AudioChunk) -> Vec<Vec<u8>> {
        if chunk.rate != self.from {
            return Vec::new();
        }
        match self.resampler.as_mut() {
            Some(r) => self.pcm.extend(r.push(&chunk.samples)),
            None => self.pcm.extend_from_slice(&chunk.samples),
        }
        let mut packets = Vec::new();
        while self.pcm.len() >= FRAME {
            let frame: Vec<i16> = self.pcm.drain(..FRAME).collect();
            let mut out = vec![0u8; 1276];
            if let Ok(n) = self.encoder.encode_i16(&frame, FRAME, &mut out) {
                out.truncate(n);
                packets.push(out);
            }
        }
        packets
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use opus_rs::OpusDecoder;

    fn sine(rate: u32, hz: f64, seconds: f64) -> Vec<i16> {
        (0..(f64::from(rate) * seconds) as usize)
            .map(|n| {
                (10_000.0 * (2.0 * std::f64::consts::PI * hz * n as f64 / f64::from(rate)).sin())
                    as i16
            })
            .collect()
    }

    fn rms(samples: &[i16]) -> f64 {
        (samples.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>() / samples.len() as f64).sqrt()
    }

    #[test]
    fn resampling_does_not_depend_on_the_chunking_and_keeps_the_level() {
        let input = sine(48_000, 1000.0, 1.0);
        let whole = Resampler::new(48_000, 16_000).push(&input);
        let mut r = Resampler::new(48_000, 16_000);
        let chunked: Vec<i16> = input.chunks(333).flat_map(|c| r.push(c)).collect();
        assert!((whole.len() as i64 - 16_000).abs() <= 2, "{}", whole.len());
        assert_eq!(whole.len(), chunked.len());
        assert!(whole.iter().zip(&chunked).all(|(a, b)| a == b));
        // A 1 kHz tone of amplitude 10000 has an RMS of 7071.
        let level = rms(&whole[200..]);
        assert!((level - 7071.0).abs() < 200.0, "{level}");
        // Above the new Nyquist frequency: filtered away, not folded back.
        let high = Resampler::new(48_000, 16_000).push(&sine(48_000, 12_000.0, 0.5));
        assert!(rms(&high[200..]) < 300.0, "{}", rms(&high[200..]));
    }

    #[test]
    fn upsampling_works_too() {
        let up = Resampler::new(8_000, 16_000).push(&sine(8_000, 500.0, 0.5));
        assert!((up.len() as i64 - 8_000).abs() <= 2);
        assert!((rms(&up[100..]) - 7071.0).abs() < 300.0);
    }

    #[test]
    fn a_tone_goes_through_opus_and_comes_back() {
        let mut pipe = OpusPipe::new(48_000, 24).unwrap();
        let mut packets = Vec::new();
        for chunk in sine(48_000, 1000.0, 1.0).chunks(480) {
            packets.extend(pipe.push(&AudioChunk {
                rate: 48_000,
                samples: chunk.to_vec(),
            }));
        }
        assert!((packets.len() as i64 - 50).abs() <= 2, "{}", packets.len());
        assert!(packets.iter().all(|p| !p.is_empty() && p.len() < 400));
        let mut decoder = OpusDecoder::new(RATE as i32, 1).unwrap();
        let mut samples_out: Vec<f32> = Vec::new();
        for p in &packets {
            let mut pcm = vec![0f32; FRAME];
            let n = decoder.decode(p, FRAME, &mut pcm).unwrap();
            samples_out.extend_from_slice(&pcm[..n]);
        }
        // The tone survived: about the energy that went in (the decoder
        // gives floats, whatever their scale).
        let tail = &samples_out[FRAME * 10..];
        let level =
            (tail.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>() / tail.len() as f64).sqrt();
        assert!(
            (0.1..0.4).contains(&level) || (3_000.0..12_000.0).contains(&level),
            "{level}"
        );
        // Another rate than the stream's is ignored.
        assert!(
            pipe.push(&AudioChunk {
                rate: 8_000,
                samples: vec![0; 400]
            })
            .is_empty()
        );
    }

    #[test]
    fn the_stream_keeps_to_the_bit_rate_asked_even_on_noise() {
        let mut seed = 12345u32;
        let noise: Vec<i16> = (0..16_000 * 3)
            .map(|_| {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                ((seed >> 16) as i16) / 4
            })
            .collect();
        for (kbps, bytes) in [(24u16, 60usize), (16, 40)] {
            let mut pipe = OpusPipe::new(16_000, kbps).unwrap();
            let packets = pipe.push(&AudioChunk {
                rate: 16_000,
                samples: noise.clone(),
            });
            assert!(packets.len() >= 140);
            assert!(
                packets.iter().all(|p| p.len() <= bytes + 2),
                "{kbps}: {:?}",
                packets.iter().map(Vec::len).max()
            );
        }
    }
}
