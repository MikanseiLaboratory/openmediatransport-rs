//! Tokio send/receive example (`--features tokio`).

#[cfg(feature = "tokio")]
#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    use openmediatransport::async_api::{AsyncReceiver, AsyncSender};
    use openmediatransport::{Codec, FrameType, MediaFrame};
    use vmx::{Codec as VmxCodec, Config as VmxConfig, Profile};

    let mut sender =
        AsyncSender::create("TokioSrc", FrameType::VIDEO | FrameType::METADATA).await?;
    let port = sender.port();
    let mut receiver =
        AsyncReceiver::connect(format!("omt://127.0.0.1:{port}"), FrameType::VIDEO).await?;

    for _ in 0..50 {
        let _ = sender.poll_accept().await?;
        sender.poll_peer_metadata().await?;
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    sender.force_subscribe(true, false, true);

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
        timestamp: 1,
        codec: Codec::Vmx1 as i32,
        width,
        height,
        data: bitstream[..len].to_vec(),
        frame_rate_n: 60,
        frame_rate_d: 1,
        aspect_ratio: 1.0,
        ..Default::default()
    };
    let _ = sender.send_video(frame).await;
    if let Some(rx) = receiver.recv_video(1000).await {
        println!("async received BGRA {} bytes", rx.pixels.len());
    } else {
        println!("async: no frame within timeout");
    }
    Ok(())
}

#[cfg(not(feature = "tokio"))]
fn main() {
    eprintln!("re-run with: cargo run --example send_receive_tokio --features tokio");
}
