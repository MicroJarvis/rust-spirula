//! Unrepresentable-lens fit surface (C-DATA-13).
//!
//! Binding: `docs/phase-3/camera.md`. Redistort decision and empty-domain
//! fallback match pinned `DistortionFit.h/.cpp`. The full Chebyshev /
//! Levenberg-Marquardt `fit_camera` / `fit_camera_auto` numeric port is
//! **in progress** in this tip: callers get a typed incomplete error until
//! C++-generated fit goldens are matched. Do **not** treat a successful
//! demote of exact OpenCV/ThinPrism tiers as “fit complete”.

use crate::DomainError;
use crate::camera_model::{
    CameraModel, DISTORTION_PARAMS, DistortionTier, RedistortDecision, redistort_decision,
};
use crate::math::{out_of_range, unimplemented_opt};

pub use crate::camera_model::EXACT_FIT_PX as K_EXACT_FIT_PX;

/// Fitted engine camera (`dsfit::FitTarget`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FitTarget {
    pub model: CameraModel,
    pub distortion: DistortionTier,
    pub fx: f64,
    pub fy: f64,
    pub cx: f64,
    pub cy: f64,
    pub coeffs: [f64; DISTORTION_PARAMS],
}

/// Fit quality (`dsfit::FitResult`).
#[derive(Clone, Debug, PartialEq)]
pub struct FitResult {
    pub target: FitTarget,
    pub rms_px: f64,
    pub max_px: f64,
    pub samples: u32,
    pub fov_deg: f64,
    pub invertible: bool,
    pub ok: bool,
}

impl FitResult {
    pub fn redistort(&self) -> Result<RedistortDecision, DomainError> {
        redistort_decision(self.max_px)
    }
}

/// Options (`dsfit::FitOptions`) — stored for API parity; LM path pending.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FitOptions {
    pub grid: u32,
    pub refine_intrinsics: bool,
    pub max_rms_px: f64,
    pub lawson_iters: u32,
    pub coeff_mask: u8,
}

impl Default for FitOptions {
    fn default() -> Self {
        Self {
            grid: 64,
            refine_intrinsics: true,
            max_rms_px: 0.5,
            lawson_iters: 8,
            coeff_mask: 0xff,
        }
    }
}

/// Source forward map: view-space direction → pixel.
pub type SourceProject = dyn Fn(f64, f64, f64) -> Result<(f64, f64), DomainError>;

/// Empty-domain / last-resort usable camera (`fit_camera_auto` fallback).
pub fn fallback_fisheye_none(width: u32, height: u32) -> Result<FitResult, DomainError> {
    if width == 0 || height == 0 {
        return Err(out_of_range(
            "image_size",
            "positive",
            format!("{width}x{height}"),
        ));
    }
    let fx = f64::from(width) * 0.5;
    let fy = fx;
    let cx = f64::from(width) * 0.5;
    let cy = f64::from(height) * 0.5;
    if !(fx.is_finite() && fy.is_finite()) {
        return Err(out_of_range("focal", "finite", "nonfinite"));
    }
    Ok(FitResult {
        target: FitTarget {
            model: CameraModel::Fisheye,
            distortion: DistortionTier::None,
            fx,
            fy,
            cx,
            cy,
            coeffs: [0.0; DISTORTION_PARAMS],
        },
        rms_px: f64::INFINITY,
        max_px: f64::INFINITY,
        samples: 0,
        fov_deg: 0.0,
        invertible: true,
        ok: false,
    })
}

/// Full Chebyshev LM fit — typed incomplete until goldens land.
pub fn fit_camera(
    _src: &SourceProject,
    width: u32,
    height: u32,
    _target_model: CameraModel,
    _opt: FitOptions,
) -> Result<FitResult, DomainError> {
    if width == 0 || height == 0 {
        return Err(out_of_range(
            "image_size",
            "positive",
            format!("{width}x{height}"),
        ));
    }
    Err(unimplemented_opt(
        "distortion_fit",
        "fit_camera Chebyshev/LM port pending C++ golden parity (C-DATA-13; AD in phase-3/camera.md)",
    ))
}

/// Auto model selection — incomplete LM; documents always-usable fallback contract.
pub fn fit_camera_auto(
    src: &SourceProject,
    width: u32,
    height: u32,
    opt: FitOptions,
) -> Result<FitResult, DomainError> {
    // Probe optical axis: if source projects nothing, return usable fallback.
    match src(0.0, 0.0, 1.0) {
        Ok(_) => fit_camera(src, width, height, CameraModel::Fisheye, opt),
        Err(_) => fallback_fisheye_none(width, height),
    }
}

/// Preserve original source params alongside a fitted camera (rollback).
#[derive(Clone, Debug, PartialEq)]
pub struct FittedWithSource {
    pub fitted: FitResult,
    pub source_model: i32,
    pub source_params: [f32; 16],
    pub decision: RedistortDecision,
}

impl FittedWithSource {
    pub fn from_fit(
        fitted: FitResult,
        source_model: i32,
        source_params: [f32; 16],
    ) -> Result<Self, DomainError> {
        let decision = fitted.redistort()?;
        let source_model = match decision {
            RedistortDecision::KeepImagesAsIs => -1,
            RedistortDecision::KeepSourceForWarp => source_model,
        };
        Ok(Self {
            fitted,
            source_model,
            source_params,
            decision,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_fit_constant() {
        assert_eq!(crate::camera_model::EXACT_FIT_PX, 0.1);
        assert_eq!(K_EXACT_FIT_PX, 0.1);
    }

    #[test]
    fn empty_domain_fallback_usable() {
        let r = fallback_fisheye_none(64, 48).unwrap();
        assert!(!r.ok);
        assert_eq!(r.target.model, CameraModel::Fisheye);
        assert_eq!(r.target.distortion, DistortionTier::None);
        assert!(r.target.fx > 0.0);
    }

    #[test]
    fn fit_camera_is_explicitly_incomplete() {
        let src = |_x, _y, _z| Ok((32.0, 24.0));
        let err =
            fit_camera(&src, 64, 48, CameraModel::Pinhole, FitOptions::default()).unwrap_err();
        assert_eq!(err.code(), crate::ErrorCode::UnimplementedOption);
    }

    #[test]
    fn keep_source_params_on_warp_decision() {
        let fitted = FitResult {
            target: FitTarget {
                model: CameraModel::Pinhole,
                distortion: DistortionTier::ThinPrism,
                fx: 100.0,
                fy: 100.0,
                cx: 32.0,
                cy: 24.0,
                coeffs: [0.0; DISTORTION_PARAMS],
            },
            rms_px: 0.2,
            max_px: 0.2,
            samples: 10,
            fov_deg: 90.0,
            invertible: true,
            ok: true,
        };
        let mut params = [0.0f32; 16];
        params[0] = 1.0;
        let wrapped = FittedWithSource::from_fit(fitted, 7, params).unwrap();
        assert_eq!(wrapped.source_model, 7);
        assert_eq!(wrapped.decision, RedistortDecision::KeepSourceForWarp);
        assert_eq!(wrapped.source_params[0], 1.0);
    }

    #[test]
    fn clear_source_when_exact() {
        let fitted = FitResult {
            target: FitTarget {
                model: CameraModel::Pinhole,
                distortion: DistortionTier::OpenCv,
                fx: 100.0,
                fy: 100.0,
                cx: 32.0,
                cy: 24.0,
                coeffs: [0.0; DISTORTION_PARAMS],
            },
            rms_px: 0.01,
            max_px: 0.05,
            samples: 10,
            fov_deg: 60.0,
            invertible: true,
            ok: true,
        };
        let wrapped = FittedWithSource::from_fit(fitted, 7, [0.0; 16]).unwrap();
        assert_eq!(wrapped.source_model, -1);
        assert_eq!(wrapped.decision, RedistortDecision::KeepImagesAsIs);
    }
}
