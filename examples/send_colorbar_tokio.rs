//! Continuously send SMPTE-style color bars + sine tone over OMT (tokio).
//!
//! Prefer `--release`. Debug builds of VMX encode are far too slow for realtime 1080p.
//!
//! Audio runs on a dedicated OS thread with wall-clock pacing and explicit timestamps.
//!
//! ```text
//! cargo run --release --example send_colorbar_tokio --features tokio -- [name] [options]
//!   (same flags as send_colorbar)
//! ```

#[cfg(feature = "tokio")]
#[path = "common/colorbar.rs"]
mod colorbar;

#[cfg(feature = "tokio")]
#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use std::thread;
    use std::time::{Duration, Instant};

    use colorbar::{SendOptions, append_sine_planar, fill_colorbar_uyvy};
    use openmediatransport::{
        Codec, ColorSpace, Discovery, FrameType, MediaFrame, Sender, SenderConfig, SenderInfo,
    };
    use vmx::{Codec as VmxCodec, Config as VmxConfig};

    const TICKS_PER_SECOND: i64 = 10_000_000;

    let opts = SendOptions::from_args(std::env::args().skip(1));
    let config = SenderConfig {
        send_buffer: opts.send_buffer,
        recv_buffer: opts.recv_buffer,
        send_queue_depth: opts.send_queue,
    };

    let sender = Arc::new(Mutex::new(Sender::create_with_config(
        &opts.name,
        FrameType::VIDEO | FrameType::AUDIO | FrameType::METADATA,
        config,
    )?));
    {
        let mut s = sender.lock().unwrap();
        s.set_sender_info(SenderInfo::new(
            "openmediatransport-rs",
            "MikanseiLaboratory",
            env!("CARGO_PKG_VERSION"),
        ));
    }

    let port = sender.lock().unwrap().port();
    {
        let name = opts.name.clone();
        tokio::task::spawn_blocking(move || {
            let mut discovery = Discovery::new()?;
            discovery.register(&name, port)?;
            Ok::<(), openmediatransport::OmtError>(())
        })
        .await??;
    }

    println!(
        "Sending '{}' on port {} ({}x{} @ {}/{} fps, tone {} Hz, audio_samples={}, animate={}, profile={}, queue={})",
        opts.name,
        port,
        opts.width,
        opts.height,
        opts.fps_n,
        opts.fps_d,
        opts.tone_hz,
        opts.audio_samples,
        opts.animate,
        opts.profile_name(),
        opts.send_queue
    );
    println!("Discoverable as omt://<host>:{port}/{}", opts.name);
    if cfg!(debug_assertions) {
        eprintln!("warning: debug build — use --release for realtime encode/send");
    }

    let epoch = Instant::now();
    let running = Arc::new(AtomicBool::new(true));
    let audio_running = Arc::clone(&running);
    let audio_sender = Arc::clone(&sender);
    let audio_opts = opts.clone();
    thread::Builder::new()
        .name("omt-colorbar-audio".into())
        .spawn(move || {
            let mut audio_buf = Vec::new();
            let mut audio_phase = 0.0f64;
            let samples = audio_opts.audio_samples.max(1) as i64;
            let rate = audio_opts.sample_rate.max(1) as i64;
            let mut samples_sent = 0i64;

            while audio_running.load(Ordering::Relaxed) {
                let want = audio_sender
                    .lock()
                    .map(|s| s.audio_subscribed())
                    .unwrap_or(false);
                if !want {
                    thread::sleep(Duration::from_millis(10));
                    continue;
                }

                let due = Duration::from_secs_f64(samples_sent as f64 / rate as f64);
                let now = epoch.elapsed();
                if due > now {
                    thread::sleep(due - now);
                } else if now > due + Duration::from_millis(50) {
                    samples_sent = (now.as_secs_f64() * rate as f64).round() as i64;
                    samples_sent -= samples_sent % samples;
                }

                audio_buf.clear();
                append_sine_planar(
                    &mut audio_buf,
                    audio_opts.channels,
                    samples as i32,
                    audio_opts.sample_rate,
                    audio_opts.tone_hz,
                    &mut audio_phase,
                );
                let timestamp = samples_sent.saturating_mul(TICKS_PER_SECOND) / rate;
                samples_sent += samples;

                let frame = MediaFrame {
                    frame_type: FrameType::AUDIO,
                    timestamp,
                    codec: Codec::Fpa1 as i32,
                    sample_rate: audio_opts.sample_rate,
                    channels: audio_opts.channels,
                    samples_per_channel: samples as i32,
                    active_channels: 0,
                    data: std::mem::take(&mut audio_buf),
                    ..Default::default()
                };
                if let Ok(mut s) = audio_sender.lock() {
                    let _ = s.send_audio(frame);
                }
            }
        })?;

    let mut vmx = VmxCodec::new(VmxConfig {
        width: opts.width,
        height: opts.height,
        profile: opts.profile,
        color_space: vmx::ColorSpace::Undefined,
    })?;

    let stride = (opts.width as usize) * 2;
    let mut uyvy = vec![0u8; stride * opts.height as usize];
    let mut vmx_buf = vec![0u8; 8 << 20];
    let mut send_payload = Vec::with_capacity(1 << 20);
    let mut cached_vmx: Option<Vec<u8>> = None;
    let mut frame_idx = 0u64;
    let mut last_stats = Instant::now();
    let mut encode_us_acc = 0u64;
    let mut video_sent = 0u64;
    let video_interval =
        (opts.fps_d as i64).saturating_mul(TICKS_PER_SECOND) / opts.fps_n.max(1) as i64;

    if !opts.animate {
        fill_colorbar_uyvy(&mut uyvy, opts.width, opts.height, 0.0);
        vmx.encode_uyvy(&uyvy, stride)?;
        let n = vmx.save_to(&mut vmx_buf)?;
        cached_vmx = Some(vmx_buf[..n].to_vec());
    }

    loop {
        let video_ok = {
            let mut s = sender.lock().unwrap();
            let _ = s.poll_accept()?;
            s.poll_peer_metadata()?;
            s.video_subscribed()
        };
        if !video_ok {
            tokio::time::sleep(Duration::from_millis(5)).await;
            continue;
        }

        let target = Duration::from_secs_f64(frame_idx as f64 / opts.fps());
        let now = epoch.elapsed();
        if target > now {
            tokio::time::sleep(target - now).await;
        } else if now > target + Duration::from_secs_f64(2.0 / opts.fps()) {
            frame_idx = (now.as_secs_f64() * opts.fps()).floor() as u64;
        }

        let t0 = Instant::now();
        let payload = if let Some(cached) = cached_vmx.as_ref() {
            cached.clone()
        } else {
            // Keep VMX encode off the async cooperative budget.
            tokio::task::block_in_place(|| {
                let phase = ((frame_idx % 300) as f32) / 300.0;
                fill_colorbar_uyvy(&mut uyvy, opts.width, opts.height, phase);
                vmx.encode_uyvy(&uyvy, stride)?;
                let n = vmx.save_to(&mut vmx_buf)?;
                send_payload.clear();
                send_payload.extend_from_slice(&vmx_buf[..n]);
                Ok::<_, Box<dyn std::error::Error>>(send_payload.clone())
            })?
        };
        encode_us_acc += t0.elapsed().as_micros() as u64;

        let timestamp = (frame_idx as i64).saturating_mul(video_interval);
        let frame = MediaFrame {
            frame_type: FrameType::VIDEO,
            timestamp,
            codec: Codec::Vmx1 as i32,
            width: opts.width,
            height: opts.height,
            frame_rate_n: opts.fps_n,
            frame_rate_d: opts.fps_d,
            aspect_ratio: opts.width as f32 / opts.height.max(1) as f32,
            color_space: ColorSpace::Undefined,
            data: payload,
            ..Default::default()
        };
        {
            let mut s = sender.lock().unwrap();
            s.send_video(frame)?;
        }
        video_sent += 1;
        frame_idx += 1;

        if last_stats.elapsed() >= Duration::from_secs(1) {
            let st = sender.lock().unwrap().statistics();
            let avg_ms = if video_sent > 0 {
                (encode_us_acc as f64 / video_sent as f64) / 1000.0
            } else {
                0.0
            };
            println!(
                "stats: video_fps≈{video_sent}/s encode_avg={avg_ms:.1}ms frames={} dropped={}",
                st.frames, st.frames_dropped
            );
            encode_us_acc = 0;
            video_sent = 0;
            last_stats = Instant::now();
        }

        tokio::task::yield_now().await;
    }
}

#[cfg(not(feature = "tokio"))]
fn main() {
    eprintln!("re-run with: cargo run --release --example send_colorbar_tokio --features tokio");
}
