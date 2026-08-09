//! FPA1 floating-point planar audio codec.
//!
//! Encode skips silent channels (sparse) and returns an active-channel bitfield.
//! Decode expands missing channels to zeros.

use crate::error::OmtError;
use crate::types::AUDIO_SAMPLE_SIZE;

fn channel_is_silent(samples: &[f32]) -> bool {
    samples.iter().all(|&s| s.to_bits() == 0)
}

/// Pack planar `f32` channels into an FPA1 payload, omitting silent channels.
///
/// Returns `(payload, active_channels_bitfield)`.
pub fn encode_planar(channels: &[&[f32]]) -> Result<(Vec<u8>, u32), OmtError> {
    if channels.is_empty() {
        return Err(OmtError::InvalidArgument(
            "FPA1 requires at least one channel".into(),
        ));
    }
    if channels.len() > 32 {
        return Err(OmtError::InvalidArgument(
            "FPA1 supports at most 32 channels".into(),
        ));
    }
    let samples = channels[0].len();
    for ch in channels {
        if ch.len() != samples {
            return Err(OmtError::InvalidArgument(
                "all FPA1 channels must have the same sample count".into(),
            ));
        }
    }

    let mut active = 0u32;
    let mut out = Vec::new();
    for (i, ch) in channels.iter().enumerate() {
        if channel_is_silent(ch) {
            continue;
        }
        active |= 1u32 << i;
        for &s in *ch {
            out.extend_from_slice(&s.to_le_bytes());
        }
    }
    Ok((out, active))
}

/// Unpack FPA1 payload into planar `f32` channels, expanding inactive channels to zeros.
pub fn decode_planar(
    data: &[u8],
    channels: usize,
    samples_per_channel: usize,
    active_channels: u32,
) -> Result<Vec<Vec<f32>>, OmtError> {
    if channels == 0 || channels > 32 {
        return Err(OmtError::InvalidArgument(
            "FPA1 channel count must be 1..=32".into(),
        ));
    }
    let active_count = active_channels.count_ones() as usize;
    let expected = active_count
        .checked_mul(samples_per_channel)
        .and_then(|n| n.checked_mul(AUDIO_SAMPLE_SIZE))
        .ok_or_else(|| OmtError::InvalidArgument("FPA1 size overflow".into()))?;
    if data.len() < expected {
        return Err(OmtError::Protocol("FPA1 payload too short".into()));
    }

    let mut planes = Vec::with_capacity(channels);
    let mut offset = 0usize;
    for i in 0..channels {
        let flag = 1u32 << i;
        if (active_channels & flag) == flag {
            let mut plane = Vec::with_capacity(samples_per_channel);
            for _ in 0..samples_per_channel {
                let bytes: [u8; 4] = data[offset..offset + 4]
                    .try_into()
                    .map_err(|_| OmtError::Protocol("FPA1 truncated sample".into()))?;
                plane.push(f32::from_le_bytes(bytes));
                offset += 4;
            }
            planes.push(plane);
        } else {
            planes.push(vec![0.0f32; samples_per_channel]);
        }
    }
    Ok(planes)
}
