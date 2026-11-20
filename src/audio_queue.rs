//! Bounded decoded-audio queue keyed by media time rather than frame count.
//!
//! A frame count says nothing about latency: OMT senders pick their own
//! `samples_per_channel` (10 ms in eiviz, one video frame ≈ 16.7 ms in the
//! official example). The queue therefore bounds the *media duration* it keeps
//! plus the wall-clock age of its oldest frame, and drops the oldest frames
//! first.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

use crate::types::DecodedAudioFrame;

/// Default upper bound for buffered audio (media time).
pub(crate) const DEFAULT_MAX_LATENCY: Duration = Duration::from_millis(100);
/// Lower clamp for `ReceiverConfig::audio_max_latency`.
const MIN_MAX_LATENCY: Duration = Duration::from_millis(1);
/// Upper clamp for `ReceiverConfig::audio_max_latency`.
const MAX_MAX_LATENCY: Duration = Duration::from_secs(2);
/// Memory safety valve; the time bounds normally keep the queue far below it.
const MAX_QUEUE_FRAMES: usize = 4096;
/// Memory safety valve for unusually large frames.
const MAX_QUEUE_BYTES: usize = 8 * 1024 * 1024;
/// OMT timestamps: 100 ns ticks per second.
const TICKS_PER_SECOND: u64 = 10_000_000;

struct QueuedAudio {
    frame: DecodedAudioFrame,
    enqueued_at: Instant,
    /// Media duration of this frame in 100 ns ticks.
    duration_ticks: u64,
    bytes: usize,
}

struct Inner {
    frames: VecDeque<QueuedAudio>,
    media_ticks: u64,
    bytes: usize,
    /// Last `(sample rate, channels)` seen; a change flushes the old timeline.
    format: Option<(i32, i32)>,
}

/// Snapshot of the queue gauges for [`crate::SessionStatistics`].
pub(crate) struct AudioQueueSnapshot {
    pub(crate) depth: u32,
    pub(crate) latency_us: u64,
    pub(crate) oldest_us: u64,
    pub(crate) peak_us: u64,
    pub(crate) stale: u64,
}

/// Decoded-audio FIFO with a media-time and wall-clock latency budget.
pub(crate) struct AudioQueue {
    inner: Mutex<Inner>,
    cv: Condvar,
    max_latency: Duration,
    depth: AtomicU32,
    stale: AtomicU64,
    latency_us: AtomicU64,
    oldest_us: AtomicU64,
    peak_us: AtomicU64,
}

impl AudioQueue {
    /// Create a queue, clamping the requested budget to a practical range.
    pub(crate) fn new(max_latency: Duration) -> Self {
        Self {
            inner: Mutex::new(Inner {
                frames: VecDeque::new(),
                media_ticks: 0,
                bytes: 0,
                format: None,
            }),
            cv: Condvar::new(),
            max_latency: max_latency.clamp(MIN_MAX_LATENCY, MAX_MAX_LATENCY),
            depth: AtomicU32::new(0),
            stale: AtomicU64::new(0),
            latency_us: AtomicU64::new(0),
            oldest_us: AtomicU64::new(0),
            peak_us: AtomicU64::new(0),
        }
    }

    /// Effective (clamped) budget.
    #[cfg(test)]
    pub(crate) fn max_latency(&self) -> Duration {
        self.max_latency
    }

    /// Enqueue a decoded frame, dropping stale frames to stay within budget.
    pub(crate) fn push(&self, frame: DecodedAudioFrame) {
        self.push_at(frame, Instant::now());
    }

    /// Non-blocking pop. Stale frames may be discarded first, so this can
    /// return `None` even when the queue held frames.
    pub(crate) fn try_pop(&self) -> Option<DecodedAudioFrame> {
        self.try_pop_at(Instant::now())
    }

    /// Wait up to `timeout` for a frame, discarding stale frames while waiting.
    pub(crate) fn recv_timeout(
        &self,
        timeout: Duration,
        stop: &AtomicBool,
    ) -> Option<DecodedAudioFrame> {
        let deadline = Instant::now() + timeout;
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            let now = Instant::now();
            self.trim_locked(&mut guard, now, false);
            if let Some(queued) = self.pop_locked(&mut guard) {
                self.publish_gauges(&guard, now);
                return Some(queued);
            }
            self.publish_gauges(&guard, now);
            if stop.load(Ordering::Acquire) {
                return None;
            }
            let now = Instant::now();
            if now >= deadline {
                return None;
            }
            let (next, _) = self
                .cv
                .wait_timeout(guard, deadline.saturating_duration_since(now))
                .unwrap_or_else(|e| e.into_inner());
            guard = next;
        }
    }

    /// Drop every queued frame (reconnect, format change, explicit flush).
    pub(crate) fn clear(&self) {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        Self::clear_locked(&mut guard);
        self.depth.store(0, Ordering::Relaxed);
        self.latency_us.store(0, Ordering::Relaxed);
        self.oldest_us.store(0, Ordering::Relaxed);
    }

    /// Wake blocked [`Self::recv_timeout`] callers (session stop).
    pub(crate) fn notify_all(&self) {
        self.cv.notify_all();
    }

    /// Current gauges. Locks briefly to read the queue; callers must not hold
    /// the statistics mutex at the same time (no code path does).
    pub(crate) fn snapshot(&self) -> AudioQueueSnapshot {
        let guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        AudioQueueSnapshot {
            depth: guard.frames.len() as u32,
            latency_us: guard.media_ticks / 10,
            oldest_us: guard
                .frames
                .front()
                .map(|q| now.saturating_duration_since(q.enqueued_at).as_micros() as u64)
                .unwrap_or(0),
            peak_us: self.peak_us.load(Ordering::Relaxed),
            stale: self.stale.load(Ordering::Relaxed),
        }
    }

    fn push_at(&self, frame: DecodedAudioFrame, now: Instant) {
        let duration_ticks = duration_ticks(&frame);
        let bytes = frame.pcm_planar_f32.len();
        let format = frame_format(&frame);
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if let (Some(previous), Some(next)) = (guard.format, format)
            && previous != next
        {
            Self::clear_locked(&mut guard);
        }
        if format.is_some() {
            guard.format = format;
        }
        guard.frames.push_back(QueuedAudio {
            frame,
            enqueued_at: now,
            duration_ticks,
            bytes,
        });
        guard.media_ticks = guard.media_ticks.saturating_add(duration_ticks);
        guard.bytes = guard.bytes.saturating_add(bytes);
        self.trim_locked(&mut guard, now, true);
        self.publish_gauges(&guard, now);
        self.cv.notify_one();
    }

    fn try_pop_at(&self, now: Instant) -> Option<DecodedAudioFrame> {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        self.trim_locked(&mut guard, now, false);
        let popped = self.pop_locked(&mut guard);
        self.publish_gauges(&guard, now);
        popped
    }

    /// Drop oldest frames until both the media-time and wall-clock budgets hold.
    ///
    /// `keep_one` protects the newest frame on the producer path: a frame longer
    /// than the whole budget cannot be split and must still be delivered.
    fn trim_locked(&self, guard: &mut Inner, now: Instant, keep_one: bool) {
        let max_ticks = (self.max_latency.as_nanos() / 100) as u64;
        while let Some(front) = guard.frames.front() {
            let over_media = guard.media_ticks > max_ticks && guard.frames.len() > 1;
            let over_time = now.saturating_duration_since(front.enqueued_at) > self.max_latency;
            let over_guard = guard.frames.len() > MAX_QUEUE_FRAMES || guard.bytes > MAX_QUEUE_BYTES;
            if !(over_media || over_time || over_guard) {
                break;
            }
            if keep_one && guard.frames.len() == 1 {
                break;
            }
            if self.pop_locked(guard).is_none() {
                break;
            }
            self.stale.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn pop_locked(&self, guard: &mut Inner) -> Option<DecodedAudioFrame> {
        let queued = guard.frames.pop_front()?;
        guard.media_ticks = guard.media_ticks.saturating_sub(queued.duration_ticks);
        guard.bytes = guard.bytes.saturating_sub(queued.bytes);
        Some(queued.frame)
    }

    fn publish_gauges(&self, guard: &Inner, now: Instant) {
        self.depth
            .store(guard.frames.len() as u32, Ordering::Relaxed);
        let latency_us = guard.media_ticks / 10;
        self.latency_us.store(latency_us, Ordering::Relaxed);
        self.oldest_us.store(
            guard
                .frames
                .front()
                .map(|q| now.saturating_duration_since(q.enqueued_at).as_micros() as u64)
                .unwrap_or(0),
            Ordering::Relaxed,
        );
        let peak = self.peak_us.load(Ordering::Relaxed).max(latency_us);
        self.peak_us.store(peak, Ordering::Relaxed);
    }

    fn clear_locked(guard: &mut Inner) {
        guard.frames.clear();
        guard.media_ticks = 0;
        guard.bytes = 0;
        guard.format = None;
    }
}

/// Media duration of a decoded frame in 100 ns ticks; `0` when the header is
/// unusable, which falls back to the wall-clock budget only.
fn duration_ticks(frame: &DecodedAudioFrame) -> u64 {
    let rate = frame.sample_rate;
    let samples = frame.samples_per_channel;
    if rate <= 0 || samples <= 0 {
        return 0;
    }
    ((samples as u128 * TICKS_PER_SECOND as u128) / rate as u128) as u64
}

fn frame_format(frame: &DecodedAudioFrame) -> Option<(i32, i32)> {
    if frame.sample_rate > 0 && frame.channels > 0 {
        Some((frame.sample_rate, frame.channels))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use super::*;

    fn frame(samples: i32, rate: i32) -> DecodedAudioFrame {
        DecodedAudioFrame {
            timestamp: 0,
            sample_rate: rate,
            channels: 2,
            samples_per_channel: samples,
            active_channels: 3,
            pcm_planar_f32: Arc::from(vec![0u8; samples as usize * 2 * 4]),
            frame_metadata: None,
        }
    }

    fn stereo_10ms() -> DecodedAudioFrame {
        frame(480, 48_000)
    }

    #[test]
    fn pushes_and_pops_in_order() {
        let queue = AudioQueue::new(Duration::from_millis(100));
        for i in 0..3 {
            let mut f = stereo_10ms();
            f.timestamp = i;
            queue.push(f);
        }
        for i in 0..3 {
            assert_eq!(queue.try_pop().map(|f| f.timestamp), Some(i));
        }
        assert!(queue.try_pop().is_none());
    }

    #[test]
    fn media_time_budget_drops_oldest() {
        let queue = AudioQueue::new(Duration::from_millis(100));
        let now = Instant::now();
        for i in 0..30 {
            let mut f = stereo_10ms();
            f.timestamp = i;
            queue.push_at(f, now);
        }
        let snapshot = queue.snapshot();
        // 10 frames x 10 ms fits the 100 ms budget; the rest is stale.
        assert_eq!(snapshot.depth, 10, "depth");
        assert_eq!(snapshot.stale, 20, "stale");
        assert!(snapshot.latency_us <= 100_000, "{}", snapshot.latency_us);
        let first = queue.try_pop().expect("frame");
        assert_eq!(first.timestamp, 20, "oldest frames must be dropped first");
    }

    #[test]
    fn wall_clock_budget_drops_stale_frames_on_pop() {
        let queue = AudioQueue::new(Duration::from_millis(100));
        let now = Instant::now();
        for i in 0..5 {
            let mut f = stereo_10ms();
            f.timestamp = i;
            queue.push_at(f, now);
        }
        let later = now + Duration::from_millis(300);
        assert!(queue.try_pop_at(later).is_none());
        assert!(queue.snapshot().stale >= 5);
        assert_eq!(queue.snapshot().depth, 0);
    }

    #[test]
    fn single_oversized_frame_is_still_delivered() {
        let queue = AudioQueue::new(Duration::from_millis(100));
        let now = Instant::now();
        queue.push_at(frame(48_000, 48_000), now); // 1 s of audio
        assert_eq!(queue.snapshot().depth, 1);
        assert!(queue.try_pop_at(now + Duration::from_millis(10)).is_some());
    }

    #[test]
    fn unknown_sample_rate_falls_back_to_wall_clock() {
        let queue = AudioQueue::new(Duration::from_millis(100));
        let now = Instant::now();
        let mut f = frame(0, 0);
        f.timestamp = 7;
        queue.push_at(f, now);
        assert_eq!(queue.snapshot().latency_us, 0);
        assert_eq!(queue.snapshot().depth, 1);
        assert!(queue.try_pop_at(now).is_some());
    }

    #[test]
    fn format_change_flushes_old_frames() {
        let queue = AudioQueue::new(Duration::from_millis(500));
        let now = Instant::now();
        let mut first = frame(480, 48_000);
        first.timestamp = 1;
        queue.push_at(first, now);
        let mut second = frame(441, 44_100);
        second.timestamp = 2;
        queue.push_at(second, now);
        let snapshot = queue.snapshot();
        assert_eq!(snapshot.depth, 1);
        assert_eq!(queue.try_pop().map(|f| f.timestamp), Some(2));
    }

    #[test]
    fn clear_empties_without_counting_stale() {
        let queue = AudioQueue::new(Duration::from_millis(100));
        for _ in 0..3 {
            queue.push(stereo_10ms());
        }
        queue.clear();
        let snapshot = queue.snapshot();
        assert_eq!(snapshot.depth, 0);
        assert_eq!(snapshot.latency_us, 0);
        assert_eq!(snapshot.stale, 0);
    }

    #[test]
    fn frame_and_byte_guards_bound_the_queue() {
        let queue = AudioQueue::new(Duration::from_secs(2));
        let now = Instant::now();
        // Unknown rate -> duration 0, so only the memory guard can bound this.
        for _ in 0..(MAX_QUEUE_FRAMES + 16) {
            queue.push_at(frame(0, 0), now);
        }
        let snapshot = queue.snapshot();
        assert!(snapshot.depth as usize <= MAX_QUEUE_FRAMES);
        assert!(snapshot.stale > 0);
    }

    #[test]
    fn latency_peak_tracks_the_largest_backlog() {
        let queue = AudioQueue::new(Duration::from_millis(100));
        let now = Instant::now();
        for _ in 0..8 {
            queue.push_at(stereo_10ms(), now);
        }
        assert!(queue.snapshot().peak_us >= 80_000);
        for _ in 0..8 {
            let _ = queue.try_pop_at(now);
        }
        assert!(queue.snapshot().peak_us >= 80_000, "peak is cumulative");
    }

    #[test]
    fn budget_is_clamped() {
        assert_eq!(
            AudioQueue::new(Duration::ZERO).max_latency(),
            MIN_MAX_LATENCY
        );
        assert_eq!(
            AudioQueue::new(Duration::from_secs(60)).max_latency(),
            MAX_MAX_LATENCY
        );
    }

    #[test]
    fn recv_timeout_wakes_on_push() {
        let queue = Arc::new(AudioQueue::new(Duration::from_millis(100)));
        let stop = Arc::new(AtomicBool::new(false));
        let producer = {
            let queue = Arc::clone(&queue);
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(20));
                queue.push(stereo_10ms());
            })
        };
        let got = queue.recv_timeout(Duration::from_secs(1), &stop);
        assert!(got.is_some());
        producer.join().unwrap();
    }

    #[test]
    fn recv_timeout_stops_without_frames() {
        let queue = AudioQueue::new(Duration::from_secs(1));
        let stop = AtomicBool::new(true);
        let started = Instant::now();
        assert!(queue.recv_timeout(Duration::from_secs(1), &stop).is_none());
        assert!(started.elapsed() < Duration::from_millis(500));
    }
}
