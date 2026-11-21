//! Optional wgpu texture I/O (`feature = "wgpu"`).
//!
//! Callers pass the `Device` / `Queue` they already own. Decode submits on
//! that queue and returns; later submits on the same queue can sample the
//! texture. CPU readback still waits in [`vmx::gpu::read_texture_bgra`].
//!
//! Decode has two coefficient paths ([`GpuDecodeMode`]): [`GpuDecodeMode::Fast`]
//! (default) drops high-frequency coefficients and clamps AC to `i8`, while
//! [`GpuDecodeMode::Exact`] is byte-identical to the CPU decoder. Pick the
//! exact path with [`crate::ReceiverConfig::gpu_decode_mode`] when the decoded
//! pixels are compared, recorded, or re-encoded downstream.

use std::sync::{Arc, Mutex, MutexGuard};

use crate::types::{ColorSpace, VideoFlags};

/// Coefficient/IDCT path for VMX texture decode.
///
/// Re-export of [`vmx::gpu::GpuDecodeMode`]; set it on
/// [`crate::ReceiverConfig::gpu_decode_mode`].
pub use vmx::gpu::GpuDecodeMode;
/// Timings from the receiver's most recent GPU decode/encode.
///
/// Re-export of [`vmx::gpu::GpuStats`]; read it from
/// [`crate::ReceiverSession::gpu_stats`].
pub use vmx::gpu::GpuStats;

/// Whether the wgpu path is expected to beat the CPU path on `adapter`.
///
/// Thin re-export of [`vmx::gpu::gpu_path_recommended`] so a host can decide
/// whether to build a [`GpuVideoContext`] before connecting.
pub fn gpu_path_recommended(adapter: &wgpu::Adapter) -> bool {
    vmx::gpu::gpu_path_recommended(adapter)
}

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
    /// Optional lock shared with the host renderer so encode/decode do not
    /// race `Surface::configure` on the same device.
    pub gpu_lock: Option<Arc<Mutex<()>>>,
}

impl GpuVideoContext {
    pub(crate) fn lock_gpu(&self) -> Option<MutexGuard<'_, ()>> {
        self.gpu_lock
            .as_ref()
            .map(|lock| lock.lock().unwrap_or_else(|e| e.into_inner()))
    }
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
    /// Later submits on the decode queue can sample it.
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
