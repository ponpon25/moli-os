//! A camera's video for one client, kept going for as long as the client
//! stays: a recorder watches for days, a camera's stream does not last that
//! long (the HomeKit doorbell lets a viewer go after ten minutes, a camera
//! drops off the network now and then).
//!
//! - Before the stream can end on its own, a second one is opened beside it
//!   and taken over on its first key frame: no gap.
//! - A stream that ends anyway is opened again, waiting longer each time.
//! - The client sees one timeline: the timestamps of each new stream are
//!   moved on from the last ones sent.
//! - Nothing for [`GIVE_UP`]: the feed ends, and with it the connection
//!   (the recorder will call again).
//!
//! - Two streams of one shared source (a HomeKit camera's viewers) carry
//!   the same timestamps: the timeline simply goes on, and the frames
//!   already sent by the old one are not sent twice.
//! - Every [`STATS_EVERY`], what went out is logged (frames, largest gap,
//!   renewals, reopenings, frames skipped): the way to watch a recorder's
//!   stream over days.
//!
//! Each stream is read by its own task into a small queue, so a slow client
//! or a slow camera never blocks the other: when a queue is full, frames are
//! dropped up to the next key frame.

use std::time::Duration;

use moli_core::DeviceId;
use moli_runtime::Hub;
use moli_runtime::media::{LiveError, LiveInfo, LiveItem, LiveStream, LiveWant, VideoFrame};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::Instant;

/// A stream is renewed this often, before any source lets it go.
pub const RENEW: Duration = Duration::from_secs(8 * 60);
/// No frame for this long: the feed ends.
pub const GIVE_UP: Duration = Duration::from_secs(60);
const FIRST_RETRY: Duration = Duration::from_secs(1);
const MAX_RETRY: Duration = Duration::from_secs(15);
/// A renewal that failed is tried again after this.
const RENEW_RETRY: Duration = Duration::from_secs(30);
/// How often the feed tells how it goes.
pub const STATS_EVERY: Duration = Duration::from_secs(5 * 60);
/// Two timestamps closer than this (2 s at 90 kHz) are of the same clock.
const SAME_CLOCK: i32 = 2 * 90_000;
/// Frames waiting between a stream and the feed, and between the feed and
/// the client (a few seconds of video).
const QUEUE: usize = 128;

/// What is asked of the camera: its best picture, no sound (the sound is
/// not served yet).
const WANT: LiveWant = LiveWant {
    width: 1920,
    height: 1080,
    audio: false,
};

/// A stream being read by its task. Dropping it stops the task, which
/// closes the stream.
#[derive(Debug)]
pub struct Pump {
    info: LiveInfo,
    frames: mpsc::Receiver<VideoFrame>,
    task: JoinHandle<()>,
}

impl Drop for Pump {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Pump {
    #[must_use]
    pub fn info(&self) -> &LiveInfo {
        &self.info
    }

    fn start(device: DeviceId, mut stream: Box<dyn LiveStream>) -> Self {
        let info = stream.info().clone();
        let (tx, frames) = mpsc::channel(QUEUE);
        let task = tokio::spawn(async move {
            let mut skipping = false;
            loop {
                let frame = match stream.next().await {
                    Err(e) => {
                        tracing::info!(device = %device, error = %format!("{e:#}"), "rtsp: the camera's stream ended");
                        return;
                    }
                    Ok(LiveItem::Audio(_)) => continue,
                    Ok(LiveItem::Video(frame)) => frame,
                };
                if skipping && !frame.keyframe {
                    continue;
                }
                skipping = false;
                match tx.try_send(frame) {
                    Ok(()) => {}
                    Err(mpsc::error::TrySendError::Full(_)) => skipping = true,
                    Err(mpsc::error::TrySendError::Closed(_)) => return,
                }
            }
        });
        Self { info, frames, task }
    }
}

/// Opens a stream of `device` and starts reading it.
pub async fn open(hub: &Hub, device: &DeviceId) -> Result<Pump, LiveError> {
    let stream = hub.camera_live(device, WANT).await?;
    Ok(Pump::start(device.clone(), stream))
}

/// The client's timeline: each stream's timestamps, moved on to follow the
/// last ones sent.
#[derive(Debug)]
struct Clock {
    first: bool,
    out_base: u32,
    src_base: u32,
    last_out: u32,
    last_at: Instant,
    /// The source timestamp of the last frame stamped.
    last_src: Option<u32>,
}

impl Clock {
    fn new(start: u32, now: Instant) -> Self {
        Self {
            first: true,
            out_base: start,
            src_base: 0,
            last_out: start,
            last_at: now,
            last_src: None,
        }
    }

    /// Starts a new stream at `src`: it goes on from the last frame sent,
    /// plus the time gone since.
    fn rebase(&mut self, src: u32, now: Instant) {
        if self.first {
            self.first = false;
        } else {
            let gone = now.saturating_duration_since(self.last_at).as_secs_f64() * 90_000.0;
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // ≤ GIVE_UP
            let ticks = (gone as u32).max(1);
            self.out_base = self.last_out.wrapping_add(ticks);
        }
        self.src_base = src;
    }

    /// Whether a new stream starting at `src` is the same source's clock
    /// (another viewer of a shared stream): then the timeline needs no new
    /// base.
    fn continues(&self, src: u32) -> bool {
        self.last_src
            .is_some_and(|last| src.wrapping_sub(last).cast_signed().abs() < SAME_CLOCK)
    }

    fn stamp(&mut self, src: u32, now: Instant) -> u32 {
        let mut out = self.out_base.wrapping_add(src.wrapping_sub(self.src_base));
        // A camera whose clock jumped back: a new start, never backwards.
        if !self.first && out.wrapping_sub(self.last_out).cast_signed() < 0 {
            self.rebase(src, now);
            out = self.out_base;
        }
        self.last_out = out;
        self.last_at = now;
        self.last_src = Some(src);
        out
    }
}

/// What went out since the last report.
#[derive(Debug)]
struct Stats {
    since: Instant,
    frames: u32,
    keyframes: u32,
    largest_gap: Duration,
    renewals: u32,
    reopenings: u32,
    skipped: u32,
}

impl Stats {
    fn new(now: Instant) -> Self {
        Self {
            since: now,
            frames: 0,
            keyframes: 0,
            largest_gap: Duration::ZERO,
            renewals: 0,
            reopenings: 0,
            skipped: 0,
        }
    }

    fn report(&mut self, device: &DeviceId, now: Instant) {
        let secs = now
            .saturating_duration_since(self.since)
            .as_secs_f64()
            .max(1.0);
        tracing::info!(
            device = %device,
            fps = format!("{:.1}", f64::from(self.frames) / secs),
            keyframes = self.keyframes,
            largest_gap_ms = self.largest_gap.as_millis(),
            renewals = self.renewals,
            reopenings = self.reopenings,
            skipped = self.skipped,
            "rtsp: feed"
        );
        *self = Self::new(now);
    }
}

async fn recv(pump: &mut Option<Pump>) -> Option<VideoFrame> {
    match pump {
        Some(p) => p.frames.recv().await,
        None => std::future::pending().await,
    }
}

async fn joined(task: &mut Option<JoinHandle<Result<Pump, LiveError>>>) -> Result<Pump, String> {
    let Some(handle) = task else {
        return std::future::pending().await;
    };
    let result = handle.await;
    *task = None;
    match result {
        Ok(Ok(pump)) => Ok(pump),
        Ok(Err(e)) => Err(e.to_string()),
        Err(e) => Err(e.to_string()),
    }
}

async fn until(at: Option<Instant>) {
    match at {
        Some(at) => tokio::time::sleep_until(at).await,
        None => std::future::pending().await,
    }
}

/// Starts the feed on `first` (the stream opened for `DESCRIBE`). The
/// first frame sent is stamped `start`. The feed ends when the receiver is
/// dropped, or after [`GIVE_UP`] without a frame.
pub fn spawn(hub: Hub, device: DeviceId, first: Pump, start: u32) -> mpsc::Receiver<VideoFrame> {
    let (tx, rx) = mpsc::channel(QUEUE);
    tokio::spawn(run(hub, device, first, start, tx));
    rx
}

#[allow(clippy::too_many_lines)] // one loop, every event in it
async fn run(hub: Hub, device: DeviceId, first: Pump, start: u32, out: mpsc::Sender<VideoFrame>) {
    let now = Instant::now();
    let mut clock = Clock::new(start, now);
    let mut current = Some(first);
    // A fresh stream that has not shown a key frame yet.
    let mut candidate: Option<Pump> = None;
    let mut opening: Option<JoinHandle<Result<Pump, LiveError>>> = None;
    let mut fresh = true; // the next frame of `current` starts a stream
    let mut renew_at = Some(now + RENEW);
    let mut retry_at: Option<Instant> = None;
    let mut retry = FIRST_RETRY;
    let mut last_sent = now;
    let mut skipping = false;
    // After a switch to a stream of the same clock: frames up to this
    // source timestamp were already sent.
    let mut sent_up_to: Option<u32> = None;
    let mut stats = Stats::new(now);
    let mut report = tokio::time::interval_at(now + STATS_EVERY, STATS_EVERY);

    let begin_opening = |opening: &mut Option<JoinHandle<Result<Pump, LiveError>>>| {
        let (hub, device) = (hub.clone(), device.clone());
        *opening = Some(tokio::spawn(async move { open(&hub, &device).await }));
    };

    loop {
        let (frame, from_candidate) = tokio::select! {
            () = out.closed() => return,
            _ = report.tick() => {
                stats.report(&device, Instant::now());
                continue;
            }
            () = tokio::time::sleep_until(last_sent + GIVE_UP) => {
                tracing::warn!(device = %device, "rtsp: no picture for a minute, the client is let go");
                return;
            }
            frame = recv(&mut current) => {
                if let Some(frame) = frame {
                    (frame, false)
                } else {
                    // The stream ended: the fresh one if there is one, or open again.
                    current = candidate.take();
                    fresh = true;
                    stats.reopenings += 1;
                    if current.is_none() && opening.is_none() && retry_at.is_none() {
                        begin_opening(&mut opening);
                    }
                    continue;
                }
            }
            frame = recv(&mut candidate) => {
                if let Some(frame) = frame {
                    (frame, true)
                } else {
                    candidate = None;
                    renew_at = Some(Instant::now() + RENEW_RETRY);
                    continue;
                }
            }
            result = joined(&mut opening) => {
                match result {
                    Ok(pump) => {
                        retry = FIRST_RETRY;
                        renew_at = Some(Instant::now() + RENEW);
                        if current.is_none() {
                            current = Some(pump);
                            fresh = true;
                        } else {
                            candidate = Some(pump);
                        }
                    }
                    Err(e) => {
                        tracing::info!(device = %device, error = %e, "rtsp: the camera did not open");
                        if current.is_none() {
                            retry_at = Some(Instant::now() + retry);
                            retry = (retry * 2).min(MAX_RETRY);
                        } else {
                            renew_at = Some(Instant::now() + RENEW_RETRY);
                        }
                    }
                }
                continue;
            }
            () = until(retry_at) => {
                retry_at = None;
                begin_opening(&mut opening);
                continue;
            }
            () = until(renew_at), if opening.is_none() && candidate.is_none() && current.is_some() => {
                renew_at = None;
                begin_opening(&mut opening);
                continue;
            }
        };
        let now = Instant::now();
        if from_candidate {
            // The fresh stream takes over on its first key frame.
            if !frame.keyframe {
                continue;
            }
            current = candidate.take();
            fresh = true;
            stats.renewals += 1;
        }
        if fresh {
            if clock.continues(frame.timestamp) {
                // Another viewer of the same stream: same timeline.
                sent_up_to = clock.last_src;
                tracing::debug!(device = %device, "rtsp: the stream goes on (same source)");
            } else {
                if !frame.keyframe {
                    continue;
                }
                let gap = now.saturating_duration_since(last_sent);
                tracing::info!(device = %device, gap_ms = gap.as_millis(), "rtsp: a new stream starts");
                clock.rebase(frame.timestamp, now);
            }
            fresh = false;
        }
        if let Some(up_to) = sent_up_to {
            if frame.timestamp.wrapping_sub(up_to).cast_signed() <= 0 {
                continue; // already sent by the stream before
            }
            sent_up_to = None;
        }
        let timestamp = clock.stamp(frame.timestamp, now);
        if skipping && !frame.keyframe {
            stats.skipped += 1;
            continue;
        }
        skipping = false;
        let keyframe = frame.keyframe;
        match out.try_send(VideoFrame { timestamp, ..frame }) {
            Ok(()) => {
                stats.frames += 1;
                stats.keyframes += u32::from(keyframe);
                stats.largest_gap = stats
                    .largest_gap
                    .max(now.saturating_duration_since(last_sent));
                last_sent = now;
            }
            Err(mpsc::error::TrySendError::Full(_)) => {
                stats.skipped += 1;
                skipping = true;
            }
            Err(mpsc::error::TrySendError::Closed(_)) => return,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_timeline_goes_on_across_streams() {
        let t0 = Instant::now();
        let mut clock = Clock::new(1000, t0);
        clock.rebase(500_000, t0);
        assert_eq!(clock.stamp(500_000, t0), 1000);
        assert_eq!(clock.stamp(503_000, t0), 4000);
        // A new stream, half a second later, with its own clock.
        let t1 = t0 + Duration::from_millis(500);
        clock.rebase(7, t1);
        assert_eq!(clock.stamp(7, t1), 4000 + 45_000);
        assert_eq!(clock.stamp(3007, t1), 4000 + 48_000);
    }

    #[test]
    fn a_clock_that_jumps_back_never_takes_the_timeline_back() {
        let t0 = Instant::now();
        let mut clock = Clock::new(u32::MAX - 10, t0);
        clock.rebase(100_000, t0);
        let a = clock.stamp(100_000, t0);
        let b = clock.stamp(103_000, t0); // wraps around u32
        assert!(b.wrapping_sub(a).cast_signed() > 0);
        let c = clock.stamp(5, t0 + Duration::from_millis(40));
        assert!(c.wrapping_sub(b).cast_signed() > 0);
    }
}
