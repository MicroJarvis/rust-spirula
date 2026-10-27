//! Camera model / distortion tier enums and 8-wide coefficient rows.
//!
//! Binding: `docs/phase-3/camera.md`. Mirrors pinned
//! `src/core/CameraModel.h` (`9a24f099…`).

use crate::DomainError;
use crate::math::{invalid, out_of_range};

/// Storage width of every coefficient row (tail zero-extended).
pub const DISTORTION_PARAMS: usize = 8;

/// Exact-fit threshold (pixels) below which parsers keep source images.
pub const EXACT_FIT_PX: f64 = 0.1;

/// Projection model (`CameraModelType`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum CameraModel {
    Pinhole = 0,
    Fisheye = 1,
    Equisolid = 2,
    Equirectangular = 3,
}

impl CameraModel {
    pub fn from_i32(v: i32) -> Result<Self, DomainError> {
        match v {
            0 => Ok(Self::Pinhole),
            1 => Ok(Self::Fisheye),
            2 => Ok(Self::Equisolid),
            3 => Ok(Self::Equirectangular),
            _ => Err(out_of_range("camera_model", "0..=3", format!("{v}"))),
        }
    }

    pub const fn as_i32(self) -> i32 {
        self as i32
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pinhole => "PINHOLE",
            Self::Fisheye => "FISHEYE",
            Self::Equisolid => "EQUISOLID",
            Self::Equirectangular => "EQUIRECTANGULAR",
        }
    }

    /// COLMAP / Nerfstudio name map (`camera_model_from_name`).
    pub fn from_name(name: &str) -> Result<Self, DomainError> {
        match name {
            "PINHOLE" | "SIMPLE_PINHOLE" | "SIMPLE_RADIAL" | "RADIAL" | "OPENCV" => {
                Ok(Self::Pinhole)
            }
            "FISHEYE"
            | "SIMPLE_FISHEYE"
            | "SIMPLE_RADIAL_FISHEYE"
            | "RADIAL_FISHEYE"
            | "OPENCV_FISHEYE"
            | "THIN_PRISM_FISHEYE" => Ok(Self::Fisheye),
            "EQUISOLID" => Ok(Self::Equisolid),
            "EQUIRECTANGULAR" => Ok(Self::Equirectangular),
            "ORTHOGRAPHIC" | "ORTHO" => Err(crate::math::unimplemented_opt(
                "camera_model",
                "orthographic is unsupported in P3-F01 (AD-P3-F01-01; no C++ CameraModelType)",
            )),
            _ => Err(invalid(
                "camera_model",
                format!("unknown model name {name}"),
            )),
        }
    }
}

/// Distortion tier (`CameraDistortionType`). Slot meaning is per-tier.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum DistortionTier {
    None = 0,
    OpenCv = 1,
    ThinPrism = 2,
}

impl DistortionTier {
    pub fn from_i32(v: i32) -> Result<Self, DomainError> {
        match v {
            0 => Ok(Self::None),
            1 => Ok(Self::OpenCv),
            2 => Ok(Self::ThinPrism),
            _ => Err(out_of_range("distortion_tier", "0..=2", format!("{v}"))),
        }
    }

    pub const fn as_i32(self) -> i32 {
        self as i32
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "NONE",
            Self::OpenCv => "OPENCV",
            Self::ThinPrism => "THIN_PRISM",
        }
    }

    pub fn from_name(name: &str) -> Result<Self, DomainError> {
        match name {
            "NONE" => Ok(Self::None),
            "OPENCV" => Ok(Self::OpenCv),
            "THIN_PRISM" => Ok(Self::ThinPrism),
            _ => Err(invalid(
                "distortion_tier",
                format!("unknown distortion name {name}"),
            )),
        }
    }

    pub const fn num_params(self) -> usize {
        match self {
            Self::None => 0,
            Self::OpenCv => 4,
            Self::ThinPrism => 8,
        }
    }
}

/// Whether CUDA/kernel instantiation compiles this pair (`camera_distortion_is_compiled`).
pub const fn is_compiled_pair(model: CameraModel, tier: DistortionTier) -> bool {
    match model {
        CameraModel::Pinhole | CameraModel::Fisheye | CameraModel::Equisolid => true,
        CameraModel::Equirectangular => matches!(tier, DistortionTier::None),
    }
}

fn exact_zero(v: f64) -> bool {
    v == 0.0
}

fn require_finite_row(coeffs: &[f64; DISTORTION_PARAMS]) -> Result<(), DomainError> {
    if coeffs.iter().all(|c| c.is_finite()) {
        Ok(())
    } else {
        Err(out_of_range("distortion", "finite", "nonfinite"))
    }
}

/// Cheapest tier that still represents `coeffs` exactly (`camera_distortion_demote`).
pub fn demote_distortion(
    tier: DistortionTier,
    coeffs: &[f64; DISTORTION_PARAMS],
) -> Result<(DistortionTier, [f64; DISTORTION_PARAMS]), DomainError> {
    require_finite_row(coeffs)?;
    let mut out = *coeffs;
    let result = match tier {
        DistortionTier::ThinPrism => {
            if exact_zero(coeffs[2])
                && exact_zero(coeffs[3])
                && exact_zero(coeffs[6])
                && exact_zero(coeffs[7])
            {
                out = [0.0; DISTORTION_PARAMS];
                out[0] = coeffs[0];
                out[1] = coeffs[1];
                out[2] = coeffs[4];
                out[3] = coeffs[5];
                return demote_distortion(DistortionTier::OpenCv, &out);
            }
            (DistortionTier::ThinPrism, out)
        }
        DistortionTier::OpenCv => {
            if exact_zero(coeffs[0])
                && exact_zero(coeffs[1])
                && exact_zero(coeffs[2])
                && exact_zero(coeffs[3])
            {
                (DistortionTier::None, [0.0; DISTORTION_PARAMS])
            } else {
                out[4] = 0.0;
                out[5] = 0.0;
                out[6] = 0.0;
                out[7] = 0.0;
                (DistortionTier::OpenCv, out)
            }
        }
        DistortionTier::None => (DistortionTier::None, [0.0; DISTORTION_PARAMS]),
    };
    Ok(result)
}

/// Rewrite coefficients from `from` into `to` when `to` contains `from`.
pub fn promote_distortion(
    from: DistortionTier,
    to: DistortionTier,
    coeffs: &[f64; DISTORTION_PARAMS],
) -> Result<[f64; DISTORTION_PARAMS], DomainError> {
    require_finite_row(coeffs)?;
    if from == to {
        return Ok(*coeffs);
    }
    let mut out = [0.0; DISTORTION_PARAMS];
    match (from, to) {
        (DistortionTier::None, _) => Ok(out),
        (DistortionTier::OpenCv, DistortionTier::ThinPrism) => {
            out[0] = coeffs[0];
            out[1] = coeffs[1];
            out[4] = coeffs[2];
            out[5] = coeffs[3];
            Ok(out)
        }
        _ => Err(invalid(
            "distortion_promote",
            format!("cannot promote {} -> {}", from.as_str(), to.as_str()),
        )),
    }
}

/// True when any used coefficient is nonzero (`has_distortion`).
pub fn has_distortion(tier: DistortionTier, coeffs: &[f64; DISTORTION_PARAMS]) -> bool {
    if matches!(tier, DistortionTier::None) {
        return false;
    }
    coeffs.iter().any(|c| *c != 0.0)
}

/// Redistort decision used by parsers: keep images when `max_px < EXACT_FIT_PX`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RedistortDecision {
    /// Fit is exact enough; clear `source_model` and keep original images.
    KeepImagesAsIs,
    /// Keep source lens for warp / redistort.
    KeepSourceForWarp,
}

pub fn redistort_decision(max_fit_px: f64) -> Result<RedistortDecision, DomainError> {
    if !max_fit_px.is_finite() || max_fit_px < 0.0 {
        return Err(out_of_range(
            "max_fit_px",
            "finite >= 0",
            format!("{max_fit_px}"),
        ));
    }
    if max_fit_px < EXACT_FIT_PX {
        Ok(RedistortDecision::KeepImagesAsIs)
    } else {
        Ok(RedistortDecision::KeepSourceForWarp)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn demote_thinprism_to_none_through_opencv() {
        let coeffs = [0.0; DISTORTION_PARAMS];
        let (t, out) = demote_distortion(DistortionTier::ThinPrism, &coeffs).unwrap();
        assert_eq!(t, DistortionTier::None);
        assert_eq!(out, [0.0; DISTORTION_PARAMS]);
    }

    #[test]
    fn promote_opencv_to_thinprism_slot_remap() {
        let mut c = [0.0; DISTORTION_PARAMS];
        c[0] = 0.1;
        c[1] = 0.2;
        c[2] = 0.3;
        c[3] = 0.4;
        let out =
            promote_distortion(DistortionTier::OpenCv, DistortionTier::ThinPrism, &c).unwrap();
        assert_eq!(out[0], 0.1);
        assert_eq!(out[1], 0.2);
        assert_eq!(out[4], 0.3);
        assert_eq!(out[5], 0.4);
        assert_eq!(out[2], 0.0);
        assert_eq!(out[3], 0.0);
    }

    #[test]
    fn orthographic_name_is_explicit_unsupported() {
        let err = CameraModel::from_name("ORTHOGRAPHIC").unwrap_err();
        assert_eq!(err.code(), crate::ErrorCode::UnimplementedOption);
    }

    #[test]
    fn redistort_threshold() {
        assert_eq!(
            redistort_decision(0.099).unwrap(),
            RedistortDecision::KeepImagesAsIs
        );
        assert_eq!(
            redistort_decision(0.1).unwrap(),
            RedistortDecision::KeepSourceForWarp
        );
    }
}
