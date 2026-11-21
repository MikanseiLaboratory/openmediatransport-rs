# openmediatransport-rs

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="assets/branding/openmediatransport-rs-logo-on-dark.svg" />
  <img alt="openmediatransport-rs" width="640" src="assets/branding/openmediatransport-rs-logo-on-light.svg" />
</picture>


[![CI](https://github.com/MikanseiLaboratory/openmediatransport-rs/actions/workflows/ci.yml/badge.svg)](https://github.com/MikanseiLaboratory/openmediatransport-rs/actions/workflows/ci.yml)

Pure Rust implementation of **Open Media Transport (OMT)** — video, audio, and metadata over TCP with DNS-SD discovery.

> **Disclaimer:** This is an independent, community-maintained project. It is **not** an official Open Media Transport product or repository.

## Related projects

| Project | Description |
|---------|-------------|
| [Open Media Transport (official)](https://github.com/openmediatransport) | Official OMT organization and documentation |
| [libomtnet](https://github.com/openmediatransport/libomtnet) | Official .NET OMT core |
| [libomt](https://github.com/openmediatransport/libomt) | Official C wrapper for libomtnet |
| [libvmx](https://github.com/openmediatransport/libvmx) | Official VMX1 video codec |
| [vmx-rs](https://github.com/MikanseiLaboratory/vmx-rs) | Pure Rust VMX1 codec ([`vmx`](https://crates.io/crates/vmx) on crates.io)

## Features

| Feature | Description |
|---------|-------------|
| *(default)* | Sync `Sender` / `ReceiverSession` / `Discovery` (VMX1 BGRA receive, optional 1/8 Preview) |
| `tokio` | Async wrappers (`AsyncSender` / `AsyncReceiver` over `ReceiverSession`) |
| `wgpu` | GPU texture receive/send on a caller-owned `wgpu::Device` (`GpuVideoContext`, `try_recv_video_gpu`, `send_video_texture`). 8-bit `Bgra8Unorm`. |
Codec SIMD / rayon details: [`vmx-rs` README](https://github.com/MikanseiLaboratory/vmx-rs).
Callers can inspect the selected VMX path via `vmx::Codec::simd_path()`
(`avx2` / `sse128` / `neon` / `scalar`) and host capabilities via
`simd_capabilities()`.

When `wgpu` is enabled, pass `Arc<wgpu::Device>` + `Arc<wgpu::Queue>` in
`ReceiverConfig.gpu` at connect. The decode thread submits compute and returns;
later submits on the same queue can sample the texture. CPU readback still
waits in `vmx::gpu::read_texture_bgra`. Receive with `try_recv_video_gpu`; send
with `Sender::send_video_texture` (or `send_video_texture_view` when the host
already owns a `wgpu::TextureView`, which skips the per-frame view allocation).

`ReceiverConfig.gpu_decode_mode` picks the VMX coefficient path: `GpuDecodeMode::Fast`
(default) drops high-frequency coefficients and clamps AC to `i8`, while
`GpuDecodeMode::Exact` uploads 132-byte records and is **byte-identical** to the
CPU decoder. Use `Exact` when decoded pixels are recorded, compared, or
re-encoded. `ReceiverSession::gpu_stats()` reports the last decode timings and
`gpu_path_recommended(adapter)` says whether the GPU path is expected to beat
the CPU one on that adapter (integrated GPUs usually are not).

`Sender::send_video` uses the slice-fused CPU encoder (same structure as libvmx).
GPU encode is `Sender::send_video_texture`. Opaque frames skip the alpha plane
(matching libvmx `EncodeBGRX`). `set_gpu_encode_pipeline(true)` overlaps GPU
FDCT of frame N with entropy coding / send of frame N-1 (one frame of latency;
call `flush_gpu_encode` after the last texture).

## Audio buffering and latency

The receiver decodes FPA1 audio on a dedicated thread and publishes it through a
bounded queue. The queue is limited by **time, not frame count**: OMT senders
choose `samples_per_channel` themselves (10 ms in eiviz, one video frame ≈ 17 ms
in the official example), so "64 frames" means anything from 64 ms to over a
second.

`ReceiverConfig::audio_max_latency` (default **100 ms**, clamped to 1 ms … 2 s)
caps both:

- the total media duration queued (playback backlog), and
- the wall-clock age of the oldest queued frame.

Older frames are dropped first. This is an upper bound on the latency the
receiver adds, not a fixed delay: a queue that is drained at playback rate sits
at zero.

```rust
let config = ReceiverConfig {
    frame_types: FrameType::AUDIO,
    audio_max_latency: Duration::from_millis(250), // slow TV / big jitter
    ..ReceiverConfig::default()
};
let session = ReceiverSession::connect(url, config)?;

// Drain from a playback thread. Stale frames are discarded by the queue.
while let Some(audio) = session.recv_audio_timeout(Duration::from_millis(20)) {
    // push to the audio device
}
```

Guidance:

- Low-latency monitoring: 40–80 ms.
- TVs / slow decoders / heavy scheduling jitter: 200–250 ms.
- `statistics()` exposes `audio_queue_depth`, `audio_latency_us` (queued media
  time), `audio_oldest_us`, `audio_latency_us_peak`,
  `audio_frames_dropped_stale` and `audio_frames_dropped_decode`;
  `frames_dropped_audio` stays the sum of the two drop counters.
- The sender has a separate `SenderConfig::send_queue_depth` (default 4 frames
  ≈ 40–67 ms). It is not part of the receiver budget.
- The official libomtnet has no receive-side audio bound; it hands frames to the
  app's `Receive()` loop. This crate decodes on its own thread, so it enforces
  the budget above instead.

## MSRV

**Rust 1.97** (`edition = "2024"`).

## Install

```toml
openmediatransport = "0.3"
```

## Quick start

```rust
use openmediatransport::{Discovery, FrameType, ReceiverConfig, ReceiverSession, Sender};

fn main() -> Result<(), openmediatransport::OmtError> {
    let mut discovery = Discovery::new()?;
    discovery.refresh()?;

    let sender = Sender::create("My Source", FrameType::VIDEO | FrameType::AUDIO)?;
    let url = format!("omt://127.0.0.1:{}/My Source", sender.port());
    let _receiver = ReceiverSession::connect(
        url,
        ReceiverConfig {
            frame_types: FrameType::VIDEO,
            ..ReceiverConfig::default()
        },
    )?;
    Ok(())
}
```

See [PROTOCOL.md](PROTOCOL.md) for the wire format and [OMT Metadata](https://github.com/openmediatransport/Metadata) for recommended XML types (`OMTWeb`, `OMTPTZ`, `OMTGroup`, …). Incoming metadata is parsed by element/attribute key (`parse_metadata` / `MetadataFrame::parse`); receivers can send tally with `ReceiverSession::set_tally`.

## Settings (`settings.xml`)

Persistent settings match [libomtnet `OMTSettings`](https://github.com/openmediatransport/libomtnet/blob/master/src/OMTSettings.cs). The file is a flat XML map (not a fixed schema):

```xml
<Settings>
  <DiscoveryServer>omt://x.x.x.x:port</DiscoveryServer>
  <NetworkPortStart>6400</NetworkPortStart>
  <NetworkPortEnd>6600</NetworkPortEnd>
</Settings>
```

| Location | Path |
|----------|------|
| Windows | `%ProgramData%\OMT\settings.xml` (`C:\ProgramData\OMT\settings.xml`) |
| macOS / Linux | `~/.OMT/settings.xml` |
| Override | `OMT_STORAGE_PATH` environment variable (directory containing `settings.xml`) |

`Settings` is a flat key/value map: `entries` / `set_string` / `remove` / `reload` / `save` cover known and unknown keys. `settings_file_path()` and `storage_dir()` expose the platform file location.

`DiscoveryServer` selects an [OMT Discovery Server](https://github.com/openmediatransport/OMTDiscoveryServer#client-configuration) instead of DNS-SD for sender registration. Leave it blank for default DNS-SD. Per-process override:

```rust
use openmediatransport::{Settings, KEY_DISCOVERY_SERVER};

Settings::global()
    .lock()
    .expect("settings lock")
    .set_string(KEY_DISCOVERY_SERVER, "omt://127.0.0.1:6399");
```

The in-process server (`DiscoveryServer` / `DiscoveryServerHandle`) binds dual-stack `[::]:6399` by default, matching the official console app. Tools can start/stop the server, snapshot connected peers and registered sources, and poll `DiscoveryServerEvent`s.

## License

MIT — Copyright (c) 2026 Open Media Transport Contributors and MikanseiLaboratory
