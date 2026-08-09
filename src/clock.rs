//! Clock / timestamp helpers.
//!
//! OMT timestamps use 100 ns ticks (`10_000_000` = 1 second).
//! A special value of `-1` asks the sender to generate timestamps automatically.

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

/// Tracks auto-generated presentation timestamps for a sender.
#[derive(Debug, Default)]
pub struct TimestampClock {
    last: Option<i64>,
}

impl TimestampClock {
    /// Create a new clock.
    pub fn new() -> Self {
        Self::default()
    }

    /// Resolve a frame timestamp.
    ///
    /// When `ts == -1`, advances based on video frame rate or audio sample rate.
    pub fn resolve(
        &mut self,
        ts: i64,
        frame_rate_n: i32,
        frame_rate_d: i32,
        sample_rate: i32,
        samples_per_channel: i32,
    ) -> i64 {
        if ts != AUTO_TIMESTAMP {
            self.last = Some(ts);
            return ts;
        }
        let next = match self.last {
            None => now_timestamp(),
            Some(prev) => {
                let delta = if sample_rate > 0 && samples_per_channel > 0 {
                    (samples_per_channel as i64)
                        .saturating_mul(TICKS_PER_SECOND)
                        / sample_rate as i64
                } else if frame_rate_n > 0 && frame_rate_d > 0 {
                    (frame_rate_d as i64)
                        .saturating_mul(TICKS_PER_SECOND)
                        / frame_rate_n as i64
                } else {
                    TICKS_PER_SECOND / 60
                };
                prev.saturating_add(delta)
            }
        };
        self.last = Some(next);
        next
    }
}
