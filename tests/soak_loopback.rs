//! Timed loopback soak for 1080p @ 60000/1001 receive → BGRA.
//!
//! Default duration is short for CI (`OMT_SOAK_SECS`, default 5).
//! For the plan gate, run: `OMT_SOAK_SECS=600 cargo test --release --test soak_loopback -- --nocapture`

use openmediatransport::{Codec, FrameType, MediaFrame, ReceiverConfig, ReceiverSession, Sender};
use std::env;
use std::thread;
use std::time::{Duration, Instant};
use vmx::{Codec as VmxCodec, Config as VmxConfig, Profile};

fn soak_secs() -> u64 {
    env::var("OMT_SOAK_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(5)
}

#[test]
fn loopback_1080p_60000_1001_soak() {
    let secs = soak_secs();
    let width = 1920i32;
    let height = 1080i32;
    let frame_n = 60_000i32;
    let frame_d = 1_001i32;
    let frame_period = Duration::from_secs_f64(frame_d as f64 / frame_n as f64);

    let mut sender = Sender::create("SoakSrc", FrameType::VIDEO).expect("sender");
    let url = format!("omt://127.0.0.1:{}", sender.port());
    let session = ReceiverSession::connect(
        url,
        ReceiverConfig {
            frame_types: FrameType::VIDEO,
            auto_reconnect: false,
            connect_timeout: Duration::from_secs(3),
            ..ReceiverConfig::default()
        },
    )
    .expect("session");

    for _ in 0..200 {
        let _ = sender.poll_accept();
        let _ = sender.poll_peer_metadata();
        if sender.video_subscribed() {
            break;
        }
        thread::sleep(Duration::from_millis(5));
    }
    if !sender.video_subscribed() {
        sender.force_subscribe(true, false, false);
    }

    let stride = (width as usize) * 2;
    let mut uyvy = vec![128u8; stride * height as usize];
    for y in 0..height as usize {
        for x in (0..width as usize).step_by(2) {
            let o = y * stride + x * 2;
            uyvy[o] = 128;
            uyvy[o + 1] = 16 + ((x / 8 + y / 8) % 220) as u8;
            uyvy[o + 2] = 128;
            uyvy[o + 3] = 16 + ((x / 8 + 1 + y / 8) % 220) as u8;
        }
    }
    let mut enc = VmxCodec::new(VmxConfig {
        width,
        height,
        profile: Profile::OmtSq,
        color_space: Default::default(),
    })
    .unwrap();
    enc.encode_uyvy(&uyvy, stride).unwrap();
    let mut bitstream = vec![0u8; 8 << 20];
    let len = enc.save_to(&mut bitstream).unwrap();
    let payload = &bitstream[..len];

    let mut times_us = Vec::new();
    let mut received = 0u64;
    let mut sent = 0u64;
    let mut ts = 0i64;
    let start = Instant::now();
    let mut next_send = start;

    while start.elapsed() < Duration::from_secs(secs) {
        let now = Instant::now();
        if now >= next_send {
            ts += (10_000_000i64 * frame_d as i64) / frame_n as i64;
            let frame = MediaFrame {
                frame_type: FrameType::VIDEO,
                timestamp: ts,
                codec: Codec::Vmx1 as i32,
                width,
                height,
                frame_rate_n: frame_n,
                frame_rate_d: frame_d,
                aspect_ratio: 16.0 / 9.0,
                data: payload.to_vec(),
                ..Default::default()
            };
            if sender.send_video(frame).is_ok() {
                sent += 1;
            }
            let _ = sender.poll_accept();
            let _ = sender.poll_peer_metadata();
            next_send += frame_period;
            if next_send < Instant::now() {
                // Catch up without bursting unbounded.
                next_send = Instant::now();
            }
        }

        let t0 = Instant::now();
        if let Some(f) = session.try_recv_video() {
            assert_eq!(f.width, width as u32);
            assert_eq!(f.height, height as u32);
            assert_eq!(f.pixels.len(), (width * height * 4) as usize);
            times_us.push(t0.elapsed().as_micros() as u64);
            received += 1;
        } else {
            thread::sleep(Duration::from_micros(200));
        }
    }

    let elapsed = start.elapsed().as_secs_f64().max(1e-6);
    let fps = received as f64 / elapsed;
    let stats = session.statistics();
    times_us.sort_unstable();
    let p99 = percentile_us(&times_us, 0.99);
    let decode_p99_ms = if stats.frames_decoded > 0 {
        (stats.codec_time_ns_peak as f64) / 1_000_000.0
    } else {
        f64::NAN
    };

    eprintln!(
        "soak {secs}s: sent={sent} recv={received} fps={fps:.2} try_recv_p99_us={p99} \
         decode_peak_ms={decode_p99_ms:.3} drops_wire={} drops_decode={} reconnects={} age_peak_us={}",
        stats.frames_dropped_wire,
        stats.frames_dropped_decode,
        stats.reconnects,
        stats.frame_age_us_peak
    );

    session.disconnect();

    assert!(received > 0, "no frames received");
    // Soft gate for short CI soaks; full 600s gate is release-only below.
    // Debug builds are not performance-gated (decode is ~10× slower without LTO).
    let release = cfg!(not(debug_assertions));
    if secs >= 600 && release {
        assert!(fps >= 59.0, "10-minute soak fps {fps:.2} below 59.0");
        assert!(
            stats.frames_dropped_wire == 0 && stats.frames_dropped_decode == 0,
            "unexpected drops under normal load: {stats:?}"
        );
        assert!(
            decode_p99_ms <= 12.0,
            "decode peak {decode_p99_ms:.3} ms > 12 ms"
        );
    } else if release {
        assert!(
            fps >= 50.0,
            "short release soak fps unexpectedly low: {fps:.2}"
        );
    } else {
        assert!(
            received >= 10,
            "debug soak received too few frames: {received}"
        );
    }
}

fn percentile_us(sorted: &[u64], q: f64) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let idx = ((sorted.len() as f64 - 1.0) * q).round() as usize;
    sorted[idx.min(sorted.len() - 1)]
}
