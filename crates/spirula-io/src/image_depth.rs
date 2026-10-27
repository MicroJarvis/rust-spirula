//! Depth aux map decode (P3-F02 / C-DATA-09).
//!
//! Parity with pin `DataManager.cpp:decode_depth_into`: 16-bit single-channel
//! PNG (stb `stbi_load_16` req_comp=1), optional quarter turns, bilinear resize.

use spirula_core::{ContextKey, DomainError, ErrorCode};

use crate::image_orient::{ExifTransform, orient_pixels, oriented_size};
use crate::image_probe::{ensure_encoded_budget, ensure_pixel_budget, probe_image_size_bytes};

fn err(code: ErrorCode, field: &str, msg: impl Into<String>) -> DomainError {
    DomainError::try_new(code, msg)
        .expect("msg")
        .with_context(ContextKey::Field, field)
        .expect("field")
}

/// Bilinear resize for `u16` single-channel (`cpu_bilinear_resize<uint16_t,1>`).
pub fn cpu_bilinear_resize_u16(
    src: &[u16],
    src_w: u32,
    src_h: u32,
    dst_w: u32,
    dst_h: u32,
    dst: &mut [u16],
) -> Result<(), DomainError> {
    ensure_pixel_budget(src_w, src_h)?;
    ensure_pixel_budget(dst_w, dst_h)?;
    let need_s = (src_w as usize) * (src_h as usize);
    let need_d = (dst_w as usize) * (dst_h as usize);
    if src.len() < need_s || dst.len() < need_d {
        return Err(err(ErrorCode::MalformedRecord, "depth", "buffer short"));
    }
    if src_w == dst_w && src_h == dst_h {
        dst[..need_d].copy_from_slice(&src[..need_s]);
        return Ok(());
    }
    let sx = (src_w as f32) / (dst_w as f32);
    let sy = (src_h as f32) / (dst_h as f32);
    let sh_i = src_h as i32;
    let sw_i = src_w as i32;
    for y in 0..dst_h {
        let v = (y as f32 + 0.5) * sy - 0.5;
        let y0 = v.floor() as i32;
        let fy = v - y0 as f32;
        let y0c = y0.clamp(0, sh_i - 1);
        let y1c = (y0 + 1).clamp(0, sh_i - 1);
        for x in 0..dst_w {
            let u = (x as f32 + 0.5) * sx - 0.5;
            let x0 = u.floor() as i32;
            let fx = u - x0 as f32;
            let x0c = x0.clamp(0, sw_i - 1);
            let x1c = (x0 + 1).clamp(0, sw_i - 1);
            let w00 = (1.0 - fx) * (1.0 - fy);
            let w10 = fx * (1.0 - fy);
            let w01 = (1.0 - fx) * fy;
            let w11 = fx * fy;
            let p00 = src[(y0c as u32 * src_w + x0c as u32) as usize] as f32;
            let p10 = src[(y0c as u32 * src_w + x1c as u32) as usize] as f32;
            let p01 = src[(y1c as u32 * src_w + x0c as u32) as usize] as f32;
            let p11 = src[(y1c as u32 * src_w + x1c as u32) as usize] as f32;
            let r = (w00 * p00 + w10 * p10 + w01 * p01 + w11 * p11).round();
            dst[(y * dst_w + x) as usize] = r.clamp(0.0, 65535.0) as u16;
        }
    }
    Ok(())
}

/// Decode 16-bit grayscale depth from PNG/TIFF bytes into `u16` plane.
pub fn decode_depth16_bytes(bytes: &[u8]) -> Result<(u32, u32, Vec<u16>), DomainError> {
    ensure_encoded_budget(bytes.len())?;
    let (w, h) = probe_image_size_bytes(bytes)?;
    let img = image::load_from_memory(bytes).map_err(|e| {
        err(
            ErrorCode::DecodeFailed,
            "depth",
            format!("decode failed: {e}"),
        )
    })?;
    let gray = img.to_luma16();
    if gray.width() != w || gray.height() != h {
        return Err(err(
            ErrorCode::MalformedRecord,
            "depth",
            format!(
                "decode size {}x{} != header {w}x{h}",
                gray.width(),
                gray.height()
            ),
        ));
    }
    Ok((w, h, gray.into_raw()))
}

/// Orient + bilinear-fit depth to `dst_w x dst_h`.
pub fn normalize_depth16(
    samples: &[u16],
    width: u32,
    height: u32,
    turns_cw: u8,
    dst_w: u32,
    dst_h: u32,
    dst: &mut [u16],
) -> Result<(), DomainError> {
    ensure_pixel_budget(width, height)?;
    ensure_pixel_budget(dst_w, dst_h)?;
    let px = (width as usize) * (height as usize);
    if samples.len() < px {
        return Err(err(ErrorCode::MalformedRecord, "depth", "buffer short"));
    }
    let xf = ExifTransform {
        turns_cw: turns_cw & 3,
        mirror: false,
    };
    let (ow, oh) = oriented_size(width, height, xf.turns_cw);
    let mut plane: Vec<u16> = samples[..px].to_vec();
    if !xf.identity() {
        // orient_pixels is u8-only; apply via byte view of u16 LE.
        let src_bytes: Vec<u8> = plane.iter().flat_map(|v| v.to_le_bytes()).collect();
        let mut dst_bytes = vec![0u8; (ow as usize) * (oh as usize) * 2];
        // Treat as 2-channel u8 pairs (same layout as interleaved LE u16).
        orient_pixels(&src_bytes, width, height, 2, xf, &mut dst_bytes)?;
        plane = dst_bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_le_bytes(*c))
            .collect();
    }
    cpu_bilinear_resize_u16(&plane, ow, oh, dst_w, dst_h, dst)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bilinear_2x2_to_1x1() {
        let src = [100u16, 200, 300, 400];
        let mut dst = [0u16; 1];
        cpu_bilinear_resize_u16(&src, 2, 2, 1, 1, &mut dst).unwrap();
        // Center sample averages all four → 250
        assert_eq!(dst[0], 250);
    }
}
