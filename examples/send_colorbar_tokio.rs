//! Continuously send SMPTE-style color bars + sine tone over OMT (tokio).
//!
//! ```text
//! cargo run --example send_colorbar_tokio --features tokio -- [name] [options]
//! ```

#[cfg(feature = "tokio")]
#[path = "common/colorbar.rs"]
mod colorbar;

#[cfg(feature = "tokio")]
#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    use std::time::{Duration, Instant};

    use colorbar::{SendOptions, append_sine_planar, fill_colorbar_uyvy};
    use openmediatransport::async_api::AsyncSender;
    use openmediatransport::{Codec, ColorSpace, Discovery, FrameType, MediaFrame, SenderInfo};
    use vmx::{Codec as VmxCodec, Config as VmxConfig, Profile};

    let opts = SendOptions::from_args(std::env::args().skip(1));

    let mut sender = AsyncSender::create(
        &opts.name,
        FrameType::VIDEO | FrameType::AUDIO | FrameType::METADATA,
    )
    .await?;
    sender.set_sender_info(SenderInfo::new(
        "openmediatransport-rs",
        "MikanseiLaboratory",
        env!("CARGO_PKG_VERSION"),
    ));

    let port = sender.port();
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
        "Sending '{}' on port {} ({}x{} @ {}/{} fps, tone {} Hz, animate={})",
        opts.name,
        port,
        opts.width,
        opts.height,
        opts.fps_n,
        opts.fps_d,
        opts.tone_hz,
        opts.animate
    );
    println!("Discoverable as omt://<host>:{port}/{}", opts.name);

    let mut vmx = VmxCodec::new(VmxConfig {
        width: opts.width,
        height: opts.height,
        profile: Profile::OmtHq,
        color_space: vmx::ColorSpace::Undefined,
    })?;

    let stride = (opts.width as usize) * 2;
    let mut uyvy = vec![0u8; stride * opts.height as usize];
    let mut vmx_buf = vec![0u8; 8 << 20];
    let mut audio_buf = Vec::new();
    let mut audio_phase = 0.0f64;
    let mut frame_idx = 0u64;
    let frame_period = Duration::from_secs_f64(1.0 / opts.fps());
    let mut next_deadline = Instant::now();
    let samples = opts.samples_per_frame();

    loop {
        let _ = sender.poll_accept().await?;
        sender.poll_peer_metadata().await?;

        let want_video = sender.video_subscribed();
        let want_audio = sender.audio_subscribed();
        if !want_video && !want_audio {
            tokio::time::sleep(Duration::from_millis(20)).await;
            continue;
        }

        let phase = if opts.animate {
            ((frame_idx % 300) as f32) / 300.0
        } else {
            0.0
        };

        if want_video {
            fill_colorbar_uyvy(&mut uyvy, opts.width, opts.height, phase);
            vmx.encode_uyvy(&uyvy, stride)?;
            let n = vmx.save_to(&mut vmx_buf)?;
            let frame = MediaFrame {
                frame_type: FrameType::VIDEO,
                timestamp: -1,
                codec: Codec::Vmx1 as i32,
                width: opts.width,
                height: opts.height,
                frame_rate_n: opts.fps_n,
                frame_rate_d: opts.fps_d,
                aspect_ratio: opts.width as f32 / opts.height.max(1) as f32,
                color_space: ColorSpace::Undefined,
                data: vmx_buf[..n].to_vec(),
                ..Default::default()
            };
            sender.send_video(frame).await?;
        }

        if want_audio {
            audio_buf.clear();
            append_sine_planar(
                &mut audio_buf,
                opts.channels,
                samples,
                opts.sample_rate,
                opts.tone_hz,
                &mut audio_phase,
            );
            let frame = MediaFrame {
                frame_type: FrameType::AUDIO,
                timestamp: -1,
                codec: Codec::Fpa1 as i32,
                sample_rate: opts.sample_rate,
                channels: opts.channels,
                samples_per_channel: samples,
                active_channels: 0,
                data: audio_buf.clone(),
                ..Default::default()
            };
            sender.send_audio(frame).await?;
        }

        frame_idx += 1;
        next_deadline += frame_period;
        let now = Instant::now();
        if next_deadline > now {
            tokio::time::sleep(next_deadline - now).await;
        } else {
            next_deadline = now;
        }
    }
}

#[cfg(not(feature = "tokio"))]
fn main() {
    eprintln!("re-run with: cargo run --example send_colorbar_tokio --features tokio");
}
