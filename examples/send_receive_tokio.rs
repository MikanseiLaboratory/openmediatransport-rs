//! Tokio send/receive example (`--features tokio`).

#[cfg(feature = "tokio")]
#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    use openmediatransport::async_api::{AsyncReceiver, AsyncSender};
    use openmediatransport::{FrameType, MediaFrame};

    let mut sender =
        AsyncSender::create("TokioSrc", FrameType::VIDEO | FrameType::METADATA).await?;
    let port = sender.port();
    let mut receiver =
        AsyncReceiver::create(format!("omt://127.0.0.1:{port}"), FrameType::VIDEO).await?;
    receiver.connect().await?;

    for _ in 0..50 {
        let _ = sender.poll_accept().await?;
        sender.poll_peer_metadata().await?;
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    sender.force_subscribe(true, false, true);

    let frame = MediaFrame {
        frame_type: FrameType::VIDEO,
        timestamp: 1,
        codec: openmediatransport::Codec::Uyvy as i32,
        width: 64,
        height: 64,
        data: vec![9, 8, 7, 6],
        frame_rate_n: 60,
        frame_rate_d: 1,
        aspect_ratio: 1.0,
        ..Default::default()
    };
    let _ = sender.send_video(frame).await;
    if let Ok(Some(rx)) = receiver.receive(500).await {
        println!("async received {} bytes", rx.data.len());
    } else {
        println!("async: no frame (subscribe timing) — API exercised");
    }
    Ok(())
}

#[cfg(not(feature = "tokio"))]
fn main() {
    eprintln!("re-run with: cargo run --example send_receive_tokio --features tokio");
}
