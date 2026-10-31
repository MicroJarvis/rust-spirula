//! Versioned GPU-independent render request and CPU projection reference.
//!
//! P4-F01 stops at a validated scene/camera boundary. Upload, projection
//! kernels, sorting, rasterization, compositing and readback belong to later
//! features and therefore do not appear in this API.

use spirula_core::{DomainError, ErrorCode, PinholeCamera, SplatSet};

pub const RENDER_CONTRACT_VERSION: u32 = 1;
const SUPPORTED_SH_DEGREE: u8 = 0;

fn error(code: ErrorCode, field: &str, message: impl Into<String>) -> DomainError {
    DomainError::try_new(code, message)
        .expect("render error message")
        .with_context(spirula_core::ContextKey::Field, field)
        .expect("render field")
}

/// Primitive parameterization admitted by the P4-F01 CPU reference.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenderPrimitive {
    /// Degree-0 Gaussian splats with log scales, logit opacity and DC color.
    Gaussian3d,
    /// Future reference primitive; kept explicit so it cannot be silently used.
    MipGaussian,
    /// Future generalized Gaussian primitive.
    GutGaussian,
}

impl RenderPrimitive {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Gaussian3d => "3dgs",
            Self::MipGaussian => "mip",
            Self::GutGaussian => "3dgut",
        }
    }
}

/// Color encoding at the scene/render boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorSpace {
    LinearRgb,
    Srgb,
}

impl ColorSpace {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::LinearRgb => "linear-rgb",
            Self::Srgb => "srgb",
        }
    }
}

/// Numerical thresholds shared by later projection/raster stages.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RenderThresholds {
    /// Hard alpha cutoff used by later rasterization stages.
    pub alpha_cutoff: f64,
    /// Stop compositing when remaining transmittance is at or below this value.
    pub early_out_transmittance: f64,
}

impl Default for RenderThresholds {
    fn default() -> Self {
        Self {
            alpha_cutoff: 1e-4,
            early_out_transmittance: 1e-4,
        }
    }
}

impl RenderThresholds {
    fn validate(self) -> Result<(), DomainError> {
        if !self.alpha_cutoff.is_finite() || !(0.0..=1.0).contains(&self.alpha_cutoff) {
            return Err(error(
                ErrorCode::ValueOutOfRange,
                "alpha_cutoff",
                "must be finite and in [0,1]",
            ));
        }
        if !self.early_out_transmittance.is_finite()
            || !(0.0..=1.0).contains(&self.early_out_transmittance)
            || self.early_out_transmittance == 0.0
        {
            return Err(error(
                ErrorCode::ValueOutOfRange,
                "early_out_transmittance",
                "must be finite and in (0,1]",
            ));
        }
        Ok(())
    }
}

/// Complete, validated host-side render request.
#[derive(Clone, Debug, PartialEq)]
pub struct RenderRequest {
    pub contract_version: u32,
    pub camera: PinholeCamera,
    pub scene: SplatSet,
    pub primitive: RenderPrimitive,
    pub color_space: ColorSpace,
    pub sh_degree: u8,
    pub thresholds: RenderThresholds,
}

impl RenderRequest {
    pub fn try_new(
        camera: PinholeCamera,
        scene: SplatSet,
        primitive: RenderPrimitive,
        color_space: ColorSpace,
        sh_degree: u8,
        thresholds: RenderThresholds,
    ) -> Result<Self, DomainError> {
        if primitive != RenderPrimitive::Gaussian3d {
            return Err(error(
                ErrorCode::UnimplementedOption,
                "primitive",
                format!("{} is not implemented by P4-F01", primitive.as_str()),
            ));
        }
        if color_space != ColorSpace::LinearRgb {
            return Err(error(
                ErrorCode::UnimplementedOption,
                "color_space",
                format!("{} is not implemented by P4-F01", color_space.as_str()),
            ));
        }
        if sh_degree != SUPPORTED_SH_DEGREE {
            return Err(error(
                ErrorCode::UnimplementedOption,
                "sh_degree",
                "only degree-0 DC color is available in P4-F01",
            ));
        }
        thresholds.validate()?;
        if scene.is_empty() {
            return Err(error(
                ErrorCode::InvalidArgument,
                "scene",
                "scene must contain at least one splat",
            ));
        }
        if camera.width() == 0 || camera.height() == 0 {
            return Err(error(
                ErrorCode::ValueOutOfRange,
                "image_size",
                "render dimensions must be positive",
            ));
        }
        Ok(Self {
            contract_version: RENDER_CONTRACT_VERSION,
            camera,
            scene,
            primitive,
            color_space,
            sh_degree,
            thresholds,
        })
    }
}

/// CPU reference output for the scene/camera boundary. Later stages consume
/// this stable projection record to build tiles and rasterize.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProjectedGaussian {
    pub source_index: usize,
    pub pixel: [f64; 2],
    /// Positive OpenGL camera depth (`-camera_z`).
    pub depth: f64,
    pub opacity: f64,
    pub base_color: [f64; 3],
}

pub struct CpuReference;

impl CpuReference {
    /// Project all splat means using the P3 camera convention. This function
    /// intentionally emits no pixels and performs no sorting or compositing.
    pub fn project(request: &RenderRequest) -> Result<Vec<ProjectedGaussian>, DomainError> {
        if request.contract_version != RENDER_CONTRACT_VERSION {
            return Err(error(
                ErrorCode::UnsupportedVersion,
                "contract_version",
                "unsupported render contract version",
            ));
        }
        let pose = request.camera.pose();
        let rotation = pose.rotation().transpose();
        let translation = pose.translation();
        request
            .scene
            .splats()
            .iter()
            .enumerate()
            .map(|(source_index, splat)| {
                let delta = (splat.mean() - translation)?;
                let camera = rotation.try_mul_vec(delta)?;
                if !camera.z().is_finite() || camera.z() >= 0.0 {
                    return Err(error(
                        ErrorCode::ValueOutOfRange,
                        "camera_z",
                        "splat is behind or on the OpenGL camera plane",
                    ));
                }
                let pixel = request.camera.project_world(splat.mean())?;
                let base = splat.base_color();
                Ok(ProjectedGaussian {
                    source_index,
                    pixel: [pixel.x(), pixel.y()],
                    depth: -camera.z(),
                    opacity: splat.opacity(),
                    base_color: base.to_array(),
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use spirula_core::{CameraPose, GaussianSplat, OpenCvDistortion, Quat, Vec3};

    fn camera() -> PinholeCamera {
        PinholeCamera::try_new(
            64,
            48,
            32.0,
            32.0,
            32.0,
            24.0,
            OpenCvDistortion::try_new(0.0, 0.0, 0.0, 0.0).unwrap(),
            CameraPose::identity(),
        )
        .unwrap()
    }

    fn scene() -> SplatSet {
        SplatSet::try_from_splats(vec![
            GaussianSplat::try_new(
                Vec3::try_new(0.0, 0.0, -2.0).unwrap(),
                Quat::identity(),
                Vec3::try_new(0.0, 0.0, 0.0).unwrap(),
                0.0,
                Vec3::try_new(0.0, 0.0, 0.0).unwrap(),
            )
            .unwrap(),
        ])
        .unwrap()
    }

    #[test]
    fn cpu_projection_matches_opengl_center_and_depth() {
        let request = RenderRequest::try_new(
            camera(),
            scene(),
            RenderPrimitive::Gaussian3d,
            ColorSpace::LinearRgb,
            0,
            RenderThresholds::default(),
        )
        .unwrap();
        let projected = CpuReference::project(&request).unwrap();
        assert_eq!(projected.len(), 1);
        assert_eq!(projected[0].source_index, 0);
        assert_eq!(projected[0].pixel, [32.0, 24.0]);
        assert_eq!(projected[0].depth, 2.0);
        assert!((projected[0].opacity - 0.5).abs() < 1e-12);
    }

    #[test]
    fn unsupported_options_and_thresholds_are_typed() {
        let err = RenderRequest::try_new(
            camera(),
            scene(),
            RenderPrimitive::MipGaussian,
            ColorSpace::LinearRgb,
            0,
            RenderThresholds::default(),
        )
        .unwrap_err();
        assert_eq!(err.code(), ErrorCode::UnimplementedOption);

        let err = RenderRequest::try_new(
            camera(),
            scene(),
            RenderPrimitive::Gaussian3d,
            ColorSpace::LinearRgb,
            1,
            RenderThresholds::default(),
        )
        .unwrap_err();
        assert_eq!(err.code(), ErrorCode::UnimplementedOption);

        let err = RenderRequest::try_new(
            camera(),
            scene(),
            RenderPrimitive::Gaussian3d,
            ColorSpace::LinearRgb,
            0,
            RenderThresholds {
                alpha_cutoff: f64::NAN,
                ..RenderThresholds::default()
            },
        )
        .unwrap_err();
        assert_eq!(err.code(), ErrorCode::ValueOutOfRange);
    }

    #[test]
    fn behind_camera_is_rejected_without_partial_output() {
        let scene = SplatSet::try_from_splats(vec![
            GaussianSplat::try_new(
                Vec3::try_new(0.0, 0.0, 2.0).unwrap(),
                Quat::identity(),
                Vec3::try_new(0.0, 0.0, 0.0).unwrap(),
                0.0,
                Vec3::try_new(0.0, 0.0, 0.0).unwrap(),
            )
            .unwrap(),
        ])
        .unwrap();
        let request = RenderRequest::try_new(
            camera(),
            scene,
            RenderPrimitive::Gaussian3d,
            ColorSpace::LinearRgb,
            0,
            RenderThresholds::default(),
        )
        .unwrap();
        let err = CpuReference::project(&request).unwrap_err();
        assert_eq!(err.code(), ErrorCode::ValueOutOfRange);
    }

    #[test]
    fn contract_version_is_checked_at_projection() {
        let mut request = RenderRequest::try_new(
            camera(),
            scene(),
            RenderPrimitive::Gaussian3d,
            ColorSpace::LinearRgb,
            0,
            RenderThresholds::default(),
        )
        .unwrap();
        request.contract_version += 1;
        let err = CpuReference::project(&request).unwrap_err();
        assert_eq!(err.code(), ErrorCode::UnsupportedVersion);
    }
}
