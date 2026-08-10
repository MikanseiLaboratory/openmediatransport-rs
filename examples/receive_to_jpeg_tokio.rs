//! Tokio receive-to-JPEG example (`--features tokio`).

#[cfg(feature = "tokio")]
#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    use std::env;
    use std::path::PathBuf;
    use std::time::{Duration, Instant};

    use openmediatransport::async_api::AsyncReceiver;
    use openmediatransport::FrameType;

    let mut args = env::args().skip(1);
    let url_arg = args.next();
    let out_dir = args
        .next()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("received_frames_tokio"));
    let max_frames: usize = args.next().and_then(|s| s.parse().ok()).unwrap_or(5);

    std::fs::create_dir_all(&out_dir)?;

    let url = match url_arg {
        Some(u) if u.starts_with("omt://") => u,
        Some(other) => {
            eprintln!("Unexpected argument (expected omt:// URL): {other}");
            std::process::exit(2);
        }
        None => tokio::task::spawn_blocking(discover_first_url)
            .await?
            .map_err(|e| -> Box<dyn std::error::Error> { e })?,
    };

    println!("Connecting to {url}");
    let mut receiver = AsyncReceiver::connect(&url, FrameType::VIDEO).await?;
    println!("Connected; waiting for video frames…");

    let mut saved = 0usize;
    let mut attempts = 0usize;
    let deadline = Instant::now() + Duration::from_secs(30);

    while saved < max_frames && Instant::now() < deadline {
        attempts += 1;
        match receiver.recv_video(500).await {
            Some(frame) => {
                println!(
                    "frame#{attempts}: {}x{} pixels={} ts={}",
                    frame.width,
                    frame.height,
                    frame.pixels.len(),
                    frame.timestamp
                );
                let rgb = bgra_to_rgb(&frame.pixels, frame.width, frame.height)?;
                let path = out_dir.join(format!("frame_{saved:03}.jpg"));
                let path_clone = path.clone();
                tokio::task::spawn_blocking(move || rgb.save(&path_clone)).await??;
                println!("  wrote {}", path.display());
                saved += 1;
            }
            None => {}
        }
    }

    let stats = receiver.statistics();
    println!(
        "Done: saved={saved}/{max_frames} attempts={attempts} bytes_received={} decoded={}",
        stats.bytes_received, stats.frames_decoded
    );
    if saved == 0 {
        Err("no decoded BGRA frames were saved".into())
    } else {
        Ok(())
    }
}

#[cfg(feature = "tokio")]
fn discover_first_url() -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    use std::time::Duration;

    use openmediatransport::Discovery;

    let mut discovery = Discovery::new()?;
    discovery.refresh_for(Duration::from_secs(3))?;
    let sources = discovery.sources();
    println!("Discovered {} source(s)", sources.len());
    for src in sources {
        println!(
            "  - {} port={} url={}",
            src.instance_name(),
            src.port,
            src.to_url()
        );
    }
    let first = sources.first().ok_or("no OMT sources discovered")?;
    Ok(first.to_url())
}

#[cfg(feature = "tokio")]
fn bgra_to_rgb(
    bgra: &[u8],
    width: u32,
    height: u32,
) -> Result<image::ImageBuffer<image::Rgb<u8>, Vec<u8>>, Box<dyn std::error::Error>> {
    let expected = (width as usize) * 4 * (height as usize);
    if bgra.len() < expected {
        return Err(format!("BGRA buffer too small: {} < {expected}", bgra.len()).into());
    }
    let mut rgb = Vec::with_capacity((width as usize) * 3 * (height as usize));
    for px in bgra[..expected].chunks_exact(4) {
        let (b, g, r, _a) = (px[0], px[1], px[2], px[3]);
        rgb.extend_from_slice(&[r, g, b]);
    }
    image::ImageBuffer::<image::Rgb<u8>, _>::from_raw(width, height, rgb)
        .ok_or_else(|| "failed to build RGB image".into())
}

#[cfg(not(feature = "tokio"))]
fn main() {
    eprintln!("re-run with: cargo run --example receive_to_jpeg_tokio --features tokio");
}
