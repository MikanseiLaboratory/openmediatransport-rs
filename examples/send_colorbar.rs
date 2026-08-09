//! Continuously send SMPTE-style color bars + sine tone over OMT (sync).
//!
//! ```text
//! cargo run --example send_colorbar -- [name] [options]
//!   --width N --height N --fps N --animate|--no-animate
//!   --tone-hz F --rate N --channels N
//! ```

#[path = "common/colorbar.rs"]
mod colorbar;

use std::thread;
use std::time::{Duration, Instant};

use colorbar::{SendOptions, append_sine_planar, fill_colorbar_uyvy};
use openmediatransport::{Codec, ColorSpace, Discovery, FrameType, MediaFrame, Sender, SenderInfo};
use vmx::{Codec as VmxCodec, Config as VmxConfig, Profile};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let opts = SendOptions::from_args(std::env::args().skip(1));
    run(opts)
}

fn run(opts: SendOptions) -> Result<(), Box<dyn std::error::Error>> {
    let mut sender = Sender::create(
        &opts.name,
        FrameType::VIDEO | FrameType::AUDIO | FrameType::METADATA,
    )?;
    sender.set_sender_info(SenderInfo::new(
        "openmediatransport-rs",
        "MikanseiLaboratory",
        env!("CARGO_PKG_VERSION"),
    ));

    let port = sender.port();
    let mut discovery = Discovery::new()?;
    discovery.register(&opts.name, port)?;

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

    let mut last_sub = (false, false);
    loop {
        let _ = sender.poll_accept()?;
        sender.poll_peer_metadata()?;

        let want_video = sender.video_subscribed();
        let want_audio = sender.audio_subscribed();
        if (want_video, want_audio) != last_sub {
            println!("subscriptions: video={want_video} audio={want_audio}");
            last_sub = (want_video, want_audio);
        }
        if !want_video && !want_audio {
            thread::sleep(Duration::from_millis(20));
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
            sender.send_video(frame)?;
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
                active_channels: 0, // trigger FPA1 encode in Sender
                data: audio_buf.clone(),
                ..Default::default()
            };
            sender.send_audio(frame)?;
        }

        frame_idx += 1;
        next_deadline += frame_period;
        let now = Instant::now();
        if next_deadline > now {
            thread::sleep(next_deadline - now);
        } else {
            next_deadline = now;
        }
    }
}
