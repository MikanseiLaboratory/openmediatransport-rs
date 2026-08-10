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

/// Unpack FPA1 payload into a single planar `f32` buffer (channel-major).
///
/// Layout: `[ch0_s0, ch0_s1, …, ch0_sN, ch1_s0, …]`.
pub fn decode_planar_into(
    data: &[u8],
    channels: usize,
    samples_per_channel: usize,
    active_channels: u32,
    out: &mut Vec<u8>,
) -> Result<(), OmtError> {
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

    let need = channels
        .checked_mul(samples_per_channel)
        .and_then(|n| n.checked_mul(AUDIO_SAMPLE_SIZE))
        .ok_or_else(|| OmtError::InvalidArgument("FPA1 size overflow".into()))?;
    out.clear();
    out.resize(need, 0);

    let mut offset = 0usize;
    for i in 0..channels {
        let flag = 1u32 << i;
        let plane_off = i * samples_per_channel * AUDIO_SAMPLE_SIZE;
        if (active_channels & flag) == flag {
            let bytes = samples_per_channel * AUDIO_SAMPLE_SIZE;
            out[plane_off..plane_off + bytes].copy_from_slice(&data[offset..offset + bytes]);
            offset += bytes;
        }
        // inactive channels already zero-filled by resize
    }
    Ok(())
}

/// Unpack FPA1 payload into planar `f32` channels, expanding inactive channels to zeros.
pub fn decode_planar(
    data: &[u8],
    channels: usize,
    samples_per_channel: usize,
    active_channels: u32,
) -> Result<Vec<Vec<f32>>, OmtError> {
    let mut flat = Vec::new();
    decode_planar_into(data, channels, samples_per_channel, active_channels, &mut flat)?;
    let mut planes = Vec::with_capacity(channels);
    for i in 0..channels {
        let mut plane = Vec::with_capacity(samples_per_channel);
        let base = i * samples_per_channel * AUDIO_SAMPLE_SIZE;
        for s in 0..samples_per_channel {
            let o = base + s * 4;
            let bytes: [u8; 4] = flat[o..o + 4].try_into().unwrap();
            plane.push(f32::from_le_bytes(bytes));
        }
        planes.push(plane);
    }
    Ok(planes)
}
