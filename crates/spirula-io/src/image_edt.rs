//! Exact Euclidean distance transform + mask boundary offset (P3-F02 / C-DATA-08).
//!
//! Parity with pin `core/DistanceTransform.h`. Fraction → pixels:
//! `fraction * sqrt(W*H)` (`DataManager.cpp:apply_boundary_offset`).

use spirula_core::{ContextKey, DomainError, ErrorCode};

use crate::image_probe::ensure_pixel_budget;

fn err(code: ErrorCode, field: &str, msg: impl Into<String>) -> DomainError {
    DomainError::try_new(code, msg)
        .expect("msg")
        .with_context(ContextKey::Field, field)
        .expect("field")
}

const COL_BLOCK: usize = 16;

#[allow(clippy::needless_range_loop)] // mirrors DistanceTransform.h indexing
fn dt_1d_squared(f: &[f32], d: &mut [f32], n: usize, v: &mut [i32], z: &mut [f64]) {
    let inf_f = f32::INFINITY;
    let inf_d = f64::INFINITY;
    let mut first = 0usize;
    while first < n && !f[first].is_finite() {
        first += 1;
    }
    if first >= n {
        d[..n].fill(inf_f);
        return;
    }
    let mut k = 0usize;
    v[0] = first as i32;
    z[0] = -inf_d;
    z[1] = inf_d;
    for q in (first + 1)..n {
        if !f[q].is_finite() {
            continue;
        }
        let s = loop {
            let vk = v[k] as usize;
            let aq = f64::from(f[q]) + (q as f64) * (q as f64);
            let av = f64::from(f[vk]) + (vk as f64) * (vk as f64);
            let cand = (aq - av) / (2.0 * (q as f64 - vk as f64));
            if k == 0 || cand > z[k] {
                break cand;
            }
            k -= 1;
        };
        k += 1;
        v[k] = q as i32;
        z[k] = s;
        z[k + 1] = inf_d;
    }
    let kmax = k;
    k = 0;
    for q in 0..n {
        while k < kmax && z[k + 1] < q as f64 {
            k += 1;
        }
        let dq = q as f64 - f64::from(v[k]);
        d[q] = (dq * dq + f64::from(f[v[k] as usize])) as f32;
    }
}

fn dt_row_squared(m: &[u8], w: usize, src_value: u8, d: &mut [f32]) {
    let inf_f = f32::INFINITY;
    let mut last: i32 = -1;
    for x in 0..w {
        if m[x] == src_value {
            last = x as i32;
        }
        d[x] = if last < 0 {
            inf_f
        } else {
            (x as i32 - last) as f32
        };
    }
    let mut next: i32 = -1;
    for x in (0..w).rev() {
        if m[x] == src_value {
            next = x as i32;
        }
        let back = if next < 0 {
            inf_f
        } else {
            (next - x as i32) as f32
        };
        let best = if d[x] < back { d[x] } else { back };
        d[x] = best * best;
    }
}

#[allow(clippy::too_many_arguments)] // separable DT scratch matches C++ signature
fn dt2d_squared(
    mask: &[u8],
    h: usize,
    w: usize,
    src_value: u8,
    d2_out: &mut [f32],
    cols: &mut [f32],
    cols_out: &mut [f32],
    v_buf: &mut [i32],
    z_buf: &mut [f64],
) {
    for y in 0..h {
        dt_row_squared(&mask[y * w..], w, src_value, &mut d2_out[y * w..]);
    }
    let mut x0 = 0usize;
    while x0 < w {
        let nc = COL_BLOCK.min(w - x0);
        for y in 0..h {
            for c in 0..nc {
                cols[c * h + y] = d2_out[y * w + x0 + c];
            }
        }
        for c in 0..nc {
            let off = c * h;
            // Split borrows: copy column into temporary for dt_1d input.
            let mut col = vec![0.0f32; h];
            col.copy_from_slice(&cols[off..off + h]);
            let mut dout = vec![0.0f32; h];
            dt_1d_squared(&col, &mut dout, h, v_buf, z_buf);
            cols_out[off..off + h].copy_from_slice(&dout);
        }
        for y in 0..h {
            for c in 0..nc {
                d2_out[y * w + x0 + c] = cols_out[c * h + y];
            }
        }
        x0 += COL_BLOCK;
    }
}

/// Dilate (`offset_px > 0`) or erode (`offset_px < 0`) a 0/1 mask in place.
pub fn apply_mask_boundary_offset_px(
    mask: &mut [u8],
    height: u32,
    width: u32,
    offset_px: f32,
) -> Result<(), DomainError> {
    if !offset_px.is_finite() {
        return Err(err(
            ErrorCode::ValueOutOfRange,
            "mask_boundary_offset",
            "offset_px must be finite",
        ));
    }
    ensure_pixel_budget(width, height)?;
    let h = height as usize;
    let w = width as usize;
    let need = h
        .checked_mul(w)
        .ok_or_else(|| err(ErrorCode::ValueOutOfRange, "mask_size", "overflow"))?;
    if mask.len() < need {
        return Err(err(ErrorCode::MalformedRecord, "mask", "buffer short"));
    }
    for v in &mask[..need] {
        if *v > 1 {
            return Err(err(
                ErrorCode::InvalidArgument,
                "mask",
                "boundary offset requires binary 0/1 samples",
            ));
        }
    }
    // Validate dims/length/binary before no-op early returns (incl. 1x1).
    if offset_px == 0.0 || (h == 1 && w == 1) {
        return Ok(());
    }
    let mx = h.max(w);
    let mut d2_fg = vec![0.0f32; need];
    let mut d2_bg = vec![0.0f32; need];
    let mut cols = vec![0.0f32; COL_BLOCK * h];
    let mut cols_out = vec![0.0f32; COL_BLOCK * h];
    let mut v_buf = vec![0i32; mx];
    let mut z_buf = vec![0.0f64; mx + 1];
    dt2d_squared(
        &mask[..need],
        h,
        w,
        1,
        &mut d2_fg,
        &mut cols,
        &mut cols_out,
        &mut v_buf,
        &mut z_buf,
    );
    dt2d_squared(
        &mask[..need],
        h,
        w,
        0,
        &mut d2_bg,
        &mut cols,
        &mut cols_out,
        &mut v_buf,
        &mut z_buf,
    );
    for i in 0..need {
        let sd = d2_fg[i].sqrt() - d2_bg[i].sqrt();
        mask[i] = u8::from(sd <= offset_px);
    }
    Ok(())
}

/// `DataManager::apply_boundary_offset`: `fraction * sqrt(W*H)` pixels.
pub fn apply_mask_boundary_offset_frac(
    mask: &mut [u8],
    height: u32,
    width: u32,
    fraction: f32,
) -> Result<(), DomainError> {
    if !fraction.is_finite() {
        return Err(err(
            ErrorCode::ValueOutOfRange,
            "mask_boundary_offset",
            "fraction must be finite",
        ));
    }
    // Always validate mask through the px path (incl. fraction == 0 no-op).
    let area = (width as f32) * (height as f32);
    if !area.is_finite() {
        return Err(err(
            ErrorCode::ValueOutOfRange,
            "mask_boundary_offset",
            "width*height product overflowed f32",
        ));
    }
    let offset_px = fraction * area.sqrt();
    if !offset_px.is_finite() {
        return Err(err(
            ErrorCode::ValueOutOfRange,
            "mask_boundary_offset",
            "offset_px overflowed f32",
        ));
    }
    apply_mask_boundary_offset_px(mask, height, width, offset_px)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_fraction_is_noop() {
        let mut m = [1u8, 0, 0, 1];
        apply_mask_boundary_offset_frac(&mut m, 2, 2, 0.0).unwrap();
        assert_eq!(m, [1, 0, 0, 1]);
    }

    #[test]
    fn rejects_nan_before_mutation() {
        let mut m = [0u8, 1, 0, 1];
        let e = apply_mask_boundary_offset_px(&mut m, 2, 2, f32::NAN).unwrap_err();
        assert_eq!(e.code(), ErrorCode::ValueOutOfRange);
        assert_eq!(m, [0, 1, 0, 1]);
    }

    #[test]
    fn one_by_one_validates_then_noop() {
        let mut m = [1u8];
        apply_mask_boundary_offset_px(&mut m, 1, 1, 3.0).unwrap();
        assert_eq!(m, [1]);
        let e = apply_mask_boundary_offset_px(&mut [2u8], 1, 1, 3.0).unwrap_err();
        assert_eq!(e.code(), ErrorCode::InvalidArgument);
    }
}
