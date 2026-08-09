//! Clock / timestamp helpers.
#![allow(dead_code)]
//!
//! OMT timestamps use 100 ns ticks (`10_000_000` = 1 second).
//! A special value of `-1` asks the sender to generate timestamps automatically.
//!
//! [`TimestampClock`] mirrors libomtnet `OMTClock`: wall-clock pacing with sleep when
//! ahead, and timestamp skip when behind.

use std::thread;
use std::time::{Duration, Instant};

/// OMT clock ticks per second (10,000,000).
pub const TICKS_PER_SECOND: i64 = 10_000_000;

/// Auto-timestamp sentinel (`-1`).
pub const AUTO_TIMESTAMP: i64 = -1;

/// Convert seconds to OMT timestamp ticks.
pub fn seconds_to_timestamp(seconds: f64) -> i64 {
    (seconds * TICKS_PER_SECOND as f64).round() as i64
}

/// Convert OMT timestamp ticks to seconds.
pub fn timestamp_to_seconds(ts: i64) -> f64 {
    ts as f64 / TICKS_PER_SECOND as f64
}

/// Wall-clock timestamp in OMT ticks since UNIX epoch.
pub fn now_timestamp() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let dur = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    (dur.as_secs() as i64)
        .saturating_mul(TICKS_PER_SECOND)
        .saturating_add((dur.subsec_nanos() as i64) * TICKS_PER_SECOND / 1_000_000_000)
}

/// Resolve `-1` to "now"; otherwise return `ts` unchanged.
pub fn resolve_timestamp(ts: i64) -> i64 {
    if ts == AUTO_TIMESTAMP {
        now_timestamp()
    } else {
        ts
    }
}

fn wall_ticks(start: Instant) -> i64 {
    let d = start.elapsed();
    (d.as_secs() as i64)
        .saturating_mul(TICKS_PER_SECOND)
        .saturating_add((d.subsec_nanos() as i64) * TICKS_PER_SECOND / 1_000_000_000)
}

/// Tracks auto-generated presentation timestamps for a sender (libomtnet `OMTClock`).
#[derive(Debug)]
pub struct TimestampClock {
    audio: bool,
    last: Option<i64>,
    start: Instant,
    /// Presentation timeline in ticks from [`Self::start`] (libomtnet `clockTimestamp`).
    clock_ts: i64,
    frame_rate_n: i32,
    frame_rate_d: i32,
    sample_rate: i32,
    frame_interval: i64,
}

impl TimestampClock {
    /// Create a video (`audio = false`) or audio (`audio = true`) clock.
    pub fn new(audio: bool) -> Self {
        Self {
            audio,
            last: None,
            start: Instant::now(),
            clock_ts: 0,
            frame_rate_n: -1,
            frame_rate_d: -1,
            sample_rate: -1,
            frame_interval: -1,
        }
    }

    fn reset(
        &mut self,
        frame_rate_n: i32,
        frame_rate_d: i32,
        sample_rate: i32,
        samples_per_channel: i32,
    ) {
        self.frame_rate_n = frame_rate_n;
        self.frame_rate_d = frame_rate_d;
        self.sample_rate = sample_rate;
        if self.audio && sample_rate > 0 && samples_per_channel > 0 {
            self.frame_interval =
                (samples_per_channel as i64).saturating_mul(TICKS_PER_SECOND) / sample_rate as i64;
        } else if frame_rate_n > 0 && frame_rate_d > 0 {
            self.frame_interval =
                (frame_rate_d as i64).saturating_mul(TICKS_PER_SECOND) / frame_rate_n as i64;
        } else {
            self.frame_interval = TICKS_PER_SECOND / 60;
        }
        self.start = Instant::now();
        self.clock_ts = 0;
        self.last = None;
    }

    /// Resolve a frame timestamp, pacing to the wall clock like libomtnet `OMTClock.Process`.
    ///
    /// When `ts == -1`:
    /// - first frame starts at timestamp `0`
    /// - subsequent frames advance by the video/audio interval
    /// - if the presentation clock is behind wall time by more than one interval, timestamps
    ///   skip forward (late frames)
    /// - if ahead of wall time, this sleeps in 1 ms steps until due
    pub fn resolve(
        &mut self,
        ts: i64,
        frame_rate_n: i32,
        frame_rate_d: i32,
        sample_rate: i32,
        samples_per_channel: i32,
    ) -> i64 {
        if (self.audio && sample_rate != self.sample_rate)
            || (!self.audio
                && (frame_rate_n != self.frame_rate_n || frame_rate_d != self.frame_rate_d))
        {
            self.reset(frame_rate_n, frame_rate_d, sample_rate, samples_per_channel);
        }

        if ts != AUTO_TIMESTAMP {
            self.last = Some(ts);
            return ts;
        }

        if self.last.is_none() {
            self.reset(frame_rate_n, frame_rate_d, sample_rate, samples_per_channel);
            self.last = Some(0);
            return 0;
        }

        if self.audio && sample_rate > 0 && samples_per_channel > 0 {
            self.frame_interval =
                (samples_per_channel as i64).saturating_mul(TICKS_PER_SECOND) / sample_rate as i64;
        }

        let interval = self.frame_interval.max(1);
        let mut timestamp = self.last.unwrap().saturating_add(interval);
        self.clock_ts = self.clock_ts.saturating_add(interval);

        let mut diff = self.clock_ts - wall_ticks(self.start);
        // Behind wall clock: skip intervals (do not sleep).
        while diff < -interval {
            timestamp = timestamp.saturating_add(interval);
            self.clock_ts = self.clock_ts.saturating_add(interval);
            diff += interval;
        }
        // Ahead of wall clock: pace.
        while self.clock_ts > wall_ticks(self.start) {
            thread::sleep(Duration::from_millis(1));
        }

        self.last = Some(timestamp);
        timestamp
    }
}
