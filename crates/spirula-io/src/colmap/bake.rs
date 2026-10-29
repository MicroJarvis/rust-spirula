//! COLMAP source params → engine model / distortion tier (+ P3-F01 fit).
//!
//! Numeric boundary (no silent sanitize): source arity + finite params, physical
//! focal / size constraints, and finite f32 conversions for bake + fit outputs.
//! Reuses the same positive-finite / finite-f32 rules as P3-F01 catalog parsing.

use spirula_core::{
    CameraModel, DISTORTION_PARAMS, DistortionTier, FitOptions, K_EXACT_FIT_PX, MAX_PARAMS,
    demote_distortion, fit_camera_auto_cancelable, source_project,
};

use super::budget::err;
use super::models::{require_exact_param_count, require_model_by_name, require_model_id_by_name};
use super::types::{BakedIntrinsics, ColmapCamera};
use spirula_core::ErrorCode;

/// f64 → f32 without silent overflow to ±inf / NaN.
fn finite_f32(v: f64, field: &str) -> Result<f32, spirula_core::DomainError> {
    if !v.is_finite() {
        return Err(err(
            ErrorCode::ValueOutOfRange,
            field,
            format!("{field} must be finite, got {v}"),
        ));
    }
    let f = v as f32;
    if !f.is_finite() {
        return Err(err(
            ErrorCode::ValueOutOfRange,
            field,
            format!("{field}={v} is not finite as f32"),
        ));
    }
    Ok(f)
}

fn positive_finite_f32(v: f64, field: &str) -> Result<f32, spirula_core::DomainError> {
    let f = finite_f32(v, field)?;
    if f <= 0.0 {
        return Err(err(
            ErrorCode::ValueOutOfRange,
            field,
            format!("{field} must be positive finite, got {v}"),
        ));
    }
    Ok(f)
}

fn p(cam: &ColmapCamera, i: usize) -> Result<f32, spirula_core::DomainError> {
    finite_f32(cam.params[i], "params")
}

/// Source-model arity, finite params, and positive resolution (pre-bake gate).
pub fn validate_source_camera(cam: &ColmapCamera) -> Result<(), spirula_core::DomainError> {
    let info = require_model_by_name(&cam.model)?;
    require_exact_param_count(info, &cam.params)?;
    if cam.width == 0 || cam.height == 0 {
        return Err(err(
            ErrorCode::ValueOutOfRange,
            "size",
            format!(
                "camera {} has non-positive size {}x{}",
                cam.camera_id, cam.width, cam.height
            ),
        ));
    }
    // Width/height must fit u32 before fit / frame emit.
    if u32::try_from(cam.width).is_err() || u32::try_from(cam.height).is_err() {
        return Err(err(
            ErrorCode::ValueOutOfRange,
            "size",
            format!(
                "camera {} size {}x{} exceeds u32",
                cam.camera_id, cam.width, cam.height
            ),
        ));
    }
    for (i, v) in cam.params.iter().enumerate() {
        if !v.is_finite() {
            return Err(err(
                ErrorCode::ValueOutOfRange,
                "params",
                format!("camera {} param {i} is nonfinite", cam.camera_id),
            ));
        }
        // Reject values that cannot round-trip into finite f32 (fit / redistort path).
        let _ = finite_f32(*v, "params")?;
    }
    Ok(())
}

/// Physical focal / principal-point / distortion constraints on baked outputs.
fn validate_baked_intrinsics(
    bi: &BakedIntrinsics,
    require_positive_focal: bool,
) -> Result<(), spirula_core::DomainError> {
    if require_positive_focal {
        if !(bi.fx.is_finite() && bi.fx > 0.0 && bi.fy.is_finite() && bi.fy > 0.0) {
            return Err(err(
                ErrorCode::ValueOutOfRange,
                "focal",
                format!(
                    "baked focal must be positive finite f32, got fx={} fy={}",
                    bi.fx, bi.fy
                ),
            ));
        }
    } else if !(bi.fx.is_finite() && bi.fy.is_finite()) {
        return Err(err(
            ErrorCode::ValueOutOfRange,
            "focal",
            format!(
                "baked focal must be finite f32, got fx={} fy={}",
                bi.fx, bi.fy
            ),
        ));
    }
    if !(bi.cx.is_finite() && bi.cy.is_finite()) {
        return Err(err(
            ErrorCode::ValueOutOfRange,
            "principal_point",
            format!(
                "baked principal point must be finite f32, got cx={} cy={}",
                bi.cx, bi.cy
            ),
        ));
    }
    for (i, v) in bi.dist.iter().enumerate() {
        if !v.is_finite() {
            return Err(err(
                ErrorCode::ValueOutOfRange,
                "dist",
                format!("baked dist[{i}] is nonfinite"),
            ));
        }
    }
    for (i, v) in bi.source.params.iter().enumerate() {
        if !v.is_finite() {
            return Err(err(
                ErrorCode::ValueOutOfRange,
                "source_params",
                format!("source_params[{i}] is nonfinite as f32"),
            ));
        }
    }
    if !bi.source.fit_max_px.is_finite() {
        return Err(err(
            ErrorCode::ValueOutOfRange,
            "fit_max_px",
            "fit_max_px is nonfinite",
        ));
    }
    Ok(())
}

/// `bake_colmap_intrins` (pin `ColmapParser.cpp:309-407`).
///
/// Low-level gate: exact source arity, finite params, and positive size before
/// any tier mapping (PINHOLE extra params / 0×0 fail here, not only in
/// [`bake_and_fit`]).
pub fn bake_colmap_intrinsics(
    cam: &ColmapCamera,
) -> Result<BakedIntrinsics, spirula_core::DomainError> {
    validate_source_camera(cam)?;
    let mut o = BakedIntrinsics::default();
    let params = &cam.params;

    if cam.model == "SIMPLE_PINHOLE" {
        o.fx = positive_finite_f32(cam.params[0], "fx")?;
        o.fy = o.fx;
        o.cx = p(cam, 1)?;
        o.cy = p(cam, 2)?;
    } else if cam.model == "PINHOLE" {
        o.fx = positive_finite_f32(cam.params[0], "fx")?;
        o.fy = positive_finite_f32(cam.params[1], "fy")?;
        o.cx = p(cam, 2)?;
        o.cy = p(cam, 3)?;
    } else if cam.model == "SIMPLE_FISHEYE" {
        o.fx = positive_finite_f32(cam.params[0], "fx")?;
        o.fy = o.fx;
        o.cx = p(cam, 1)?;
        o.cy = p(cam, 2)?;
        o.model = CameraModel::Fisheye;
    } else if cam.model == "FISHEYE" {
        o.fx = positive_finite_f32(cam.params[0], "fx")?;
        o.fy = positive_finite_f32(cam.params[1], "fy")?;
        o.cx = p(cam, 2)?;
        o.cy = p(cam, 3)?;
        o.model = CameraModel::Fisheye;
    } else if cam.model == "SIMPLE_RADIAL"
        || cam.model == "RADIAL"
        || cam.model == "SIMPLE_RADIAL_FISHEYE"
        || cam.model == "RADIAL_FISHEYE"
    {
        let two = cam.model == "RADIAL" || cam.model == "RADIAL_FISHEYE";
        o.fx = positive_finite_f32(cam.params[0], "fx")?;
        o.fy = o.fx;
        o.cx = p(cam, 1)?;
        o.cy = p(cam, 2)?;
        o.distortion = DistortionTier::OpenCv;
        o.dist[0] = p(cam, 3)?;
        if two {
            o.dist[1] = p(cam, 4)?;
        }
        if cam.model == "SIMPLE_RADIAL_FISHEYE" || cam.model == "RADIAL_FISHEYE" {
            o.model = CameraModel::Fisheye;
        }
    } else if cam.model == "OPENCV" {
        o.fx = positive_finite_f32(cam.params[0], "fx")?;
        o.fy = positive_finite_f32(cam.params[1], "fy")?;
        o.cx = p(cam, 2)?;
        o.cy = p(cam, 3)?;
        o.distortion = DistortionTier::OpenCv;
        o.dist[0] = p(cam, 4)?;
        o.dist[1] = p(cam, 5)?;
        o.dist[2] = p(cam, 6)?;
        o.dist[3] = p(cam, 7)?;
    } else if cam.model == "FULL_OPENCV"
        && params[9] == 0.0
        && params[10] == 0.0
        && params[11] == 0.0
    {
        o.fx = positive_finite_f32(cam.params[0], "fx")?;
        o.fy = positive_finite_f32(cam.params[1], "fy")?;
        o.cx = p(cam, 2)?;
        o.cy = p(cam, 3)?;
        o.distortion = DistortionTier::ThinPrism;
        o.dist[0] = p(cam, 4)?;
        o.dist[1] = p(cam, 5)?;
        o.dist[2] = p(cam, 8)?;
        o.dist[4] = p(cam, 6)?;
        o.dist[5] = p(cam, 7)?;
    } else if cam.model == "OPENCV_FISHEYE" {
        o.fx = positive_finite_f32(cam.params[0], "fx")?;
        o.fy = positive_finite_f32(cam.params[1], "fy")?;
        o.cx = p(cam, 2)?;
        o.cy = p(cam, 3)?;
        o.model = CameraModel::Fisheye;
        o.distortion = DistortionTier::ThinPrism;
        o.dist[0] = p(cam, 4)?;
        o.dist[1] = p(cam, 5)?;
        o.dist[2] = p(cam, 6)?;
        o.dist[3] = p(cam, 7)?;
    } else if cam.model == "THIN_PRISM_FISHEYE" {
        o.fx = positive_finite_f32(cam.params[0], "fx")?;
        o.fy = positive_finite_f32(cam.params[1], "fy")?;
        o.cx = p(cam, 2)?;
        o.cy = p(cam, 3)?;
        o.model = CameraModel::Fisheye;
        o.distortion = DistortionTier::ThinPrism;
        o.dist[0] = p(cam, 4)?;
        o.dist[1] = p(cam, 5)?;
        o.dist[2] = p(cam, 8)?;
        o.dist[3] = p(cam, 9)?;
        o.dist[4] = p(cam, 6)?;
        o.dist[5] = p(cam, 7)?;
        o.dist[6] = p(cam, 10)?;
        o.dist[7] = p(cam, 11)?;
    } else if cam.model == "EQUIRECTANGULAR" {
        let w = cam.params[0];
        let h = cam.params[1];
        o.fx = positive_finite_f32(w / (2.0 * std::f64::consts::PI), "fx")?;
        o.fy = positive_finite_f32(h / std::f64::consts::PI, "fy")?;
        o.cx = finite_f32(w / 2.0, "cx")?;
        o.cy = finite_f32(h / 2.0, "cy")?;
        o.model = CameraModel::Equirectangular;
    } else {
        let id = require_model_id_by_name(&cam.model)?;
        o.source.source_model = id;
        for (i, v) in params.iter().enumerate().take(MAX_PARAMS) {
            o.source.params[i] = finite_f32(*v, "source_params")?;
        }
        return Ok(o);
    }

    let mut coeffs = [0.0f64; DISTORTION_PARAMS];
    for (i, v) in o.dist.iter().enumerate() {
        coeffs[i] = f64::from(*v);
    }
    let (tier, demoted) = demote_distortion(o.distortion, &coeffs)?;
    o.distortion = tier;
    for (i, v) in demoted.iter().enumerate() {
        o.dist[i] = finite_f32(*v, "dist")?;
    }
    Ok(o)
}

/// Fit an unrepresentable COLMAP source onto a compiled tier (`fit_colmap_source`).
///
/// # Cancel boundary
///
/// Load polls immediately before/after each camera. Inside the fit,
/// [`fit_camera_auto_cancelable`] also polls between domain-scan azimuth steps
/// and between candidate models. Dense mid-`fit_model` least-squares remains
/// non-preemptible (HOLD gap). Host FOV 64×48 typically tens of ms.
pub fn fit_colmap_source(
    cam: &ColmapCamera,
    bi: &mut BakedIntrinsics,
) -> Result<(), spirula_core::DomainError> {
    fit_colmap_source_cancelable(cam, bi, None)
}

/// Same as [`fit_colmap_source`] with a cancel token for the auto-fit ladder.
pub fn fit_colmap_source_cancelable(
    cam: &ColmapCamera,
    bi: &mut BakedIntrinsics,
    cancel: Option<&spirula_core::CancelToken>,
) -> Result<(), spirula_core::DomainError> {
    if bi.source.source_model < 0 {
        return Ok(());
    }
    let src_model = bi.source.source_model;
    let src_params = bi.source.params;
    let project = |x: f64, y: f64, z: f64| -> Result<(f64, f64), spirula_core::DomainError> {
        source_project(src_model, &src_params, x, y, z)
    };
    let w = u32::try_from(cam.width).map_err(|_| {
        err(
            ErrorCode::ValueOutOfRange,
            "width",
            format!("camera width {} exceeds u32", cam.width),
        )
    })?;
    let h = u32::try_from(cam.height).map_err(|_| {
        err(
            ErrorCode::ValueOutOfRange,
            "height",
            format!("camera height {} exceeds u32", cam.height),
        )
    })?;
    if w == 0 || h == 0 {
        return Err(err(
            ErrorCode::ValueOutOfRange,
            "size",
            format!("camera {w}x{h} must be positive"),
        ));
    }
    let fit = fit_camera_auto_cancelable(&project, w, h, FitOptions::default(), cancel)?;
    bi.model = fit.target.model;
    bi.fx = finite_f32(fit.target.fx, "fx")?;
    bi.fy = finite_f32(fit.target.fy, "fy")?;
    bi.cx = finite_f32(fit.target.cx, "cx")?;
    bi.cy = finite_f32(fit.target.cy, "cy")?;
    // Fitted focals must be physically positive (same rule as P3-F01 Camera::try_new).
    let _ = positive_finite_f32(f64::from(bi.fx), "fx")?;
    let _ = positive_finite_f32(f64::from(bi.fy), "fy")?;
    for i in 0..DISTORTION_PARAMS {
        bi.dist[i] = finite_f32(fit.target.coeffs[i], "dist")?;
    }
    let mut coeffs = [0.0f64; DISTORTION_PARAMS];
    for (i, v) in bi.dist.iter().enumerate() {
        coeffs[i] = f64::from(*v);
    }
    let (tier, demoted) = demote_distortion(fit.target.distortion, &coeffs)?;
    bi.distortion = tier;
    for (i, v) in demoted.iter().enumerate() {
        bi.dist[i] = finite_f32(*v, "dist")?;
    }
    bi.source.fit_max_px = finite_f32(fit.max_px, "fit_max_px")?;
    if (!fit.ok && fit.samples == 0) || fit.max_px < K_EXACT_FIT_PX {
        bi.source.source_model = -1;
    }
    Ok(())
}

/// Bake + optional fit for one camera record.
pub fn bake_and_fit(cam: &ColmapCamera) -> Result<BakedIntrinsics, spirula_core::DomainError> {
    validate_source_camera(cam)?;
    validate_equirectangular(cam)?;
    let mut bi = bake_colmap_intrinsics(cam)?;
    let fitted = bi.source.source_model >= 0;
    if fitted {
        fit_colmap_source(cam, &mut bi)?;
    }
    // Exact-tier bake always needs positive focals; fit path validated above.
    validate_baked_intrinsics(&bi, true)?;
    Ok(bi)
}

/// Validate EQUIRECTANGULAR param/resolution contract (pin load path).
pub fn validate_equirectangular(cam: &ColmapCamera) -> Result<(), spirula_core::DomainError> {
    if cam.model != "EQUIRECTANGULAR" {
        return Ok(());
    }
    if cam.params.len() != 2 {
        return Err(err(
            ErrorCode::MalformedRecord,
            "params",
            format!(
                "EQUIRECTANGULAR camera {} needs 2 parameters, got {}",
                cam.camera_id,
                cam.params.len()
            ),
        ));
    }
    let pw = cam.params[0];
    let ph = cam.params[1];
    if (pw - cam.width as f64).abs() > 0.5 || (ph - cam.height as f64).abs() > 0.5 {
        return Err(err(
            ErrorCode::ValueOutOfRange,
            "equirect_params",
            format!(
                "EQUIRECTANGULAR camera {} params [{pw}, {ph}] disagree with resolution {}x{}",
                cam.camera_id, cam.width, cam.height
            ),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pinhole(params: Vec<f64>) -> ColmapCamera {
        ColmapCamera {
            camera_id: 1,
            model: "PINHOLE".into(),
            width: 64,
            height: 48,
            params,
        }
    }

    #[test]
    fn pinhole_bakes_exact() {
        let bi = bake_and_fit(&pinhole(vec![100.0, 100.0, 32.0, 24.0])).unwrap();
        assert_eq!(bi.model, CameraModel::Pinhole);
        assert_eq!(bi.distortion, DistortionTier::None);
        assert!((bi.fx - 100.0).abs() < 1e-6);
        assert_eq!(bi.source.source_model, -1);
    }

    #[test]
    fn bake_colmap_intrinsics_rejects_extra_params_and_zero_size() {
        // Direct low-level API (not only bake_and_fit).
        let extra = ColmapCamera {
            camera_id: 1,
            model: "PINHOLE".into(),
            width: 64,
            height: 48,
            params: vec![100.0, 100.0, 32.0, 24.0, 0.01],
        };
        assert!(
            bake_colmap_intrinsics(&extra).is_err(),
            "PINHOLE with 5 params must fail exact arity"
        );
        let zero = ColmapCamera {
            camera_id: 1,
            model: "PINHOLE".into(),
            width: 0,
            height: 0,
            params: vec![100.0, 100.0, 32.0, 24.0],
        };
        assert!(
            bake_colmap_intrinsics(&zero).is_err(),
            "0x0 size must fail at bake_colmap_intrinsics"
        );
        let ok = bake_colmap_intrinsics(&pinhole(vec![100.0, 100.0, 32.0, 24.0])).unwrap();
        assert!((ok.fx - 100.0).abs() < 1e-6);
    }

    #[test]
    fn rejects_negative_zero_and_overflow_focal() {
        let neg = bake_and_fit(&pinhole(vec![-100.0, -100.0, 32.0, 24.0]));
        assert!(neg.is_err(), "negative focal must err");
        let zero = bake_and_fit(&pinhole(vec![0.0, 0.0, 32.0, 24.0]));
        assert!(zero.is_err(), "zero focal must err");
        let huge = bake_and_fit(&pinhole(vec![1e100, 1e100, 32.0, 24.0]));
        assert!(huge.is_err(), "f32-overflow focal must err, got {huge:?}");
    }

    #[test]
    fn keeps_source_metadata_on_fit_models() {
        // FOV is unrepresentable → fit path; original source params retained.
        let cam = ColmapCamera {
            camera_id: 2,
            model: "FOV".into(),
            width: 64,
            height: 48,
            params: vec![50.0, 50.0, 32.0, 24.0, 0.7],
        };
        let bi = bake_and_fit(&cam).unwrap();
        assert!(bi.fx.is_finite() && bi.fx > 0.0);
        assert_eq!(bi.source.params[0], 50.0);
        assert_eq!(bi.source.params[4], 0.7);
        // Either kept as active source or cleared on exact fit — params bytes stay.
        assert!(bi.source.source_model == 7 || bi.source.source_model == -1);
    }

    #[test]
    fn eighteen_models_accepted_with_legal_params() {
        use super::super::models::COLMAP_MODELS;
        for m in COLMAP_MODELS {
            let mut params = vec![0.0; m.num_params];
            // Plausible defaults: focals / sizes positive where models expect them.
            match m.name {
                "EQUIRECTANGULAR" => {
                    params[0] = 64.0;
                    params[1] = 48.0;
                }
                "FOV" => {
                    params[0] = 50.0;
                    params[1] = 50.0;
                    params[2] = 32.0;
                    params[3] = 24.0;
                    params[4] = 0.5;
                }
                _ => {
                    if m.num_params >= 1 {
                        params[0] = 80.0;
                    }
                    if m.num_params >= 2 {
                        params[1] = 80.0;
                    }
                    if m.num_params >= 3 {
                        params[2] = 32.0;
                    }
                    if m.num_params >= 4 {
                        params[3] = 24.0;
                    }
                    // SIMPLE_* share fx=fy via params[0]; principal at 1,2.
                    if m.name.starts_with("SIMPLE_") && m.name != "SIMPLE_DIVISION" {
                        if m.num_params >= 2 {
                            params[1] = 32.0;
                        }
                        if m.num_params >= 3 {
                            params[2] = 24.0;
                        }
                    }
                }
            }
            let cam = ColmapCamera {
                camera_id: m.id,
                model: m.name.into(),
                width: 64,
                height: 48,
                params,
            };
            bake_and_fit(&cam).unwrap_or_else(|e| panic!("{}: {e}", m.name));
        }
    }
}
