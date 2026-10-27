//! EXIF Orientation and pixel bake (P3-F02).
//!
//! Binding: `docs/phase-3/image-orient-mask.md`. Parity with pin
//! `9a24f099` `sfm/core/Exif.h` + `core/ImageOrient.h`. DataManager `apply`
//! turns-only policy is [`ApplyPolicy::TurnsOnly`].

use std::path::Path;

use spirula_core::{CancelToken, ContextKey, DomainError, ErrorCode};

use crate::cancel_checkpoint::CancelCheckpoint;

/// Hard cap on decoded / oriented pixel count (`AD-P3-F02-02`).
pub const MAX_IMAGE_PIXELS: u64 = 16_777_216;

/// Hard cap on encoded file / buffer size before decode (`AD-P3-F02-02`).
pub const MAX_ENCODED_BYTES: u64 = 64 * 1024 * 1024;

/// Config modes from `TrainConfig::exif_orientation`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExifOrientationMode {
    /// Do not read or apply EXIF.
    None,
    /// Level the scene; leave pixels as stored.
    Orient,
    /// Bake orientation into pixels at load.
    Apply,
}

impl ExifOrientationMode {
    pub fn parse(s: &str) -> Result<Self, DomainError> {
        match s {
            "none" => Ok(Self::None),
            "orient" => Ok(Self::Orient),
            "apply" => Ok(Self::Apply),
            other => Err(err(
                ErrorCode::InvalidArgument,
                "exif_orientation",
                format!("expected none|orient|apply, got {other}"),
            )),
        }
    }
}

/// How `apply` bakes pixels (AD-P3-F02-01).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApplyPolicy {
    /// `DataManager::turn_decoded` / parser `exif_quarter_turns` — mirror dropped.
    TurnsOnly,
    /// Full `ImageOrient.h` bake (turns then horizontal mirror).
    FullTransform,
}

/// EXIF Orientation tag value in `1..=8` (1 = stored as shown).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExifOrientation(u8);

impl ExifOrientation {
    pub const IDENTITY: Self = Self(1);

    pub fn from_u8(v: u8) -> Result<Self, DomainError> {
        Self::from_u16(u16::from(v))
    }

    /// EXIF Orientation is a TIFF SHORT; validate the full `u16` (257/262 must not become 1/6).
    pub fn from_u16(v: u16) -> Result<Self, DomainError> {
        if (1..=8).contains(&v) {
            Ok(Self(v as u8))
        } else {
            Err(err(
                ErrorCode::ValueOutOfRange,
                "exif_orientation",
                format!("expected 1..=8, got {v}"),
            ))
        }
    }

    pub fn get(self) -> u8 {
        self.0
    }

    pub fn transform(self) -> ExifTransform {
        exif_transform(self)
    }
}

/// Stored → displayed: clockwise quarter turns, then optional horizontal mirror.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExifTransform {
    pub turns_cw: u8,
    pub mirror: bool,
}

impl ExifTransform {
    pub fn identity(self) -> bool {
        self.turns_cw == 0 && !self.mirror
    }
}

/// `sfm::exifTransform` (`Exif.h:414-425`).
pub fn exif_transform(orientation: ExifOrientation) -> ExifTransform {
    match orientation.get() {
        2 => ExifTransform {
            turns_cw: 0,
            mirror: true,
        },
        3 => ExifTransform {
            turns_cw: 2,
            mirror: false,
        },
        4 => ExifTransform {
            turns_cw: 2,
            mirror: true,
        },
        5 => ExifTransform {
            turns_cw: 1,
            mirror: true,
        },
        6 => ExifTransform {
            turns_cw: 1,
            mirror: false,
        },
        7 => ExifTransform {
            turns_cw: 3,
            mirror: true,
        },
        8 => ExifTransform {
            turns_cw: 3,
            mirror: false,
        },
        _ => ExifTransform {
            turns_cw: 0,
            mirror: false,
        },
    }
}

/// Size after `turns_cw` quarter turns (`ImageOrient.h:oriented_size`).
pub fn oriented_size(width: u32, height: u32, turns_cw: u8) -> (u32, u32) {
    if turns_cw & 1 == 0 {
        (width, height)
    } else {
        (height, width)
    }
}

fn err(code: ErrorCode, field: &str, msg: impl Into<String>) -> DomainError {
    DomainError::try_new(code, msg)
        .expect("msg")
        .with_context(ContextKey::Field, field)
        .expect("field")
}

fn ensure_pixel_budget(w: u32, h: u32) -> Result<(), DomainError> {
    let n = u64::from(w).saturating_mul(u64::from(h));
    if w == 0 || h == 0 {
        return Err(err(
            ErrorCode::ValueOutOfRange,
            "image_size",
            format!("positive dimensions required, got {w}x{h}"),
        ));
    }
    if n > MAX_IMAGE_PIXELS {
        return Err(err(
            ErrorCode::ValueOutOfRange,
            "image_size",
            format!("{w}x{h} exceeds MAX_IMAGE_PIXELS ({MAX_IMAGE_PIXELS})"),
        ));
    }
    Ok(())
}

/// Interleaved pixel bake (`ImageOrient.h:orient_pixels`). `dst` length
/// `out_w*out_h*channels`; must not alias `src`.
pub fn orient_pixels(
    src: &[u8],
    width: u32,
    height: u32,
    channels: u32,
    transform: ExifTransform,
    dst: &mut [u8],
) -> Result<(u32, u32), DomainError> {
    orient_pixels_cancelable(src, width, height, channels, transform, dst, None)
}

/// Like [`orient_pixels`], observing cancel via bounded-chunk ticks on each
/// output pixel (wide rows included).
pub fn orient_pixels_cancelable(
    src: &[u8],
    width: u32,
    height: u32,
    channels: u32,
    transform: ExifTransform,
    dst: &mut [u8],
    cancel: Option<&CancelToken>,
) -> Result<(u32, u32), DomainError> {
    let mut cp = CancelCheckpoint::new(cancel);
    ensure_pixel_budget(width, height)?;
    if channels == 0 || channels > 4 {
        return Err(err(
            ErrorCode::InvalidArgument,
            "channels",
            format!("expected 1..=4, got {channels}"),
        ));
    }
    let t = (transform.turns_cw & 3) as u32;
    let (dw, dh) = oriented_size(width, height, transform.turns_cw);
    let c = channels as usize;
    let need_src = (width as usize)
        .checked_mul(height as usize)
        .and_then(|n| n.checked_mul(c))
        .ok_or_else(|| err(ErrorCode::ValueOutOfRange, "image_size", "overflow"))?;
    let need_dst = (dw as usize)
        .checked_mul(dh as usize)
        .and_then(|n| n.checked_mul(c))
        .ok_or_else(|| err(ErrorCode::ValueOutOfRange, "image_size", "overflow"))?;
    if src.len() < need_src {
        return Err(err(
            ErrorCode::MalformedRecord,
            "pixels",
            format!("src short: {} < {need_src}", src.len()),
        ));
    }
    if dst.len() < need_dst {
        return Err(err(
            ErrorCode::MalformedRecord,
            "pixels",
            format!("dst short: {} < {need_dst}", dst.len()),
        ));
    }
    let w = width as i32;
    let h = height as i32;
    let dw_i = dw as i32;
    let dh_i = dh as i32;
    for dy in 0..dh_i {
        for dx in 0..dw_i {
            cp.tick(1)?;
            let mx = if transform.mirror { dw_i - 1 - dx } else { dx };
            let (sx, sy) = match t {
                1 => (dy, h - 1 - mx),
                2 => (w - 1 - mx, h - 1 - dy),
                3 => (w - 1 - dy, mx),
                _ => (mx, dy),
            };
            let s = ((sy as usize) * width as usize + sx as usize) * c;
            let d = ((dy as usize) * dw as usize + dx as usize) * c;
            dst[d..d + c].copy_from_slice(&src[s..s + c]);
        }
    }
    Ok((dw, dh))
}

/// Bake using the selected apply policy.
pub fn apply_orientation(
    src: &[u8],
    width: u32,
    height: u32,
    channels: u32,
    orientation: ExifOrientation,
    policy: ApplyPolicy,
    dst: &mut [u8],
) -> Result<(u32, u32), DomainError> {
    apply_orientation_cancelable(src, width, height, channels, orientation, policy, dst, None)
}

/// Like [`apply_orientation`], observing cancel via bounded-chunk ticks in the
/// pixel bake loop when the transform is non-identity.
#[allow(clippy::too_many_arguments)]
pub fn apply_orientation_cancelable(
    src: &[u8],
    width: u32,
    height: u32,
    channels: u32,
    orientation: ExifOrientation,
    policy: ApplyPolicy,
    dst: &mut [u8],
    cancel: Option<&CancelToken>,
) -> Result<(u32, u32), DomainError> {
    let mut cp = CancelCheckpoint::new(cancel);
    cp.poll_now()?;
    let mut xf = exif_transform(orientation);
    if matches!(policy, ApplyPolicy::TurnsOnly) {
        xf.mirror = false;
    }
    if xf.identity() {
        // Still validate channels 1..=4 and budgets (ImageOrient contract).
        ensure_pixel_budget(width, height)?;
        if channels == 0 || channels > 4 {
            return Err(err(
                ErrorCode::InvalidArgument,
                "channels",
                format!("expected 1..=4, got {channels}"),
            ));
        }
        let n = (width as usize)
            .checked_mul(height as usize)
            .and_then(|n| n.checked_mul(channels as usize))
            .ok_or_else(|| err(ErrorCode::ValueOutOfRange, "image_size", "overflow"))?;
        if src.len() < n || dst.len() < n {
            return Err(err(ErrorCode::MalformedRecord, "pixels", "buffer short"));
        }
        dst[..n].copy_from_slice(&src[..n]);
        return Ok((width, height));
    }
    orient_pixels_cancelable(src, width, height, channels, xf, dst, cancel)
}

/// Read Orientation from a JPEG byte buffer. Missing / unreadable → 1.
pub fn read_jpeg_exif_orientation(bytes: &[u8]) -> Result<ExifOrientation, DomainError> {
    parse_jpeg_orientation(bytes)
}

/// Path helper: non-JPEG extensions return identity without opening (DatasetCommon).
pub fn read_path_exif_orientation(path: &Path) -> Result<ExifOrientation, DomainError> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if ext != "jpg" && ext != "jpeg" {
        return Ok(ExifOrientation::IDENTITY);
    }
    let bytes = crate::image_probe::read_image_bytes_capped(path)?;
    read_jpeg_exif_orientation(&bytes)
}

/// `read_exif_orientations` for a path list under a mode.
pub fn read_exif_orientations(
    mode: ExifOrientationMode,
    paths: &[impl AsRef<Path>],
) -> Result<Vec<ExifOrientation>, DomainError> {
    if matches!(mode, ExifOrientationMode::None) {
        return Ok(Vec::new());
    }
    let mut out = Vec::with_capacity(paths.len());
    let mut any_non_identity = false;
    for p in paths {
        let o = read_path_exif_orientation(p.as_ref())?;
        any_non_identity |= o != ExifOrientation::IDENTITY;
        out.push(o);
    }
    if !any_non_identity {
        out.clear();
    }
    Ok(out)
}

fn parse_jpeg_orientation(bytes: &[u8]) -> Result<ExifOrientation, DomainError> {
    if bytes.len() < 4 || bytes[0] != 0xFF || bytes[1] != 0xD8 {
        return Err(err(
            ErrorCode::UnsupportedFormat,
            "jpeg",
            "missing SOI marker",
        ));
    }
    let mut i = 2usize;
    while i + 3 < bytes.len() {
        if bytes[i] != 0xFF {
            return Err(err(
                ErrorCode::MalformedRecord,
                "jpeg",
                "expected marker 0xFF",
            ));
        }
        while i < bytes.len() && bytes[i] == 0xFF {
            i += 1;
        }
        if i >= bytes.len() {
            break;
        }
        let marker = bytes[i];
        i += 1;
        if marker == 0xD9 || marker == 0xDA {
            break;
        }
        if i + 1 >= bytes.len() {
            return Err(err(
                ErrorCode::MalformedRecord,
                "jpeg",
                "truncated segment length",
            ));
        }
        let seglen = u16::from_be_bytes([bytes[i], bytes[i + 1]]) as usize;
        if seglen < 2 || i + seglen > bytes.len() {
            return Err(err(
                ErrorCode::MalformedRecord,
                "jpeg",
                "invalid segment length",
            ));
        }
        let seg = &bytes[i + 2..i + seglen];
        i += seglen;
        if marker == 0xE1
            && let Some(o) = orientation_from_exif_app1(seg)?
        {
            return Ok(o);
        }
    }
    Ok(ExifOrientation::IDENTITY)
}

fn orientation_from_exif_app1(seg: &[u8]) -> Result<Option<ExifOrientation>, DomainError> {
    // Unrelated / non-EXIF APP1 → absent (caller keeps scanning / identity).
    if seg.len() < 6 || &seg[0..6] != b"Exif\0\0" {
        return Ok(None);
    }
    // Recognized Exif signature: require a full TIFF header (8 bytes). Truncation
    // here is malformed metadata, not "missing EXIF".
    if seg.len() < 14 {
        return Err(err(
            ErrorCode::MalformedRecord,
            "exif",
            "truncated EXIF TIFF header after Exif signature",
        ));
    }
    let tiff = &seg[6..];
    let le = if tiff.starts_with(b"II") {
        true
    } else if tiff.starts_with(b"MM") {
        false
    } else {
        return Err(err(
            ErrorCode::MalformedRecord,
            "exif",
            "bad TIFF endian marker",
        ));
    };
    let read_u16 = |off: usize| -> Result<u16, DomainError> {
        if off + 2 > tiff.len() {
            return Err(err(ErrorCode::MalformedRecord, "exif", "truncated TIFF"));
        }
        Ok(if le {
            u16::from_le_bytes([tiff[off], tiff[off + 1]])
        } else {
            u16::from_be_bytes([tiff[off], tiff[off + 1]])
        })
    };
    let read_u32 = |off: usize| -> Result<u32, DomainError> {
        if off + 4 > tiff.len() {
            return Err(err(ErrorCode::MalformedRecord, "exif", "truncated TIFF"));
        }
        Ok(if le {
            u32::from_le_bytes([tiff[off], tiff[off + 1], tiff[off + 2], tiff[off + 3]])
        } else {
            u32::from_be_bytes([tiff[off], tiff[off + 1], tiff[off + 2], tiff[off + 3]])
        })
    };
    if read_u16(2)? != 42 {
        return Err(err(ErrorCode::MalformedRecord, "exif", "bad TIFF magic"));
    }
    let ifd0 = read_u32(4)? as usize;
    // Reject IFD offsets that cannot hold the entry count.
    if ifd0.checked_add(2).is_none_or(|end| end > tiff.len()) {
        return Err(err(
            ErrorCode::MalformedRecord,
            "exif",
            "IFD0 offset out of range",
        ));
    }
    let count = read_u16(ifd0)? as usize;
    let entries_end = ifd0
        .checked_add(2)
        .and_then(|s| s.checked_add(count.saturating_mul(12)))
        .ok_or_else(|| err(ErrorCode::MalformedRecord, "exif", "IFD size overflow"))?;
    if entries_end > tiff.len() {
        return Err(err(
            ErrorCode::MalformedRecord,
            "exif",
            "truncated IFD entries",
        ));
    }
    let mut ifd = ifd0 + 2;
    for _ in 0..count {
        let off = ifd;
        ifd += 12;
        let tag = read_u16(off)?;
        let typ = read_u16(off + 2)?;
        let n = read_u32(off + 4)?;
        if tag != 0x0112 {
            continue;
        }
        // Orientation must be TIFF SHORT count 1; reject wrong type/count.
        if typ != 3 || n != 1 {
            return Err(err(
                ErrorCode::MalformedRecord,
                "exif",
                format!("Orientation tag type={typ} count={n}, expected SHORT/1"),
            ));
        }
        // Value fits in the 4-byte value/offset field for SHORT/1.
        let v = read_u16(off + 8)?;
        return Ok(Some(ExifOrientation::from_u16(v)?));
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transform_table_matches_exif_h() {
        let cases = [
            (1, 0, false),
            (2, 0, true),
            (3, 2, false),
            (4, 2, true),
            (5, 1, true),
            (6, 1, false),
            (7, 3, true),
            (8, 3, false),
        ];
        for (o, t, m) in cases {
            let xf = exif_transform(ExifOrientation::from_u8(o).unwrap());
            assert_eq!(xf.turns_cw, t);
            assert_eq!(xf.mirror, m);
        }
    }

    #[test]
    fn oriented_size_swaps_on_odd_turns() {
        assert_eq!(oriented_size(64, 48, 0), (64, 48));
        assert_eq!(oriented_size(64, 48, 1), (48, 64));
        assert_eq!(oriented_size(64, 48, 2), (64, 48));
        assert_eq!(oriented_size(64, 48, 3), (48, 64));
    }

    #[test]
    fn orient_pixels_rot90_cw_nonsquare() {
        // 2x1 RGB: left=(10,0,0) right=(20,0,0) → after 90° CW: 1x2 tall
        let src = [10u8, 0, 0, 20, 0, 0];
        let mut dst = [0u8; 6];
        let xf = ExifTransform {
            turns_cw: 1,
            mirror: false,
        };
        let (w, h) = orient_pixels(&src, 2, 1, 3, xf, &mut dst).unwrap();
        assert_eq!((w, h), (1, 2));
        // dy=0 → top = former left? case1: sx=dy=0, sy=h-1-mx=0 → left
        // dy=1 → sx=1, sy=0 → right
        assert_eq!(&dst[0..3], &[10, 0, 0]);
        assert_eq!(&dst[3..6], &[20, 0, 0]);
    }

    #[test]
    fn turns_only_drops_mirror() {
        let src = [1u8, 2, 3, 4]; // 2x2 gray
        let mut full = [0u8; 4];
        let mut turns = [0u8; 4];
        let o = ExifOrientation::from_u8(2).unwrap(); // mirror only
        apply_orientation(&src, 2, 2, 1, o, ApplyPolicy::FullTransform, &mut full).unwrap();
        apply_orientation(&src, 2, 2, 1, o, ApplyPolicy::TurnsOnly, &mut turns).unwrap();
        assert_ne!(full, turns);
        assert_eq!(turns, src); // turns_cw=0 → identity under TurnsOnly
    }

    #[test]
    fn rejects_zero_and_oversize() {
        let src = [0u8; 4];
        let mut dst = [0u8; 4];
        let xf = ExifTransform {
            turns_cw: 0,
            mirror: false,
        };
        assert!(orient_pixels(&src, 0, 1, 1, xf, &mut dst).is_err());
        assert!(ensure_pixel_budget(4097, 4097).is_err());
    }

    #[test]
    fn jpeg_without_exif_is_identity() {
        // Minimal SOI + EOI
        let jpeg = [0xFFu8, 0xD8, 0xFF, 0xD9];
        assert_eq!(
            read_jpeg_exif_orientation(&jpeg).unwrap(),
            ExifOrientation::IDENTITY
        );
    }

    #[test]
    fn exif_signature_truncated_tiff_is_malformed_not_identity() {
        // APP1 payload is exactly "Exif\0\0" (no TIFF header).
        let jpeg = [
            0xFFu8, 0xD8, 0xFF, 0xE1, 0x00, 0x08, b'E', b'x', b'i', b'f', 0, 0, 0xFF, 0xD9,
        ];
        let e = read_jpeg_exif_orientation(&jpeg).unwrap_err();
        assert_eq!(e.code(), ErrorCode::MalformedRecord);
    }

    #[test]
    fn unrelated_app1_without_exif_signature_is_identity() {
        let mut jpeg = vec![0xFF, 0xD8, 0xFF, 0xE1, 0x00, 0x08];
        jpeg.extend_from_slice(b"Hello!");
        jpeg.extend_from_slice(&[0xFF, 0xD9]);
        assert_eq!(
            read_jpeg_exif_orientation(&jpeg).unwrap(),
            ExifOrientation::IDENTITY
        );
    }

    #[test]
    fn exif_truncation_boundaries_le_and_be() {
        for le in [true, false] {
            // Endian only (2 bytes) after Exif\0\0 → still short of 8-byte TIFF hdr.
            let mut short = Vec::from(if le {
                b"II".as_slice()
            } else {
                b"MM".as_slice()
            });
            let jpeg = wrap_app1_jpeg(&short);
            // wrap adds Exif\0\0 + tiff; total after signature < 8 when tiff is 2 bytes
            // → seg len = 6+2 = 8 < 14 → truncated header error.
            let e = read_jpeg_exif_orientation(&jpeg).unwrap_err();
            assert_eq!(e.code(), ErrorCode::MalformedRecord, "le={le} short endian");

            // Full endian+magic but truncated IFD offset field.
            short.clear();
            if le {
                short.extend_from_slice(b"II");
                short.extend_from_slice(&42u16.to_le_bytes());
                short.extend_from_slice(&[1, 2]); // incomplete u32 IFD offset
            } else {
                short.extend_from_slice(b"MM");
                short.extend_from_slice(&42u16.to_be_bytes());
                short.extend_from_slice(&[1, 2]);
            }
            let e = read_jpeg_exif_orientation(&wrap_app1_jpeg(&short)).unwrap_err();
            assert_eq!(
                e.code(),
                ErrorCode::MalformedRecord,
                "le={le} trunc IFD off"
            );
        }
    }

    #[test]
    fn identity_apply_still_rejects_bad_channels() {
        let src = [1u8, 2, 3];
        let mut dst = [0u8; 3];
        let e = apply_orientation(
            &src,
            1,
            1,
            5,
            ExifOrientation::IDENTITY,
            ApplyPolicy::TurnsOnly,
            &mut dst,
        )
        .unwrap_err();
        assert_eq!(e.code(), ErrorCode::InvalidArgument);
    }

    #[test]
    fn from_u16_rejects_257_and_262() {
        // Truncating to u8 would wrongly yield 1 and 6.
        assert!(ExifOrientation::from_u16(257).is_err());
        assert!(ExifOrientation::from_u16(262).is_err());
        assert_eq!(ExifOrientation::from_u16(6).unwrap().get(), 6);
    }

    fn wrap_app1_jpeg(tiff: &[u8]) -> Vec<u8> {
        let mut app1 = Vec::new();
        app1.extend_from_slice(b"Exif\0\0");
        app1.extend_from_slice(tiff);
        let seglen = (app1.len() + 2) as u16;
        let mut jpeg = vec![0xFF, 0xD8, 0xFF, 0xE1];
        jpeg.extend_from_slice(&seglen.to_be_bytes());
        jpeg.extend_from_slice(&app1);
        jpeg.extend_from_slice(&[0xFF, 0xD9]);
        jpeg
    }

    fn tiff_with_orientation(le: bool, value: u16) -> Vec<u8> {
        let mut t = Vec::new();
        if le {
            t.extend_from_slice(b"II");
        } else {
            t.extend_from_slice(b"MM");
        }
        let put_u16 = |t: &mut Vec<u8>, v: u16| {
            if le {
                t.extend_from_slice(&v.to_le_bytes());
            } else {
                t.extend_from_slice(&v.to_be_bytes());
            }
        };
        let put_u32 = |t: &mut Vec<u8>, v: u32| {
            if le {
                t.extend_from_slice(&v.to_le_bytes());
            } else {
                t.extend_from_slice(&v.to_be_bytes());
            }
        };
        put_u16(&mut t, 42);
        put_u32(&mut t, 8); // IFD0 at offset 8
        put_u16(&mut t, 1); // 1 entry
        put_u16(&mut t, 0x0112);
        put_u16(&mut t, 3); // SHORT
        put_u32(&mut t, 1);
        put_u16(&mut t, value);
        put_u16(&mut t, 0); // pad
        put_u32(&mut t, 0); // next IFD
        t
    }

    #[test]
    fn exif_orientation_le_and_be() {
        for le in [true, false] {
            let jpeg = wrap_app1_jpeg(&tiff_with_orientation(le, 6));
            assert_eq!(
                read_jpeg_exif_orientation(&jpeg).unwrap().get(),
                6,
                "le={le}"
            );
        }
    }

    #[test]
    fn exif_rejects_short_value_257() {
        let jpeg = wrap_app1_jpeg(&tiff_with_orientation(true, 257));
        let e = read_jpeg_exif_orientation(&jpeg).unwrap_err();
        assert_eq!(e.code(), ErrorCode::ValueOutOfRange);
    }

    #[test]
    fn exif_rejects_bad_ifd_offset() {
        let mut t = Vec::new();
        t.extend_from_slice(b"II");
        t.extend_from_slice(&42u16.to_le_bytes());
        t.extend_from_slice(&10_000u32.to_le_bytes()); // IFD far past end
        let jpeg = wrap_app1_jpeg(&t);
        let e = read_jpeg_exif_orientation(&jpeg).unwrap_err();
        assert_eq!(e.code(), ErrorCode::MalformedRecord);
    }

    #[test]
    fn exif_rejects_wrong_orientation_type() {
        let mut t = Vec::new();
        t.extend_from_slice(b"II");
        t.extend_from_slice(&42u16.to_le_bytes());
        t.extend_from_slice(&8u32.to_le_bytes());
        t.extend_from_slice(&1u16.to_le_bytes());
        t.extend_from_slice(&0x0112u16.to_le_bytes());
        t.extend_from_slice(&4u16.to_le_bytes()); // LONG instead of SHORT
        t.extend_from_slice(&1u32.to_le_bytes());
        t.extend_from_slice(&6u32.to_le_bytes());
        t.extend_from_slice(&0u32.to_le_bytes());
        let jpeg = wrap_app1_jpeg(&t);
        let e = read_jpeg_exif_orientation(&jpeg).unwrap_err();
        assert_eq!(e.code(), ErrorCode::MalformedRecord);
    }
}
