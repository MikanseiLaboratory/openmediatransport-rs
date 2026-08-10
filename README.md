# openmediatransport-rs

[![CI](https://github.com/MikanseiLaboratory/openmediatransport-rs/actions/workflows/ci.yml/badge.svg)](https://github.com/MikanseiLaboratory/openmediatransport-rs/actions/workflows/ci.yml)

Pure Rust implementation of **Open Media Transport (OMT)** — video, audio, and metadata over TCP with DNS-SD discovery.

Depends on [`vmx-rs`](../vmx-rs) for the VMX1 video codec. No Bonjour, Avahi, .NET, or native libomtnet linkage.

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

```bash
cargo check
cargo check --features tokio
cargo test
cargo run --example discovery
cargo run --example send_receive
cargo run --example send_receive_tokio --features tokio
cargo run --example receive_to_jpeg
cargo run --example receive_to_jpeg_tokio --features tokio
cargo run --example send_colorbar
cargo run --example send_colorbar_tokio --features tokio
```

See [PROTOCOL.md](PROTOCOL.md) for the wire format.

## License

MIT — Copyright (c) 2026 Open Media Transport Contributors and MikanseiLaboratory
