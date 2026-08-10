//! Receive live OMT video (e.g. from vMix) and dump decoded frames as JPEG.

use std::env;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use image::{ImageBuffer, Rgb};
use openmediatransport::{Discovery, FrameType, ReceiverConfig, ReceiverSession};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = env::args().skip(1);
    let url_arg = args.next();
    let out_dir = args
        .next()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("received_frames"));
    let max_frames: usize = args.next().and_then(|s| s.parse().ok()).unwrap_or(5);

    std::fs::create_dir_all(&out_dir)?;

    let (url, addresses) = match url_arg {
        Some(u) if u.starts_with("omt://") => (u, Vec::new()),
        Some(other) => {
            eprintln!("Unexpected argument (expected omt:// URL): {other}");
            std::process::exit(2);
        }
        None => discover_first()?,
    };

    println!("Connecting to {url}");
    let session = ReceiverSession::connect_with_addresses(
        &url,
        &addresses,
        ReceiverConfig {
            frame_types: FrameType::VIDEO,
            connect_timeout: Duration::from_secs(5),
            ..ReceiverConfig::default()
        },
    )?;
    println!("Connected; waiting for video frames…");

    let mut saved = 0usize;
    let mut attempts = 0usize;
    let deadline = Instant::now() + Duration::from_secs(30);

    while saved < max_frames && Instant::now() < deadline {
        attempts += 1;
        match session.recv_video_timeout(Duration::from_millis(500)) {
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
                rgb.save(&path)?;
                println!("  wrote {}", path.display());
                saved += 1;
            }
            None => {}
        }
    }

    let stats = session.statistics();
    println!(
        "Done: saved={saved}/{max_frames} attempts={attempts} bytes_received={} decoded={} drops_wire={} drops_decode={}",
        stats.bytes_received,
        stats.frames_decoded,
        stats.frames_dropped_wire,
        stats.frames_dropped_decode
    );
    session.disconnect();
    if saved == 0 {
        Err("no decoded BGRA frames were saved".into())
    } else {
        Ok(())
    }
}

fn discover_first() -> Result<(String, Vec<String>), Box<dyn std::error::Error>> {
    let mut discovery = Discovery::new()?;
    discovery.refresh_for(Duration::from_secs(3))?;
    let sources = discovery.sources();
    println!("Discovered {} source(s)", sources.len());
    for src in sources {
        println!(
            "  - {} port={} url={} addrs={:?}",
            src.instance_name(),
            src.port,
            src.to_url(),
            src.addresses
        );
    }
    let first = sources.first().ok_or("no OMT sources discovered")?;
    Ok((first.to_url(), first.addresses.clone()))
}

fn bgra_to_rgb(
    bgra: &[u8],
    width: u32,
    height: u32,
) -> Result<ImageBuffer<Rgb<u8>, Vec<u8>>, Box<dyn std::error::Error>> {
    let expected = (width as usize) * 4 * (height as usize);
    if bgra.len() < expected {
        return Err(format!("BGRA buffer too small: {} < {expected}", bgra.len()).into());
    }
    let mut rgb = Vec::with_capacity((width as usize) * 3 * (height as usize));
    for px in bgra[..expected].chunks_exact(4) {
        let (b, g, r, _a) = (px[0], px[1], px[2], px[3]);
        rgb.extend_from_slice(&[r, g, b]);
    }
    ImageBuffer::from_raw(width, height, rgb).ok_or_else(|| "failed to build RGB image".into())
}
