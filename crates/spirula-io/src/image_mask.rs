//! Mask normalize (P3-F02).
//!
//! Binding: `docs/phase-3/image-orient-mask.md`. Parity with pin
//! `DataManager.cpp:decode_mask_into` / alpha gate (without boundary EDT).

use spirula_core::{ContextKey, DomainError, ErrorCode};

use crate::image_orient::{
    ApplyPolicy, ExifOrientation, MAX_IMAGE_PIXELS, orient_pixels, oriented_size,
};

fn err(code: ErrorCode, field: &str, msg: impl Into<String>) -> DomainError {
    DomainError::try_new(code, msg)
        .expect("msg")
        .with_context(ContextKey::Field, field)
        .expect("field")
}

/// Re-export budget check for callers without making `ensure_pixel_budget` pub twice.
fn ensure_budget(w: u32, h: u32) -> Result<(), DomainError> {
    let n = u64::from(w).saturating_mul(u64::from(h));
    if w == 0 || h == 0 {
        return Err(err(
            ErrorCode::ValueOutOfRange,
            "mask_size",
            format!("positive dimensions required, got {w}x{h}"),
        ));
    }
    if n > MAX_IMAGE_PIXELS {
        return Err(err(
            ErrorCode::ValueOutOfRange,
            "mask_size",
            format!("{w}x{h} exceeds MAX_IMAGE_PIXELS ({MAX_IMAGE_PIXELS})"),
        ));
    }
    Ok(())
}

/// How to interpret source samples before resize.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MaskSourceKind {
    /// File mask: any nonzero sample → 1 (`decode_mask_into`).
    NonZero,
    /// Alpha channel: keep ≥ 128 after optional resize (`decode_alpha_mask_into`).
    AlphaThreshold128,
}

/// Options for [`normalize_mask`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MaskNormalizeOptions {
    pub kind: MaskSourceKind,
    pub flip: bool,
    pub orientation: ExifOrientation,
    pub apply_policy: ApplyPolicy,
}

impl Default for MaskNormalizeOptions {
    fn default() -> Self {
        Self {
            kind: MaskSourceKind::NonZero,
            flip: false,
            orientation: ExifOrientation::IDENTITY,
            apply_policy: ApplyPolicy::TurnsOnly,
        }
    }
}

/// Nearest-neighbor resize of a single-channel buffer (row-major).
pub fn nearest_resize_u8(
    src: &[u8],
    src_w: u32,
    src_h: u32,
    dst_w: u32,
    dst_h: u32,
    dst: &mut [u8],
) -> Result<(), DomainError> {
    ensure_budget(src_w, src_h)?;
    ensure_budget(dst_w, dst_h)?;
    let need_s = (src_w as usize)
        .checked_mul(src_h as usize)
        .ok_or_else(|| err(ErrorCode::ValueOutOfRange, "mask_size", "overflow"))?;
    let need_d = (dst_w as usize)
        .checked_mul(dst_h as usize)
        .ok_or_else(|| err(ErrorCode::ValueOutOfRange, "mask_size", "overflow"))?;
    if src.len() < need_s || dst.len() < need_d {
        return Err(err(ErrorCode::MalformedRecord, "mask", "buffer short"));
    }
    if src_w == dst_w && src_h == dst_h {
        dst[..need_d].copy_from_slice(&src[..need_s]);
        return Ok(());
    }
    for y in 0..dst_h {
        let sy = ((u64::from(y) * u64::from(src_h)) / u64::from(dst_h)) as u32;
        for x in 0..dst_w {
            let sx = ((u64::from(x) * u64::from(src_w)) / u64::from(dst_w)) as u32;
            dst[(y * dst_w + x) as usize] = src[(sy * src_w + sx) as usize];
        }
    }
    Ok(())
}

/// Normalize a single-channel (or take last channel of multi-channel) mask to
/// `dst_w x dst_h` binary 0/1. Does not modify the source file.
#[allow(clippy::too_many_arguments)]
pub fn normalize_mask(
    samples: &[u8],
    width: u32,
    height: u32,
    channels: u32,
    dst_w: u32,
    dst_h: u32,
    opts: MaskNormalizeOptions,
    dst: &mut [u8],
) -> Result<(), DomainError> {
    ensure_budget(width, height)?;
    ensure_budget(dst_w, dst_h)?;
    if !(1..=4).contains(&channels) {
        return Err(err(
            ErrorCode::InvalidArgument,
            "channels",
            format!("mask channels must be 1..=4, got {channels}"),
        ));
    }
    let px = (width as usize)
        .checked_mul(height as usize)
        .ok_or_else(|| err(ErrorCode::ValueOutOfRange, "mask_size", "overflow"))?;
    let need = px
        .checked_mul(channels as usize)
        .ok_or_else(|| err(ErrorCode::ValueOutOfRange, "mask_size", "overflow"))?;
    if samples.len() < need {
        return Err(err(ErrorCode::MalformedRecord, "mask", "buffer short"));
    }

    // Extract plane used as mask.
    let mut plane = vec![0u8; px];
    let ch = channels as usize;
    let take = ch - 1;
    for i in 0..px {
        plane[i] = samples[i * ch + take];
    }

    match opts.kind {
        MaskSourceKind::NonZero => {
            for v in &mut plane {
                *v = u8::from(*v != 0);
            }
        }
        MaskSourceKind::AlphaThreshold128 => {
            // Threshold after resize in reference; we threshold after orient+resize too.
        }
    }

    // Orient (single channel).
    let xf = {
        let mut t = opts.orientation.transform();
        if matches!(opts.apply_policy, ApplyPolicy::TurnsOnly) {
            t.mirror = false;
        }
        t
    };
    let (ow, oh) = oriented_size(width, height, xf.turns_cw);
    let mut oriented = plane;
    if !xf.identity() {
        let mut buf = vec![0u8; (ow as usize) * (oh as usize)];
        orient_pixels(&oriented, width, height, 1, xf, &mut buf)?;
        oriented = buf;
    } else {
        let _ = xf;
    }

    let mut resized = vec![0u8; (dst_w as usize) * (dst_h as usize)];
    nearest_resize_u8(&oriented, ow, oh, dst_w, dst_h, &mut resized)?;

    if matches!(opts.kind, MaskSourceKind::AlphaThreshold128) {
        for v in &mut resized {
            *v = u8::from(*v >= 128);
        }
    }

    if opts.flip {
        for v in &mut resized {
            *v = u8::from(*v == 0);
        }
    }

    let n = (dst_w as usize) * (dst_h as usize);
    if dst.len() < n {
        return Err(err(ErrorCode::MalformedRecord, "mask", "dst short"));
    }
    dst[..n].copy_from_slice(&resized);
    Ok(())
}

/// Decode PNG/JPEG bytes for masking via `image`.
///
/// Gray / Luma files stay 1 channel (matches `stbi_load(..., req_comp=1)`).
/// Color files keep RGBA so callers can use [`MaskSourceKind::AlphaThreshold128`].
pub fn decode_mask_image_bytes(bytes: &[u8]) -> Result<(u32, u32, u32, Vec<u8>), DomainError> {
    let img = image::load_from_memory(bytes).map_err(|e| {
        err(
            ErrorCode::DecodeFailed,
            "mask",
            format!("decode failed: {e}"),
        )
    })?;
    ensure_budget(img.width(), img.height())?;
    use image::DynamicImage;
    match img {
        DynamicImage::ImageLuma8(g) => {
            let w = g.width();
            let h = g.height();
            Ok((w, h, 1, g.into_raw()))
        }
        DynamicImage::ImageLuma16(g) => {
            let w = g.width();
            let h = g.height();
            let raw: Vec<u8> = g.pixels().map(|p| (p.0[0] >> 8) as u8).collect();
            Ok((w, h, 1, raw))
        }
        other => {
            let rgba = other.to_rgba8();
            let w = rgba.width();
            let h = rgba.height();
            Ok((w, h, 4, rgba.into_raw()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nearest_resize_broadcast_1x1() {
        let src = [1u8];
        let mut dst = [0u8; 4];
        nearest_resize_u8(&src, 1, 1, 2, 2, &mut dst).unwrap();
        assert_eq!(dst, [1, 1, 1, 1]);
    }

    #[test]
    fn nonzero_binarize_and_flip() {
        let samples = [0u8, 10, 0, 255]; // 2x2
        let mut dst = [0u8; 4];
        let opts = MaskNormalizeOptions {
            kind: MaskSourceKind::NonZero,
            flip: true,
            ..MaskNormalizeOptions::default()
        };
        normalize_mask(&samples, 2, 2, 1, 2, 2, opts, &mut dst).unwrap();
        assert_eq!(dst, [1, 0, 1, 0]);
    }

    #[test]
    fn alpha_threshold_after_resize() {
        let samples = [0u8, 0, 0, 200, 0, 0, 0, 50]; // 2x1 RGBA
        let mut dst = [0u8; 2];
        let opts = MaskNormalizeOptions {
            kind: MaskSourceKind::AlphaThreshold128,
            ..MaskNormalizeOptions::default()
        };
        normalize_mask(&samples, 2, 1, 4, 2, 1, opts, &mut dst).unwrap();
        assert_eq!(dst, [1, 0]);
    }

    #[test]
    fn rejects_bad_channel_count() {
        let mut dst = [0u8; 1];
        let err = normalize_mask(
            &[0],
            1,
            1,
            0,
            1,
            1,
            MaskNormalizeOptions::default(),
            &mut dst,
        )
        .unwrap_err();
        assert_eq!(err.code(), ErrorCode::InvalidArgument);
    }
}
