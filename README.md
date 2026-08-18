# openmediatransport-rs

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
| [vmx-rs](https://github.com/MikanseiLaboratory/vmx-rs) | Pure Rust VMX1 codec

## Features

| Feature | Description |
|---------|-------------|
| *(default)* | Sync `Sender` / `ReceiverSession` / `Discovery` (VMX1 BGRA receive, optional 1/8 Preview) |
| `tokio` | Async wrappers (`AsyncSender` / `AsyncReceiver` over `ReceiverSession`) |

Codec SIMD / rayon details: [`vmx-rs` README](https://github.com/MikanseiLaboratory/vmx-rs).
Callers can inspect the selected VMX path via `vmx::Codec::simd_path()`
(`avx2` / `sse128` / `neon` / `scalar`) and host capabilities via
`simd_capabilities()`.

## MSRV

**Rust 1.97** (`edition = "2024"`).

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

See [PROTOCOL.md](PROTOCOL.md) for the wire format.

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

`DiscoveryServer` selects an [OMT Discovery Server](https://github.com/openmediatransport/OMTDiscoveryServer#client-configuration) instead of DNS-SD for sender registration. Leave it blank for default DNS-SD. Per-process override:

```rust
use openmediatransport::{Settings, KEY_DISCOVERY_SERVER};

Settings::global()
    .lock()
    .expect("settings lock")
    .set_string(KEY_DISCOVERY_SERVER, "omt://127.0.0.1:6399");
```

## License

MIT — Copyright (c) 2026 Open Media Transport Contributors and MikanseiLaboratory
