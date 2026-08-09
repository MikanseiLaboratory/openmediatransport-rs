//! Basic sync send/receive loopback over localhost.

use openmediatransport::{FrameType, MediaFrame, Receiver, Sender};
use std::thread;
use std::time::Duration;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut sender = Sender::create("Loopback", FrameType::VIDEO | FrameType::METADATA)?;
    let port = sender.port();
    println!("sender listening on port {port}");

    let addr = format!("omt://127.0.0.1:{port}");
    let mut receiver = Receiver::create(&addr, FrameType::VIDEO | FrameType::METADATA)?;
    receiver.connect(Some(Duration::from_secs(2)))?;

    // Allow accept + subscribe
    for _ in 0..50 {
        let _ = sender.poll_accept()?;
        let _ = sender.poll_peer_metadata()?;
        if sender.video_subscribed() {
            break;
        }
        thread::sleep(Duration::from_millis(20));
    }
    println!("video subscribed: {}", sender.video_subscribed());

    let frame = MediaFrame {
        frame_type: FrameType::VIDEO,
        timestamp: 0,
        codec: openmediatransport::Codec::Vmx1 as i32,
        width: 64,
        height: 64,
        frame_rate_n: 60,
        frame_rate_d: 1,
        aspect_ratio: 1.0,
        data: vec![0u8; 64], // placeholder compressed payload for framing test
        ..Default::default()
    };
    sender.send_video(frame)?;

    if let Some(rx) = receiver.receive(1000)? {
        println!(
            "received frame type={} bytes={}",
            rx.frame_type.0,
            rx.data.len()
        );
    } else {
        println!("no frame received within timeout");
    }
    Ok(())
}
