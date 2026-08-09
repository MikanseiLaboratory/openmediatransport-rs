//! Protocol header round-trip tests.

use openmediatransport::protocol::frame::{
    AssembledFrame, AudioHeader, FrameHeader, HEADER_SIZE, PROTOCOL_VERSION, VIDEO_EXT_HEADER_SIZE,
    VideoHeader,
};
use openmediatransport::types::{Codec, ColorSpace, FrameType, VideoFlags};

#[test]
fn frame_header_roundtrip() {
    let h = FrameHeader {
        version: PROTOCOL_VERSION,
        frame_type: FrameType::VIDEO,
        timestamp: 12_345_678,
        metadata_length: 4,
        data_length: 100,
    };
    let bytes = h.to_bytes();
    assert_eq!(bytes.len(), HEADER_SIZE);
    let parsed = FrameHeader::from_bytes(&bytes).unwrap();
    assert_eq!(parsed, h);
}

#[test]
fn video_header_roundtrip() {
    let h = VideoHeader {
        codec: Codec::Vmx1,
        width: 1920,
        height: 1080,
        frame_rate_n: 60,
        frame_rate_d: 1,
        aspect_ratio: 16.0 / 9.0,
        flags: VideoFlags::NONE,
        color_space: ColorSpace::Bt709,
    };
    let bytes = h.to_bytes();
    assert_eq!(bytes.len(), VIDEO_EXT_HEADER_SIZE);
    let parsed = VideoHeader::from_bytes(&bytes).unwrap();
    assert_eq!(parsed.codec, h.codec);
    assert_eq!(parsed.width, h.width);
    assert_eq!(parsed.height, h.height);
    assert_eq!(parsed.color_space, h.color_space);
}

#[test]
fn audio_header_roundtrip() {
    let h = AudioHeader {
        codec: Codec::Fpa1,
        sample_rate: 48000,
        samples_per_channel: 1024,
        channels: 2,
        active_channels: 0b11,
        reserved1: 0,
    };
    let bytes = h.to_bytes();
    let parsed = AudioHeader::from_bytes(&bytes).unwrap();
    assert_eq!(parsed, h);
}

#[test]
fn assembled_video_frame_roundtrip() {
    let video = VideoHeader {
        codec: Codec::Uyvy,
        width: 8,
        height: 2,
        frame_rate_n: 30,
        frame_rate_d: 1,
        aspect_ratio: 16.0 / 9.0,
        flags: VideoFlags::NONE,
        color_space: ColorSpace::Undefined,
    };
    let data = vec![0u8; 32];
    let metadata = b"<x/>\0".to_vec();
    let frame = AssembledFrame {
        header: FrameHeader {
            version: PROTOCOL_VERSION,
            frame_type: FrameType::VIDEO,
            timestamp: 1,
            metadata_length: metadata.len() as u16,
            data_length: (VIDEO_EXT_HEADER_SIZE + data.len() + metadata.len()) as i32,
        },
        video: Some(video),
        audio: None,
        data,
        metadata,
    };
    let bytes = frame.to_bytes();
    let parsed = AssembledFrame::from_bytes(&bytes).unwrap();
    assert_eq!(parsed.header, frame.header);
    assert_eq!(parsed.video, frame.video);
    assert_eq!(parsed.data, frame.data);
}
