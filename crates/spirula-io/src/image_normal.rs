//! Normal aux map decode (P3-F02 / C-DATA-09).
//!
//! Parity with pin `DataManager.cpp:decode_normal_into` + `turn_normal_vectors`:
//! RGB8 normals, optional quarter turns on pixels, then vector remapping, then
//! bilinear resize to destination.

use spirula_core::{ContextKey, DomainError, ErrorCode};

use crate::image_orient::{ExifTransform, orient_pixels, oriented_size};
use crate::image_probe::{ensure_encoded_budget, ensure_pixel_budget, probe_image_size_bytes};

fn err(code: ErrorCode, field: &str, msg: impl Into<String>) -> DomainError {
    DomainError::try_new(code, msg)
        .expect("msg")
        .with_context(ContextKey::Field, field)
        .expect("field")
}

/// Remap packed RGB normal vectors after a pixel turn (`turn_normal_vectors`).
pub fn turn_normal_vectors(px: &mut [u8], n_pixels: usize, turns_cw: u8) {
    let t = turns_cw & 3;
    if t == 0 {
        return;
    }
    for i in 0..n_pixels {
        let p = &mut px[i * 3..i * 3 + 3];
        let x = p[0];
        let y = p[1];
        match t {
            1 => {
                p[0] = 255 - y;
                p[1] = x;
            }
            2 => {
                p[0] = 255 - x;
                p[1] = 255 - y;
            }
            3 => {
                p[0] = y;
                p[1] = 255 - x;
            }
            _ => {}
        }
    }
}

/// Bilinear RGB8 resize (`cpu_bilinear_resize<uint8_t,3>`).
pub fn cpu_bilinear_resize_rgb8(
    src: &[u8],
    src_w: u32,
    src_h: u32,
    dst_w: u32,
    dst_h: u32,
    dst: &mut [u8],
) -> Result<(), DomainError> {
    ensure_pixel_budget(src_w, src_h)?;
    ensure_pixel_budget(dst_w, dst_h)?;
    let need_s = (src_w as usize) * (src_h as usize) * 3;
    let need_d = (dst_w as usize) * (dst_h as usize) * 3;
    if src.len() < need_s || dst.len() < need_d {
        return Err(err(ErrorCode::MalformedRecord, "normal", "buffer short"));
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
        let y0c = y0.clamp(0, sh_i - 1) as u32;
        let y1c = (y0 + 1).clamp(0, sh_i - 1) as u32;
        for x in 0..dst_w {
            let u = (x as f32 + 0.5) * sx - 0.5;
            let x0 = u.floor() as i32;
            let fx = u - x0 as f32;
            let x0c = x0.clamp(0, sw_i - 1) as u32;
            let x1c = (x0 + 1).clamp(0, sw_i - 1) as u32;
            let w00 = (1.0 - fx) * (1.0 - fy);
            let w10 = fx * (1.0 - fy);
            let w01 = (1.0 - fx) * fy;
            let w11 = fx * fy;
            let i00 = ((y0c * src_w + x0c) * 3) as usize;
            let i10 = ((y0c * src_w + x1c) * 3) as usize;
            let i01 = ((y1c * src_w + x0c) * 3) as usize;
            let i11 = ((y1c * src_w + x1c) * 3) as usize;
            let o = ((y * dst_w + x) * 3) as usize;
            for c in 0..3 {
                let r = w00 * src[i00 + c] as f32
                    + w10 * src[i10 + c] as f32
                    + w01 * src[i01 + c] as f32
                    + w11 * src[i11 + c] as f32;
                dst[o + c] = r.round().clamp(0.0, 255.0) as u8;
            }
        }
    }
    Ok(())
}

pub fn decode_normal_rgb8_bytes(bytes: &[u8]) -> Result<(u32, u32, Vec<u8>), DomainError> {
    ensure_encoded_budget(bytes.len())?;
    let (w, h) = probe_image_size_bytes(bytes)?;
    let img = image::load_from_memory(bytes).map_err(|e| {
        err(
            ErrorCode::DecodeFailed,
            "normal",
            format!("decode failed: {e}"),
        )
    })?;
    let rgb = img.to_rgb8();
    if rgb.width() != w || rgb.height() != h {
        return Err(err(
            ErrorCode::MalformedRecord,
            "normal",
            format!(
                "decode size {}x{} != header {w}x{h}",
                rgb.width(),
                rgb.height()
            ),
        ));
    }
    Ok((w, h, rgb.into_raw()))
}

/// Orient pixels, remap vectors, bilinear to destination (DataManager order).
pub fn normalize_normal_rgb8(
    samples: &[u8],
    width: u32,
    height: u32,
    turns_cw: u8,
    dst_w: u32,
    dst_h: u32,
    dst: &mut [u8],
) -> Result<(), DomainError> {
    ensure_pixel_budget(width, height)?;
    ensure_pixel_budget(dst_w, dst_h)?;
    let need = (width as usize) * (height as usize) * 3;
    if samples.len() < need {
        return Err(err(ErrorCode::MalformedRecord, "normal", "buffer short"));
    }
    let xf = ExifTransform {
        turns_cw: turns_cw & 3,
        mirror: false,
    };
    let (ow, oh) = oriented_size(width, height, xf.turns_cw);
    let mut plane = samples[..need].to_vec();
    if !xf.identity() {
        let mut buf = vec![0u8; (ow as usize) * (oh as usize) * 3];
        orient_pixels(&plane, width, height, 3, xf, &mut buf)?;
        plane = buf;
    }
    turn_normal_vectors(&mut plane, (ow as usize) * (oh as usize), xf.turns_cw);
    cpu_bilinear_resize_rgb8(&plane, ow, oh, dst_w, dst_h, dst)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn turn1_matches_datamanager() {
        let mut nrm = [200u8, 100, 128];
        turn_normal_vectors(&mut nrm, 1, 1);
        assert_eq!(nrm, [155, 200, 128]); // 255-100, 200, 128
    }
}
