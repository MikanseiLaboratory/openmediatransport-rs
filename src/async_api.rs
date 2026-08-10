//! Tokio-based async API (feature = `tokio`).
//!
//! [`AsyncReceiver`] wraps [`ReceiverSession`] and polls its bounded output
//! channels without nesting VMX decode on the Tokio worker via `block_in_place`.

use std::time::Duration;

use crate::error::OmtError;
use crate::receive::{ReceiverConfig, ReceiverSession};
use crate::send::{Sender, SenderConfig};
use crate::types::{
    DecodedAudioFrame, DecodedVideoFrame, FrameType, MediaFrame, MetadataFrame, SessionStatistics,
    Statistics,
};

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

/// Async receiver over [`ReceiverSession`].
pub struct AsyncReceiver {
    inner: ReceiverSession,
}

impl AsyncReceiver {
    /// Connect an async receiver session.
    pub async fn connect(
        address: impl Into<String>,
        frame_types: FrameType,
    ) -> Result<Self, OmtError> {
        let address = address.into();
        let config = ReceiverConfig {
            frame_types,
            ..ReceiverConfig::default()
        };
        let inner = tokio::task::spawn_blocking(move || ReceiverSession::connect(address, config))
            .await
            .map_err(|e| OmtError::Network(e.to_string()))??;
        Ok(Self { inner })
    }

    /// Connection address.
    pub fn address(&self) -> &str {
        self.inner.address()
    }

    /// Configured frame types.
    pub fn frame_types(&self) -> FrameType {
        self.inner.frame_types()
    }

    /// Wait up to `timeout_ms` for the next decoded video frame.
    pub async fn recv_video(&mut self, timeout_ms: i32) -> Option<DecodedVideoFrame> {
        let timeout = if timeout_ms < 0 {
            Duration::from_secs(3600)
        } else {
            Duration::from_millis(timeout_ms as u64)
        };
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            if let Some(frame) = self.inner.try_recv_video() {
                return Some(frame);
            }
            if tokio::time::Instant::now() >= deadline {
                return None;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    }

    /// Non-blocking audio poll.
    pub fn try_recv_audio(&self) -> Option<DecodedAudioFrame> {
        self.inner.try_recv_audio()
    }

    /// Non-blocking metadata poll.
    pub fn try_recv_metadata(&self) -> Option<MetadataFrame> {
        self.inner.try_recv_metadata()
    }

    /// Snapshot of receive statistics.
    pub fn statistics(&self) -> SessionStatistics {
        self.inner.statistics()
    }
}
