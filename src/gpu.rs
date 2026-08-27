//! Optional wgpu texture I/O (`feature = "wgpu"`).
//!
//! Callers pass the `Device` / `Queue` they already own. Decode waits for GPU
//! work before [`crate::ReceiverSession::try_recv_video_gpu`] returns, so the
//! texture can be bound immediately.

use std::sync::Arc;

use crate::types::{ColorSpace, VideoFlags};

/// Host GPU objects used for VMX texture decode / encode.
///
/// Clone is cheap (`Arc`). Pass this on [`crate::ReceiverConfig`] at connect
/// time so the decode thread can submit compute without borrowing per frame.
#[derive(Clone)]
pub struct GpuVideoContext {
    /// Caller's `wgpu` device.
    pub device: Arc<wgpu::Device>,
    /// Caller's `wgpu` queue (same device as [`Self::device`]).
    pub queue: Arc<wgpu::Queue>,
}

impl std::fmt::Debug for GpuVideoContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GpuVideoContext").finish_non_exhaustive()
    }
}

/// Decoded VMX1 frame as a `Bgra8Unorm` texture on the caller's device.
#[derive(Debug, Clone)]
pub struct DecodedVideoGpuFrame {
    /// Pixel width (preview frames use 1/8 size).
    pub width: u32,
    /// Pixel height (preview frames use 1/8 size).
    pub height: u32,
    /// Timestamp (100 ns ticks).
    pub timestamp: i64,
    /// Frame rate numerator.
    pub frame_rate_n: i32,
    /// Frame rate denominator.
    pub frame_rate_d: i32,
    /// Color space used for YUV→RGB.
    pub color_space: ColorSpace,
    /// `Bgra8Unorm` texture (`TEXTURE_BINDING | COPY_DST | COPY_SRC`).
    pub texture: wgpu::Texture,
    /// Optional per-frame metadata XML.
    pub frame_metadata: Option<Arc<str>>,
}

/// Metadata for [`crate::Sender::send_video_texture`].
#[derive(Debug, Clone)]
pub struct VideoTextureMeta {
    /// Pixel width; must match `texture.size().width`.
    pub width: u32,
    /// Pixel height; must match `texture.size().height`.
    pub height: u32,
    /// Timestamp in 100 ns ticks (`-1` = auto).
    pub timestamp: i64,
    /// Frame rate numerator.
    pub frame_rate_n: i32,
    /// Frame rate denominator.
    pub frame_rate_d: i32,
    /// Color space for RGB→YUV.
    pub color_space: ColorSpace,
    /// Video flags (alpha / preview bits as on the wire).
    pub flags: VideoFlags,
    /// Optional per-frame metadata XML.
    pub frame_metadata: Option<String>,
}

impl Default for VideoTextureMeta {
    fn default() -> Self {
        Self {
            width: 0,
            height: 0,
            timestamp: -1,
            frame_rate_n: 60,
            frame_rate_d: 1,
            color_space: ColorSpace::Undefined,
            flags: VideoFlags::NONE,
            frame_metadata: None,
        }
    }
}
