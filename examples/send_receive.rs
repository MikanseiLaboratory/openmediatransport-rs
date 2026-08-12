//! Basic sync send/receive loopback over localhost (UYVY → VMX1 → BGRA).

use openmediatransport::{Codec, FrameType, MediaFrame, ReceiverConfig, ReceiverSession, Sender};
use std::thread;
use std::time::Duration;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut sender = Sender::create("Loopback", FrameType::VIDEO | FrameType::METADATA)?;
    let port = sender.port();
    println!("sender listening on port {port}");

    let addr = format!("omt://127.0.0.1:{port}");
    let session = ReceiverSession::connect(
        &addr,
        ReceiverConfig {
            frame_types: FrameType::VIDEO | FrameType::METADATA,
            connect_timeout: Duration::from_secs(2),
            ..ReceiverConfig::default()
        },
    )?;

    for _ in 0..50 {
        let _ = sender.poll_accept()?;
        sender.poll_peer_metadata()?;
        if sender.video_subscribed() {
            break;
        }
        thread::sleep(Duration::from_millis(20));
    }
    if !sender.video_subscribed() {
        sender.force_subscribe(true, false, true);
    }
    println!("video subscribed: {}", sender.video_subscribed());

    let width = 64i32;
    let height = 64i32;
    let stride = width * 2;
    let uyvy = vec![128u8; (stride * height) as usize];

    let frame = MediaFrame {
        frame_type: FrameType::VIDEO,
        timestamp: 0,
        codec: Codec::Uyvy as i32,
        width,
        height,
        stride,
        frame_rate_n: 60,
        frame_rate_d: 1,
        aspect_ratio: 1.0,
        data: uyvy,
        ..Default::default()
    };
    sender.send_video(frame)?;

    if let Some(rx) = session.recv_video_timeout(Duration::from_secs(2)) {
        println!(
            "received BGRA {}x{} bytes={}",
            rx.width,
            rx.height,
            rx.pixels.len()
        );
    } else {
        println!("no frame received within timeout");
    }
    session.disconnect();
    Ok(())
}
