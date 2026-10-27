//! Mask normalize (P3-F02).
//!
//! Binding: `docs/phase-3/image-orient-mask.md`. Parity with pin
//! `DataManager.cpp:decode_mask_into` / `decode_alpha_mask_into` /
//! `cpu_resize` (without boundary EDT).

use spirula_core::{ContextKey, DomainError, ErrorCode};

use crate::image_orient::{ApplyPolicy, ExifOrientation, orient_pixels, oriented_size};
use crate::image_probe::{ensure_encoded_budget, ensure_pixel_budget, probe_image_size_bytes};

fn err(code: ErrorCode, field: &str, msg: impl Into<String>) -> DomainError {
    DomainError::try_new(code, msg)
        .expect("msg")
        .with_context(ContextKey::Field, field)
        .expect("field")
}

/// How to interpret source samples before resize.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MaskSourceKind {
    /// File mask: stbi `req_comp=1` gray, then any nonzero → 1 (alpha ignored).
    NonZero,
    /// Alpha plane: LA/RGBA alpha; L/RGB without alpha → all opaque (255).
    /// After area/bilinear resize, gate `>= 128`.
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

/// stb_image `stbi__compute_y` integer luma (RGB→gray when `req_comp=1`).
#[inline]
pub fn stbi_luma_u8(r: u8, g: u8, b: u8) -> u8 {
    ((u32::from(r) * 77 + u32::from(g) * 150 + u32::from(b) * 29) >> 8) as u8
}

/// Extract NonZero plane: gray via stbi Y, alpha channel ignored for RGBA/LA.
pub fn extract_nonzero_plane(samples: &[u8], width: u32, height: u32, channels: u32) -> Vec<u8> {
    let px = (width as usize) * (height as usize);
    let ch = channels as usize;
    let mut plane = vec![0u8; px];
    for (i, out) in plane.iter_mut().enumerate() {
        let base = i * ch;
        let y = match ch {
            1 => samples[base],
            2 => samples[base], // L of LA — ignore A for NonZero file masks
            3 | 4 => stbi_luma_u8(samples[base], samples[base + 1], samples[base + 2]),
            _ => samples[base],
        };
        *out = u8::from(y != 0);
    }
    plane
}

/// Extract alpha plane for AlphaThreshold128. Returns 0..=255 samples.
/// Channels without alpha (L/RGB) yield all-255 (opaque).
pub fn extract_alpha_plane(samples: &[u8], width: u32, height: u32, channels: u32) -> Vec<u8> {
    let px = (width as usize) * (height as usize);
    let ch = channels as usize;
    let mut plane = vec![255u8; px];
    match ch {
        2 => {
            for i in 0..px {
                plane[i] = samples[i * 2 + 1];
            }
        }
        4 => {
            for i in 0..px {
                plane[i] = samples[i * 4 + 3];
            }
        }
        _ => {
            // L / RGB: no alpha → opaque
        }
    }
    plane
}

/// Nearest-neighbor resize matching `cpu_nearest_resize_u8` (DataManager.cpp).
pub fn nearest_resize_u8(
    src: &[u8],
    src_w: u32,
    src_h: u32,
    dst_w: u32,
    dst_h: u32,
    dst: &mut [u8],
) -> Result<(), DomainError> {
    ensure_pixel_budget(src_w, src_h)?;
    ensure_pixel_budget(dst_w, dst_h)?;
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
    let sx = (src_w as f32) / (dst_w as f32);
    let sy = (src_h as f32) / (dst_h as f32);
    let sh = src_h as i32;
    let sw = src_w as i32;
    for y in 0..dst_h {
        let v = (y as f32 + 0.5) * sy - 0.5;
        let ys = ((v + 0.5).floor() as i32).clamp(0, sh - 1) as u32;
        for x in 0..dst_w {
            let u = (x as f32 + 0.5) * sx - 0.5;
            let xs = ((u + 0.5).floor() as i32).clamp(0, sw - 1) as u32;
            dst[(y * dst_w + x) as usize] = src[(ys * src_w + xs) as usize];
        }
    }
    Ok(())
}

/// Area (down) / bilinear (up/equal) single-channel resize — `cpu_resize<T,1>`.
pub fn cpu_resize_u8(
    src: &[u8],
    src_w: u32,
    src_h: u32,
    dst_w: u32,
    dst_h: u32,
    dst: &mut [u8],
) -> Result<(), DomainError> {
    ensure_pixel_budget(src_w, src_h)?;
    ensure_pixel_budget(dst_w, dst_h)?;
    let need_s = (src_w as usize) * (src_h as usize);
    let need_d = (dst_w as usize) * (dst_h as usize);
    if src.len() < need_s || dst.len() < need_d {
        return Err(err(ErrorCode::MalformedRecord, "mask", "buffer short"));
    }
    if src_w == dst_w && src_h == dst_h {
        dst[..need_d].copy_from_slice(&src[..need_s]);
        return Ok(());
    }
    let down = dst_h <= src_h && dst_w <= src_w && (dst_h < src_h || dst_w < src_w);
    if down {
        cpu_area_resize_u8(src, src_w, src_h, dst_w, dst_h, dst)
    } else {
        cpu_bilinear_resize_u8(src, src_w, src_h, dst_w, dst_h, dst)
    }
}

fn cpu_bilinear_resize_u8(
    src: &[u8],
    sw: u32,
    sh: u32,
    dw: u32,
    dh: u32,
    dst: &mut [u8],
) -> Result<(), DomainError> {
    let sx = (sw as f32) / (dw as f32);
    let sy = (sh as f32) / (dh as f32);
    let sh_i = sh as i32;
    let sw_i = sw as i32;
    for y in 0..dh {
        let v = (y as f32 + 0.5) * sy - 0.5;
        let y0 = v.floor() as i32;
        let fy = v - y0 as f32;
        let y0c = y0.clamp(0, sh_i - 1);
        let y1c = (y0 + 1).clamp(0, sh_i - 1);
        for x in 0..dw {
            let u = (x as f32 + 0.5) * sx - 0.5;
            let x0 = u.floor() as i32;
            let fx = u - x0 as f32;
            let x0c = x0.clamp(0, sw_i - 1);
            let x1c = (x0 + 1).clamp(0, sw_i - 1);
            let w00 = (1.0 - fx) * (1.0 - fy);
            let w10 = fx * (1.0 - fy);
            let w01 = (1.0 - fx) * fy;
            let w11 = fx * fy;
            let p00 = src[(y0c as u32 * sw + x0c as u32) as usize] as f32;
            let p10 = src[(y0c as u32 * sw + x1c as u32) as usize] as f32;
            let p01 = src[(y1c as u32 * sw + x0c as u32) as usize] as f32;
            let p11 = src[(y1c as u32 * sw + x1c as u32) as usize] as f32;
            let r = (w00 * p00 + w10 * p10 + w01 * p01 + w11 * p11).round();
            dst[(y * dw + x) as usize] = r.clamp(0.0, 255.0) as u8;
        }
    }
    Ok(())
}

fn cpu_area_resize_u8(
    src: &[u8],
    sw: u32,
    sh: u32,
    dw: u32,
    dh: u32,
    dst: &mut [u8],
) -> Result<(), DomainError> {
    let sy = (sh as f64) / (dh as f64);
    let sx = (sw as f64) / (dw as f64);
    for y in 0..dh {
        let v0 = f64::from(y) * sy;
        let v1 = f64::from(y + 1) * sy;
        let y0 = v0 as i32;
        let y1 = ((v1.ceil() as i32).max(y0 + 1)).min(sh as i32);
        for x in 0..dw {
            let u0 = f64::from(x) * sx;
            let u1 = f64::from(x + 1) * sx;
            let x0 = u0 as i32;
            let x1 = ((u1.ceil() as i32).max(x0 + 1)).min(sw as i32);
            let mut acc = 0.0f64;
            let mut wsum = 0.0f64;
            for yy in y0..y1 {
                let wy = ((f64::from(yy) + 1.0).min(v1)) - (f64::from(yy).max(v0));
                for xx in x0..x1 {
                    let wx = ((f64::from(xx) + 1.0).min(u1)) - (f64::from(xx).max(u0));
                    let w = wy * wx;
                    let p = src[(yy as u32 * sw + xx as u32) as usize] as f64;
                    acc += w * p;
                    wsum += w;
                }
            }
            let r = if wsum > 0.0 { acc / wsum } else { 0.0 };
            dst[(y * dw + x) as usize] = r.round().clamp(0.0, 255.0) as u8;
        }
    }
    Ok(())
}

/// Normalize mask to `dst_w x dst_h` binary 0/1. Does not modify the source file.
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
    ensure_pixel_budget(width, height)?;
    ensure_pixel_budget(dst_w, dst_h)?;
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

    let mut plane = match opts.kind {
        MaskSourceKind::NonZero => extract_nonzero_plane(samples, width, height, channels),
        MaskSourceKind::AlphaThreshold128 => extract_alpha_plane(samples, width, height, channels),
    };

    let xf = {
        let mut t = opts.orientation.transform();
        if matches!(opts.apply_policy, ApplyPolicy::TurnsOnly) {
            t.mirror = false;
        }
        t
    };
    let (ow, oh) = oriented_size(width, height, xf.turns_cw);
    if !xf.identity() {
        let mut buf = vec![0u8; (ow as usize) * (oh as usize)];
        orient_pixels(&plane, width, height, 1, xf, &mut buf)?;
        plane = buf;
    }

    let mut resized = vec![0u8; (dst_w as usize) * (dst_h as usize)];
    match opts.kind {
        MaskSourceKind::NonZero => {
            nearest_resize_u8(&plane, ow, oh, dst_w, dst_h, &mut resized)?;
        }
        MaskSourceKind::AlphaThreshold128 => {
            cpu_resize_u8(&plane, ow, oh, dst_w, dst_h, &mut resized)?;
            for v in &mut resized {
                *v = u8::from(*v >= 128);
            }
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

/// Decode PNG/JPEG bytes for masking, preserving real channel layout.
///
/// Budgets (encoded + header pixels) run before `load_from_memory`.
pub fn decode_mask_image_bytes(bytes: &[u8]) -> Result<(u32, u32, u32, Vec<u8>), DomainError> {
    ensure_encoded_budget(bytes.len())?;
    let (w, h) = probe_image_size_bytes(bytes)?;
    let img = image::load_from_memory(bytes).map_err(|e| {
        err(
            ErrorCode::DecodeFailed,
            "mask",
            format!("decode failed: {e}"),
        )
    })?;
    use image::DynamicImage;
    let (cw, ch, channels, raw) = match img {
        DynamicImage::ImageLuma8(g) => (g.width(), g.height(), 1u32, g.into_raw()),
        DynamicImage::ImageLumaA8(g) => (g.width(), g.height(), 2, g.into_raw()),
        DynamicImage::ImageRgb8(g) => (g.width(), g.height(), 3, g.into_raw()),
        DynamicImage::ImageRgba8(g) => (g.width(), g.height(), 4, g.into_raw()),
        DynamicImage::ImageLuma16(g) => {
            let raw: Vec<u8> = g.pixels().map(|p| (p.0[0] >> 8) as u8).collect();
            (g.width(), g.height(), 1, raw)
        }
        DynamicImage::ImageLumaA16(g) => {
            let mut raw = Vec::with_capacity(g.width() as usize * g.height() as usize * 2);
            for p in g.pixels() {
                raw.push((p.0[0] >> 8) as u8);
                raw.push((p.0[1] >> 8) as u8);
            }
            (g.width(), g.height(), 2, raw)
        }
        other => {
            // 16-bit RGB/RGBA etc. — keep true channel count via conversion.
            if other.color().has_alpha() {
                let rgba = other.to_rgba8();
                (rgba.width(), rgba.height(), 4, rgba.into_raw())
            } else {
                let rgb = other.to_rgb8();
                (rgb.width(), rgb.height(), 3, rgb.into_raw())
            }
        }
    };
    if cw != w || ch != h {
        return Err(err(
            ErrorCode::MalformedRecord,
            "mask",
            format!("decode size {cw}x{ch} != header {w}x{h}"),
        ));
    }
    Ok((w, h, channels, raw))
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
    fn nonzero_ignores_rgba_alpha() {
        // RGB all zero, alpha 255 → gray 0 → mask 0 (must not take alpha).
        let samples = [0u8, 0, 0, 255, 0, 0, 0, 255];
        let mut dst = [0u8; 2];
        let opts = MaskNormalizeOptions {
            kind: MaskSourceKind::NonZero,
            ..MaskNormalizeOptions::default()
        };
        normalize_mask(&samples, 2, 1, 4, 2, 1, opts, &mut dst).unwrap();
        assert_eq!(dst, [0, 0]);
    }

    #[test]
    fn alpha_from_rgba_threshold() {
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
    fn alpha_rgb_without_alpha_is_all_ones() {
        let samples = [10u8, 20, 30, 40, 50, 60]; // 2x1 RGB
        let mut dst = [0u8; 2];
        let opts = MaskNormalizeOptions {
            kind: MaskSourceKind::AlphaThreshold128,
            ..MaskNormalizeOptions::default()
        };
        normalize_mask(&samples, 2, 1, 3, 2, 1, opts, &mut dst).unwrap();
        assert_eq!(dst, [1, 1]);
    }

    #[test]
    fn rejects_bad_channel_count() {
        let mut dst = [0u8; 1];
        let e = normalize_mask(
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
        assert_eq!(e.code(), ErrorCode::InvalidArgument);
    }
}
