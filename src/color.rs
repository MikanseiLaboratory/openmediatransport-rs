//! Packed pixel helpers for decoded / previewed OMT video.
//!
//! VMX owns the hot YUV↔BGRA paths inside encode/decode (SSSE3/SSE2). This
//! module covers conversions that receivers and viewers commonly need after
//! decode (UI toolkits that expect RGBA, alpha visualization, UYVY previews).
//!
//! SIMD entry points follow the same convention as `vmx::color::convert`:
//! runtime `is_x86_feature_detected!` dispatch, scalar fallback, and
//! `#[target_feature]` on the intrinsic path.

/// Convert tightly packed BGRA8 to RGBA8.
pub fn bgra_to_rgba(bgra: &[u8]) -> Vec<u8> {
    let mut rgba = vec![0u8; bgra.len()];
    bgra_to_rgba_into(bgra, &mut rgba);
    rgba
}

/// Runtime-dispatched BGRA8 → RGBA8 into an existing buffer.
///
/// `bgra` and `rgba` should be the same length; conversion stops at the shorter
/// of the two (same semantics as a zip over bytes).
pub fn bgra_to_rgba_into(bgra: &[u8], rgba: &mut [u8]) {
    let n = bgra.len().min(rgba.len());
    #[cfg(target_arch = "x86_64")]
    {
        if is_x86_feature_detected!("ssse3") {
            // SAFETY: SSSE3 detected; only `[0..n)` is touched and lengths match.
            return unsafe { bgra_to_rgba_ssse3(&bgra[..n], &mut rgba[..n]) };
        }
    }
    bgra_to_rgba_scalar(&bgra[..n], &mut rgba[..n]);
}

/// Scalar BGRA8 → RGBA8 (also used as the SSSE3 tail).
pub fn bgra_to_rgba_scalar(bgra: &[u8], rgba: &mut [u8]) {
    for (dst, src) in rgba.chunks_exact_mut(4).zip(bgra.chunks_exact(4)) {
        dst[0] = src[2];
        dst[1] = src[1];
        dst[2] = src[0];
        dst[3] = src[3];
    }
}

/// SSSE3 BGRA → RGBA swizzle (16 bytes / 4 pixels per iteration).
///
/// # Safety
/// Caller must have detected SSSE3. `bgra` and `rgba` must have equal length.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "ssse3")]
pub unsafe fn bgra_to_rgba_ssse3(bgra: &[u8], rgba: &mut [u8]) {
    use std::arch::x86_64::*;
    // SAFETY: caller gated on SSSE3; pointer arithmetic stays within `len`.
    unsafe {
        // B G R A | B G R A | B G R A | B G R A
        // → R G B A | R G B A | R G B A | R G B A
        let shuffle = _mm_set_epi8(15, 12, 13, 14, 11, 8, 9, 10, 7, 4, 5, 6, 3, 0, 1, 2);
        let len = bgra.len();
        let simd_end = len & !15;
        let mut i = 0usize;
        while i < simd_end {
            let v = _mm_loadu_si128(bgra.as_ptr().add(i).cast());
            let out = _mm_shuffle_epi8(v, shuffle);
            _mm_storeu_si128(rgba.as_mut_ptr().add(i).cast(), out);
            i += 16;
        }
        if i < len {
            bgra_to_rgba_scalar(&bgra[i..], &mut rgba[i..]);
        }
    }
}

/// Replace RGB with grayscale alpha visualization (A→RGB, A=255).
pub fn bgra_alpha_mask(bgra: &[u8]) -> Vec<u8> {
    let mut rgba = Vec::with_capacity(bgra.len());
    for px in bgra.chunks_exact(4) {
        let a = px[3];
        rgba.extend_from_slice(&[a, a, a, 255]);
    }
    rgba
}

/// Convert studio-range BT.709 YUV to RGB (clamped).
fn yuv_to_rgb(y: u8, u: u8, v: u8) -> (u8, u8, u8) {
    let y = (y as i32 - 16).max(0);
    let u = u as i32 - 128;
    let v = v as i32 - 128;
    let r = (298 * y + 459 * v + 128) >> 8;
    let g = (298 * y - 55 * u - 136 * v + 128) >> 8;
    let b = (298 * y + 541 * u + 128) >> 8;
    (
        r.clamp(0, 255) as u8,
        g.clamp(0, 255) as u8,
        b.clamp(0, 255) as u8,
    )
}

/// Convert a UYVY frame to tightly packed RGBA8 for display / preview.
///
/// Full-frame codec I/O should use VMX decode (`decode_bgra` / `decode_uyvy`).
/// This helper is for lightweight UI thumbs and stills.
pub fn uyvy_to_rgba(uyvy: &[u8], width: u32, height: u32) -> Vec<u8> {
    let w = width as usize;
    let h = height as usize;
    let mut rgba = vec![0u8; w * h * 4];
    for y in 0..h {
        for x in (0..w).step_by(2) {
            let o = y * w * 2 + x * 2;
            if o + 3 >= uyvy.len() {
                break;
            }
            let u = uyvy[o];
            let y0 = uyvy[o + 1];
            let v = uyvy[o + 2];
            let y1 = uyvy[o + 3];
            let (r0, g0, b0) = yuv_to_rgb(y0, u, v);
            let (r1, g1, b1) = yuv_to_rgb(y1, u, v);
            let i0 = (y * w + x) * 4;
            rgba[i0] = r0;
            rgba[i0 + 1] = g0;
            rgba[i0 + 2] = b0;
            rgba[i0 + 3] = 255;
            if x + 1 < w {
                let i1 = i0 + 4;
                rgba[i1] = r1;
                rgba[i1 + 1] = g1;
                rgba[i1 + 2] = b1;
                rgba[i1 + 3] = 255;
            }
        }
    }
    rgba
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bgra_to_rgba_swaps_channels() {
        let bgra = [10u8, 20, 30, 255];
        assert_eq!(bgra_to_rgba(&bgra), vec![30, 20, 10, 255]);
    }

    #[test]
    fn bgra_to_rgba_handles_multi_pixel() {
        let mut bgra = Vec::new();
        for i in 0..8u8 {
            bgra.extend_from_slice(&[i, i + 1, i + 2, 255]);
        }
        let rgba = bgra_to_rgba(&bgra);
        assert_eq!(rgba.len(), bgra.len());
        for i in 0..8usize {
            let s = i * 4;
            assert_eq!(rgba[s], bgra[s + 2]);
            assert_eq!(rgba[s + 1], bgra[s + 1]);
            assert_eq!(rgba[s + 2], bgra[s]);
            assert_eq!(rgba[s + 3], 255);
        }
    }

    #[test]
    fn bgra_to_rgba_scalar_matches_dispatch() {
        let mut bgra = vec![0u8; 64];
        for (i, b) in bgra.iter_mut().enumerate() {
            *b = (i % 251) as u8;
        }
        let via_dispatch = bgra_to_rgba(&bgra);
        let mut via_scalar = vec![0u8; bgra.len()];
        bgra_to_rgba_scalar(&bgra, &mut via_scalar);
        assert_eq!(via_dispatch, via_scalar);
    }

    #[test]
    fn alpha_mask_uses_alpha() {
        let bgra = [1u8, 2, 3, 128];
        assert_eq!(bgra_alpha_mask(&bgra), vec![128, 128, 128, 255]);
    }

    #[test]
    fn uyvy_to_rgba_emits_opaque_pixels() {
        let uyvy = [128u8, 16, 128, 16]; // black pair
        let rgba = uyvy_to_rgba(&uyvy, 2, 1);
        assert_eq!(rgba.len(), 8);
        assert_eq!(rgba[3], 255);
        assert_eq!(rgba[7], 255);
    }
}
