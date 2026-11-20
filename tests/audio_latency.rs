//! The receiver audio queue is bounded by media time, not by frame count.

use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use openmediatransport::{
    Codec, FrameType, MediaFrame, ReceiverConfig, ReceiverSession, Sender, SenderConfig,
    SessionStatistics,
};

const SAMPLES: i32 = 480;
const RATE: i32 = 48_000;
/// 10 ms at 48 kHz, in OMT's 100 ns ticks.
const FRAME_TICKS: i64 = 100_000;

fn audio_frame(timestamp: i64) -> MediaFrame {
    MediaFrame {
        frame_type: FrameType::AUDIO,
        timestamp,
        codec: Codec::Fpa1 as i32,
        sample_rate: RATE,
        channels: 2,
        samples_per_channel: SAMPLES,
        data: vec![0u8; SAMPLES as usize * 2 * 4],
        ..Default::default()
    }
}

fn connect(sender: &mut Sender, latency: Option<Duration>, name: &str) -> ReceiverSession {
    let url = format!("omt://127.0.0.1:{}", sender.port());
    let mut config = ReceiverConfig {
        frame_types: FrameType::AUDIO,
        auto_reconnect: false,
        connect_timeout: Duration::from_secs(3),
        ..ReceiverConfig::default()
    };
    if let Some(latency) = latency {
        config.audio_max_latency = latency;
    }
    let session = ReceiverSession::connect(url, config).expect("receiver connect");

    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        let _ = sender.poll_accept();
        let _ = sender.poll_peer_metadata();
        if sender.audio_subscribed() {
            return session;
        }
        thread::sleep(Duration::from_millis(5));
    }
    sender.force_subscribe(true, true, false);
    eprintln!("{name}: subscription forced");
    session
}

/// Stream audio for `duration`, stall the consumer for the first 250 ms, then
/// drain until the sender stops. Returns how far behind the newest sent frame
/// the delivered audio was, so staleness can be asserted directly.
fn stream_and_drain(name: &str, latency: Option<Duration>) -> (usize, i64, SessionStatistics) {
    let mut sender = Sender::create(name, FrameType::AUDIO).expect("sender");
    let session = connect(&mut sender, latency, name);

    let newest = Arc::new(AtomicI64::new(i64::MIN));
    let producer = {
        let newest = Arc::clone(&newest);
        thread::spawn(move || {
            let duration = Duration::from_millis(1200);
            let start = Instant::now();
            let mut index = 0i64;
            while start.elapsed() < duration {
                // Two frames per wake-up keeps the media rate above real time
                // even where thread::sleep is quantised to ~15 ms (Windows).
                for _ in 0..2 {
                    let timestamp = index * FRAME_TICKS;
                    sender
                        .send_audio(audio_frame(timestamp))
                        .expect("send audio");
                    newest.store(timestamp, Ordering::Release);
                    index += 1;
                }
                thread::sleep(Duration::from_millis(10));
            }
        })
    };

    // Stall the consumer so the queue has to bound itself.
    thread::sleep(Duration::from_millis(400));

    let mut delivered = 0usize;
    let mut max_lag_ticks = 0i64;
    let mut last_timestamp = i64::MIN;
    while !producer.is_finished() {
        while let Some(frame) = session.recv_audio_timeout(Duration::from_millis(5)) {
            assert!(
                frame.timestamp >= last_timestamp,
                "timestamps must be monotonic"
            );
            last_timestamp = frame.timestamp;
            let newest = newest.load(Ordering::Acquire);
            if newest != i64::MIN {
                max_lag_ticks = max_lag_ticks.max(newest - frame.timestamp);
            }
            delivered += 1;
        }
    }
    producer.join().expect("producer");
    let stats = session.statistics();
    session.disconnect();
    (delivered, max_lag_ticks, stats)
}

fn assert_bounded(
    name: &str,
    delivered: usize,
    max_lag_ticks: i64,
    stats: &SessionStatistics,
    bound: Duration,
) {
    assert!(delivered > 0, "{name}: no audio delivered");
    let bound_ticks = (bound.as_nanos() / 100) as i64;
    // The producer batches two frames per wake-up, so allow a few frames of
    // measurement slack on top of the budget.
    assert!(
        max_lag_ticks <= bound_ticks + 6 * FRAME_TICKS,
        "{name}: delivered audio was {max_lag_ticks} ticks behind the newest frame, budget {bound_ticks}"
    );

    let bound_us = bound.as_micros() as u64;
    assert!(
        stats.audio_latency_us <= bound_us + 5_000,
        "{name}: queued latency {} us exceeds {bound_us} us",
        stats.audio_latency_us
    );
    let max_depth = (bound_us / 10_000) as u32 + 1;
    assert!(
        stats.audio_queue_depth <= max_depth,
        "{name}: depth {} exceeds {max_depth}",
        stats.audio_queue_depth
    );
    assert!(
        stats.audio_frames_dropped_stale > 0,
        "{name}: expected stale drops, stats={stats:?}"
    );
    assert!(
        stats.frames_dropped_audio >= stats.audio_frames_dropped_stale,
        "{name}: total drop counter must include stale drops"
    );
}

#[test]
fn default_budget_is_100ms() {
    let (delivered, max_lag, stats) = stream_and_drain("aud-default", None);
    assert_bounded(
        "default",
        delivered,
        max_lag,
        &stats,
        Duration::from_millis(100),
    );
}

#[test]
fn configurable_budget_is_applied() {
    let (delivered, max_lag, stats) = stream_and_drain("aud-250", Some(Duration::from_millis(250)));
    assert_bounded(
        "250ms",
        delivered,
        max_lag,
        &stats,
        Duration::from_millis(250),
    );
}

#[test]
fn audio_only_session_round_trips() {
    // Synchronous writes keep the sender's async queue out of this check.
    let mut sender = Sender::create_with_config(
        "aud-roundtrip",
        FrameType::AUDIO,
        SenderConfig {
            send_queue_depth: 0,
            ..SenderConfig::default()
        },
    )
    .expect("sender");
    let session = connect(&mut sender, None, "roundtrip");

    for index in 0..8 {
        sender
            .send_audio(audio_frame(index * FRAME_TICKS))
            .expect("send");
    }

    let mut got = 0;
    let deadline = Instant::now() + Duration::from_secs(2);
    while got < 8 && Instant::now() < deadline {
        if session
            .recv_audio_timeout(Duration::from_millis(50))
            .is_some()
        {
            got += 1;
        }
    }
    assert_eq!(got, 8, "audio frames lost");
    let stats = session.statistics();
    assert_eq!(stats.audio_queue_depth, 0);
    assert_eq!(stats.audio_latency_us, 0);
    assert_eq!(stats.audio_frames_dropped_stale, 0);
    session.disconnect();
}
