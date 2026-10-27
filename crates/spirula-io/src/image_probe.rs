//! Image size probe and decode helpers (P3-F02 / C-DATA-07).
//!
//! Formats: PNG, JPEG, BMP, TGA, TIFF, EXR. Pixel and encoded-byte budgets
//! are enforced before allocating decoded buffers. Preferred path: magic /
//! header size; TGA (weak magic) uses `image` dimension probe after the
//! encoded-byte cap without retaining pixels.

use std::fs;
use std::io::Cursor;
use std::io::Read;
use std::path::Path;

use image::ImageDecoder;
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
        "png" | "jpg" | "jpeg" | "bmp" | "tga" | "tif" | "tiff" | "exr" => Ok(()),
        "" => Err(err(
            ErrorCode::UnsupportedFormat,
            "path",
            "missing image extension (supported: png, jpg, jpeg, bmp, tga, tif, tiff, exr)",
        )),
        other => Err(err(
            ErrorCode::UnsupportedFormat,
            "path",
            format!("unsupported image extension .{other} in P3-F02"),
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

pub fn probe_image_size(path: &Path) -> Result<(u32, u32), DomainError> {
    supported_ext(path)?;
    let bytes = read_image_bytes_capped(path)?;
    let hint = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_ascii_lowercase());
    probe_image_size_bytes_hinted(&bytes, hint.as_deref())
}

pub fn probe_image_size_bytes(bytes: &[u8]) -> Result<(u32, u32), DomainError> {
    probe_image_size_bytes_hinted(bytes, None)
}

fn probe_image_size_bytes_hinted(
    bytes: &[u8],
    ext_hint: Option<&str>,
) -> Result<(u32, u32), DomainError> {
    ensure_encoded_budget(bytes.len())?;
    let (w, h) = if is_png(bytes) {
        probe_png_ihdr(bytes)?
    } else if is_jpeg(bytes) {
        probe_jpeg_sof(bytes)?
    } else if is_bmp(bytes) {
        probe_bmp_header(bytes)?
    } else if is_exr(bytes) || is_tiff(bytes) {
        probe_via_image_decoder(bytes, None)?
    } else {
        // TGA lacks a reliable magic; use extension hint or guess.
        probe_via_image_decoder(bytes, ext_hint)?
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

fn is_bmp(bytes: &[u8]) -> bool {
    bytes.len() >= 2 && bytes[0] == b'B' && bytes[1] == b'M'
}

fn is_exr(bytes: &[u8]) -> bool {
    bytes.len() >= 4 && bytes[..4] == [0x76, 0x2f, 0x31, 0x01]
}

fn is_tiff(bytes: &[u8]) -> bool {
    bytes.len() >= 4
        && ((bytes[0] == b'I' && bytes[1] == b'I' && bytes[2] == 42 && bytes[3] == 0)
            || (bytes[0] == b'M' && bytes[1] == b'M' && bytes[2] == 0 && bytes[3] == 42))
}

fn probe_png_ihdr(bytes: &[u8]) -> Result<(u32, u32), DomainError> {
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

fn probe_bmp_header(bytes: &[u8]) -> Result<(u32, u32), DomainError> {
    // BITMAPINFOHEADER at offset 14: width i32, height i32
    if bytes.len() < 26 {
        return Err(err(
            ErrorCode::MalformedRecord,
            "bmp",
            "truncated BMP header",
        ));
    }
    let w = i32::from_le_bytes([bytes[18], bytes[19], bytes[20], bytes[21]]);
    let h = i32::from_le_bytes([bytes[22], bytes[23], bytes[24], bytes[25]]).unsigned_abs();
    if w <= 0 {
        return Err(err(
            ErrorCode::MalformedRecord,
            "bmp",
            format!("non-positive BMP width {w}"),
        ));
    }
    Ok((w as u32, h))
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

fn probe_via_image_decoder(
    bytes: &[u8],
    ext_hint: Option<&str>,
) -> Result<(u32, u32), DomainError> {
    let mut reader = image::ImageReader::new(Cursor::new(bytes));
    if matches!(ext_hint, Some("tga")) {
        reader.set_format(image::ImageFormat::Tga);
    } else {
        reader = reader.with_guessed_format().map_err(|e| {
            err(
                ErrorCode::UnsupportedFormat,
                "image",
                format!("cannot guess format: {e}"),
            )
        })?;
    }
    let decoder = reader.into_decoder().map_err(|e| {
        err(
            ErrorCode::DecodeFailed,
            "image",
            format!("dimension probe failed: {e}"),
        )
    })?;
    Ok(decoder.dimensions())
}

/// Decoded RGB payload — dtype matches on-disk / DataManager probe.
#[derive(Clone, Debug)]
pub enum DecodedRgb {
    U8 {
        width: u32,
        height: u32,
        rgb: Vec<u8>,
    },
    U16 {
        width: u32,
        height: u32,
        rgb: Vec<u16>,
    },
    F32 {
        width: u32,
        height: u32,
        rgb: Vec<f32>,
    },
}

/// Decode to interleaved RGB8 for orientation bake / LDR callers.
pub fn decode_rgb8(path: &Path) -> Result<(u32, u32, Vec<u8>), DomainError> {
    supported_ext(path)?;
    let bytes = read_image_bytes_capped(path)?;
    let hint = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_ascii_lowercase());
    decode_rgb8_bytes_hinted(&bytes, hint.as_deref())
}

pub fn decode_rgb8_bytes(bytes: &[u8]) -> Result<(u32, u32, Vec<u8>), DomainError> {
    decode_rgb8_bytes_hinted(bytes, None)
}

fn decode_rgb8_bytes_hinted(
    bytes: &[u8],
    ext_hint: Option<&str>,
) -> Result<(u32, u32, Vec<u8>), DomainError> {
    let (w, h) = probe_image_size_bytes_hinted(bytes, ext_hint)?;
    let img = load_dynamic(bytes, ext_hint)?;
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

fn load_dynamic(bytes: &[u8], ext_hint: Option<&str>) -> Result<image::DynamicImage, DomainError> {
    if matches!(ext_hint, Some("tga")) {
        let mut reader = image::ImageReader::new(Cursor::new(bytes));
        reader.set_format(image::ImageFormat::Tga);
        return reader.decode().map_err(|e| {
            err(
                ErrorCode::DecodeFailed,
                "image",
                format!("decode failed: {e}"),
            )
        });
    }
    image::load_from_memory(bytes).map_err(|e| {
        err(
            ErrorCode::DecodeFailed,
            "image",
            format!("decode failed: {e}"),
        )
    })
}

/// Probe dtype then decode: EXR → F32; 16-bit → U16; else U8.
pub fn decode_rgb_auto_bytes(bytes: &[u8]) -> Result<DecodedRgb, DomainError> {
    let (w, h) = probe_image_size_bytes(bytes)?;
    if is_exr(bytes) {
        let img = image::load_from_memory(bytes).map_err(|e| {
            err(
                ErrorCode::DecodeFailed,
                "exr",
                format!("decode failed: {e}"),
            )
        })?;
        let rgb = img.to_rgb32f();
        if rgb.width() != w || rgb.height() != h {
            return Err(err(
                ErrorCode::MalformedRecord,
                "exr",
                "decode size mismatch",
            ));
        }
        return Ok(DecodedRgb::F32 {
            width: w,
            height: h,
            rgb: rgb.into_raw(),
        });
    }
    let img = image::load_from_memory(bytes).map_err(|e| {
        err(
            ErrorCode::DecodeFailed,
            "image",
            format!("decode failed: {e}"),
        )
    })?;
    // Prefer 16-bit when the decoder produced Luma16/Rgb16.
    use image::DynamicImage;
    match img {
        DynamicImage::ImageLuma16(_)
        | DynamicImage::ImageRgb16(_)
        | DynamicImage::ImageRgba16(_) => {
            let rgb = img.to_rgb16();
            Ok(DecodedRgb::U16 {
                width: rgb.width(),
                height: rgb.height(),
                rgb: rgb.into_raw(),
            })
        }
        other => {
            let rgb = other.to_rgb8();
            Ok(DecodedRgb::U8 {
                width: rgb.width(),
                height: rgb.height(),
                rgb: rgb.into_raw(),
            })
        }
    }
}

pub fn decode_rgb_auto(path: &Path) -> Result<DecodedRgb, DomainError> {
    supported_ext(path)?;
    let bytes = read_image_bytes_capped(path)?;
    let hint = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_ascii_lowercase());
    if matches!(hint.as_deref(), Some("tga")) {
        let (w, h, rgb) = decode_rgb8_bytes_hinted(&bytes, hint.as_deref())?;
        return Ok(DecodedRgb::U8 {
            width: w,
            height: h,
            rgb,
        });
    }
    decode_rgb_auto_bytes(&bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_unsupported_extension() {
        let e = probe_image_size(Path::new("x.webp")).unwrap_err();
        assert_eq!(e.code(), ErrorCode::UnsupportedFormat);
    }

    #[test]
    fn rejects_oversized_png_ihdr_without_decode() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&[137, 80, 78, 71, 13, 10, 26, 10]);
        bytes.extend_from_slice(&13u32.to_be_bytes());
        bytes.extend_from_slice(b"IHDR");
        bytes.extend_from_slice(&50_000u32.to_be_bytes());
        bytes.extend_from_slice(&50_000u32.to_be_bytes());
        bytes.extend_from_slice(&[8, 2, 0, 0, 0]);
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
