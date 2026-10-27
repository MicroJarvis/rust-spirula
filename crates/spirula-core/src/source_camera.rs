//! Forward projection of COLMAP / skewed source camera models (CPU).
//!
//! Host mirror of pinned `src/data/SourceCamera.h/.cpp` (`srccam`, pin
//! `9a24f099`). Params are stored as `f32` (C++ `float`); math promotes to
//! `f64` exactly as the reference casts. Binding: `docs/phase-3/camera.md`.

use crate::DomainError;
use crate::math::{invalid, out_of_range};

/// COLMAP `FULL_OPENCV` (`CameraModelId` 6).
pub const COLMAP_FULL_OPENCV: i32 = 6;
/// COLMAP `FOV` (7).
pub const COLMAP_FOV: i32 = 7;
/// COLMAP `RAD_TAN_THIN_PRISM_FISHEYE` (11).
pub const COLMAP_RAD_TAN_THIN_PRISM_FISHEYE: i32 = 11;
/// COLMAP `SIMPLE_DIVISION` (12).
pub const COLMAP_SIMPLE_DIVISION: i32 = 12;
/// COLMAP `DIVISION` (13).
pub const COLMAP_DIVISION: i32 = 13;
/// COLMAP `EUCM` (16).
pub const COLMAP_EUCM: i32 = 16;

/// Agisoft Metashape skew extension (params layout in datasets docs).
pub const SKEWED: i32 = 1000;

/// `SKEWED` base camera selector in `params[13]`.
pub const SKEW_BASE_PERSPECTIVE: f32 = 0.0;
pub const SKEW_BASE_FISHEYE: f32 = 1.0;
pub const SKEW_BASE_EQUISOLID: f32 = 2.0;

/// `SKEWED` radial form in `params[14]`.
pub const SKEW_RADIAL_POLYNOMIAL: f32 = 0.0;
pub const SKEW_RADIAL_RATIONAL: f32 = 1.0;

/// Maximum parameter slot count (C++ `kMaxParams`).
pub const MAX_PARAMS: usize = 16;

const BEHIND_EPS: f64 = 1e-12;
const UNFOLD_STEP: f64 = 1e-3;

fn project_fov(prm: &[f32; MAX_PARAMS], x: f64, y: f64, z: f64) -> Result<(f64, f64), DomainError> {
    if z < BEHIND_EPS {
        return Err(out_of_range("camera_z", "> 1e-12 (CV +Z)", format!("{z}")));
    }
    let xn = x / z;
    let yn = y / z;
    let omega = f64::from(prm[4]);
    let r2 = xn * xn + yn * yn;
    let om2 = omega * omega;
    const K_EPS: f64 = 1e-4;
    let factor = if om2 < K_EPS {
        (om2 * r2) / 3.0 - om2 / 12.0 + 1.0
    } else if r2 < K_EPS {
        let t = (omega / 2.0).tan();
        (-2.0 * t * (4.0 * r2 * t * t - 3.0)) / (3.0 * omega)
    } else {
        let r = r2.sqrt();
        (r * 2.0 * (omega / 2.0).tan()).atan() / (r * omega)
    };
    finish_uv(
        f64::from(prm[0]) * xn * factor + f64::from(prm[2]),
        f64::from(prm[1]) * yn * factor + f64::from(prm[3]),
    )
}

fn project_full_opencv(
    prm: &[f32; MAX_PARAMS],
    x: f64,
    y: f64,
    z: f64,
) -> Result<(f64, f64), DomainError> {
    if z < BEHIND_EPS {
        return Err(out_of_range("camera_z", "> 1e-12 (CV +Z)", format!("{z}")));
    }
    let xn = x / z;
    let yn = y / z;
    let r2 = xn * xn + yn * yn;
    let den = 1.0 + r2 * (f64::from(prm[9]) + r2 * (f64::from(prm[10]) + r2 * f64::from(prm[11])));
    if den <= 1e-6 {
        return Err(out_of_range("full_opencv_den", "> 1e-6", format!("{den}")));
    }
    let radial =
        (1.0 + r2 * (f64::from(prm[4]) + r2 * (f64::from(prm[5]) + r2 * f64::from(prm[8])))) / den;
    let p1 = f64::from(prm[6]);
    let p2 = f64::from(prm[7]);
    finish_uv(
        f64::from(prm[0]) * (xn * radial + 2.0 * p1 * xn * yn + p2 * (r2 + 2.0 * xn * xn))
            + f64::from(prm[2]),
        f64::from(prm[1]) * (yn * radial + 2.0 * p2 * xn * yn + p1 * (r2 + 2.0 * yn * yn))
            + f64::from(prm[3]),
    )
}

fn project_division(
    prm: &[f32; MAX_PARAMS],
    simple: bool,
    x: f64,
    y: f64,
    z: f64,
) -> Result<(f64, f64), DomainError> {
    let fx = f64::from(prm[0]);
    let fy = if simple {
        f64::from(prm[0])
    } else {
        f64::from(prm[1])
    };
    let cx = if simple {
        f64::from(prm[1])
    } else {
        f64::from(prm[2])
    };
    let cy = if simple {
        f64::from(prm[2])
    } else {
        f64::from(prm[3])
    };
    let k = if simple {
        f64::from(prm[3])
    } else {
        f64::from(prm[4])
    };
    let rho2 = x * x + y * y;
    let disc = z * z - 4.0 * rho2 * k;
    if disc < 0.0 {
        return Err(out_of_range("division_disc", ">= 0", format!("{disc}")));
    }
    let r = 2.0 / (z + disc.sqrt());
    finish_uv(fx * r * x + cx, fy * r * y + cy)
}

fn project_eucm(
    prm: &[f32; MAX_PARAMS],
    x: f64,
    y: f64,
    z: f64,
) -> Result<(f64, f64), DomainError> {
    if z < BEHIND_EPS {
        return Err(out_of_range("camera_z", "> 1e-12 (CV +Z)", format!("{z}")));
    }
    let alpha = f64::from(prm[4]);
    let beta = f64::from(prm[5]);
    let rho2 = beta * (x * x + y * y) + z * z;
    if rho2 < 0.0 {
        return Err(out_of_range("eucm_rho2", ">= 0", format!("{rho2}")));
    }
    let den = alpha * rho2.sqrt() + (1.0 - alpha) * z;
    if den < BEHIND_EPS {
        return Err(out_of_range("eucm_den", ">= 1e-12", format!("{den}")));
    }
    finish_uv(
        f64::from(prm[0]) * x / den + f64::from(prm[2]),
        f64::from(prm[1]) * y / den + f64::from(prm[3]),
    )
}

fn project_rad_tan_thin_prism(
    prm: &[f32; MAX_PARAMS],
    x: f64,
    y: f64,
    z: f64,
) -> Result<(f64, f64), DomainError> {
    if z < BEHIND_EPS {
        return Err(out_of_range("camera_z", "> 1e-12 (CV +Z)", format!("{z}")));
    }
    let xn = x / z;
    let yn = y / z;
    let r = (xn * xn + yn * yn).sqrt();
    let mut uu = xn;
    let mut vv = yn;
    if r > BEHIND_EPS {
        let th = r.atan();
        uu *= th / r;
        vv *= th / r;
    }
    let th2 = uu * uu + vv * vv;
    let mut pw = 1.0;
    let mut rad = 1.0;
    for i in 0..6 {
        pw *= th2;
        rad += f64::from(prm[4 + i]) * pw;
    }
    let xd0 = rad * uu;
    let yd0 = rad * vv;
    let p0 = f64::from(prm[10]);
    let p1 = f64::from(prm[11]);
    let s0 = f64::from(prm[12]);
    let s1 = f64::from(prm[13]);
    let s2 = f64::from(prm[14]);
    let s3 = f64::from(prm[15]);
    let x2 = xd0 * xd0;
    let y2 = yd0 * yd0;
    let xy = xd0 * yd0;
    let r2 = x2 + y2;
    let r4 = r2 * r2;
    let xd = xd0 + 2.0 * p1 * xy + p0 * (r2 + 2.0 * x2) + s0 * r2 + s1 * r4;
    let yd = yd0 + 2.0 * p0 * xy + p1 * (r2 + 2.0 * y2) + s2 * r2 + s3 * r4;
    finish_uv(
        f64::from(prm[0]) * xd + f64::from(prm[2]),
        f64::from(prm[1]) * yd + f64::from(prm[3]),
    )
}

fn project_skewed(
    p: &[f32; MAX_PARAMS],
    x: f64,
    y: f64,
    z: f64,
) -> Result<(f64, f64), DomainError> {
    let base = p[13] as i32;
    let (qx, qy) = if base == SKEW_BASE_PERSPECTIVE as i32 {
        if z < BEHIND_EPS {
            return Err(out_of_range("camera_z", "> 1e-12 (CV +Z)", format!("{z}")));
        }
        (x / z, y / z)
    } else {
        let r = (x * x + y * y).sqrt();
        let theta = r.atan2(z);
        let k = if base == SKEW_BASE_FISHEYE as i32 {
            if theta < 1e-3 {
                (1.0 - theta * theta / 3.0) / z
            } else {
                theta / r
            }
        } else if r < 1e-6 {
            (1.0 - theta * theta / 24.0) / z
        } else {
            (2.0 * (0.5 * theta).sin()) / r
        };
        (x * k, y * k)
    };
    let r2 = qx * qx + qy * qy;
    let (dx, dy) = if p[14] != 0.0f32 {
        let radial = (1.0 + r2 * (f64::from(p[5]) + r2 * (f64::from(p[6]) + r2 * f64::from(p[7]))))
            / (1.0 + r2 * (f64::from(p[8]) + r2 * (f64::from(p[9]) + r2 * f64::from(p[10]))));
        (
            qx * radial
                + 2.0 * f64::from(p[11]) * qx * qy
                + f64::from(p[12]) * (r2 + 2.0 * qx * qx),
            qy * radial
                + 2.0 * f64::from(p[12]) * qx * qy
                + f64::from(p[11]) * (r2 + 2.0 * qy * qy),
        )
    } else {
        let radial = 1.0
            + r2 * (f64::from(p[5])
                + r2 * (f64::from(p[6]) + r2 * (f64::from(p[7]) + r2 * f64::from(p[8]))));
        (
            qx * radial
                + 2.0 * f64::from(p[9]) * qx * qy
                + f64::from(p[10]) * (r2 + 2.0 * qx * qx)
                + f64::from(p[11]) * r2,
            qy * radial
                + 2.0 * f64::from(p[10]) * qx * qy
                + f64::from(p[9]) * (r2 + 2.0 * qy * qy)
                + f64::from(p[12]) * r2,
        )
    };
    finish_uv(
        f64::from(p[0]) * dx + f64::from(p[4]) * dy + f64::from(p[2]),
        f64::from(p[1]) * dy + f64::from(p[3]),
    )
}

fn finish_uv(u: f64, v: f64) -> Result<(f64, f64), DomainError> {
    if u.is_finite() && v.is_finite() {
        Ok((u, v))
    } else {
        Err(out_of_range("source_pixel", "finite", "nonfinite"))
    }
}

/// Model formula only — no fold check (C++ `project_raw`).
fn project_raw(
    model_id: i32,
    params: &[f32; MAX_PARAMS],
    x: f64,
    y: f64,
    z: f64,
) -> Result<(f64, f64), DomainError> {
    match model_id {
        SKEWED => project_skewed(params, x, y, z),
        COLMAP_FOV => project_fov(params, x, y, z),
        COLMAP_FULL_OPENCV => project_full_opencv(params, x, y, z),
        COLMAP_SIMPLE_DIVISION => project_division(params, true, x, y, z),
        COLMAP_DIVISION => project_division(params, false, x, y, z),
        COLMAP_EUCM => project_eucm(params, x, y, z),
        COLMAP_RAD_TAN_THIN_PRISM_FISHEYE => project_rad_tan_thin_prism(params, x, y, z),
        _ => Err(invalid(
            "model_id",
            format!("unsupported source camera model_id {model_id}"),
        )),
    }
}

/// `source_unfolded` mirror — reject directions past the fold.
fn unfolded(
    model_id: i32,
    params: &[f32; MAX_PARAMS],
    x: f64,
    y: f64,
    z: f64,
    u: f64,
    v: f64,
) -> Result<(), DomainError> {
    let l = (x * x + y * y + z * z).sqrt();
    if l <= 0.0 {
        return Err(out_of_range("ray_length", "> 0", format!("{l}")));
    }
    let n = [x / l, y / l, z / l];
    let mut t = [1.0, 0.0, 0.0];
    if n[0].abs() >= 0.9 {
        t = [0.0, 1.0, 0.0];
    }
    let mut e1 = [
        t[1] * n[2] - t[2] * n[1],
        t[2] * n[0] - t[0] * n[2],
        t[0] * n[1] - t[1] * n[0],
    ];
    let e1l = (e1[0] * e1[0] + e1[1] * e1[1] + e1[2] * e1[2]).sqrt();
    for c in &mut e1 {
        *c /= e1l;
    }
    let e2 = [
        n[1] * e1[2] - n[2] * e1[1],
        n[2] * e1[0] - n[0] * e1[2],
        n[0] * e1[1] - n[1] * e1[0],
    ];
    let (au, av) = project_raw(
        model_id,
        params,
        n[0] + UNFOLD_STEP * e1[0],
        n[1] + UNFOLD_STEP * e1[1],
        n[2] + UNFOLD_STEP * e1[2],
    )?;
    let (bu, bv) = project_raw(
        model_id,
        params,
        n[0] + UNFOLD_STEP * e2[0],
        n[1] + UNFOLD_STEP * e2[1],
        n[2] + UNFOLD_STEP * e2[2],
    )?;
    let det = (au - u) * (bv - v) - (av - v) * (bu - u);
    if det > 0.0 {
        Ok(())
    } else {
        Err(invalid("source_project", "forward map folded (unfolded)"))
    }
}

/// View-space direction → source pixel. Err behind camera, out of domain, or folded.
pub fn project(
    model_id: i32,
    params: &[f32; MAX_PARAMS],
    x: f64,
    y: f64,
    z: f64,
) -> Result<(f64, f64), DomainError> {
    let (u, v) = project_raw(model_id, params, x, y, z)?;
    unfolded(model_id, params, x, y, z, u, v)?;
    Ok((u, v))
}

/// Whether `model_id` is a projectable SourceCamera id (not `-1` exact-fit).
pub fn is_supported_source_model(model_id: i32) -> bool {
    matches!(
        model_id,
        COLMAP_FULL_OPENCV
            | COLMAP_FOV
            | COLMAP_RAD_TAN_THIN_PRISM_FISHEYE
            | COLMAP_SIMPLE_DIVISION
            | COLMAP_DIVISION
            | COLMAP_EUCM
            | SKEWED
    )
}

/// Scale pixel-valued intrinsics by resolution factors (in-place).
///
/// Mirrors C++ `if (!(sx > 0.0) || !(sy > 0.0)) return` plus finite: NaN,
/// ±Infinity, and non-positive scales are no-ops (do not poison params).
pub fn rescale(model_id: i32, params: &mut [f32; MAX_PARAMS], sx: f64, sy: f64) {
    // C++ uses negated `> 0`; NaN makes that true. Also reject Infinity so
    // scaled slots cannot become Inf.
    if !(sx.is_finite() && sx > 0.0) || !(sy.is_finite() && sy > 0.0) || (sx == 1.0 && sy == 1.0) {
        return;
    }
    if model_id == COLMAP_SIMPLE_DIVISION {
        // One focal for both axes; non-square resize uses sx for the shared f / cx.
        params[0] = (f64::from(params[0]) * sx) as f32;
        params[1] = (f64::from(params[1]) * sx) as f32;
        params[2] = (f64::from(params[2]) * sy) as f32;
        return;
    }
    params[0] = (f64::from(params[0]) * sx) as f32;
    params[1] = (f64::from(params[1]) * sy) as f32;
    params[2] = (f64::from(params[2]) * sx) as f32;
    params[3] = (f64::from(params[3]) * sy) as f32;
    if model_id == SKEWED {
        params[4] = (f64::from(params[4]) * sx) as f32;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ErrorCode;

    fn fov_identity_params() -> [f32; MAX_PARAMS] {
        let mut p = [0.0f32; MAX_PARAMS];
        p[0] = 800.0; // fx
        p[1] = 800.0; // fy
        p[2] = 400.0; // cx
        p[3] = 300.0; // cy
        p[4] = 0.8; // omega
        p
    }

    #[test]
    fn project_rejects_behind_camera_z() {
        let p = fov_identity_params();
        let err = project(COLMAP_FOV, &p, 0.1, -0.2, 0.0).unwrap_err();
        assert_eq!(err.code(), ErrorCode::ValueOutOfRange);
        let err_neg = project(COLMAP_FOV, &p, 0.0, 0.0, -1.0).unwrap_err();
        assert_eq!(err_neg.code(), ErrorCode::ValueOutOfRange);
    }

    #[test]
    fn rescale_nan_infinity_nonpositive_are_noop() {
        let mut params = [0.0f32; MAX_PARAMS];
        params[0] = 100.0;
        params[1] = 100.0;
        params[2] = 50.0;
        params[3] = 50.0;
        let before = params;
        for (sx, sy) in [
            (f64::NAN, 1.0),
            (1.0, f64::NAN),
            (f64::INFINITY, 1.0),
            (1.0, f64::NEG_INFINITY),
            (0.0, 1.0),
            (-1.0, 2.0),
            (2.0, 0.0),
        ] {
            let mut p = before;
            rescale(COLMAP_FOV, &mut p, sx, sy);
            assert_eq!(p, before, "sx={sx} sy={sy} must no-op");
        }
        let mut p = before;
        rescale(COLMAP_FOV, &mut p, 2.0, 2.0);
        assert_eq!(p[0], 200.0);
        assert_eq!(p[1], 200.0);
        assert_eq!(p[2], 100.0);
        assert_eq!(p[3], 100.0);
    }

    #[test]
    fn fov_forward_optical_axis_sample() {
        let p = fov_identity_params();
        // On-axis: factor path uses r2 < eps; u,v = principal point.
        let (u, v) = project(COLMAP_FOV, &p, 0.0, 0.0, 1.0).unwrap();
        assert!((u - 400.0).abs() < 1e-9, "u={u}");
        assert!((v - 300.0).abs() < 1e-9, "v={v}");

        // Off-axis closed-form check against project_fov (omega large, r2 large).
        let x = 0.25f64;
        let y = -0.125f64;
        let z = 1.0f64;
        let omega = f64::from(p[4]);
        let xn = x / z;
        let yn = y / z;
        let r = (xn * xn + yn * yn).sqrt();
        let factor = (r * 2.0 * (omega / 2.0).tan()).atan() / (r * omega);
        let expect_u = f64::from(p[0]) * xn * factor + f64::from(p[2]);
        let expect_v = f64::from(p[1]) * yn * factor + f64::from(p[3]);
        let (u, v) = project(COLMAP_FOV, &p, x, y, z).unwrap();
        assert!((u - expect_u).abs() < 1e-9, "u={u} expect={expect_u}");
        assert!((v - expect_v).abs() < 1e-9, "v={v} expect={expect_v}");
        assert!(u.is_finite() && v.is_finite());
    }
}
