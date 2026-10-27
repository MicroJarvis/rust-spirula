//! Pinhole projection with the OpenCV `k1 k2 p1 p2` prefix (P1-F04 API).
//!
//! Wider models, Newton undistort, and ray generation live in
//! [`crate::camera_math`] / [`crate::camera_model`] (P3-F01). See
//! `docs/phase-1/geometry.md` and `docs/phase-3/camera.md`.

use crate::DomainError;
use crate::math::{Mat3, Vec2, Vec3, invalid, out_of_range};

/// OpenCV radial/tangential prefix. Slots after `p2` are not modeled here.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OpenCvDistortion {
    k1: f64,
    k2: f64,
    p1: f64,
    p2: f64,
}

impl OpenCvDistortion {
    pub fn try_new(k1: f64, k2: f64, p1: f64, p2: f64) -> Result<Self, DomainError> {
        if [k1, k2, p1, p2].iter().all(|value| value.is_finite()) {
            Ok(Self { k1, k2, p1, p2 })
        } else {
            Err(out_of_range("distortion", "finite", "nonfinite"))
        }
    }

    pub const fn coefficients(self) -> [f64; 4] {
        [self.k1, self.k2, self.p1, self.p2]
    }

    fn apply(self, u: f64, v: f64) -> Result<(f64, f64), DomainError> {
        let r2 = u * u + v * v;
        let radial = 1.0 + self.k1 * r2 + self.k2 * r2 * r2;
        let u_d = u * radial + 2.0 * self.p1 * u * v + self.p2 * (r2 + 2.0 * u * u);
        let v_d = v * radial + self.p1 * (r2 + 2.0 * v * v) + 2.0 * self.p2 * u * v;
        if u_d.is_finite() && v_d.is_finite() {
            Ok((u_d, v_d))
        } else {
            Err(out_of_range("distortion", "finite pixel", "nonfinite"))
        }
    }
}

/// OpenGL camera-to-world pose. `rotation` is row-major.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CameraPose {
    rotation: Mat3,
    translation: Vec3,
}

impl CameraPose {
    pub fn try_new(rotation: Mat3, translation: Vec3) -> Result<Self, DomainError> {
        Ok(Self {
            rotation: rotation.try_rotation()?,
            translation,
        })
    }

    pub fn identity() -> Self {
        Self {
            rotation: Mat3::identity(),
            translation: Vec3::try_new(0.0, 0.0, 0.0).expect("zero"),
        }
    }

    pub const fn rotation(self) -> Mat3 {
        self.rotation
    }

    pub const fn translation(self) -> Vec3 {
        self.translation
    }
}

/// Pinhole camera. Intrinsics are `[fx, fy, cx, cy]`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PinholeCamera {
    width: u32,
    height: u32,
    fx: f64,
    fy: f64,
    cx: f64,
    cy: f64,
    distortion: OpenCvDistortion,
    pose: CameraPose,
}

impl PinholeCamera {
    #[allow(clippy::too_many_arguments)]
    pub fn try_new(
        width: u32,
        height: u32,
        fx: f64,
        fy: f64,
        cx: f64,
        cy: f64,
        distortion: OpenCvDistortion,
        pose: CameraPose,
    ) -> Result<Self, DomainError> {
        if width == 0 || height == 0 {
            return Err(out_of_range(
                "image_size",
                "positive",
                format!("{width}x{height}"),
            ));
        }
        if !(fx.is_finite() && fy.is_finite() && fx > 0.0 && fy > 0.0) {
            return Err(out_of_range("focal", "positive finite", "invalid"));
        }
        if !(cx.is_finite() && cy.is_finite()) {
            return Err(out_of_range("principal_point", "finite", "nonfinite"));
        }
        Ok(Self {
            width,
            height,
            fx,
            fy,
            cx,
            cy,
            distortion,
            pose,
        })
    }

    pub const fn width(self) -> u32 {
        self.width
    }

    pub const fn height(self) -> u32 {
        self.height
    }

    pub const fn intrinsics(self) -> [f64; 4] {
        [self.fx, self.fy, self.cx, self.cy]
    }

    pub const fn pose(self) -> CameraPose {
        self.pose
    }

    /// Project a world point to pixels.
    ///
    /// OpenGL camera-space `z >= 0` is behind or on the plane and returns
    /// `SP-INP-003` with field `camera_z`. No pixel is produced.
    pub fn project_world(self, world: Vec3) -> Result<Vec2, DomainError> {
        let delta = (world - self.pose.translation)?;
        let gl = self.pose.rotation.transpose().try_mul_vec(delta)?;
        if !gl.z().is_finite() || gl.z() >= 0.0 {
            return Err(out_of_range("camera_z", "< 0", format!("{}", gl.z())));
        }
        let depth = -gl.z();
        let u = gl.x() / depth;
        let v = -gl.y() / depth;
        if !(u.is_finite() && v.is_finite()) {
            return Err(invalid(
                "projection",
                "normalized coordinates are not finite",
            ));
        }
        let (u_d, v_d) = self.distortion.apply(u, v)?;
        Vec2::new_finite(self.fx * u_d + self.cx, self.fy * v_d + self.cy)
    }
}
