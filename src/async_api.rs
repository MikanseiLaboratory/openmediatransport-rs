//! Tokio-based async API (feature = `tokio`).
//!
//! Thin wrappers around the sync sender/receiver. Blocking socket / VMX work uses
//! [`tokio::task::block_in_place`] so the multi-thread scheduler can continue
//! serving other tasks. Slice-level codec parallelism stays in `vmx` via rayon.

use std::time::Duration;

use crate::error::OmtError;
use crate::receive::{ReceivedFrame, Receiver};
use crate::send::{Sender, SenderConfig};
use crate::types::{FrameType, MediaFrame, PreferredVideoFormat, Statistics};

/// Async sender wrapping the sync [`Sender`] protocol logic.
#[derive(Debug)]
pub struct AsyncSender {
    inner: Sender,
}

impl AsyncSender {
    /// Create an async sender (binds a listening port).
    pub async fn create(name: impl Into<String>, frame_types: FrameType) -> Result<Self, OmtError> {
        Self::create_with_config(name, frame_types, SenderConfig::default()).await
    }

    /// Create an async sender with transport / buffering settings.
    pub async fn create_with_config(
        name: impl Into<String>,
        frame_types: FrameType,
        config: SenderConfig,
    ) -> Result<Self, OmtError> {
        Ok(Self {
            inner: Sender::create_with_config(name, frame_types, config)?,
        })
    }

    /// Current transport / buffering configuration.
    pub fn transport_config(&self) -> SenderConfig {
        self.inner.transport_config()
    }

    /// Source name.
    pub fn name(&self) -> &str {
        self.inner.name()
    }

    /// Listening port.
    pub fn port(&self) -> u16 {
        self.inner.port()
    }

    /// Configured frame types.
    pub fn frame_types(&self) -> FrameType {
        self.inner.frame_types()
    }

    /// Accept one pending peer connection.
    pub async fn poll_accept(&mut self) -> Result<bool, OmtError> {
        tokio::task::block_in_place(|| self.inner.poll_accept())
    }

    /// Process inbound subscribe / tally / quality metadata from peers.
    pub async fn poll_peer_metadata(&mut self) -> Result<(), OmtError> {
        tokio::task::block_in_place(|| self.inner.poll_peer_metadata())
    }

    /// Accept peers and process subscribe metadata.
    pub async fn poll(&mut self) -> Result<(), OmtError> {
        tokio::task::block_in_place(|| {
            self.inner.poll_accept()?;
            self.inner.poll_peer_metadata()?;
            Ok(())
        })
    }

    /// Send a video frame asynchronously.
    ///
    /// Uses `block_in_place` so sync socket writes do not stall the multi-thread
    /// scheduler's ability to run other tasks.
    pub async fn send_video(&mut self, frame: MediaFrame) -> Result<(), OmtError> {
        tokio::task::block_in_place(|| self.inner.send_video(frame))
    }

    /// Send an audio frame asynchronously.
    pub async fn send_audio(&mut self, frame: MediaFrame) -> Result<(), OmtError> {
        tokio::task::block_in_place(|| self.inner.send_audio(frame))
    }

    /// Force subscriptions (tests / offline).
    pub fn force_subscribe(&mut self, video: bool, audio: bool, metadata: bool) {
        self.inner.force_subscribe(video, audio, metadata);
    }

    /// Set sender product info metadata.
    pub fn set_sender_info(&mut self, info: crate::types::SenderInfo) {
        self.inner.set_sender_info(info);
    }

    /// True when any peer has subscribed to video.
    pub fn video_subscribed(&self) -> bool {
        self.inner.video_subscribed()
    }

    /// True when any peer has subscribed to audio.
    pub fn audio_subscribed(&self) -> bool {
        self.inner.audio_subscribed()
    }

    /// Snapshot of send statistics.
    pub fn statistics(&self) -> Statistics {
        self.inner.statistics()
    }
}

/// Async receiver wrapping the sync [`Receiver`] protocol logic.
pub struct AsyncReceiver {
    inner: Receiver,
}

impl AsyncReceiver {
    /// Create an async receiver.
    pub async fn create(
        address: impl Into<String>,
        frame_types: FrameType,
    ) -> Result<Self, OmtError> {
        Ok(Self {
            inner: Receiver::create(address, frame_types)?,
        })
    }

    /// Connection address.
    pub fn address(&self) -> &str {
        self.inner.address()
    }

    /// Configured frame types.
    pub fn frame_types(&self) -> FrameType {
        self.inner.frame_types()
    }

    /// Set preferred uncompressed video format.
    pub fn set_preferred_format(&mut self, format: PreferredVideoFormat) {
        self.inner.set_preferred_format(format);
    }

    /// Preferred format.
    pub fn preferred_format(&self) -> PreferredVideoFormat {
        self.inner.preferred_format()
    }

    /// Connect dual TCP sessions (no connect timeout).
    pub async fn connect(&mut self) -> Result<(), OmtError> {
        self.inner.connect(None)
    }

    /// Connect with an optional TCP connect timeout.
    pub async fn connect_timeout(&mut self, timeout: Option<Duration>) -> Result<(), OmtError> {
        self.inner.connect(timeout)
    }

    /// Receive the next frame with a timeout in milliseconds.
    ///
    /// VMX decode is CPU-bound; `block_in_place` keeps the Tokio pool healthy
    /// while the sync receiver runs (same pattern as `spawn_blocking`, without
    /// requiring `'static` ownership of the receiver).
    pub async fn receive(&mut self, timeout_ms: i32) -> Result<Option<ReceivedFrame>, OmtError> {
        tokio::task::block_in_place(|| self.inner.receive(timeout_ms))
    }

    /// Snapshot of receive statistics.
    pub fn statistics(&self) -> Statistics {
        self.inner.statistics()
    }
}
