//! Mask normalize (P3-F02).
//!
//! Binding: `docs/phase-3/image-orient-mask.md`. Parity with pin
//! `DataManager.cpp:decode_mask_into` / `decode_alpha_mask_into` /
//! `cpu_resize` (without boundary EDT).

use spirula_core::{CancelToken, ContextKey, DomainError, ErrorCode};

use crate::cancel_checkpoint::CancelCheckpoint;
use crate::image_edt::apply_mask_boundary_offset_frac_cancelable;
use crate::image_orient::{ApplyPolicy, ExifOrientation, orient_pixels_cancelable, oriented_size};
use crate::image_probe::{
    ensure_encoded_budget, ensure_pixel_budget, load_dynamic, probe_image_size_bytes_hinted,
};

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
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MaskNormalizeOptions {
    pub kind: MaskSourceKind,
    pub flip: bool,
    /// `DataManagerConfig::mask_boundary_offset` (fraction of `sqrt(W*H)`).
    pub boundary_offset_frac: f32,
    pub orientation: ExifOrientation,
    pub apply_policy: ApplyPolicy,
}

impl Default for MaskNormalizeOptions {
    fn default() -> Self {
        Self {
            kind: MaskSourceKind::NonZero,
            flip: false,
            boundary_offset_frac: 0.0,
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

fn plane_layout(
    samples: &[u8],
    width: u32,
    height: u32,
    channels: u32,
) -> Result<(usize, usize), DomainError> {
    if !(1..=4).contains(&channels) {
        return Err(err(
            ErrorCode::InvalidArgument,
            "channels",
            format!("mask channels must be 1..=4, got {channels}"),
        ));
    }
    ensure_pixel_budget(width, height)?;
    let px = (width as usize)
        .checked_mul(height as usize)
        .ok_or_else(|| err(ErrorCode::ValueOutOfRange, "mask_size", "overflow"))?;
    let need = px
        .checked_mul(channels as usize)
        .ok_or_else(|| err(ErrorCode::ValueOutOfRange, "mask_size", "overflow"))?;
    if samples.len() < need {
        return Err(err(
            ErrorCode::MalformedRecord,
            "mask",
            format!("buffer short: need {need}, got {}", samples.len()),
        ));
    }
    Ok((px, channels as usize))
}

/// Extract NonZero plane: gray via stbi Y, alpha channel ignored for RGBA/LA.
pub fn extract_nonzero_plane(
    samples: &[u8],
    width: u32,
    height: u32,
    channels: u32,
) -> Result<Vec<u8>, DomainError> {
    extract_nonzero_plane_cancelable(samples, width, height, channels, None)
}

fn extract_nonzero_plane_cancelable(
    samples: &[u8],
    width: u32,
    height: u32,
    channels: u32,
    cancel: Option<&CancelToken>,
) -> Result<Vec<u8>, DomainError> {
    let mut cp = CancelCheckpoint::new(cancel);
    let (px, ch) = plane_layout(samples, width, height, channels)?;
    let mut plane = vec![0u8; px];
    for (i, out) in plane.iter_mut().enumerate() {
        cp.tick(1)?;
        let base = i * ch;
        let y = match ch {
            1 => samples[base],
            2 => samples[base], // L of LA — ignore A for NonZero file masks
            3 | 4 => stbi_luma_u8(samples[base], samples[base + 1], samples[base + 2]),
            _ => samples[base],
        };
        *out = u8::from(y != 0);
    }
    Ok(plane)
}

/// Extract alpha plane for AlphaThreshold128. Returns 0..=255 samples.
/// Channels without alpha (L/RGB) yield all-255 (opaque).
pub fn extract_alpha_plane(
    samples: &[u8],
    width: u32,
    height: u32,
    channels: u32,
) -> Result<Vec<u8>, DomainError> {
    extract_alpha_plane_cancelable(samples, width, height, channels, None)
}

fn extract_alpha_plane_cancelable(
    samples: &[u8],
    width: u32,
    height: u32,
    channels: u32,
    cancel: Option<&CancelToken>,
) -> Result<Vec<u8>, DomainError> {
    let mut cp = CancelCheckpoint::new(cancel);
    let (px, ch) = plane_layout(samples, width, height, channels)?;
    let mut plane = vec![255u8; px];
    match ch {
        2 => {
            for i in 0..px {
                cp.tick(1)?;
                plane[i] = samples[i * 2 + 1];
            }
        }
        4 => {
            for i in 0..px {
                cp.tick(1)?;
                plane[i] = samples[i * 4 + 3];
            }
        }
        _ => {
            // L / RGB: no alpha → opaque
            cp.tick(px.max(1))?;
        }
    }
    Ok(plane)
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
    nearest_resize_u8_cancelable(src, src_w, src_h, dst_w, dst_h, dst, None)
}

pub(crate) fn nearest_resize_u8_cancelable(
    src: &[u8],
    src_w: u32,
    src_h: u32,
    dst_w: u32,
    dst_h: u32,
    dst: &mut [u8],
    cancel: Option<&CancelToken>,
) -> Result<(), DomainError> {
    let mut cp = CancelCheckpoint::new(cancel);
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
            cp.tick(1)?;
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
    cpu_resize_u8_cancelable(src, src_w, src_h, dst_w, dst_h, dst, None)
}

pub(crate) fn cpu_resize_u8_cancelable(
    src: &[u8],
    src_w: u32,
    src_h: u32,
    dst_w: u32,
    dst_h: u32,
    dst: &mut [u8],
    cancel: Option<&CancelToken>,
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
        cpu_area_resize_u8(src, src_w, src_h, dst_w, dst_h, dst, cancel)
    } else {
        cpu_bilinear_resize_u8(src, src_w, src_h, dst_w, dst_h, dst, cancel)
    }
}

fn cpu_bilinear_resize_u8(
    src: &[u8],
    sw: u32,
    sh: u32,
    dw: u32,
    dh: u32,
    dst: &mut [u8],
    cancel: Option<&CancelToken>,
) -> Result<(), DomainError> {
    let mut cp = CancelCheckpoint::new(cancel);
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
            cp.tick(1)?;
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
    cancel: Option<&CancelToken>,
) -> Result<(), DomainError> {
    let mut cp = CancelCheckpoint::new(cancel);
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
                    cp.tick(1)?;
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
    normalize_mask_cancelable(
        samples, width, height, channels, dst_w, dst_h, opts, dst, None,
    )
}

/// Like [`normalize_mask`], observing `cancel` via bounded-chunk
/// [`CancelCheckpoint::tick`] in owned CPU loops (extract / orient / resize /
/// threshold / flip / EDT). Returns `SP-CAN-001` when canceled.
#[allow(clippy::too_many_arguments)]
pub fn normalize_mask_cancelable(
    samples: &[u8],
    width: u32,
    height: u32,
    channels: u32,
    dst_w: u32,
    dst_h: u32,
    opts: MaskNormalizeOptions,
    dst: &mut [u8],
    cancel: Option<&CancelToken>,
) -> Result<(), DomainError> {
    let mut cp = CancelCheckpoint::new(cancel);
    cp.poll_now()?;
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
        MaskSourceKind::NonZero => {
            extract_nonzero_plane_cancelable(samples, width, height, channels, cancel)?
        }
        MaskSourceKind::AlphaThreshold128 => {
            extract_alpha_plane_cancelable(samples, width, height, channels, cancel)?
        }
    };
    cp.poll_now()?;

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
        orient_pixels_cancelable(&plane, width, height, 1, xf, &mut buf, cancel)?;
        plane = buf;
    }
    cp.poll_now()?;

    let mut resized = vec![0u8; (dst_w as usize) * (dst_h as usize)];
    match opts.kind {
        MaskSourceKind::NonZero => {
            nearest_resize_u8_cancelable(&plane, ow, oh, dst_w, dst_h, &mut resized, cancel)?;
        }
        MaskSourceKind::AlphaThreshold128 => {
            cpu_resize_u8_cancelable(&plane, ow, oh, dst_w, dst_h, &mut resized, cancel)?;
            for v in &mut resized {
                cp.tick(1)?;
                *v = u8::from(*v >= 128);
            }
        }
    }
    cp.poll_now()?;

    // Flip before boundary offset (DataManager.cpp:547-551).
    if opts.flip {
        for v in &mut resized {
            cp.tick(1)?;
            *v = u8::from(*v == 0);
        }
    }
    apply_mask_boundary_offset_frac_cancelable(
        &mut resized,
        dst_h,
        dst_w,
        opts.boundary_offset_frac,
        cancel,
    )?;
    cp.poll_now()?;

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
    decode_mask_image_bytes_hinted(bytes, None)
}

/// Decode mask bytes with an optional lowercase extension hint.
///
/// TGA has no reliable magic; path-based prepare must pass `Some("tga")`.
pub fn decode_mask_image_bytes_hinted(
    bytes: &[u8],
    ext_hint: Option<&str>,
) -> Result<(u32, u32, u32, Vec<u8>), DomainError> {
    ensure_encoded_budget(bytes.len())?;
    let (w, h) = probe_image_size_bytes_hinted(bytes, ext_hint)?;
    let img = load_dynamic(bytes, ext_hint).map_err(|e| {
        // Remap field context to mask for callers.
        err(e.code(), "mask", e.message().to_string())
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

    /// HOLD c0b4c50 counterexample: black opaque / red transparent → [0,1] not [1,0].
    #[test]
    fn nonzero_black_opaque_red_transparent() {
        let samples = [0u8, 0, 0, 255, 255, 0, 0, 0];
        let mut dst = [0u8; 2];
        normalize_mask(
            &samples,
            2,
            1,
            4,
            2,
            1,
            MaskNormalizeOptions {
                kind: MaskSourceKind::NonZero,
                ..MaskNormalizeOptions::default()
            },
            &mut dst,
        )
        .unwrap();
        assert_eq!(dst, [0, 1]);
    }

    /// HOLD c0b4c50: alpha [0,255] 2→1 via area/bilinear must gate to 1 (not nearest→0).
    #[test]
    fn alpha_2x1_to_1x1_is_one() {
        let samples = [0u8, 0, 0, 0, 0, 0, 0, 255]; // 2x1 RGBA
        let mut dst = [0u8; 1];
        normalize_mask(
            &samples,
            2,
            1,
            4,
            1,
            1,
            MaskNormalizeOptions {
                kind: MaskSourceKind::AlphaThreshold128,
                ..MaskNormalizeOptions::default()
            },
            &mut dst,
        )
        .unwrap();
        assert_eq!(dst, [1]);
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

    #[test]
    fn normalize_mask_cancelable_observes_pre_canceled_token() {
        use spirula_core::CancelReason;
        let cancel = CancelToken::new();
        assert!(cancel.request(CancelReason::User));
        let samples = [0u8, 1, 0, 1];
        let mut dst = [0u8; 4];
        let e = normalize_mask_cancelable(
            &samples,
            2,
            2,
            1,
            2,
            2,
            MaskNormalizeOptions::default(),
            &mut dst,
            Some(&cancel),
        )
        .unwrap_err();
        assert_eq!(e.code(), ErrorCode::Canceled);
    }

    #[test]
    fn tga_mask_bytes_require_ext_hint() {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/images/tiny_2x2.tga");
        let b = std::fs::read(&path).unwrap();
        let (w, h, ch, _) = decode_mask_image_bytes_hinted(&b, Some("tga")).unwrap();
        assert_eq!((w, h, ch), (2, 2, 3));
        assert!(
            decode_mask_image_bytes(&b).is_err(),
            "unhinted TGA mask bytes must fail so path/extension hint stays required"
        );
    }

    /// Wide single-row bilinear upscale: cancel after work starts must observe
    /// within [`crate::CANCEL_CHECK_INTERVAL`] (not only at row boundaries).
    #[test]
    fn wide_row_bilinear_cancel_after_start_within_250ms() {
        use std::time::{Duration, Instant};

        use spirula_core::CancelReason;

        use crate::CANCEL_CHECK_INTERVAL;

        let width = 16_777_216u32;
        let mut dst = vec![0u8; width as usize];
        let token = CancelToken::new();
        let signal = token.clone();
        let start = Instant::now();
        let requester = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            signal.request(CancelReason::User);
            Instant::now()
        });
        let result = normalize_mask_cancelable(
            &[255],
            1,
            1,
            1,
            width,
            1,
            MaskNormalizeOptions {
                kind: MaskSourceKind::AlphaThreshold128,
                ..MaskNormalizeOptions::default()
            },
            &mut dst,
            Some(&token),
        );
        let end = Instant::now();
        let canceled_at = requester.join().expect("requester");
        assert_eq!(result.unwrap_err().code(), ErrorCode::Canceled);
        let observe = end.saturating_duration_since(canceled_at);
        assert!(
            observe <= CANCEL_CHECK_INTERVAL,
            "cancel observation {observe:?} exceeded {CANCEL_CHECK_INTERVAL:?}"
        );
        assert!(
            end.duration_since(start) < Duration::from_secs(2),
            "total {:?}",
            end.duration_since(start)
        );
    }

    /// Area downsample where one dest pixel accumulates the whole source:
    /// cancel after start must observe inside the source accumulation loop.
    #[test]
    fn area_downsample_cancel_after_start_within_250ms() {
        use std::time::{Duration, Instant};

        use spirula_core::CancelReason;

        use crate::CANCEL_CHECK_INTERVAL;

        // 4096x4096 → 1x1 area path; source loop is the long owned work.
        let side = 4096u32;
        let src = vec![255u8; (side as usize) * (side as usize)];
        let mut dst = [0u8; 1];
        let token = CancelToken::new();
        let signal = token.clone();
        let requester = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(20));
            signal.request(CancelReason::User);
            Instant::now()
        });
        let result = normalize_mask_cancelable(
            &src,
            side,
            side,
            1,
            1,
            1,
            MaskNormalizeOptions {
                kind: MaskSourceKind::AlphaThreshold128,
                ..MaskNormalizeOptions::default()
            },
            &mut dst,
            Some(&token),
        );
        let end = Instant::now();
        let canceled_at = requester.join().expect("requester");
        assert_eq!(result.unwrap_err().code(), ErrorCode::Canceled);
        let observe = end.saturating_duration_since(canceled_at);
        assert!(
            observe <= CANCEL_CHECK_INTERVAL,
            "cancel observation {observe:?} exceeded {CANCEL_CHECK_INTERVAL:?}"
        );
    }
}
