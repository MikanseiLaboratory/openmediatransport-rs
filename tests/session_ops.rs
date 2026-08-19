//! Session lifecycle: shutdown join, stats, and reconnect interrupt.

use openmediatransport::{Codec, FrameType, MediaFrame, ReceiverConfig, ReceiverSession, Sender};
use std::thread;
use std::time::{Duration, Instant};
use vmx::{Codec as VmxCodec, Config as VmxConfig, Profile};

fn wait_subscribed(sender: &mut Sender) {
    for _ in 0..200 {
        let _ = sender.poll_accept();
        let _ = sender.poll_peer_metadata();
        if sender.video_subscribed() && sender.connection_count() > 0 {
            return;
        }
        thread::sleep(Duration::from_millis(10));
    }
    sender.force_subscribe(true, false, false);
    // Ensure at least one accepted peer before sending (Windows CI is slower).
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        let _ = sender.poll_accept();
        let _ = sender.poll_peer_metadata();
        if sender.connection_count() > 0 {
            return;
        }
        thread::sleep(Duration::from_millis(10));
    }
    panic!(
        "sender never accepted a peer; connections={}",
        sender.connection_count()
    );
}

fn encode_pattern(width: i32, height: i32) -> Vec<u8> {
    let stride = (width as usize) * 2;
    let mut uyvy = vec![128u8; stride * height as usize];
    for y in 0..height as usize {
        for x in (0..width as usize).step_by(2) {
            let o = y * stride + x * 2;
            uyvy[o] = 128;
            uyvy[o + 1] = 16 + ((x + y) % 220) as u8;
            uyvy[o + 2] = 128;
            uyvy[o + 3] = 16 + ((x + 1 + y) % 220) as u8;
        }
    }
    let mut enc = VmxCodec::new(VmxConfig {
        width,
        height,
        profile: Profile::OmtLq,
        color_space: Default::default(),
    })
    .unwrap();
    enc.encode_uyvy(&uyvy, stride).unwrap();
    let mut bitstream = vec![0u8; 2 << 20];
    let len = enc.save_to(&mut bitstream).unwrap();
    bitstream.truncate(len);
    bitstream
}

#[test]
fn disconnect_joins_without_hang() {
    let mut sender = Sender::create("ShutSrc", FrameType::VIDEO).unwrap();
    let url = format!("omt://127.0.0.1:{}", sender.port());
    let session = ReceiverSession::connect(
        url,
        ReceiverConfig {
            frame_types: FrameType::VIDEO,
            auto_reconnect: false,
            connect_timeout: Duration::from_secs(2),
            ..ReceiverConfig::default()
        },
    )
    .unwrap();
    wait_subscribed(&mut sender);
    let t0 = Instant::now();
    session.disconnect();
    assert!(t0.elapsed() < Duration::from_secs(3), "disconnect hung");
}

#[test]
fn latest_wins_under_slow_consumer() {
    let mut sender = Sender::create("LatestSrc", FrameType::VIDEO).unwrap();
    let url = format!("omt://127.0.0.1:{}", sender.port());
    let session = ReceiverSession::connect(
        url,
        ReceiverConfig {
            frame_types: FrameType::VIDEO,
            auto_reconnect: false,
            connect_timeout: Duration::from_secs(2),
            ..ReceiverConfig::default()
        },
    )
    .unwrap();
    wait_subscribed(&mut sender);

    let payload = encode_pattern(128, 128);
    for i in 0..30 {
        let _ = sender.poll_accept();
        let _ = sender.poll_peer_metadata();
        let frame = MediaFrame {
            frame_type: FrameType::VIDEO,
            timestamp: (i + 1) * 10_000_000,
            codec: Codec::Vmx1 as i32,
            width: 128,
            height: 128,
            frame_rate_n: 60_000,
            frame_rate_d: 1_001,
            aspect_ratio: 16.0 / 9.0,
            data: payload.clone(),
            ..Default::default()
        };
        sender.send_video(frame).expect("send");
        // Intentionally do not drain video on the consumer yet.
        thread::sleep(Duration::from_millis(5));
    }

    // Keep accepting while waiting for decode (peer metadata / ACKs).
    let deadline = Instant::now() + Duration::from_secs(8);
    let mut frame = None;
    while Instant::now() < deadline {
        let _ = sender.poll_accept();
        let _ = sender.poll_peer_metadata();
        if let Some(f) = session.try_recv_video() {
            frame = Some(f);
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    let frame = frame.unwrap_or_else(|| {
        panic!(
            "decoded frame missing; stats={:?} err={:?} state={:?} connections={}",
            session.statistics(),
            session.last_error(),
            session.state(),
            sender.connection_count()
        )
    });
    assert!(frame.timestamp > 0);
    let stats = session.statistics();
    assert!(stats.frames_decoded >= 1, "stats={stats:?}");
    session.disconnect();
}

#[test]
fn shutdown_interrupts_reconnect_wait() {
    // Connect to a closed port — auto_reconnect would spin; disconnect must return quickly.
    let session = ReceiverSession::connect(
        "omt://127.0.0.1:1",
        ReceiverConfig {
            frame_types: FrameType::VIDEO,
            auto_reconnect: true,
            connect_timeout: Duration::from_millis(200),
            ..ReceiverConfig::default()
        },
    );
    // Port 1 should refuse quickly.
    assert!(session.is_err());
}

#[test]
fn second_session_receives_video_after_disconnect() {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    let sender = Arc::new(Mutex::new(
        Sender::create("ReconnectSrc", FrameType::VIDEO).unwrap(),
    ));
    let port = sender.lock().unwrap().port();
    let payload = encode_pattern(64, 64);
    let running = Arc::new(AtomicBool::new(true));
    let pump_sender = Arc::clone(&sender);
    let pump_running = Arc::clone(&running);
    let pump_payload = payload;
    let pump = thread::spawn(move || {
        let mut ts = 1i64;
        while pump_running.load(Ordering::Relaxed) {
            {
                let mut s = pump_sender.lock().unwrap();
                let _ = s.poll_accept();
                let _ = s.poll_peer_metadata();
                if s.video_subscribed() {
                    let frame = MediaFrame {
                        frame_type: FrameType::VIDEO,
                        timestamp: ts * 10_000_000,
                        codec: Codec::Vmx1 as i32,
                        width: 64,
                        height: 64,
                        frame_rate_n: 60,
                        frame_rate_d: 1,
                        aspect_ratio: 1.0,
                        data: pump_payload.clone(),
                        ..Default::default()
                    };
                    let _ = s.send_video(frame);
                    ts += 1;
                }
            }
            thread::sleep(Duration::from_millis(5));
        }
    });

    fn wait_frame(session: &ReceiverSession, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if session.try_recv_video().is_some() {
                return true;
            }
            thread::sleep(Duration::from_millis(5));
        }
        false
    }

    let cfg = ReceiverConfig {
        frame_types: FrameType::VIDEO,
        auto_reconnect: false,
        connect_timeout: Duration::from_secs(2),
        ..ReceiverConfig::default()
    };
    let url = format!("omt://127.0.0.1:{port}");

    let first = ReceiverSession::connect(&url, cfg.clone()).unwrap();
    assert!(
        wait_frame(&first, Duration::from_secs(3)),
        "first session got no video; err={:?} stats={:?}",
        first.last_error(),
        first.statistics()
    );
    first.disconnect();

    // Immediate re-select, matching Studio Monitor source switching.
    let second = ReceiverSession::connect(&url, cfg.clone()).unwrap();
    assert!(
        wait_frame(&second, Duration::from_secs(3)),
        "second session got no video; err={:?} stats={:?} state={:?} connections={}",
        second.last_error(),
        second.statistics(),
        second.state(),
        sender.lock().unwrap().connection_count()
    );
    second.disconnect();

    let third = ReceiverSession::connect(&url, cfg).unwrap();
    assert!(
        wait_frame(&third, Duration::from_secs(3)),
        "third session got no video; err={:?} stats={:?}",
        third.last_error(),
        third.statistics()
    );
    third.disconnect();

    running.store(false, Ordering::Relaxed);
    let _ = pump.join();
}
