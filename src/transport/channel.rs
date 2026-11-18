//! Streaming frame reassembly over a byte stream.

use std::io::{Read, Write};

use crate::error::OmtError;
use crate::protocol::frame::{AssembledFrame, FrameHeader, HEADER_SIZE};
use crate::types::{
    AUDIO_MAX_SIZE, FrameType, METADATA_FRAME_SIZE, NETWORK_RECEIVE_MAX_TRANSFER, VIDEO_MAX_SIZE,
};

/// Bidirectional OMT channel with streaming reassembly (128 KiB read cap).
#[derive(Debug)]
pub struct Channel {
    /// Primary frame type for this channel (hint).
    #[allow(dead_code)]
    pub frame_type: FrameType,
    /// Incoming reassembly buffer.
    buf: Vec<u8>,
    /// Read scratch reused by [`Channel::recv_frame`] (lazily sized).
    scratch: Vec<u8>,
}

impl Channel {
    /// Create a channel for the given frame-type hint.
    pub fn new(frame_type: FrameType) -> Self {
        Self {
            frame_type,
            buf: Vec::with_capacity(VIDEO_MIN_HINT),
            scratch: Vec::new(),
        }
    }

    /// Send a fully serialized frame.
    #[allow(dead_code)]
    pub fn send_bytes<W: Write>(&mut self, writer: &mut W, data: &[u8]) -> Result<(), OmtError> {
        writer.write_all(data)?;
        writer.flush()?;
        Ok(())
    }

    /// Send an assembled frame.
    #[allow(dead_code)]
    pub fn send_frame<W: Write>(
        &mut self,
        writer: &mut W,
        frame: &AssembledFrame,
    ) -> Result<(), OmtError> {
        self.send_bytes(writer, &frame.to_bytes())
    }

    /// Read available data into a caller-provided scratch buffer and try to pop a frame.
    pub fn recv_frame_into<R: Read>(
        &mut self,
        reader: &mut R,
        scratch: &mut [u8],
    ) -> Result<Option<AssembledFrame>, OmtError> {
        recv_frame_impl(&mut self.buf, scratch, reader)
    }

    /// Read available data (capped at 128 KiB per call) and try to pop a complete frame.
    ///
    /// Uses a scratch buffer owned by the channel, so polling loops do not
    /// allocate on every call. Callers with their own reusable scratch can
    /// use [`Channel::recv_frame_into`] instead.
    pub fn recv_frame<R: Read>(
        &mut self,
        reader: &mut R,
    ) -> Result<Option<AssembledFrame>, OmtError> {
        if self.scratch.len() < NETWORK_RECEIVE_MAX_TRANSFER {
            self.scratch.resize(NETWORK_RECEIVE_MAX_TRANSFER, 0);
        }
        recv_frame_impl(&mut self.buf, &mut self.scratch, reader)
    }

    /// Push bytes into the reassembly buffer (for testing / async adapters).
    /// Append bytes for tests / offline reassembly.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn push_bytes(&mut self, data: &[u8]) {
        self.buf.extend_from_slice(data);
    }

    /// Try to extract one complete frame from the buffer without reading.
    #[allow(dead_code)]
    pub fn try_pop_frame(&mut self) -> Result<Option<AssembledFrame>, OmtError> {
        try_pop_frame_impl(&mut self.buf)
    }

    /// Drop buffered bytes (protocol desync recovery).
    pub fn clear(&mut self) {
        self.buf.clear();
    }

    /// Bytes currently buffered awaiting a complete frame.
    #[allow(dead_code)]
    pub fn buffered_len(&self) -> usize {
        self.buf.len()
    }
}

/// `Channel::try_pop_frame` over a raw reassembly buffer.
///
/// The consumed prefix is removed with `drain`, which shifts the remainder to
/// the front of the same allocation. The previous `split_off` variant moved the
/// remainder into a fresh `Vec` every frame, so the reassembly buffer lost its
/// capacity and every subsequent read regrew it while recopying the contents.
fn try_pop_frame_impl(buf: &mut Vec<u8>) -> Result<Option<AssembledFrame>, OmtError> {
    if buf.len() < HEADER_SIZE {
        return Ok(None);
    }
    let header = match FrameHeader::from_bytes(buf) {
        Ok(h) => h,
        Err(e) => {
            buf.clear();
            return Err(e);
        }
    };
    if header.data_length < 0 {
        buf.clear();
        return Err(OmtError::Protocol("negative data_length".into()));
    }
    let data_len = header.data_length as usize;
    let max = max_payload_for(header.frame_type);
    if data_len > max {
        buf.clear();
        return Err(OmtError::Protocol(format!(
            "data_length {data_len} exceeds max {max}"
        )));
    }
    if !is_valid_frame_type(header.frame_type) {
        buf.clear();
        return Err(OmtError::Protocol(format!(
            "invalid frame type {}",
            header.frame_type.0
        )));
    }
    let total = HEADER_SIZE + data_len;
    if buf.len() < total {
        return Ok(None);
    }
    let frame = match AssembledFrame::from_bytes(&buf[..total]) {
        Ok(frame) => frame,
        Err(e) => {
            buf.clear();
            return Err(e);
        }
    };
    buf.drain(..total);
    Ok(Some(frame))
}

/// `Channel::recv_frame` / `Channel::recv_frame_into` over a raw reassembly
/// buffer and scratch.
fn recv_frame_impl<R: Read>(
    buf: &mut Vec<u8>,
    scratch: &mut [u8],
    reader: &mut R,
) -> Result<Option<AssembledFrame>, OmtError> {
    if let Some(frame) = try_pop_frame_impl(buf)? {
        return Ok(Some(frame));
    }
    match reader.read(scratch) {
        Ok(0) => {
            if buf.is_empty() {
                return Ok(None);
            }
            return Err(OmtError::Network("connection closed mid-frame".into()));
        }
        Ok(n) => {
            if buf.len().saturating_add(n) > MAX_REASSEMBLY {
                buf.clear();
                return Err(OmtError::Protocol("reassembly buffer overflow".into()));
            }
            buf.extend_from_slice(&scratch[..n]);
        }
        Err(e)
            if matches!(
                e.kind(),
                std::io::ErrorKind::WouldBlock
                    | std::io::ErrorKind::TimedOut
                    | std::io::ErrorKind::Interrupted
            ) => {}
        Err(e) => return Err(e.into()),
    }
    match try_pop_frame_impl(buf)? {
        Some(frame) => Ok(Some(frame)),
        // Allocation-free: this path runs for every partial read of a large
        // frame. Callers match on the error kind, not the message.
        None => Err(OmtError::Io(std::io::Error::from(
            std::io::ErrorKind::TimedOut,
        ))),
    }
}

const VIDEO_MIN_HINT: usize = 65_536;
const MAX_REASSEMBLY: usize = VIDEO_MAX_SIZE + HEADER_SIZE + 64 * 1024;

fn max_payload_for(ft: FrameType) -> usize {
    if ft.contains(FrameType::VIDEO) {
        VIDEO_MAX_SIZE
    } else if ft.contains(FrameType::AUDIO) {
        AUDIO_MAX_SIZE
    } else {
        METADATA_FRAME_SIZE
    }
}

fn is_valid_frame_type(ft: FrameType) -> bool {
    let v = ft.0;
    v != 0 && (v & !(FrameType::VIDEO.0 | FrameType::AUDIO.0 | FrameType::METADATA.0)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::frame::PROTOCOL_VERSION;

    #[test]
    fn rejects_oversized_data_length() {
        let mut ch = Channel::new(FrameType::VIDEO);
        let mut hdr = [0u8; HEADER_SIZE];
        hdr[0] = PROTOCOL_VERSION;
        hdr[1] = FrameType::VIDEO.0;
        let huge = (VIDEO_MAX_SIZE as i32).saturating_add(1);
        hdr[12..16].copy_from_slice(&huge.to_le_bytes());
        ch.push_bytes(&hdr);
        let err = ch.try_pop_frame().unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("exceeds max") || msg.contains("protocol"),
            "{msg}"
        );
    }

    #[test]
    fn rejects_invalid_frame_type() {
        let mut ch = Channel::new(FrameType::VIDEO);
        let mut hdr = [0u8; HEADER_SIZE];
        hdr[0] = PROTOCOL_VERSION;
        hdr[1] = 0xFF;
        hdr[12..16].copy_from_slice(&0i32.to_le_bytes());
        ch.push_bytes(&hdr);
        assert!(ch.try_pop_frame().is_err());
    }

    #[test]
    fn accepts_complete_metadata_frame() {
        let mut ch = Channel::new(FrameType::METADATA);
        let header = FrameHeader {
            version: PROTOCOL_VERSION,
            frame_type: FrameType::METADATA,
            timestamp: 1,
            metadata_length: 0,
            data_length: 4,
        };
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&header.to_bytes());
        bytes.extend_from_slice(&[1, 2, 3, 4]);
        ch.push_bytes(&bytes);
        let frame = ch.try_pop_frame().unwrap().expect("frame");
        assert_eq!(frame.data, [1, 2, 3, 4]);
    }

    #[test]
    fn bad_version_clears_reassembly_buffer() {
        let mut ch = Channel::new(FrameType::VIDEO);
        let mut hdr = [0u8; HEADER_SIZE];
        hdr[0] = 99;
        hdr[1] = FrameType::VIDEO.0;
        ch.push_bytes(&hdr);
        assert!(ch.try_pop_frame().is_err());
        assert_eq!(ch.buffered_len(), 0);
    }
}
