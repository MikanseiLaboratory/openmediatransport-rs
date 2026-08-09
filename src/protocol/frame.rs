//! Frame header structures.

use crate::error::OmtError;
use crate::types::{Codec, ColorSpace, FrameType, VideoFlags};

/// Protocol version (must be 1).
pub const PROTOCOL_VERSION: u8 = 1;
/// Size of the common frame header in bytes.
pub const HEADER_SIZE: usize = 16;
/// Size of the video extended header in bytes.
pub const VIDEO_EXT_HEADER_SIZE: usize = 32;
/// Size of the audio extended header in bytes.
pub const AUDIO_EXT_HEADER_SIZE: usize = 24;

/// Common 16-byte frame header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameHeader {
    /// Protocol version (1).
    pub version: u8,
    /// Frame type.
    pub frame_type: FrameType,
    /// Timestamp (10,000,000 = 1 second).
    pub timestamp: i64,
    /// Per-frame metadata length including NUL.
    pub metadata_length: u16,
    /// Extended header + data + metadata length (excludes this header).
    pub data_length: i32,
}

impl FrameHeader {
    /// Encode header to little-endian bytes.
    pub fn to_bytes(self) -> [u8; HEADER_SIZE] {
        let mut out = [0u8; HEADER_SIZE];
        out[0] = self.version;
        out[1] = self.frame_type.0;
        out[2..10].copy_from_slice(&self.timestamp.to_le_bytes());
        out[10..12].copy_from_slice(&self.metadata_length.to_le_bytes());
        out[12..16].copy_from_slice(&self.data_length.to_le_bytes());
        out
    }

    /// Decode header from little-endian bytes.
    pub fn from_bytes(buf: &[u8]) -> Result<Self, OmtError> {
        if buf.len() < HEADER_SIZE {
            return Err(OmtError::Protocol("frame header too short".into()));
        }
        let version = buf[0];
        if version != PROTOCOL_VERSION {
            return Err(OmtError::Protocol(format!(
                "unsupported protocol version {version}"
            )));
        }
        Ok(Self {
            version,
            frame_type: FrameType(buf[1]),
            timestamp: i64::from_le_bytes(buf[2..10].try_into().unwrap()),
            metadata_length: u16::from_le_bytes(buf[10..12].try_into().unwrap()),
            data_length: i32::from_le_bytes(buf[12..16].try_into().unwrap()),
        })
    }
}

/// Video extended header (32 bytes).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VideoHeader {
    /// Video codec FourCC.
    pub codec: Codec,
    /// Width in pixels.
    pub width: i32,
    /// Height in pixels.
    pub height: i32,
    /// Frame rate numerator.
    pub frame_rate_n: i32,
    /// Frame rate denominator.
    pub frame_rate_d: i32,
    /// Display aspect ratio (width/height).
    pub aspect_ratio: f32,
    /// Video flags.
    pub flags: VideoFlags,
    /// Color space.
    pub color_space: ColorSpace,
}

impl VideoHeader {
    /// Encode to little-endian bytes.
    pub fn to_bytes(self) -> [u8; VIDEO_EXT_HEADER_SIZE] {
        let mut out = [0u8; VIDEO_EXT_HEADER_SIZE];
        out[0..4].copy_from_slice(&self.codec.as_i32().to_le_bytes());
        out[4..8].copy_from_slice(&self.width.to_le_bytes());
        out[8..12].copy_from_slice(&self.height.to_le_bytes());
        out[12..16].copy_from_slice(&self.frame_rate_n.to_le_bytes());
        out[16..20].copy_from_slice(&self.frame_rate_d.to_le_bytes());
        out[20..24].copy_from_slice(&self.aspect_ratio.to_le_bytes());
        out[24..28].copy_from_slice(&self.flags.0.to_le_bytes());
        out[28..32].copy_from_slice(&(self.color_space as i32).to_le_bytes());
        out
    }

    /// Decode from little-endian bytes.
    pub fn from_bytes(buf: &[u8]) -> Result<Self, OmtError> {
        if buf.len() < VIDEO_EXT_HEADER_SIZE {
            return Err(OmtError::Protocol("video header too short".into()));
        }
        let codec_raw = i32::from_le_bytes(buf[0..4].try_into().unwrap());
        let codec = Codec::from_i32(codec_raw)
            .ok_or_else(|| OmtError::Protocol(format!("unknown video codec {codec_raw:#x}")))?;
        let cs = i32::from_le_bytes(buf[28..32].try_into().unwrap());
        let color_space = match cs {
            601 => ColorSpace::Bt601,
            709 => ColorSpace::Bt709,
            _ => ColorSpace::Undefined,
        };
        Ok(Self {
            codec,
            width: i32::from_le_bytes(buf[4..8].try_into().unwrap()),
            height: i32::from_le_bytes(buf[8..12].try_into().unwrap()),
            frame_rate_n: i32::from_le_bytes(buf[12..16].try_into().unwrap()),
            frame_rate_d: i32::from_le_bytes(buf[16..20].try_into().unwrap()),
            aspect_ratio: f32::from_le_bytes(buf[20..24].try_into().unwrap()),
            flags: VideoFlags(i32::from_le_bytes(buf[24..28].try_into().unwrap())),
            color_space,
        })
    }
}

/// Audio extended header (24 bytes).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioHeader {
    /// Audio codec FourCC (FPA1).
    pub codec: Codec,
    /// Sample rate.
    pub sample_rate: i32,
    /// Samples per channel in this frame.
    pub samples_per_channel: i32,
    /// Channel count.
    pub channels: i32,
    /// Active channel bitfield.
    pub active_channels: u32,
    /// Reserved.
    pub reserved1: i32,
}

impl AudioHeader {
    /// Encode to little-endian bytes.
    pub fn to_bytes(self) -> [u8; AUDIO_EXT_HEADER_SIZE] {
        let mut out = [0u8; AUDIO_EXT_HEADER_SIZE];
        out[0..4].copy_from_slice(&self.codec.as_i32().to_le_bytes());
        out[4..8].copy_from_slice(&self.sample_rate.to_le_bytes());
        out[8..12].copy_from_slice(&self.samples_per_channel.to_le_bytes());
        out[12..16].copy_from_slice(&self.channels.to_le_bytes());
        out[16..20].copy_from_slice(&self.active_channels.to_le_bytes());
        out[20..24].copy_from_slice(&self.reserved1.to_le_bytes());
        out
    }

    /// Decode from little-endian bytes.
    pub fn from_bytes(buf: &[u8]) -> Result<Self, OmtError> {
        if buf.len() < AUDIO_EXT_HEADER_SIZE {
            return Err(OmtError::Protocol("audio header too short".into()));
        }
        let codec_raw = i32::from_le_bytes(buf[0..4].try_into().unwrap());
        let codec = Codec::from_i32(codec_raw)
            .ok_or_else(|| OmtError::Protocol(format!("unknown audio codec {codec_raw:#x}")))?;
        Ok(Self {
            codec,
            sample_rate: i32::from_le_bytes(buf[4..8].try_into().unwrap()),
            samples_per_channel: i32::from_le_bytes(buf[8..12].try_into().unwrap()),
            channels: i32::from_le_bytes(buf[12..16].try_into().unwrap()),
            active_channels: u32::from_le_bytes(buf[16..20].try_into().unwrap()),
            reserved1: i32::from_le_bytes(buf[20..24].try_into().unwrap()),
        })
    }
}

/// A fully assembled on-wire frame (header + payload sections).
#[derive(Debug, Clone, PartialEq)]
pub struct AssembledFrame {
    /// Common header.
    pub header: FrameHeader,
    /// Optional video extended header.
    pub video: Option<VideoHeader>,
    /// Optional audio extended header.
    pub audio: Option<AudioHeader>,
    /// Codec payload (after extended header, before metadata).
    pub data: Vec<u8>,
    /// Per-frame metadata including trailing NUL when present.
    pub metadata: Vec<u8>,
}

impl AssembledFrame {
    /// Serialize the full frame (header + body).
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(HEADER_SIZE + self.header.data_length.max(0) as usize);
        out.extend_from_slice(&self.header.to_bytes());
        if let Some(v) = self.video {
            out.extend_from_slice(&v.to_bytes());
        }
        if let Some(a) = self.audio {
            out.extend_from_slice(&a.to_bytes());
        }
        out.extend_from_slice(&self.data);
        out.extend_from_slice(&self.metadata);
        out
    }

    /// Parse a complete frame from bytes (header + body already available).
    pub fn from_bytes(buf: &[u8]) -> Result<Self, OmtError> {
        let header = FrameHeader::from_bytes(buf)?;
        if header.data_length < 0 {
            return Err(OmtError::Protocol("negative data_length".into()));
        }
        let body_len = header.data_length as usize;
        if buf.len() < HEADER_SIZE + body_len {
            return Err(OmtError::Protocol("frame truncated".into()));
        }
        let body = &buf[HEADER_SIZE..HEADER_SIZE + body_len];
        let meta_len = header.metadata_length as usize;
        if body_len < meta_len {
            return Err(OmtError::Protocol("metadata longer than body".into()));
        }
        let payload_end = body_len - meta_len;
        let (ext_and_data, metadata) = body.split_at(payload_end);

        let mut video = None;
        let mut audio = None;
        let data = if header.frame_type.contains(FrameType::VIDEO) {
            if ext_and_data.len() < VIDEO_EXT_HEADER_SIZE {
                return Err(OmtError::Protocol("missing video extended header".into()));
            }
            video = Some(VideoHeader::from_bytes(ext_and_data)?);
            ext_and_data[VIDEO_EXT_HEADER_SIZE..].to_vec()
        } else if header.frame_type.contains(FrameType::AUDIO) {
            if ext_and_data.len() < AUDIO_EXT_HEADER_SIZE {
                return Err(OmtError::Protocol("missing audio extended header".into()));
            }
            audio = Some(AudioHeader::from_bytes(ext_and_data)?);
            ext_and_data[AUDIO_EXT_HEADER_SIZE..].to_vec()
        } else {
            // Metadata-only: body before metadata is empty / unused.
            ext_and_data.to_vec()
        };

        Ok(Self {
            header,
            video,
            audio,
            data,
            metadata: metadata.to_vec(),
        })
    }
}
