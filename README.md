# openmediatransport-rs

Pure Rust implementation of **Open Media Transport (OMT)** — video, audio, and metadata over TCP with DNS-SD discovery.

Depends on [`vmx`](../vmx-rs) for the VMX1 video codec. No Bonjour, Avahi, .NET, or native libomtnet linkage.

## Features

| Feature | Description |
|---------|-------------|
| *(default)* | Sync `Sender` / `Receiver` / `Discovery` |
| `tokio` | Async wrappers (`AsyncSender` / `AsyncReceiver`) |

## MSRV

**Rust 1.88** (`edition = "2024"`).

## Quick start

```rust
use openmediatransport::{Discovery, FrameType, Receiver, Sender};

fn main() -> Result<(), openmediatransport::OmtError> {
    let mut discovery = Discovery::new()?;
    discovery.refresh()?;

    let sender = Sender::create("My Source", FrameType::VIDEO | FrameType::AUDIO)?;
    let url = format!("omt://127.0.0.1:{}/My Source", sender.port());
    let _receiver = Receiver::create(url, FrameType::VIDEO)?;
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
```

See [PROTOCOL.md](PROTOCOL.md) for the wire format.

## License

MIT — Copyright (c) 2025 Open Media Transport Contributors and MikanseiLaboratory
