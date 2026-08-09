//! Receive live OMT video (e.g. from vMix) and dump decoded frames as JPEG.

use std::env;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use image::{ImageBuffer, Rgb};
use openmediatransport::{Codec, Discovery, FrameType, PreferredVideoFormat, Receiver};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = env::args().skip(1);
    let url_arg = args.next();
    let out_dir = args
        .next()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("received_frames"));
    let max_frames: usize = args.next().and_then(|s| s.parse().ok()).unwrap_or(5);

    std::fs::create_dir_all(&out_dir)?;

    let url = match url_arg {
        Some(u) if u.starts_with("omt://") => u,
        Some(other) => {
            eprintln!("Unexpected argument (expected omt:// URL): {other}");
            std::process::exit(2);
        }
        None => discover_first_url()?,
    };

    println!("Connecting to {url}");
    let mut receiver = Receiver::create(&url, FrameType::VIDEO)?;
    receiver.set_preferred_format(PreferredVideoFormat::Bgra);
    receiver.connect(Some(Duration::from_secs(5)))?;
    println!("Connected; waiting for video frames…");

    let mut saved = 0usize;
    let mut attempts = 0usize;
    let deadline = Instant::now() + Duration::from_secs(30);

    while saved < max_frames && Instant::now() < deadline {
        attempts += 1;
        match receiver.receive(500)? {
            Some(frame) if frame.frame_type.contains(FrameType::VIDEO) => {
                let m = &frame.media;
                let codec = Codec::from_i32(m.codec);
                println!(
                    "frame#{attempts}: {}x{} codec={:?} data_len={} ts={}",
                    m.width,
                    m.height,
                    codec,
                    frame.data.len(),
                    frame.timestamp
                );

                let expected_bgra = (m.width as usize) * 4 * (m.height as usize);
                if codec != Some(Codec::Bgra) || frame.data.len() != expected_bgra {
                    eprintln!(
                        "  skip: expected decoded BGRA ({expected_bgra} bytes), got codec={:?} len={}",
                        codec,
                        frame.data.len()
                    );
                    // Still dump compressed / unexpected payload for diagnosis once.
                    if saved == 0 {
                        let raw_path = out_dir.join(format!("frame_{saved:03}_raw.bin"));
                        std::fs::write(&raw_path, &frame.data)?;
                        eprintln!("  wrote raw payload to {}", raw_path.display());
                    }
                    continue;
                }

                let rgb = bgra_to_rgb(&frame.data, m.width as u32, m.height as u32)?;
                let path = out_dir.join(format!("frame_{saved:03}.jpg"));
                rgb.save(&path)?;
                println!("  wrote {}", path.display());
                saved += 1;
            }
            Some(frame) => {
                let meta = frame.metadata.as_deref().unwrap_or("");
                let data_preview = String::from_utf8_lossy(&frame.data);
                println!(
                    "frame#{attempts}: non-video type={} len={} data={:?} meta={meta:?}",
                    frame.frame_type.0,
                    frame.data.len(),
                    data_preview.chars().take(80).collect::<String>()
                );
            }
            None => {}
        }
    }

    let stats = receiver.statistics();
    println!(
        "Done: saved={saved}/{max_frames} attempts={attempts} bytes_received={} frames={}",
        stats.bytes_received, stats.frames
    );
    if saved == 0 {
        Err("no decoded BGRA frames were saved".into())
    } else {
        Ok(())
    }
}

fn discover_first_url() -> Result<String, Box<dyn std::error::Error>> {
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
