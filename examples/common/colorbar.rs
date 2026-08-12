//! Shared helpers for colorbar + tone sender examples.

use std::f32::consts::TAU;

use openmediatransport::{
    NETWORK_ASYNC_COUNT, NETWORK_SEND_BUFFER, NETWORK_SEND_RECEIVE_BUFFER, Quality,
};

/// Default audio chunk size (10 ms at 48 kHz). Independent of video frame rate.
pub const DEFAULT_AUDIO_SAMPLES: i32 = 480;

/// CLI / runtime options for the colorbar sender.
#[derive(Debug, Clone)]
pub struct SendOptions {
    pub name: String,
    pub width: i32,
    pub height: i32,
    pub fps_n: i32,
    pub fps_d: i32,
    pub animate: bool,
    pub tone_hz: f32,
    pub sample_rate: i32,
    pub channels: i32,
    /// Samples per audio packet (default 480 = 10 ms @ 48 kHz).
    pub audio_samples: i32,
    /// Encoding quality. Default `Medium`.
    pub quality: Quality,
    /// Sender outbound queue depth (`0` = blocking). Default = libomtnet `NETWORK_ASYNC_COUNT` (4).
    pub send_queue: usize,
    /// TCP send buffer. Default = `NETWORK_SEND_BUFFER` (64 KiB).
    pub send_buffer: usize,
    /// TCP recv buffer on sender peers. Default = `NETWORK_SEND_RECEIVE_BUFFER` (64 KiB).
    pub recv_buffer: usize,
}

impl Default for SendOptions {
    fn default() -> Self {
        Self {
            name: "Colorbars".into(),
            width: 1920,
            height: 1080,
            fps_n: 30,
            fps_d: 1,
            animate: true,
            tone_hz: 1000.0,
            sample_rate: 48_000,
            channels: 2,
            audio_samples: DEFAULT_AUDIO_SAMPLES,
            quality: Quality::Medium,
            send_queue: NETWORK_ASYNC_COUNT,
            send_buffer: NETWORK_SEND_BUFFER,
            recv_buffer: NETWORK_SEND_RECEIVE_BUFFER,
        }
    }
}

impl SendOptions {
    /// Parse `args` after the program name.
    ///
    /// ```text
    /// [name] [--width N] [--height N] [--fps N] [--animate|--no-animate]
    ///        [--tone-hz F] [--rate N] [--channels N] [--audio-samples N]
    ///        [--profile lq|sq|hq] [--send-queue N] [--send-buffer N] [--recv-buffer N]
    /// ```
    pub fn from_args(mut args: impl Iterator<Item = String>) -> Self {
        let mut opts = Self::default();
        if let Some(first) = args.next() {
            if first.starts_with('-') {
                apply_flag(&mut opts, &first, &mut args);
            } else {
                opts.name = first;
            }
        }
        while let Some(flag) = args.next() {
            apply_flag(&mut opts, &flag, &mut args);
        }
        opts
    }

    pub fn fps(&self) -> f64 {
        self.fps_n as f64 / self.fps_d.max(1) as f64
    }

    #[allow(dead_code)]
    pub fn samples_per_frame(&self) -> i32 {
        ((self.sample_rate as f64) / self.fps()).round() as i32
    }

    pub fn profile_name(&self) -> &'static str {
        match self.quality {
            Quality::Low => "lq",
            Quality::High => "hq",
            Quality::Medium | Quality::Default => "sq",
        }
    }
}

fn apply_flag(opts: &mut SendOptions, flag: &str, args: &mut impl Iterator<Item = String>) {
    match flag {
        "--width" => {
            if let Some(v) = args.next().and_then(|s| s.parse::<i32>().ok()) {
                opts.width = v;
            }
        }
        "--height" => {
            if let Some(v) = args.next().and_then(|s| s.parse::<i32>().ok()) {
                opts.height = v;
            }
        }
        "--fps" => {
            if let Some(v) = args.next().and_then(|s| s.parse::<i32>().ok()) {
                opts.fps_n = v;
                opts.fps_d = 1;
            }
        }
        "--animate" => opts.animate = true,
        "--no-animate" => opts.animate = false,
        "--tone-hz" => {
            if let Some(v) = args.next().and_then(|s| s.parse::<f32>().ok()) {
                opts.tone_hz = v;
            }
        }
        "--rate" => {
            if let Some(v) = args.next().and_then(|s| s.parse::<i32>().ok()) {
                opts.sample_rate = v;
            }
        }
        "--channels" => {
            if let Some(v) = args.next().and_then(|s| s.parse::<i32>().ok()) {
                opts.channels = v.max(1);
            }
        }
        "--audio-samples" => {
            if let Some(v) = args.next().and_then(|s| s.parse::<i32>().ok()) {
                opts.audio_samples = v.max(1);
            }
        }
        "--profile" => {
            if let Some(v) = args.next() {
                opts.quality = match v.to_ascii_lowercase().as_str() {
                    "lq" | "low" => Quality::Low,
                    "hq" | "high" => Quality::High,
                    _ => Quality::Medium,
                };
            }
        }
        "--send-queue" => {
            if let Some(v) = args.next().and_then(|s| s.parse::<usize>().ok()) {
                opts.send_queue = v;
            }
        }
        "--send-buffer" => {
            if let Some(v) = args.next().and_then(|s| s.parse::<usize>().ok()) {
                opts.send_buffer = v.max(4096);
            }
        }
        "--recv-buffer" => {
            if let Some(v) = args.next().and_then(|s| s.parse::<usize>().ok()) {
                opts.recv_buffer = v.max(4096);
            }
        }
        _ => {}
    }
}

/// SMPTE-style 75% color bars as (R,G,B) 0..255.
const BARS_RGB: [(u8, u8, u8); 8] = [
    (191, 191, 191), // white / gray
    (191, 191, 0),   // yellow
    (0, 191, 191),   // cyan
    (0, 191, 0),     // green
    (191, 0, 191),   // magenta
    (191, 0, 0),     // red
    (0, 0, 191),     // blue
    (0, 0, 0),       // black
];

/// Fill a UYVY frame with color bars. `phase` (0..1) scrolls bars when animating.
pub fn fill_colorbar_uyvy(dst: &mut [u8], width: i32, height: i32, phase: f32) {
    let w = width as usize;
    let h = height as usize;
    let stride = w * 2;
    assert!(dst.len() >= stride * h);

    let offset = if phase == 0.0 {
        0
    } else {
        ((phase.rem_euclid(1.0)) * w as f32) as usize
    };

    // Build one scanline then replicate (bars are constant per column).
    let mut line = vec![0u8; stride];
    for x in (0..w).step_by(2) {
        let x0 = (x + offset) % w;
        let x1 = (x + 1 + offset) % w;
        let (y0, u0, v0) = rgb_to_yuv(bar_at(x0, w));
        let (y1, u1, v1) = rgb_to_yuv(bar_at(x1, w));
        let u = ((u0 as u16 + u1 as u16) / 2) as u8;
        let v = ((v0 as u16 + v1 as u16) / 2) as u8;
        let o = x * 2;
        line[o] = u;
        line[o + 1] = y0;
        line[o + 2] = v;
        line[o + 3] = y1;
    }

    for y in 0..h {
        dst[y * stride..(y + 1) * stride].copy_from_slice(&line);
    }

    // Moving horizontal marker for animation visibility.
    if phase != 0.0 {
        let my = ((phase.rem_euclid(1.0)) * h as f32) as usize % h;
        let row = &mut dst[my * stride..(my + 1) * stride];
        for x in (0..w).step_by(2) {
            let o = x * 2;
            row[o + 1] = 235;
            row[o + 3] = 235;
        }
    }
}

fn bar_at(x: usize, width: usize) -> (u8, u8, u8) {
    let idx = (x * BARS_RGB.len()) / width.max(1);
    BARS_RGB[idx.min(BARS_RGB.len() - 1)]
}

fn rgb_to_yuv(rgb: (u8, u8, u8)) -> (u8, u8, u8) {
    let (r, g, b) = (rgb.0 as i32, rgb.1 as i32, rgb.2 as i32);
    // BT.709 full-ish studio range approximation.
    let y = ((66 * r + 129 * g + 25 * b + 128) >> 8) + 16;
    let u = ((-38 * r - 74 * g + 112 * b + 128) >> 8) + 128;
    let v = ((112 * r - 94 * g - 18 * b + 128) >> 8) + 128;
    (
        y.clamp(16, 235) as u8,
        u.clamp(16, 240) as u8,
        v.clamp(16, 240) as u8,
    )
}

/// Append planar float32 LE sine tones for `channels` × `samples`.
pub fn append_sine_planar(
    dst: &mut Vec<u8>,
    channels: i32,
    samples: i32,
    sample_rate: i32,
    tone_hz: f32,
    phase: &mut f64,
) {
    let ch = channels.max(1) as usize;
    let n = samples.max(0) as usize;
    let rate = sample_rate.max(1) as f64;
    let freq = tone_hz as f64;
    dst.reserve(ch * n * 4);
    let start = *phase;
    for _c in 0..ch {
        let mut p = start;
        for _s in 0..n {
            let sample = (TAU as f64 * freq * p / rate).sin() as f32 * 0.2;
            dst.extend_from_slice(&sample.to_le_bytes());
            p += 1.0;
            if p >= rate {
                p -= rate;
            }
        }
    }
    *phase = start + n as f64;
    if *phase >= rate {
        *phase %= rate;
    }
}
