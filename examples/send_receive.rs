//! Basic sync send/receive loopback over localhost (VMX1 → BGRA).

use openmediatransport::{Codec, FrameType, MediaFrame, ReceiverConfig, ReceiverSession, Sender};
use std::thread;
use std::time::Duration;
use vmx::{Codec as VmxCodec, Config as VmxConfig, Profile};

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
    let stride = (width as usize) * 2;
    let uyvy = vec![128u8; stride * height as usize];
    let mut enc = VmxCodec::new(VmxConfig {
        width,
        height,
        profile: Profile::OmtLq,
        color_space: Default::default(),
    })?;
    enc.encode_uyvy(&uyvy, stride)?;
    let mut bitstream = vec![0u8; 1 << 20];
    let len = enc.save_to(&mut bitstream)?;

    let frame = MediaFrame {
        frame_type: FrameType::VIDEO,
        timestamp: 0,
        codec: Codec::Vmx1 as i32,
        width,
        height,
        frame_rate_n: 60,
        frame_rate_d: 1,
        aspect_ratio: 1.0,
        data: bitstream[..len].to_vec(),
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
