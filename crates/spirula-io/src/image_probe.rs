//! Image size probe and decode helpers (P3-F02 PNG/JPEG subset).

use std::fs;
use std::path::Path;

use spirula_core::{ContextKey, DomainError, ErrorCode};

use crate::image_orient::MAX_IMAGE_PIXELS;

fn err(code: ErrorCode, field: &str, msg: impl Into<String>) -> DomainError {
    DomainError::try_new(code, msg)
        .expect("msg")
        .with_context(ContextKey::Field, field)
        .expect("field")
}

fn ensure_budget(w: u32, h: u32) -> Result<(), DomainError> {
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

/// Header-level size probe (`ImageProbe::probe_image_size` subset).
pub fn probe_image_size(path: &Path) -> Result<(u32, u32), DomainError> {
    supported_ext(path)?;
    let bytes = fs::read(path).map_err(|e| {
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
                format!("read {}: {e}", path.display()),
            )
        }
    })?;
    probe_image_size_bytes(&bytes)
}

pub fn probe_image_size_bytes(bytes: &[u8]) -> Result<(u32, u32), DomainError> {
    let img = image::load_from_memory(bytes).map_err(|e| {
        err(
            ErrorCode::DecodeFailed,
            "image",
            format!("probe decode failed: {e}"),
        )
    })?;
    let w = img.width();
    let h = img.height();
    ensure_budget(w, h)?;
    Ok((w, h))
}

/// Decode to interleaved RGB8 (3 channels) for orientation bake tests / callers.
pub fn decode_rgb8(path: &Path) -> Result<(u32, u32, Vec<u8>), DomainError> {
    supported_ext(path)?;
    let bytes = fs::read(path).map_err(|e| {
        err(
            ErrorCode::ReadFailed,
            "path",
            format!("read {}: {e}", path.display()),
        )
    })?;
    decode_rgb8_bytes(&bytes)
}

pub fn decode_rgb8_bytes(bytes: &[u8]) -> Result<(u32, u32, Vec<u8>), DomainError> {
    let img = image::load_from_memory(bytes).map_err(|e| {
        err(
            ErrorCode::DecodeFailed,
            "image",
            format!("decode failed: {e}"),
        )
    })?;
    let rgb = img.to_rgb8();
    let w = rgb.width();
    let h = rgb.height();
    ensure_budget(w, h)?;
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
}
