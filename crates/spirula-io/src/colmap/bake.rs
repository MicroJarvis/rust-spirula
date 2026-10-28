//! COLMAP source params → engine model / distortion tier (+ P3-F01 fit).

use spirula_core::{
    CameraModel, DISTORTION_PARAMS, DistortionTier, FitOptions, K_EXACT_FIT_PX, MAX_PARAMS,
    demote_distortion, fit_camera_auto, source_project,
};

use super::budget::err;
use super::models::require_model_id_by_name;
use super::types::{BakedIntrinsics, ColmapCamera};
use spirula_core::ErrorCode;

fn need(cam: &ColmapCamera, n: usize) -> Result<(), spirula_core::DomainError> {
    if cam.params.len() < n {
        return Err(err(
            ErrorCode::MalformedRecord,
            "params",
            format!(
                "camera model {} needs {n} parameters, got {}",
                cam.model,
                cam.params.len()
            ),
        ));
    }
    Ok(())
}

fn p(cam: &ColmapCamera, i: usize) -> f32 {
    cam.params[i] as f32
}

/// `bake_colmap_intrins` (pin `ColmapParser.cpp:309-407`).
pub fn bake_colmap_intrinsics(
    cam: &ColmapCamera,
) -> Result<BakedIntrinsics, spirula_core::DomainError> {
    let mut o = BakedIntrinsics::default();
    let params = &cam.params;

    if cam.model == "SIMPLE_PINHOLE" {
        need(cam, 3)?;
        o.fx = p(cam, 0);
        o.fy = p(cam, 0);
        o.cx = p(cam, 1);
        o.cy = p(cam, 2);
    } else if cam.model == "PINHOLE" {
        need(cam, 4)?;
        o.fx = p(cam, 0);
        o.fy = p(cam, 1);
        o.cx = p(cam, 2);
        o.cy = p(cam, 3);
    } else if cam.model == "SIMPLE_FISHEYE" {
        need(cam, 3)?;
        o.fx = p(cam, 0);
        o.fy = p(cam, 0);
        o.cx = p(cam, 1);
        o.cy = p(cam, 2);
        o.model = CameraModel::Fisheye;
    } else if cam.model == "FISHEYE" {
        need(cam, 4)?;
        o.fx = p(cam, 0);
        o.fy = p(cam, 1);
        o.cx = p(cam, 2);
        o.cy = p(cam, 3);
        o.model = CameraModel::Fisheye;
    } else if cam.model == "SIMPLE_RADIAL"
        || cam.model == "RADIAL"
        || cam.model == "SIMPLE_RADIAL_FISHEYE"
        || cam.model == "RADIAL_FISHEYE"
    {
        let two = cam.model == "RADIAL" || cam.model == "RADIAL_FISHEYE";
        need(cam, if two { 5 } else { 4 })?;
        o.fx = p(cam, 0);
        o.fy = p(cam, 0);
        o.cx = p(cam, 1);
        o.cy = p(cam, 2);
        o.distortion = DistortionTier::OpenCv;
        o.dist[0] = p(cam, 3);
        if two {
            o.dist[1] = p(cam, 4);
        }
        if cam.model == "SIMPLE_RADIAL_FISHEYE" || cam.model == "RADIAL_FISHEYE" {
            o.model = CameraModel::Fisheye;
        }
    } else if cam.model == "OPENCV" {
        need(cam, 8)?;
        o.fx = p(cam, 0);
        o.fy = p(cam, 1);
        o.cx = p(cam, 2);
        o.cy = p(cam, 3);
        o.distortion = DistortionTier::OpenCv;
        o.dist[0] = p(cam, 4);
        o.dist[1] = p(cam, 5);
        o.dist[2] = p(cam, 6);
        o.dist[3] = p(cam, 7);
    } else if cam.model == "FULL_OPENCV"
        && params.len() >= 12
        && params[9] == 0.0
        && params[10] == 0.0
        && params[11] == 0.0
    {
        o.fx = p(cam, 0);
        o.fy = p(cam, 1);
        o.cx = p(cam, 2);
        o.cy = p(cam, 3);
        o.distortion = DistortionTier::ThinPrism;
        o.dist[0] = p(cam, 4);
        o.dist[1] = p(cam, 5);
        o.dist[2] = p(cam, 8);
        o.dist[4] = p(cam, 6);
        o.dist[5] = p(cam, 7);
    } else if cam.model == "OPENCV_FISHEYE" {
        need(cam, 8)?;
        o.fx = p(cam, 0);
        o.fy = p(cam, 1);
        o.cx = p(cam, 2);
        o.cy = p(cam, 3);
        o.model = CameraModel::Fisheye;
        o.distortion = DistortionTier::ThinPrism;
        o.dist[0] = p(cam, 4);
        o.dist[1] = p(cam, 5);
        o.dist[2] = p(cam, 6);
        o.dist[3] = p(cam, 7);
    } else if cam.model == "THIN_PRISM_FISHEYE" {
        need(cam, 12)?;
        o.fx = p(cam, 0);
        o.fy = p(cam, 1);
        o.cx = p(cam, 2);
        o.cy = p(cam, 3);
        o.model = CameraModel::Fisheye;
        o.distortion = DistortionTier::ThinPrism;
        o.dist[0] = p(cam, 4);
        o.dist[1] = p(cam, 5);
        o.dist[2] = p(cam, 8);
        o.dist[3] = p(cam, 9);
        o.dist[4] = p(cam, 6);
        o.dist[5] = p(cam, 7);
        o.dist[6] = p(cam, 10);
        o.dist[7] = p(cam, 11);
    } else if cam.model == "EQUIRECTANGULAR" {
        need(cam, 2)?;
        let w = params[0];
        let h = params[1];
        o.fx = (w / (2.0 * std::f64::consts::PI)) as f32;
        o.fy = (h / std::f64::consts::PI) as f32;
        o.cx = (w / 2.0) as f32;
        o.cy = (h / 2.0) as f32;
        o.model = CameraModel::Equirectangular;
    } else {
        let id = require_model_id_by_name(&cam.model)?;
        let info_n = super::models::model_by_id(id)
            .map(|m| m.num_params)
            .unwrap_or(0);
        need(cam, info_n)?;
        o.source.source_model = id;
        for (i, v) in params.iter().enumerate().take(MAX_PARAMS) {
            o.source.params[i] = *v as f32;
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
        o.dist[i] = *v as f32;
    }
    Ok(o)
}

/// Fit an unrepresentable COLMAP source onto a compiled tier (`fit_colmap_source`).
pub fn fit_colmap_source(
    cam: &ColmapCamera,
    bi: &mut BakedIntrinsics,
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
    let fit = fit_camera_auto(&project, w, h, FitOptions::default())?;
    bi.model = fit.target.model;
    bi.fx = fit.target.fx as f32;
    bi.fy = fit.target.fy as f32;
    bi.cx = fit.target.cx as f32;
    bi.cy = fit.target.cy as f32;
    for i in 0..DISTORTION_PARAMS {
        bi.dist[i] = fit.target.coeffs[i] as f32;
    }
    let mut coeffs = [0.0f64; DISTORTION_PARAMS];
    for (i, v) in bi.dist.iter().enumerate() {
        coeffs[i] = f64::from(*v);
    }
    let (tier, demoted) = demote_distortion(fit.target.distortion, &coeffs)?;
    bi.distortion = tier;
    for (i, v) in demoted.iter().enumerate() {
        bi.dist[i] = *v as f32;
    }
    bi.source.fit_max_px = fit.max_px as f32;
    if (!fit.ok && fit.samples == 0) || fit.max_px < K_EXACT_FIT_PX {
        bi.source.source_model = -1;
    }
    Ok(())
}

/// Bake + optional fit for one camera record.
pub fn bake_and_fit(cam: &ColmapCamera) -> Result<BakedIntrinsics, spirula_core::DomainError> {
    let mut bi = bake_colmap_intrinsics(cam)?;
    if bi.source.source_model >= 0 {
        fit_colmap_source(cam, &mut bi)?;
    }
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

    #[test]
    fn pinhole_bakes_exact() {
        let cam = ColmapCamera {
            camera_id: 1,
            model: "PINHOLE".into(),
            width: 64,
            height: 48,
            params: vec![100.0, 100.0, 32.0, 24.0],
        };
        let bi = bake_and_fit(&cam).unwrap();
        assert_eq!(bi.model, CameraModel::Pinhole);
        assert_eq!(bi.distortion, DistortionTier::None);
        assert!((bi.fx - 100.0).abs() < 1e-6);
        assert_eq!(bi.source.source_model, -1);
    }
}
