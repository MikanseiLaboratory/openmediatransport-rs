//! Internal VMX1 encode for uncompressed sender frames.

use std::time::{Duration, Instant};

use crate::error::OmtError;
use crate::types::{Codec, ColorSpace, MediaFrame, Quality, VideoFlags};

/// Reusable VMX encoder keyed by geometry, profile, and color space.
pub(crate) struct VideoEncoder {
    codec: Option<vmx::Codec>,
    width: i32,
    height: i32,
    profile: vmx::Profile,
    color_space: vmx::ColorSpace,
    buf: Vec<u8>,
}

impl std::fmt::Debug for VideoEncoder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VideoEncoder")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("profile", &self.profile)
            .field("color_space", &self.color_space)
            .field("ready", &self.codec.is_some())
            .finish()
    }
}

impl VideoEncoder {
    pub(crate) fn new() -> Self {
        Self {
            codec: None,
            width: 0,
            height: 0,
            profile: vmx::Profile::Default,
            color_space: vmx::ColorSpace::Undefined,
            buf: Vec::new(),
        }
    }

    /// Encode an uncompressed frame to a VMX1 bitstream.
    pub(crate) fn encode_raw(
        &mut self,
        frame: &MediaFrame,
        quality: Quality,
    ) -> Result<(Vec<u8>, Duration), OmtError> {
        let codec = Codec::from_i32(frame.codec).ok_or_else(|| {
            OmtError::InvalidArgument(format!("unsupported video codec {}", frame.codec))
        })?;
        let width = frame.width;
        let height = frame.height;
        if width < 16 || height < 16 {
            return Err(OmtError::InvalidArgument(format!(
                "video frame too small: {width}x{height}"
            )));
        }
        if frame.data.is_empty() {
            return Err(OmtError::InvalidArgument(
                "video frame data is empty".into(),
            ));
        }

        let stride = effective_stride(codec, width, frame.stride)?;
        let profile = vmx_profile(quality);
        let color_space = map_color_space(frame.color_space);
        self.ensure_codec(width, height, profile, color_space)?;
        let vmx = self.codec.as_mut().expect("codec after ensure");

        let t0 = Instant::now();
        match codec {
            Codec::Uyvy => {
                require_len(&frame.data, stride * height as usize, "UYVY")?;
                vmx.encode_uyvy(&frame.data, stride)?;
            }
            Codec::Yuy2 => {
                require_len(&frame.data, stride * height as usize, "YUY2")?;
                vmx.encode_yuy2(&frame.data, stride)?;
            }
            Codec::Bgra => {
                require_len(&frame.data, stride * height as usize, "BGRA")?;
                if frame.flags.contains(VideoFlags::ALPHA) {
                    vmx.encode_bgra(&frame.data, stride)?;
                } else {
                    vmx.encode_bgrx(&frame.data, stride)?;
                }
            }
            Codec::Nv12 => {
                let y_size = stride * height as usize;
                require_len(
                    &frame.data,
                    y_size + stride * (height as usize).div_ceil(2),
                    "NV12",
                )?;
                let (y, uv) = frame.data.split_at(y_size);
                vmx.encode_nv12(y, stride, uv, stride)?;
            }
            Codec::Yv12 => {
                let half_stride = stride / 2;
                let half_h = (height as usize) / 2;
                let y_size = stride * height as usize;
                let uv_size = half_stride * half_h;
                require_len(&frame.data, y_size + uv_size * 2, "YV12")?;
                let y = &frame.data[..y_size];
                let v = &frame.data[y_size..y_size + uv_size];
                let u = &frame.data[y_size + uv_size..y_size + uv_size * 2];
                vmx.encode_yv12(y, stride, u, half_stride, v, half_stride)?;
            }
            Codec::Vmx1 => {
                return Err(OmtError::InvalidArgument(
                    "VMX1 frames are passed through without encoding".into(),
                ));
            }
            Codec::Uyva => return Err(OmtError::NotImplemented("UYVA encode")),
            Codec::P216 => return Err(OmtError::NotImplemented("P216 encode")),
            Codec::Pa16 => return Err(OmtError::NotImplemented("PA16 encode")),
            Codec::Fpa1 => {
                return Err(OmtError::InvalidArgument("FPA1 is an audio codec".into()));
            }
        }

        let bitstream = self.save_bitstream()?;
        Ok((bitstream, t0.elapsed()))
    }

    fn ensure_codec(
        &mut self,
        width: i32,
        height: i32,
        profile: vmx::Profile,
        color_space: vmx::ColorSpace,
    ) -> Result<(), OmtError> {
        let recreate = self.codec.is_none()
            || self.width != width
            || self.height != height
            || self.profile != profile
            || self.color_space != color_space;
        if !recreate {
            return Ok(());
        }
        let prev_q = self.codec.as_ref().map(|c| c.quality());
        let mut codec = vmx::Codec::new(vmx::Config {
            width,
            height,
            profile,
            color_space,
        })?;
        if let Some(q) = prev_q {
            codec.set_quality(q);
        }
        self.codec = Some(codec);
        self.width = width;
        self.height = height;
        self.profile = profile;
        self.color_space = color_space;
        Ok(())
    }

    fn save_bitstream(&mut self) -> Result<Vec<u8>, OmtError> {
        let codec = self.codec.as_mut().expect("codec after encode");
        if self.buf.len() < (1 << 20) {
            self.buf.resize(1 << 20, 0);
        }
        loop {
            match codec.save_to(&mut self.buf) {
                Ok(n) => return Ok(self.buf[..n].to_vec()),
                Err(vmx::VmxError::OutputTooSmall { need, .. }) => {
                    self.buf
                        .resize(need.max(self.buf.len().saturating_mul(2)), 0);
                }
                Err(e) => return Err(e.into()),
            }
        }
    }
}

pub(crate) fn vmx_profile(quality: Quality) -> vmx::Profile {
    match quality {
        Quality::Low => vmx::Profile::OmtLq,
        Quality::Medium => vmx::Profile::OmtSq,
        Quality::High => vmx::Profile::OmtHq,
        Quality::Default => vmx::Profile::Default,
    }
}

fn map_color_space(cs: ColorSpace) -> vmx::ColorSpace {
    match cs {
        ColorSpace::Undefined => vmx::ColorSpace::Undefined,
        ColorSpace::Bt601 => vmx::ColorSpace::Bt601,
        ColorSpace::Bt709 => vmx::ColorSpace::Bt709,
    }
}

fn effective_stride(codec: Codec, width: i32, stride: i32) -> Result<usize, OmtError> {
    let packed = match codec {
        Codec::Uyvy | Codec::Yuy2 => width.saturating_mul(2),
        Codec::Bgra => width.saturating_mul(4),
        Codec::Nv12 | Codec::Yv12 => width,
        _ => width,
    };
    let stride = if stride <= 0 { packed } else { stride };
    if stride < width {
        return Err(OmtError::InvalidArgument(format!(
            "stride {stride} is smaller than width {width}"
        )));
    }
    Ok(stride as usize)
}

fn require_len(data: &[u8], need: usize, label: &str) -> Result<(), OmtError> {
    if data.len() < need {
        return Err(OmtError::InvalidArgument(format!(
            "{label} frame data too short: need {need}, have {}",
            data.len()
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::FrameType;

    fn uyvy_frame(width: i32, height: i32) -> MediaFrame {
        let stride = width * 2;
        MediaFrame {
            frame_type: FrameType::VIDEO,
            codec: Codec::Uyvy as i32,
            width,
            height,
            stride,
            data: vec![128u8; (stride * height) as usize],
            ..Default::default()
        }
    }

    #[test]
    fn encode_uyvy_returns_bitstream() {
        let mut enc = VideoEncoder::new();
        let frame = uyvy_frame(64, 64);
        let (bits, _elapsed) = enc.encode_raw(&frame, Quality::Medium).unwrap();
        assert!(!bits.is_empty());
        assert_ne!(bits, frame.data);
    }

    #[test]
    fn recreate_on_size_change() {
        let mut enc = VideoEncoder::new();
        enc.encode_raw(&uyvy_frame(64, 64), Quality::Low).unwrap();
        enc.encode_raw(&uyvy_frame(128, 64), Quality::Low).unwrap();
        assert_eq!(enc.width, 128);
        assert_eq!(enc.height, 64);
    }

    #[test]
    fn uyva_is_unimplemented() {
        let mut enc = VideoEncoder::new();
        let frame = MediaFrame {
            frame_type: FrameType::VIDEO,
            codec: Codec::Uyva as i32,
            width: 64,
            height: 64,
            stride: 128,
            data: vec![0u8; 64 * 64 * 3],
            ..Default::default()
        };
        match enc.encode_raw(&frame, Quality::Default) {
            Err(OmtError::NotImplemented("UYVA encode")) => {}
            other => panic!("expected NotImplemented, got {other:?}"),
        }
    }

    #[test]
    fn bgra_alpha_and_opaque() {
        let mut enc = VideoEncoder::new();
        let mut frame = MediaFrame {
            frame_type: FrameType::VIDEO,
            codec: Codec::Bgra as i32,
            width: 32,
            height: 32,
            stride: 32 * 4,
            data: vec![0u8; 32 * 32 * 4],
            flags: VideoFlags::ALPHA,
            ..Default::default()
        };
        assert!(enc.encode_raw(&frame, Quality::High).is_ok());
        frame.flags = VideoFlags::NONE;
        assert!(enc.encode_raw(&frame, Quality::High).is_ok());
    }
}
