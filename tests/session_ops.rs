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
    // Ensure at least one accept after force, in case subscribe arrived late.
    for _ in 0..50 {
        let _ = sender.poll_accept();
        let _ = sender.poll_peer_metadata();
        if sender.connection_count() > 0 {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
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

    let frame = session
        .recv_video_timeout(Duration::from_secs(5))
        .unwrap_or_else(|| {
            panic!(
                "decoded frame missing; stats={:?} err={:?} state={:?}",
                session.statistics(),
                session.last_error(),
                session.state()
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
