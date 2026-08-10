//! Public OMT types (aligned with libomtnet).

/// Frame type bit flags (wire: Metadata=1, Video=2, Audio=4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct FrameType(pub u8);

impl FrameType {
    /// No frame type.
    pub const NONE: Self = Self(0);
    /// Metadata frames.
    pub const METADATA: Self = Self(1);
    /// Video frames.
    pub const VIDEO: Self = Self(2);
    /// Audio frames.
    pub const AUDIO: Self = Self(4);

    /// Returns true if `other` bits are set.
    pub const fn contains(self, other: Self) -> bool {
        (self.0 & other.0) == other.0
    }
}

impl std::ops::BitOr for FrameType {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

impl std::ops::BitOrAssign for FrameType {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

/// Video frame flags (wire values).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct VideoFlags(pub i32);

impl VideoFlags {
    /// No flags.
    pub const NONE: Self = Self(0);
    /// Interlaced.
    pub const INTERLACED: Self = Self(1);
    /// Alpha channel present.
    pub const ALPHA: Self = Self(2);
    /// Premultiplied alpha.
    pub const PREMULTIPLIED: Self = Self(4);
    /// 1/8 preview frame.
    pub const PREVIEW: Self = Self(8);
    /// High bit depth (P216/PA16).
    pub const HIGH_BIT_DEPTH: Self = Self(16);

    /// Returns true if `other` bits are set.
    pub const fn contains(self, other: Self) -> bool {
        (self.0 & other.0) == other.0
    }
}

impl std::ops::BitOr for VideoFlags {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

/// Codec FourCC values as signed 32-bit wire integers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(i32)]
pub enum Codec {
    /// VMX1 video codec (`VMX1`).
    Vmx1 = 0x3158_4D56,
    /// Floating-point planar audio (`FPA1`).
    Fpa1 = 0x3141_5046,
    /// UYVY 4:2:2.
    Uyvy = 0x5956_5955,
    /// YUY2 4:2:2.
    Yuy2 = 0x3259_5559,
    /// BGRA.
    Bgra = 0x4152_4742,
    /// NV12.
    Nv12 = 0x3231_564E,
    /// YV12.
    Yv12 = 0x3231_5659,
    /// UYVA.
    Uyva = 0x4156_5955,
    /// P216.
    P216 = 0x3631_3250,
    /// PA16.
    Pa16 = 0x3631_4150,
}

impl Codec {
    /// Create from raw FourCC (`i32` on the wire).
    pub fn from_i32(v: i32) -> Option<Self> {
        match v {
            x if x == Self::Vmx1 as i32 => Some(Self::Vmx1),
            x if x == Self::Fpa1 as i32 => Some(Self::Fpa1),
            x if x == Self::Uyvy as i32 => Some(Self::Uyvy),
            x if x == Self::Yuy2 as i32 => Some(Self::Yuy2),
            x if x == Self::Bgra as i32 => Some(Self::Bgra),
            x if x == Self::Nv12 as i32 => Some(Self::Nv12),
            x if x == Self::Yv12 as i32 => Some(Self::Yv12),
            x if x == Self::Uyva as i32 => Some(Self::Uyva),
            x if x == Self::P216 as i32 => Some(Self::P216),
            x if x == Self::Pa16 as i32 => Some(Self::Pa16),
            _ => None,
        }
    }

    /// Create from unsigned FourCC bits.
    pub fn from_u32(v: u32) -> Option<Self> {
        Self::from_i32(v as i32)
    }

    /// Wire value as `i32`.
    pub const fn as_i32(self) -> i32 {
        self as i32
    }
}

/// Color space (matches OMT / VMX).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[repr(i32)]
pub enum ColorSpace {
    /// Undefined (BT601 SD / BT709 HD).
    #[default]
    Undefined = 0,
    /// BT.601.
    Bt601 = 601,
    /// BT.709.
    Bt709 = 709,
}

/// Preferred uncompressed video format on receive.
///
/// Deprecated: [`ReceiverSession`](crate::ReceiverSession) always decodes VMX1 to BGRA.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[deprecated(note = "ReceiverSession always outputs BGRA; this enum is ignored")]
pub enum PreferredVideoFormat {
    /// Always UYVY.
    #[default]
    Uyvy = 0,
    /// UYVY, or BGRA when alpha is present.
    UyvyOrBgra = 1,
    /// Always BGRA.
    Bgra = 2,
    /// UYVY, or UYVA when alpha is present.
    UyvyOrUyva = 3,
    /// Prefer high-bit-depth when available.
    UyvyOrUyvaOrP216OrPa16 = 4,
    /// Always P216.
    P216 = 5,
}

/// Receiver feature flags.
///
/// Deprecated: preview / compressed-only receive modes are not supported.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[deprecated(note = "ReceiverSession does not support preview/compressed-only flags")]
pub struct ReceiveFlags(pub u32);

#[allow(deprecated)]
impl ReceiveFlags {
    /// No flags.
    pub const NONE: Self = Self(0);
    /// Preview only.
    pub const PREVIEW: Self = Self(1);
    /// Include compressed copy.
    pub const INCLUDE_COMPRESSED: Self = Self(2);
    /// Compressed only (no decode).
    pub const COMPRESSED_ONLY: Self = Self(4);

    /// Returns true if `other` bits are set.
    pub const fn contains(self, other: Self) -> bool {
        (self.0 & other.0) == other.0
    }
}

/// Video encoding quality suggestion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[repr(i32)]
pub enum Quality {
    /// Defer to peer / default medium.
    #[default]
    Default = 0,
    /// Low quality.
    Low = 1,
    /// Medium quality.
    Medium = 50,
    /// High quality.
    High = 100,
}

impl Quality {
    /// Quality name used in metadata templates.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Default => "Default",
            Self::Low => "Low",
            Self::Medium => "Medium",
            Self::High => "High",
        }
    }
}

/// Tally lights (0 = off, 1 = on).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Tally {
    /// Preview tally.
    pub preview: i32,
    /// Program tally.
    pub program: i32,
}

impl Tally {
    /// Create a tally state.
    pub const fn new(preview: i32, program: i32) -> Self {
        Self { preview, program }
    }
}

/// Sender product information published via metadata.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SenderInfo {
    /// Product name.
    pub product_name: String,
    /// Manufacturer.
    pub manufacturer: String,
    /// Version string.
    pub version: String,
}

impl SenderInfo {
    /// Create sender info.
    pub fn new(
        product_name: impl Into<String>,
        manufacturer: impl Into<String>,
        version: impl Into<String>,
    ) -> Self {
        Self {
            product_name: product_name.into(),
            manufacturer: manufacturer.into(),
            version: version.into(),
        }
    }

    /// Serialize to OMTInfo XML.
    pub fn to_xml(&self) -> String {
        format!(
            "<OMTInfo ProductName=\"{}\" Manufacturer=\"{}\" Version=\"{}\" />",
            escape_xml_attr(&self.product_name),
            escape_xml_attr(&self.manufacturer),
            escape_xml_attr(&self.version),
        )
    }
}

fn escape_xml_attr(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Cumulative and interval statistics (matches libomtnet `OMTStatistics`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Statistics {
    /// Total bytes sent.
    pub bytes_sent: i64,
    /// Total bytes received.
    pub bytes_received: i64,
    /// Bytes sent since last sample.
    pub bytes_sent_since_last: i64,
    /// Bytes received since last sample.
    pub bytes_received_since_last: i64,
    /// Total frames.
    pub frames: i64,
    /// Frames since last sample.
    pub frames_since_last: i64,
    /// Dropped frames.
    pub frames_dropped: i64,
    /// Codec time accumulator.
    pub codec_time: i64,
    /// Codec time since last sample.
    pub codec_time_since_last: i64,
}

impl Statistics {
    /// Reset interval counters after a sample.
    pub fn mark_sample(&mut self) {
        self.bytes_sent_since_last = 0;
        self.bytes_received_since_last = 0;
        self.frames_since_last = 0;
        self.codec_time_since_last = 0;
    }

    /// Record a sent frame of `nbytes`.
    pub fn record_sent(&mut self, nbytes: usize) {
        let n = nbytes as i64;
        self.bytes_sent = self.bytes_sent.saturating_add(n);
        self.bytes_sent_since_last = self.bytes_sent_since_last.saturating_add(n);
        self.frames = self.frames.saturating_add(1);
        self.frames_since_last = self.frames_since_last.saturating_add(1);
    }

    /// Record a received frame of `nbytes`.
    pub fn record_received(&mut self, nbytes: usize) {
        let n = nbytes as i64;
        self.bytes_received = self.bytes_received.saturating_add(n);
        self.bytes_received_since_last = self.bytes_received_since_last.saturating_add(n);
        self.frames = self.frames.saturating_add(1);
        self.frames_since_last = self.frames_since_last.saturating_add(1);
    }

    /// Record a frame dropped because the send queue was full (libomtnet async pool exhaustion).
    pub fn record_dropped(&mut self) {
        self.frames_dropped = self.frames_dropped.saturating_add(1);
    }
}

/// Owned media frame (video, audio, or metadata).
#[derive(Debug, Clone, PartialEq)]
pub struct MediaFrame {
    /// Frame type.
    pub frame_type: FrameType,
    /// Timestamp in 100 ns ticks (`-1` = auto).
    pub timestamp: i64,
    /// Codec FourCC as `i32`.
    pub codec: i32,
    /// Video width.
    pub width: i32,
    /// Video height.
    pub height: i32,
    /// Row stride in bytes.
    pub stride: i32,
    /// Video flags.
    pub flags: VideoFlags,
    /// Frame rate numerator.
    pub frame_rate_n: i32,
    /// Frame rate denominator.
    pub frame_rate_d: i32,
    /// Display aspect ratio.
    pub aspect_ratio: f32,
    /// Color space.
    pub color_space: ColorSpace,
    /// Audio sample rate.
    pub sample_rate: i32,
    /// Audio channel count.
    pub channels: i32,
    /// Samples per channel.
    pub samples_per_channel: i32,
    /// Active audio channel bitfield (FPA1).
    pub active_channels: u32,
    /// Payload bytes.
    pub data: Vec<u8>,
    /// Optional compressed VMX1 copy.
    pub compressed: Option<Vec<u8>>,
    /// Optional per-frame metadata XML (without requiring trailing NUL in this field).
    pub frame_metadata: Option<String>,
}

impl Default for MediaFrame {
    fn default() -> Self {
        Self {
            frame_type: FrameType::NONE,
            timestamp: 0,
            codec: 0,
            width: 0,
            height: 0,
            stride: 0,
            flags: VideoFlags::NONE,
            frame_rate_n: 60,
            frame_rate_d: 1,
            aspect_ratio: 16.0 / 9.0,
            color_space: ColorSpace::Undefined,
            sample_rate: 0,
            channels: 0,
            samples_per_channel: 0,
            active_channels: 0,
            data: Vec::new(),
            compressed: None,
            frame_metadata: None,
        }
    }
}

/// TCP port range start for senders.
pub const NETWORK_PORT_START: u16 = 6400;
/// TCP port range end for senders (inclusive).
pub const NETWORK_PORT_END: u16 = 6600;
/// Default discovery server port.
pub const DISCOVERY_SERVER_DEFAULT_PORT: u16 = 6399;

/// Socket send buffer size (libomtnet `NETWORK_SEND_BUFFER`).
pub const NETWORK_SEND_BUFFER: usize = 65_536;
/// Receive buffer used on sender-side peer sockets (libomtnet `NETWORK_SEND_RECEIVE_BUFFER`).
pub const NETWORK_SEND_RECEIVE_BUFFER: usize = 65_536;
/// Socket receive buffer size on receivers (8 MiB; libomtnet `NETWORK_RECEIVE_BUFFER`).
pub const NETWORK_RECEIVE_BUFFER: usize = 1_048_576 * 8;
/// Maximum bytes per async receive transfer.
pub const NETWORK_RECEIVE_MAX_TRANSFER: usize = 128 * 1024;
/// Outstanding async sends per channel before frames are dropped (libomtnet `NETWORK_ASYNC_COUNT`).
pub const NETWORK_ASYNC_COUNT: usize = 4;
/// Video frame pool depth.
pub const VIDEO_FRAME_POOL_COUNT: usize = 4;
/// Audio frame pool depth.
pub const AUDIO_FRAME_POOL_COUNT: usize = 10;
/// Minimum video buffer size.
pub const VIDEO_MIN_SIZE: usize = 65_536;
/// Maximum video buffer size.
pub const VIDEO_MAX_SIZE: usize = 10_485_760;
/// Minimum audio buffer size.
pub const AUDIO_MIN_SIZE: usize = 65_536;
/// Maximum audio buffer size.
pub const AUDIO_MAX_SIZE: usize = 1_048_576;
/// Bytes per audio sample (f32).
pub const AUDIO_SAMPLE_SIZE: usize = 4;
/// Maximum metadata queue depth.
pub const METADATA_MAX_COUNT: usize = 60;
/// Metadata frame buffer size.
pub const METADATA_FRAME_SIZE: usize = 65_536;

/// OMT URL scheme prefix.
pub const URL_PREFIX: &str = "omt://";
/// DNS-SD service type (without `.local`).
pub const DNSSD_SERVICE_TYPE: &str = "_omt._tcp";
/// Full DNS-SD name including `.local`.
pub const DNSSD_SERVICE_TYPE_LOCAL: &str = "_omt._tcp.local";
/// Maximum DNS-SD instance name length (`MACHINE (Name)`).
pub const MAX_INSTANCE_NAME_LENGTH: usize = 63;

/// Decoded VMX1 video frame (BGRA8, tightly packed unless `stride` > width×4).
#[derive(Debug, Clone)]
pub struct DecodedVideoFrame {
    /// Pixel width.
    pub width: u32,
    /// Pixel height.
    pub height: u32,
    /// Row stride in bytes.
    pub stride: u32,
    /// Timestamp (100 ns ticks).
    pub timestamp: i64,
    /// Frame rate numerator.
    pub frame_rate_n: i32,
    /// Frame rate denominator.
    pub frame_rate_d: i32,
    /// Color space used for YUV→RGB.
    pub color_space: ColorSpace,
    /// BGRA8 pixels (`Arc` for cheap handoff).
    pub pixels: std::sync::Arc<[u8]>,
    /// Optional per-frame metadata XML.
    pub frame_metadata: Option<std::sync::Arc<str>>,
}

/// Decoded FPA1 audio frame (planar f32 bytes).
#[derive(Debug, Clone)]
pub struct DecodedAudioFrame {
    /// Timestamp (100 ns ticks).
    pub timestamp: i64,
    /// Sample rate.
    pub sample_rate: i32,
    /// Channel count.
    pub channels: i32,
    /// Samples per channel.
    pub samples_per_channel: i32,
    /// Active channel bitfield from the wire.
    pub active_channels: u32,
    /// Planar f32 samples concatenated per channel.
    pub pcm_planar_f32: std::sync::Arc<[u8]>,
    /// Optional per-frame metadata XML.
    pub frame_metadata: Option<std::sync::Arc<str>>,
}

/// Metadata-only frame.
#[derive(Debug, Clone)]
pub struct MetadataFrame {
    /// Timestamp (100 ns ticks).
    pub timestamp: i64,
    /// XML / text payload.
    pub xml: std::sync::Arc<str>,
}

/// Receiver session statistics (transport + decode).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SessionStatistics {
    /// Total bytes received on the wire.
    pub bytes_received: u64,
    /// Bytes since last sample.
    pub bytes_received_since_last: u64,
    /// Successfully decoded video frames.
    pub frames_decoded: u64,
    /// Video frames dropped at the wire queue (backpressure).
    pub frames_dropped_wire: u64,
    /// Video frames dropped at decode failure or latest-wins overwrite.
    pub frames_dropped_decode: u64,
    /// Audio frames dropped or rejected.
    pub frames_dropped_audio: u64,
    /// Accumulated VMX decode time in nanoseconds.
    pub codec_time_ns: u64,
    /// Peak single-frame decode time in nanoseconds.
    pub codec_time_ns_peak: u64,
    /// Peak wire→decode age observed (microseconds).
    pub frame_age_us_peak: u64,
    /// Current compressed-video wire queue depth.
    pub wire_queue_depth: u32,
    /// Current decoded-video slot occupancy (0 or 1).
    pub decoded_queue_depth: u32,
    /// Reconnect attempts.
    pub reconnects: u64,
}

impl SessionStatistics {
    /// Reset interval counters.
    pub fn mark_sample(&mut self) {
        self.bytes_received_since_last = 0;
    }
}

