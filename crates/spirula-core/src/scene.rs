//! Similarity transform and raw Gaussian splat values.
//!
//! JSON spelling, centering modes, and PLY bytes stay in P3-F05 and P3-F06.
//! See `docs/phase-1/geometry.md`.

use crate::DomainError;
use crate::math::{Mat3, Quat, Vec3, invalid, out_of_range};

/// `C0` from the reference SH DC conversion.
pub const SH_C0: f64 = 0.28209479177387814;

/// `p_train = scale * R * p_world + t`, with row-major `R`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SceneTransform {
    rotation: Mat3,
    translation: Vec3,
    scale: f64,
}

impl SceneTransform {
    pub fn try_new(rotation: Mat3, translation: Vec3, scale: f64) -> Result<Self, DomainError> {
        if !(scale.is_finite() && scale > 0.0) {
            return Err(out_of_range("scale", "positive finite", format!("{scale}")));
        }
        Ok(Self {
            rotation: rotation.try_rotation()?,
            translation,
            scale,
        })
    }

    pub const fn rotation(self) -> Mat3 {
        self.rotation
    }

    pub const fn translation(self) -> Vec3 {
        self.translation
    }

    pub const fn scale(self) -> f64 {
        self.scale
    }

    /// `R^T t / scale`, the shift applied before rotation.
    pub fn translation_before_rotation(self) -> Vec3 {
        self.rotation
            .transpose()
            .mul_vec(self.translation)
            .scale(1.0 / self.scale)
    }

    pub fn apply(self, world: Vec3) -> Result<Vec3, DomainError> {
        let rotated = self.rotation.mul_vec(world).scale(self.scale);
        Vec3::try_new(
            rotated.x() + self.translation.x(),
            rotated.y() + self.translation.y(),
            rotated.z() + self.translation.z(),
        )
    }

    /// `R_w = R^T`, `scale_w = 1/scale`, `t_w = -(1/scale) R^T t`.
    pub fn inverse(self) -> Result<Self, DomainError> {
        let rotation = self.rotation.transpose();
        let scale = 1.0 / self.scale;
        let shifted = rotation.mul_vec(self.translation).scale(-scale);
        let translation = Vec3::try_new(shifted.x(), shifted.y(), shifted.z())?;
        Self::try_new(rotation, translation, scale)
    }
}

/// One Gaussian as stored: log scale, logit opacity, `wxyz`, DC.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GaussianSplat {
    mean: Vec3,
    rotation: Quat,
    scale_log: Vec3,
    opacity_logit: f64,
    dc: Vec3,
}

impl GaussianSplat {
    pub fn try_new(
        mean: Vec3,
        rotation: Quat,
        scale_log: Vec3,
        opacity_logit: f64,
        dc: Vec3,
    ) -> Result<Self, DomainError> {
        if !opacity_logit.is_finite() {
            return Err(out_of_range("opacity_logit", "finite", "nonfinite"));
        }
        for axis in scale_log.to_array() {
            if !axis.exp().is_finite() {
                return Err(out_of_range("scale_log", "exp finite", format!("{axis}")));
            }
        }
        Ok(Self {
            mean,
            rotation,
            scale_log,
            opacity_logit,
            dc,
        })
    }

    pub const fn mean(self) -> Vec3 {
        self.mean
    }

    pub const fn rotation(self) -> Quat {
        self.rotation
    }

    pub const fn scale_log(self) -> Vec3 {
        self.scale_log
    }

    pub const fn opacity_logit(self) -> f64 {
        self.opacity_logit
    }

    pub const fn dc(self) -> Vec3 {
        self.dc
    }

    /// Activated scales. Construction already rejected a non-finite `exp`.
    pub fn scales(self) -> Vec3 {
        Vec3::try_new(
            self.scale_log.x().exp(),
            self.scale_log.y().exp(),
            self.scale_log.z().exp(),
        )
        .expect("exp checked")
    }

    pub fn opacity(self) -> f64 {
        let x = self.opacity_logit;
        if x >= 0.0 {
            1.0 / (1.0 + (-x).exp())
        } else {
            let e = x.exp();
            e / (1.0 + e)
        }
    }

    pub fn base_color(self) -> Vec3 {
        Vec3::try_new(
            self.dc.x() * SH_C0 + 0.5,
            self.dc.y() * SH_C0 + 0.5,
            self.dc.z() * SH_C0 + 0.5,
        )
        .expect("finite dc")
    }
}

/// Ordered splats. Bounds are the axis-aligned box of the means.
#[derive(Clone, Debug, PartialEq)]
pub struct SplatSet {
    splats: Vec<GaussianSplat>,
}

impl SplatSet {
    pub fn try_from_splats(splats: Vec<GaussianSplat>) -> Result<Self, DomainError> {
        if splats.is_empty() {
            return Err(invalid("splats", "splat set is empty"));
        }
        Ok(Self { splats })
    }

    pub fn len(&self) -> usize {
        self.splats.len()
    }

    pub fn is_empty(&self) -> bool {
        self.splats.is_empty()
    }

    pub fn splats(&self) -> &[GaussianSplat] {
        &self.splats
    }

    pub fn bounds(&self) -> (Vec3, Vec3) {
        let mut min = self.splats[0].mean().to_array();
        let mut max = min;
        for splat in &self.splats[1..] {
            for axis in 0..3 {
                let value = splat.mean().to_array()[axis];
                min[axis] = min[axis].min(value);
                max[axis] = max[axis].max(value);
            }
        }
        (
            Vec3::try_new(min[0], min[1], min[2]).expect("mean"),
            Vec3::try_new(max[0], max[1], max[2]).expect("mean"),
        )
    }
}
