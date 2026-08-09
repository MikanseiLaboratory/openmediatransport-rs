//! Frame buffer pools.
#![allow(dead_code)]

use crate::types::{
    AUDIO_FRAME_POOL_COUNT, AUDIO_MIN_SIZE, METADATA_FRAME_SIZE, VIDEO_FRAME_POOL_COUNT,
    VIDEO_MIN_SIZE,
};

/// Simple reusable byte-buffer pool.
#[derive(Debug, Default)]
pub struct BufferPool {
    free: Vec<Vec<u8>>,
    max_retained: usize,
}

impl BufferPool {
    /// Create an empty pool that retains up to `max_retained` buffers.
    pub fn with_capacity(max_retained: usize) -> Self {
        Self {
            free: Vec::new(),
            max_retained,
        }
    }

    /// Create an empty pool with default retention.
    pub fn new() -> Self {
        Self::with_capacity(8)
    }

    /// Video frame pool (libomtnet depth).
    pub fn video() -> Self {
        Self::with_capacity(VIDEO_FRAME_POOL_COUNT)
    }

    /// Audio frame pool (libomtnet depth).
    pub fn audio() -> Self {
        Self::with_capacity(AUDIO_FRAME_POOL_COUNT)
    }

    /// Metadata frame pool.
    pub fn metadata() -> Self {
        Self::with_capacity(8)
    }

    /// Take a buffer of at least `min_capacity` bytes.
    pub fn take(&mut self, min_capacity: usize) -> Vec<u8> {
        if let Some(mut buf) = self.free.pop() {
            buf.clear();
            if buf.capacity() < min_capacity {
                buf.reserve(min_capacity - buf.capacity());
            }
            buf
        } else {
            Vec::with_capacity(min_capacity)
        }
    }

    /// Take a video-sized buffer.
    pub fn take_video(&mut self) -> Vec<u8> {
        self.take(VIDEO_MIN_SIZE)
    }

    /// Take an audio-sized buffer.
    pub fn take_audio(&mut self) -> Vec<u8> {
        self.take(AUDIO_MIN_SIZE)
    }

    /// Take a metadata-sized buffer.
    pub fn take_metadata(&mut self) -> Vec<u8> {
        self.take(METADATA_FRAME_SIZE)
    }

    /// Return a buffer to the pool.
    pub fn give(&mut self, mut buf: Vec<u8>) {
        buf.clear();
        if self.free.len() < self.max_retained {
            self.free.push(buf);
        }
    }

    /// Number of free buffers.
    pub fn len(&self) -> usize {
        self.free.len()
    }

    /// Whether the pool is empty.
    pub fn is_empty(&self) -> bool {
        self.free.is_empty()
    }
}
