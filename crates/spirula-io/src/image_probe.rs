//! Image size probe and decode helpers (P3-F02 PNG/JPEG subset).
//!
//! Probe is **header-only** (PNG IHDR / JPEG SOF). Pixel and encoded-byte
//! budgets are enforced before `image::load_from_memory`.

use std::fs;
use std::io::Read;
use std::path::Path;

use spirula_core::{ContextKey, DomainError, ErrorCode};

use crate::image_orient::{MAX_ENCODED_BYTES, MAX_IMAGE_PIXELS};

fn err(code: ErrorCode, field: &str, msg: impl Into<String>) -> DomainError {
    DomainError::try_new(code, msg)
        .expect("msg")
        .with_context(ContextKey::Field, field)
        .expect("field")
}

pub(crate) fn ensure_pixel_budget(w: u32, h: u32) -> Result<(), DomainError> {
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

pub(crate) fn ensure_encoded_budget(len: usize) -> Result<(), DomainError> {
    if (len as u64) > MAX_ENCODED_BYTES {
        return Err(err(
            ErrorCode::ValueOutOfRange,
            "encoded_bytes",
            format!("{len} exceeds MAX_ENCODED_BYTES ({MAX_ENCODED_BYTES})"),
        ));
    }
    Ok(())
}

fn supported_ext(path: &Path) -> Result<(), DomainError> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "png" | "jpg" | "jpeg" => Ok(()),
        "" => Err(err(
            ErrorCode::UnsupportedFormat,
            "path",
            "missing image extension (supported: png, jpg, jpeg)",
        )),
        other => Err(err(
            ErrorCode::UnsupportedFormat,
            "path",
            format!("unsupported image extension .{other} in P3-F02 (png/jpeg only)"),
        )),
    }
}

/// Read a path with an encoded-byte cap (size checked before full read).
pub fn read_image_bytes_capped(path: &Path) -> Result<Vec<u8>, DomainError> {
    let meta = fs::metadata(path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            err(
                ErrorCode::NotFound,
                "path",
                format!("missing {}", path.display()),
            )
        } else {
            err(
                ErrorCode::ReadFailed,
                "path",
                format!("stat {}: {e}", path.display()),
            )
        }
    })?;
    let len = meta.len();
    if len > MAX_ENCODED_BYTES {
        return Err(err(
            ErrorCode::ValueOutOfRange,
            "encoded_bytes",
            format!("{len} exceeds MAX_ENCODED_BYTES ({MAX_ENCODED_BYTES})"),
        ));
    }
    let mut f = fs::File::open(path).map_err(|e| {
        err(
            ErrorCode::ReadFailed,
            "path",
            format!("open {}: {e}", path.display()),
        )
    })?;
    let mut buf = Vec::with_capacity(len as usize);
    f.by_ref()
        .take(MAX_ENCODED_BYTES + 1)
        .read_to_end(&mut buf)
        .map_err(|e| {
            err(
                ErrorCode::ReadFailed,
                "path",
                format!("read {}: {e}", path.display()),
            )
        })?;
    ensure_encoded_budget(buf.len())?;
    Ok(buf)
}

/// Header-level size probe (`ImageProbe` / stb header path). No pixel decode.
pub fn probe_image_size(path: &Path) -> Result<(u32, u32), DomainError> {
    supported_ext(path)?;
    let bytes = read_image_bytes_capped(path)?;
    probe_image_size_bytes(&bytes)
}

pub fn probe_image_size_bytes(bytes: &[u8]) -> Result<(u32, u32), DomainError> {
    ensure_encoded_budget(bytes.len())?;
    let (w, h) = if is_png(bytes) {
        probe_png_ihdr(bytes)?
    } else if is_jpeg(bytes) {
        probe_jpeg_sof(bytes)?
    } else {
        return Err(err(
            ErrorCode::UnsupportedFormat,
            "image",
            "P3-F02 probe supports PNG/JPEG headers only",
        ));
    };
    ensure_pixel_budget(w, h)?;
    Ok((w, h))
}

fn is_png(bytes: &[u8]) -> bool {
    bytes.len() >= 8 && bytes[..8] == [137, 80, 78, 71, 13, 10, 26, 10]
}

fn is_jpeg(bytes: &[u8]) -> bool {
    bytes.len() >= 2 && bytes[0] == 0xFF && bytes[1] == 0xD8
}

fn probe_png_ihdr(bytes: &[u8]) -> Result<(u32, u32), DomainError> {
    // sig(8) + len(4) + "IHDR"(4) + width(4) + height(4) …
    if bytes.len() < 24 {
        return Err(err(
            ErrorCode::MalformedRecord,
            "png",
            "truncated PNG header",
        ));
    }
    if &bytes[12..16] != b"IHDR" {
        return Err(err(ErrorCode::MalformedRecord, "png", "missing IHDR chunk"));
    }
    let w = u32::from_be_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]);
    let h = u32::from_be_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]);
    Ok((w, h))
}

fn probe_jpeg_sof(bytes: &[u8]) -> Result<(u32, u32), DomainError> {
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
        // Standalone markers have no length.
        if (0xD0..=0xD7).contains(&marker) || marker == 0x01 {
            continue;
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
        // SOF0..SOF3, SOF5..SOF7, SOF9..SOF11, SOF13..SOF15 (baseline/progressive)
        let is_sof = matches!(
            marker,
            0xC0 | 0xC1
                | 0xC2
                | 0xC3
                | 0xC5
                | 0xC6
                | 0xC7
                | 0xC9
                | 0xCA
                | 0xCB
                | 0xCD
                | 0xCE
                | 0xCF
        );
        if is_sof {
            if seg.len() < 5 {
                return Err(err(ErrorCode::MalformedRecord, "jpeg", "truncated SOF"));
            }
            let h = u16::from_be_bytes([seg[1], seg[2]]) as u32;
            let w = u16::from_be_bytes([seg[3], seg[4]]) as u32;
            return Ok((w, h));
        }
    }
    Err(err(
        ErrorCode::MalformedRecord,
        "jpeg",
        "SOF dimensions not found",
    ))
}

/// Decode to interleaved RGB8 (3 channels) for orientation bake tests / callers.
pub fn decode_rgb8(path: &Path) -> Result<(u32, u32, Vec<u8>), DomainError> {
    supported_ext(path)?;
    let bytes = read_image_bytes_capped(path)?;
    decode_rgb8_bytes(&bytes)
}

pub fn decode_rgb8_bytes(bytes: &[u8]) -> Result<(u32, u32, Vec<u8>), DomainError> {
    // Budgets before full decode.
    let (w, h) = probe_image_size_bytes(bytes)?;
    let img = image::load_from_memory(bytes).map_err(|e| {
        err(
            ErrorCode::DecodeFailed,
            "image",
            format!("decode failed: {e}"),
        )
    })?;
    let rgb = img.to_rgb8();
    if rgb.width() != w || rgb.height() != h {
        return Err(err(
            ErrorCode::MalformedRecord,
            "image",
            format!(
                "decode size {}x{} != header {}x{}",
                rgb.width(),
                rgb.height(),
                w,
                h
            ),
        ));
    }
    Ok((w, h, rgb.into_raw()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_unsupported_extension() {
        let e = probe_image_size(Path::new("x.exr")).unwrap_err();
        assert_eq!(e.code(), ErrorCode::UnsupportedFormat);
    }

    #[test]
    fn rejects_oversized_png_ihdr_without_decode() {
        // Minimal PNG sig + IHDR claiming huge dimensions; no IDAT needed.
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&[137, 80, 78, 71, 13, 10, 26, 10]);
        bytes.extend_from_slice(&13u32.to_be_bytes()); // IHDR length
        bytes.extend_from_slice(b"IHDR");
        bytes.extend_from_slice(&50_000u32.to_be_bytes()); // w
        bytes.extend_from_slice(&50_000u32.to_be_bytes()); // h
        bytes.extend_from_slice(&[8, 2, 0, 0, 0]); // bit/color/… stub
        let e = probe_image_size_bytes(&bytes).unwrap_err();
        assert_eq!(e.code(), ErrorCode::ValueOutOfRange);
    }

    #[test]
    fn rejects_encoded_budget() {
        let mut huge = vec![0xFFu8, 0xD8];
        huge.resize((MAX_ENCODED_BYTES as usize) + 1, 0);
        let e = probe_image_size_bytes(&huge).unwrap_err();
        assert_eq!(e.code(), ErrorCode::ValueOutOfRange);
    }
}
