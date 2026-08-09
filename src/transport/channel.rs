//! Streaming frame reassembly over a byte stream.

use std::io::{Read, Write};

use crate::error::OmtError;
use crate::protocol::frame::{AssembledFrame, FrameHeader, HEADER_SIZE};
use crate::types::{FrameType, NETWORK_RECEIVE_MAX_TRANSFER};

/// Bidirectional OMT channel with streaming reassembly (128 KiB read cap).
#[derive(Debug)]
pub struct Channel {
    /// Primary frame type for this channel (hint).
    #[allow(dead_code)]
    pub frame_type: FrameType,
    /// Incoming reassembly buffer.
    buf: Vec<u8>,
}

impl Channel {
    /// Create a channel for the given frame-type hint.
    pub fn new(frame_type: FrameType) -> Self {
        Self {
            frame_type,
            buf: Vec::new(),
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

    /// Read available data (capped at 128 KiB per call) and try to pop a complete frame.
    pub fn recv_frame<R: Read>(
        &mut self,
        reader: &mut R,
    ) -> Result<Option<AssembledFrame>, OmtError> {
        let mut tmp = vec![0u8; NETWORK_RECEIVE_MAX_TRANSFER];
        match reader.read(&mut tmp) {
            Ok(0) => {
                if self.buf.is_empty() {
                    return Ok(None);
                }
                // Peer closed mid-frame.
                return Err(OmtError::Network("connection closed mid-frame".into()));
            }
            Ok(n) => self.buf.extend_from_slice(&tmp[..n]),
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock
                        | std::io::ErrorKind::TimedOut
                        | std::io::ErrorKind::Interrupted
                ) => {}
            Err(e) => return Err(e.into()),
        }
        self.try_pop_frame()
    }

    /// Push bytes into the reassembly buffer (for testing / async adapters).
    pub fn push_bytes(&mut self, data: &[u8]) {
        self.buf.extend_from_slice(data);
    }

    /// Try to extract one complete frame from the buffer without reading.
    pub fn try_pop_frame(&mut self) -> Result<Option<AssembledFrame>, OmtError> {
        if self.buf.len() < HEADER_SIZE {
            return Ok(None);
        }
        let header = FrameHeader::from_bytes(&self.buf)?;
        if header.data_length < 0 {
            return Err(OmtError::Protocol("negative data_length".into()));
        }
        let total = HEADER_SIZE + header.data_length as usize;
        if self.buf.len() < total {
            return Ok(None);
        }
        let frame_bytes: Vec<u8> = self.buf.drain(..total).collect();
        AssembledFrame::from_bytes(&frame_bytes).map(Some)
    }

    /// Bytes currently buffered awaiting a complete frame.
    #[allow(dead_code)]
    pub fn buffered_len(&self) -> usize {
        self.buf.len()
    }
}
