//! Tokio-based async API (feature = `tokio`).
//!
//! Thin async wrappers around the sync sender/receiver protocol logic.

use crate::error::OmtError;
use crate::receive::{ReceivedFrame, Receiver};
use crate::send::Sender;
use crate::types::{FrameType, MediaFrame};

/// Async sender wrapping the sync [`Sender`] protocol logic.
#[derive(Debug)]
pub struct AsyncSender {
    inner: Sender,
}

impl AsyncSender {
    /// Create an async sender (binds a listening port).
    pub async fn create(name: impl Into<String>, frame_types: FrameType) -> Result<Self, OmtError> {
        Ok(Self {
            inner: Sender::create(name, frame_types)?,
        })
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
        self.inner.poll_accept()
    }

    /// Process inbound subscribe / tally / quality metadata from peers.
    pub async fn poll_peer_metadata(&mut self) -> Result<(), OmtError> {
        self.inner.poll_peer_metadata()
    }

    /// Accept peers and process subscribe metadata.
    pub async fn poll(&mut self) -> Result<(), OmtError> {
        self.inner.poll_accept()?;
        self.inner.poll_peer_metadata()?;
        Ok(())
    }

    /// Send a video frame asynchronously.
    pub async fn send_video(&mut self, frame: MediaFrame) -> Result<(), OmtError> {
        self.inner.send_video(frame)
    }

    /// Send an audio frame asynchronously.
    pub async fn send_audio(&mut self, frame: MediaFrame) -> Result<(), OmtError> {
        self.inner.send_audio(frame)
    }

    /// Force subscriptions (tests / offline).
    pub fn force_subscribe(&mut self, video: bool, audio: bool, metadata: bool) {
        self.inner.force_subscribe(video, audio, metadata);
    }
}

/// Async receiver wrapping the sync [`Receiver`] protocol logic.
#[derive(Debug)]
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

    /// Connect dual TCP sessions.
    pub async fn connect(&mut self) -> Result<(), OmtError> {
        self.inner.connect(None)
    }

    /// Receive the next frame with a timeout in milliseconds.
    pub async fn receive(&mut self, timeout_ms: i32) -> Result<Option<ReceivedFrame>, OmtError> {
        self.inner.receive(timeout_ms)
    }
}
