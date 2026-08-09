//! Open Media Transport (OMT) — pure Rust protocol stack.

#![deny(missing_docs)]

mod clock;
/// Media codecs (FPA1, etc.).
pub mod codec;
mod discovery;
mod error;
mod logging;
/// Wire protocol framing and metadata helpers.
pub mod protocol;
mod receive;
mod redirect;
mod send;
mod settings;
mod statistics;
mod transport;
/// Public protocol types and constants.
pub mod types;

#[cfg(feature = "tokio")]
pub mod async_api;

pub use discovery::{Discovery, DiscoveryClient, DiscoveryServer, OmtAddress};
pub use error::OmtError;
pub use receive::{ReceivedFrame, Receiver};
pub use send::{Sender, SenderConfig};
pub use types::*;

/// Re-export of the VMX codec crate used for VMX1 video.
pub use vmx;
