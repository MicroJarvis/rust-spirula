//! Host camera projection math (CPU `f64`).
//!
//! Mirrors pinned `src/data/CameraMath.cpp` (`camhost`). Coordinates are
//! **normalized** image coords `((px-cx)/fx)`; rays are CV camera space
//! (+Z forward, +Y down). Binding: `docs/phase-3/camera.md`.

use crate::DomainError;
use crate::camera_model::{CameraModel, DISTORTION_PARAMS, DistortionTier, has_distortion};
use crate::math::{Vec2, Vec3, invalid, out_of_range};

const PI: f64 = std::f64::consts::PI;
const JAC_EPS: f64 = 1e-5;
const NEWTON_ITERS: usize = 8;
const DET_BREAK: f64 = 1e-12;
const UNDISTORT_RESIDUAL: f64 = 0.01;
const BEHIND_EPS: f64 = 1e-12;

/// IEEE-like `std::remainder` (nearest, ties to even via `round`).
fn remainder(x: f64, period: f64) -> f64 {
    x - period * (x / period).round()
}

fn distort_lens(
    u: f64,
    v: f64,
    tier: DistortionTier,
    d: &[f64; DISTORTION_PARAMS],
) -> Result<(f64, f64), DomainError> {
    if !(u.is_finite() && v.is_finite()) {
        return Err(out_of_range("normalized", "finite", "nonfinite"));
    }
    let r2 = u * u + v * v;
    let (out_u, out_v) = match tier {
        DistortionTier::None => (u, v),
        DistortionTier::OpenCv => {
            let radial = 1.0 + r2 * (d[0] + r2 * d[1]);
            (
                u * radial + 2.0 * d[2] * u * v + d[3] * (r2 + 2.0 * u * u),
                v * radial + 2.0 * d[3] * u * v + d[2] * (r2 + 2.0 * v * v),
            )
        }
        DistortionTier::ThinPrism => {
            let radial = 1.0 + r2 * (d[0] + r2 * (d[1] + r2 * (d[2] + r2 * d[3])));
            (
                u * radial + 2.0 * d[4] * u * v + d[5] * (r2 + 2.0 * u * u) + d[6] * r2,
                v * radial + 2.0 * d[5] * u * v + d[4] * (r2 + 2.0 * v * v) + d[7] * r2,
            )
        }
    };
    if out_u.is_finite() && out_v.is_finite() {
        Ok((out_u, out_v))
    } else {
        Err(out_of_range("distortion", "finite pixel", "nonfinite"))
    }
}

fn distort_jac(
    qx: f64,
    qy: f64,
    tier: DistortionTier,
    d: &[f64; DISTORTION_PARAMS],
) -> Result<[f64; 4], DomainError> {
    let f = distort_lens(qx, qy, tier, d)?;
    let fx = distort_lens(qx + JAC_EPS, qy, tier, d)?;
    let fy = distort_lens(qx, qy + JAC_EPS, tier, d)?;
    Ok([
        (fx.0 - f.0) / JAC_EPS,
        (fx.1 - f.1) / JAC_EPS,
        (fy.0 - f.0) / JAC_EPS,
        (fy.1 - f.1) / JAC_EPS,
    ])
}

/// Newton undistort; `Err` on nonfinite / folded / residual failure (no NaN).
pub fn undistort_lens(
    u: f64,
    v: f64,
    tier: DistortionTier,
    d: &[f64; DISTORTION_PARAMS],
) -> Result<(f64, f64), DomainError> {
    // Finite check before any identity short-circuit (None / zero coeffs).
    if !(u.is_finite() && v.is_finite()) {
        return Err(out_of_range("undistort", "finite", "nonfinite"));
    }
    if !d.iter().all(|c| c.is_finite()) {
        return Err(out_of_range("distortion", "finite", "nonfinite"));
    }
    if matches!(tier, DistortionTier::None) || !has_distortion(tier, d) {
        return Ok((u, v));
    }
    let mut qx = u;
    let mut qy = v;
    for _ in 0..NEWTON_ITERS {
        let f = distort_lens(qx, qy, tier, d)?;
        let j = distort_jac(qx, qy, tier, d)?;
        let det = j[0] * j[3] - j[2] * j[1];
        if det.abs() < DET_BREAK {
            break;
        }
        let rx = f.0 - u;
        let ry = f.1 - v;
        let inv = 1.0 / det;
        qx -= (j[3] * rx - j[2] * ry) * inv;
        qy -= (-j[1] * rx + j[0] * ry) * inv;
    }
    if !(qx.is_finite() && qy.is_finite()) {
        return Err(out_of_range("undistort", "finite", "nonfinite"));
    }
    let j = distort_jac(qx, qy, tier, d)?;
    let det = j[0] * j[3] - j[2] * j[1];
    if det.min(j[0].min(j[3])) <= 0.0 {
        return Err(invalid("undistort", "forward map folded (Jacobian)"));
    }
    let f = distort_lens(qx, qy, tier, d)?;
    if (f.0 - u).hypot(f.1 - v) >= UNDISTORT_RESIDUAL {
        return Err(invalid("undistort", "Newton residual too large"));
    }
    Ok((qx, qy))
}

/// Device `is_valid_distortion` bounds.
pub fn valid_distortion(
    u: f64,
    v: f64,
    tier: DistortionTier,
    d: &[f64; DISTORTION_PARAMS],
) -> Result<bool, DomainError> {
    if !(u.is_finite() && v.is_finite()) {
        return Err(out_of_range("normalized", "finite", "nonfinite"));
    }
    if !d.iter().all(|c| c.is_finite()) {
        return Err(out_of_range("distortion", "finite", "nonfinite"));
    }
    if matches!(tier, DistortionTier::None) {
        return Ok(true);
    }
    let j = distort_jac(u, v, tier, d)?;
    let det = j[0] * j[3] - j[2] * j[1];
    let jd = det.min(j[0].min(j[3]));
    let f = distort_lens(u, v, tier, d)?;
    Ok(jd > 0.25 && jd < 4.0 && (u * f.0 + v * f.1) >= 0.0)
}

/// Model + lens forward map in normalized coordinates.
pub fn distort_point(
    u: f64,
    v: f64,
    model: CameraModel,
    tier: DistortionTier,
    d: &[f64; DISTORTION_PARAMS],
) -> Result<(f64, f64), DomainError> {
    if !(u.is_finite() && v.is_finite()) {
        return Err(out_of_range("normalized", "finite", "nonfinite"));
    }
    if !d.iter().all(|c| c.is_finite()) {
        return Err(out_of_range("distortion", "finite", "nonfinite"));
    }
    if matches!(model, CameraModel::Equirectangular) {
        return Ok((u, v));
    }
    let (mut uu, mut vv) = (u, v);
    if matches!(model, CameraModel::Fisheye | CameraModel::Equisolid) {
        let r = uu.hypot(vv);
        let theta = r.atan();
        let k = if matches!(model, CameraModel::Fisheye) {
            if r < 1e-3 {
                1.0 - theta * theta / 6.0
            } else {
                theta / r
            }
        } else if r < 1e-3 {
            1.0 - theta * theta / 24.0
        } else {
            2.0 * (0.5 * theta).sin() / r
        };
        uu *= k;
        vv *= k;
    }
    distort_lens(uu, vv, tier, d)
}

fn norm3(x: f64, y: f64, z: f64) -> Result<Vec3, DomainError> {
    let mut l = (x * x + y * y + z * z).sqrt();
    if l < 1e-12 {
        l = 1.0;
    }
    Vec3::try_new(x / l, y / l, z / l)
}

/// Normalized source coords → unit CV ray.
pub fn generate_ray(
    u: f64,
    v: f64,
    model: CameraModel,
    tier: DistortionTier,
    d: &[f64; DISTORTION_PARAMS],
) -> Result<Vec3, DomainError> {
    if matches!(model, CameraModel::Equirectangular) {
        if u.abs() > PI || v.abs() > PI / 2.0 {
            return Err(out_of_range(
                "equirect",
                "|u|<=pi,|v|<=pi/2",
                format!("{u},{v}"),
            ));
        }
        let cl = v.cos();
        return Vec3::try_new(cl * u.sin(), v.sin(), cl * u.cos());
    }
    let (uu, vv) = if has_distortion(tier, d) {
        undistort_lens(u, v, tier, d)?
    } else {
        (u, v)
    };
    let r = uu.hypot(vv);
    match model {
        CameraModel::Fisheye => {
            if r >= PI {
                return Err(out_of_range("fisheye", "r < pi", format!("{r}")));
            }
            let s = if r < 1e-3 {
                1.0 - r * r / 6.0
            } else {
                r.sin() / r
            };
            norm3(uu * s, vv * s, r.cos())
        }
        CameraModel::Equisolid => {
            if r >= 2.0 {
                return Err(out_of_range("equisolid", "r < 2", format!("{r}")));
            }
            let k = (1.0 - 0.25 * r * r).max(0.0).sqrt();
            norm3(uu * k, vv * k, 1.0 - 0.5 * r * r)
        }
        CameraModel::Pinhole => norm3(uu, vv, 1.0),
        CameraModel::Equirectangular => unreachable!(),
    }
}

/// Inverse of generate_ray for perspective-normalized recovery.
pub fn undistort_point(
    u: f64,
    v: f64,
    model: CameraModel,
    tier: DistortionTier,
    d: &[f64; DISTORTION_PARAMS],
) -> Result<(f64, f64), DomainError> {
    let ray = generate_ray(u, v, model, tier, d)?;
    if ray.z() <= BEHIND_EPS {
        return Err(out_of_range(
            "undistort_point",
            "ray.z > 1e-12",
            format!("{}", ray.z()),
        ));
    }
    Ok((ray.x() / ray.z(), ray.y() / ray.z()))
}

/// Ray → normalized source coordinates.
pub fn project_ray(
    ray: Vec3,
    model: CameraModel,
    tier: DistortionTier,
    d: &[f64; DISTORTION_PARAMS],
) -> Result<(f64, f64), DomainError> {
    let (x, y, z) = (ray.x(), ray.y(), ray.z());
    if !(x.is_finite() && y.is_finite() && z.is_finite()) {
        return Err(out_of_range("ray", "finite", "nonfinite"));
    }
    if matches!(model, CameraModel::Equirectangular) {
        // C++: atan2(ray[0], ray[2]), atan2(ray[1], hypot(ray[0], ray[2]))
        let lon = x.atan2(z);
        let lat = y.atan2(x.hypot(z));
        return Ok((lon, lat));
    }
    let rz = z;
    let rxy = x.hypot(y);
    match model {
        CameraModel::Pinhole => {
            if rz <= BEHIND_EPS {
                return Err(out_of_range("camera_z", "> 1e-12 (CV +Z)", format!("{rz}")));
            }
            let u = x / rz;
            let v = y / rz;
            if !valid_distortion(u, v, tier, d)? {
                return Err(invalid("project_ray", "distortion invalid at projection"));
            }
            distort_lens(u, v, tier, d)
        }
        CameraModel::Fisheye | CameraModel::Equisolid => {
            let theta = rxy.atan2(rz);
            if theta >= PI {
                return Err(out_of_range("theta", "< pi", format!("{theta}")));
            }
            let r_ang = if matches!(model, CameraModel::Fisheye) {
                theta
            } else {
                2.0 * (0.5 * theta).sin()
            };
            let s = if rxy < 1e-12 { 0.0 } else { r_ang / rxy };
            let u = x * s;
            let v = y * s;
            if !valid_distortion(u, v, tier, d)? {
                return Err(invalid("project_ray", "distortion invalid at projection"));
            }
            distort_lens(u, v, tier, d)
        }
        CameraModel::Equirectangular => unreachable!(),
    }
}

/// Frame camera (`camhost::Camera`): fitted model + optional true source lens.
///
/// When `source_model >= 0`, forward `ray_pixel` uses [`crate::source_project`]
/// (source is ground truth). Fitted `model`/`tier`/`coeffs` seed
/// [`Self::pixel_ray`] only.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FrameCamera {
    pub model: CameraModel,
    pub tier: DistortionTier,
    pub width: u32,
    pub height: u32,
    pub fx: f64,
    pub fy: f64,
    pub cx: f64,
    pub cy: f64,
    pub coeffs: [f64; DISTORTION_PARAMS],
    /// Active redistort source id; `-1` = exact tier (fitted forward).
    pub source_model: i32,
    pub source_params: [f32; 16],
}

impl FrameCamera {
    #[allow(clippy::too_many_arguments)]
    pub fn try_new(
        model: CameraModel,
        tier: DistortionTier,
        width: u32,
        height: u32,
        fx: f64,
        fy: f64,
        cx: f64,
        cy: f64,
        coeffs: [f64; DISTORTION_PARAMS],
    ) -> Result<Self, DomainError> {
        Self::try_new_with_source(
            model, tier, width, height, fx, fy, cx, cy, coeffs, -1, [0.0; 16],
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn try_new_with_source(
        model: CameraModel,
        tier: DistortionTier,
        width: u32,
        height: u32,
        fx: f64,
        fy: f64,
        cx: f64,
        cy: f64,
        coeffs: [f64; DISTORTION_PARAMS],
        source_model: i32,
        source_params: [f32; 16],
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
        if !coeffs.iter().all(|c| c.is_finite()) {
            return Err(out_of_range("distortion", "finite", "nonfinite"));
        }
        Ok(Self {
            model,
            tier,
            width,
            height,
            fx,
            fy,
            cx,
            cy,
            coeffs,
            source_model,
            source_params,
        })
    }

    pub fn longitude_period(self) -> f64 {
        if matches!(self.model, CameraModel::Equirectangular) && self.fx > 0.0 {
            2.0 * PI * self.fx
        } else {
            0.0
        }
    }

    pub fn full_longitude(self) -> bool {
        let period = self.longitude_period();
        if period <= 0.0 || self.width == 0 {
            return false;
        }
        let w = f64::from(self.width);
        (period - w).abs() <= 1e-5 * w
    }

    /// Strict in-frame normalize (no pole slack) — `camhost::normalize_pixel`.
    pub fn normalize_pixel(self, px: f64, py: f64) -> Result<(f64, f64), DomainError> {
        if !(px.is_finite() && py.is_finite()) {
            return Err(out_of_range("pixel", "finite", "nonfinite"));
        }
        let w = f64::from(self.width);
        let h = f64::from(self.height);
        let mut x = px;
        let y = py;
        if self.full_longitude() {
            x %= w;
            if x < 0.0 {
                x += w;
            }
        } else {
            let period = self.longitude_period();
            if period > 0.0 && (x < 0.0 || x > w) {
                x = self.cx + remainder(x - self.cx, period);
            }
        }
        if x < 0.0 || x > w || y < 0.0 || y > h {
            return Err(out_of_range("pixel", "inside frame", format!("{x},{y}")));
        }
        Ok((x, y))
    }

    /// Horizontal residual with panorama wrap (`pixel_difference`).
    pub fn pixel_difference(self, a: (f64, f64), b: (f64, f64)) -> (f64, f64) {
        let mut dx = a.0 - b.0;
        let dy = a.1 - b.1;
        let period = self.longitude_period();
        if period > 0.0 {
            dx = remainder(dx, period);
        }
        (dx, dy)
    }

    /// Forward ray → pixel. Source lens is truth when `source_model >= 0`.
    pub fn ray_pixel(self, ray: Vec3) -> Result<(f64, f64), DomainError> {
        let (px, py) = if self.source_model >= 0 {
            crate::source_camera::project(
                self.source_model,
                &self.source_params,
                ray.x(),
                ray.y(),
                ray.z(),
            )?
        } else {
            let (u, v) = project_ray(ray, self.model, self.tier, &self.coeffs)?;
            (self.fx * u + self.cx, self.fy * v + self.cy)
        };
        if px.is_finite() && py.is_finite() {
            Ok((px, py))
        } else {
            Err(out_of_range("ray_pixel", "finite", "nonfinite"))
        }
    }

    /// Visibility with equirect pole ±1 px slack (`ray_in_frame`).
    pub fn ray_in_frame(self, ray: Vec3) -> Result<(f64, f64), DomainError> {
        let (mut px, py) = self.ray_pixel(ray)?;
        let w = f64::from(self.width);
        let h = f64::from(self.height);
        if self.full_longitude() {
            px %= w;
            if px < 0.0 {
                px += w;
            }
        }
        let tol = if matches!(self.model, CameraModel::Equirectangular) {
            1.0
        } else {
            0.0
        };
        if px >= 0.0 && px <= w && py >= -tol && py <= h + tol {
            Ok((px, py))
        } else {
            Err(out_of_range(
                "ray_in_frame",
                "inside frame",
                format!("{px},{py}"),
            ))
        }
    }

    /// Numerical projection Jacobian 2×3 (`projection_jacobian`).
    pub fn projection_jacobian(self, ray: Vec3) -> Result<[[f64; 3]; 2], DomainError> {
        let length = (ray.x().hypot(ray.y())).hypot(ray.z());
        if !length.is_finite() || length <= 0.0 {
            return Err(out_of_range("ray", "nonzero finite", format!("{length}")));
        }
        let step = f64::EPSILON.cbrt() * length;
        let base = self.ray_pixel(ray)?;
        let mut jac = [[0.0; 3]; 2];
        for c in 0..3 {
            let mut plus = [ray.x(), ray.y(), ray.z()];
            let mut minus = plus;
            plus[c] += step;
            minus[c] -= step;
            let pa = self.ray_pixel(Vec3::try_new(plus[0], plus[1], plus[2])?);
            let pb = self.ray_pixel(Vec3::try_new(minus[0], minus[1], minus[2])?);
            let (delta, divisor) = match (pa, pb) {
                (Ok(a), Ok(b)) => (self.pixel_difference(a, b), 2.0 * step),
                (Ok(a), Err(_)) => (self.pixel_difference(a, base), step),
                (Err(_), Ok(b)) => (self.pixel_difference(base, b), step),
                (Err(_), Err(_)) => {
                    return Err(invalid("projection_jacobian", "both FD steps failed"));
                }
            };
            jac[0][c] = delta.0 / divisor;
            jac[1][c] = delta.1 / divisor;
            if !(jac[0][c].is_finite() && jac[1][c].is_finite()) {
                return Err(out_of_range("jacobian", "finite", "nonfinite"));
            }
        }
        Ok(jac)
    }

    /// Fitted inverse seed + source-forward refine (`pixel_ray`).
    pub fn pixel_ray(self, input_px: f64, input_py: f64) -> Result<Vec3, DomainError> {
        if !(self.fx.is_finite() && self.fx > 0.0 && self.fy.is_finite() && self.fy > 0.0) {
            return Err(out_of_range("focal", "positive", "invalid"));
        }
        let (px, py) = self.normalize_pixel(input_px, input_py)?;
        let mut ray = generate_ray(
            (px - self.cx) / self.fx,
            (py - self.cy) / self.fy,
            self.model,
            self.tier,
            &self.coeffs,
        )?;
        const CONVERGED: f64 = 1e-6;
        const ACCEPTABLE: f64 = 1e-3;
        for _ in 0..32 {
            let projected = self.ray_pixel(ray)?;
            let delta = self.pixel_difference(projected, (px, py));
            let residual = delta.0.hypot(delta.1);
            if residual < CONVERGED {
                break;
            }
            let Ok(jac) = self.projection_jacobian(ray) else {
                break;
            };
            let mut xx = 0.0;
            let mut xy = 0.0;
            let mut yy = 0.0;
            for (jx, jy) in jac[0].iter().zip(jac[1].iter()) {
                xx += jx * jx;
                xy += jx * jy;
                yy += jy * jy;
            }
            let det = xx * yy - xy * xy;
            if det <= 1e-12 * xx * yy {
                break;
            }
            let u = (yy * delta.0 - xy * delta.1) / det;
            let v = (xx * delta.1 - xy * delta.0) / det;
            let mut improved = false;
            for trial in 0..12 {
                let scale = (-(trial as f64)).exp2();
                let cand = Vec3::try_new(
                    ray.x() - scale * (jac[0][0] * u + jac[1][0] * v),
                    ray.y() - scale * (jac[0][1] * u + jac[1][1] * v),
                    ray.z() - scale * (jac[0][2] * u + jac[1][2] * v),
                );
                let Ok(cand) = cand else {
                    continue;
                };
                let mut l = (cand.x().hypot(cand.y())).hypot(cand.z());
                if l < 1e-12 {
                    l = 1.0;
                }
                let Ok(cand) = Vec3::try_new(cand.x() / l, cand.y() / l, cand.z() / l) else {
                    continue;
                };
                let Ok(next) = self.ray_pixel(cand) else {
                    continue;
                };
                let rem = self.pixel_difference(next, (px, py));
                if rem.0.hypot(rem.1) >= residual {
                    continue;
                }
                ray = cand;
                improved = true;
                break;
            }
            if !improved {
                break;
            }
        }
        let projected = self.ray_pixel(ray)?;
        let delta = self.pixel_difference(projected, (px, py));
        if delta.0.hypot(delta.1) >= ACCEPTABLE {
            return Err(invalid("pixel_ray", "residual above acceptance"));
        }
        let _ = self.ray_in_frame(ray)?;
        Ok(ray)
    }

    /// Seed-only ray (no source refine) — kept for simple callers.
    pub fn pixel_to_ray(self, px: f64, py: f64) -> Result<Vec3, DomainError> {
        let (x, y) = self.normalize_pixel(px, py)?;
        let u = (x - self.cx) / self.fx;
        let v = (y - self.cy) / self.fy;
        generate_ray(u, v, self.model, self.tier, &self.coeffs)
    }

    pub fn ray_to_pixel(self, ray: Vec3) -> Result<Vec2, DomainError> {
        let (u, v) = project_ray(ray, self.model, self.tier, &self.coeffs)?;
        Vec2::new_finite(self.fx * u + self.cx, self.fy * v + self.cy)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn undistort_lens_rejects_nonfinite_before_identity() {
        let zero = [0.0; DISTORTION_PARAMS];
        assert!(undistort_lens(f64::NAN, 0.0, DistortionTier::None, &zero).is_err());
        assert!(undistort_lens(0.0, f64::INFINITY, DistortionTier::None, &zero).is_err());
        assert!(undistort_lens(0.1, 0.2, DistortionTier::OpenCv, &zero).is_ok());
        let mut bad = zero;
        bad[0] = f64::NAN;
        assert!(undistort_lens(0.1, 0.2, DistortionTier::OpenCv, &bad).is_err());
        assert!(undistort_lens(0.1, 0.2, DistortionTier::None, &bad).is_err());
    }

    #[test]
    fn valid_distortion_rejects_nonfinite_before_none_identity() {
        let zero = [0.0; DISTORTION_PARAMS];
        assert!(valid_distortion(f64::NAN, 0.0, DistortionTier::None, &zero).is_err());
        assert!(valid_distortion(0.0, f64::INFINITY, DistortionTier::None, &zero).is_err());
        assert!(valid_distortion(0.1, 0.2, DistortionTier::None, &zero).unwrap());
        let mut bad = zero;
        bad[3] = f64::NEG_INFINITY;
        assert!(valid_distortion(0.1, 0.2, DistortionTier::None, &bad).is_err());
    }

    #[test]
    fn distort_point_equirect_rejects_nonfinite_before_passthrough() {
        let zero = [0.0; DISTORTION_PARAMS];
        assert!(
            distort_point(
                f64::NAN,
                0.0,
                CameraModel::Equirectangular,
                DistortionTier::None,
                &zero
            )
            .is_err()
        );
        assert!(
            distort_point(
                0.0,
                f64::INFINITY,
                CameraModel::Equirectangular,
                DistortionTier::None,
                &zero
            )
            .is_err()
        );
        assert_eq!(
            distort_point(
                0.5,
                -0.25,
                CameraModel::Equirectangular,
                DistortionTier::None,
                &zero
            )
            .unwrap(),
            (0.5, -0.25)
        );
        let mut bad = zero;
        bad[7] = f64::NAN;
        assert!(
            distort_point(
                0.5,
                -0.25,
                CameraModel::Equirectangular,
                DistortionTier::None,
                &bad
            )
            .is_err()
        );
    }
}
