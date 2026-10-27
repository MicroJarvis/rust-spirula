//! Holdout: Rust camera math / source / fit vs C++-generated golden.
//!
//! Golden: `fixtures/camera/cpp-golden/camera_math.json` from
//! `tools/camera-golden` against pin `9a24f099…` (schema v2).

use std::fs;
use std::path::PathBuf;

use serde_json::Value;
use spirula_core::{
    COLMAP_EUCM, COLMAP_FOV, CameraModel, DistortionTier, DomainError, ErrorCode, FitOptions,
    FittedWithSource, FrameCamera, K_EXACT_FIT_PX, Vec3, distort_point, fit_camera_auto,
    generate_ray, project_ray, source_project, undistort_lens, undistort_point, valid_distortion,
};

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/camera/cpp-golden/camera_math.json")
}

fn model(v: i64) -> CameraModel {
    CameraModel::from_i32(v as i32).expect("model")
}
fn tier(v: i64) -> DistortionTier {
    DistortionTier::from_i32(v as i32).expect("tier")
}

fn coeffs(v: &Value) -> [f64; 8] {
    let mut out = [0.0; 8];
    for (i, c) in v.as_array().expect("coeffs").iter().enumerate() {
        out[i] = c.as_f64().expect("c");
    }
    out
}

fn close(a: f64, b: f64, tol: f64) -> bool {
    if a == b {
        return true;
    }
    (a - b).abs() <= tol * (1.0 + a.abs().max(b.abs()))
}

fn load() -> Value {
    let raw =
        fs::read_to_string(fixture()).expect("golden present — run tools/camera-golden/build.sh");
    serde_json::from_str(&raw).expect("json")
}

#[test]
fn cpp_camera_math_golden_holdout() {
    let doc = load();
    assert_eq!(doc["schema"], "spirula-camera-golden");
    assert_eq!(doc["version"], 2);
    assert_eq!(
        doc["reference_commit"].as_str().unwrap(),
        "9a24f09927944a795e7ba1bd8243a625bf419479"
    );
    assert!((doc["exact_fit_px"].as_f64().unwrap() - K_EXACT_FIT_PX).abs() < 1e-15);

    let cases = doc["math_cases"].as_array().expect("math_cases");
    assert_eq!(cases.len(), 10, "all model×tier compiled pairs");

    let mut checked = 0u32;
    for case in cases {
        let m = model(case["model"].as_i64().unwrap());
        let t = tier(case["tier"].as_i64().unwrap());
        let d = coeffs(&case["coeffs"]);
        for sample in case["samples"].as_array().unwrap() {
            let u = sample["u"].as_f64().unwrap();
            let v = sample["v"].as_f64().unwrap();
            let (du, dv) = distort_point(u, v, m, t, &d).expect("distort");
            let g_dist = sample["distort"].as_array().unwrap();
            assert!(
                close(du, g_dist[0].as_f64().unwrap(), 1e-9)
                    && close(dv, g_dist[1].as_f64().unwrap(), 1e-9),
                "{} distort",
                case["name"]
            );
            let valid = valid_distortion(u, v, t, &d).expect("valid");
            assert_eq!(valid, sample["valid"].as_bool().unwrap());

            let und_ok = sample["undistort_ok"].as_bool().unwrap();
            match undistort_point(du, dv, m, t, &d) {
                Ok((uu, vv)) => {
                    assert!(und_ok, "{} expected undistort fail", case["name"]);
                    let g = sample["undistort"].as_array().unwrap();
                    assert!(
                        close(uu, g[0].as_f64().unwrap(), 1e-9)
                            && close(vv, g[1].as_f64().unwrap(), 1e-9),
                        "{} undistort",
                        case["name"]
                    );
                }
                Err(_) => assert!(!und_ok, "{} expected undistort ok", case["name"]),
            }

            let ray_ok = sample["ray_ok"].as_bool().unwrap();
            match generate_ray(du, dv, m, t, &d) {
                Ok(ray) => {
                    assert!(ray_ok, "{} expected ray fail", case["name"]);
                    let g = sample["ray"].as_array().unwrap();
                    assert!(
                        close(ray.x(), g[0].as_f64().unwrap(), 1e-12)
                            && close(ray.y(), g[1].as_f64().unwrap(), 1e-12)
                            && close(ray.z(), g[2].as_f64().unwrap(), 1e-12),
                        "{} ray",
                        case["name"]
                    );
                    let proj_ok = sample["project_ok"].as_bool().unwrap();
                    match project_ray(ray, m, t, &d) {
                        Ok((pu, pv)) => {
                            assert!(proj_ok, "{} expected project fail", case["name"]);
                            let g = sample["project"].as_array().unwrap();
                            assert!(
                                close(pu, g[0].as_f64().unwrap(), 1e-9)
                                    && close(pv, g[1].as_f64().unwrap(), 1e-9),
                                "{} project",
                                case["name"]
                            );
                        }
                        Err(_) => assert!(!proj_ok, "{} expected project ok", case["name"]),
                    }
                }
                Err(_) => assert!(!ray_ok, "{} expected ray ok", case["name"]),
            }
            checked += 1;
        }
    }
    assert!(checked >= 80);

    for dom in doc["domain"].as_array().unwrap() {
        match dom["name"].as_str().unwrap() {
            "pinhole_behind" => {
                let ray = Vec3::try_new(0.0, 0.0, -1.0).unwrap();
                let z = [0.0; 8];
                assert!(project_ray(ray, CameraModel::Pinhole, DistortionTier::None, &z).is_err());
                assert!(!dom["project_ok"].as_bool().unwrap());
            }
            "pinhole_side_90" => {
                let ray = Vec3::try_new(1.0, 0.0, 0.0).unwrap();
                let z = [0.0; 8];
                assert!(project_ray(ray, CameraModel::Pinhole, DistortionTier::None, &z).is_err());
                assert!(!dom["project_ok"].as_bool().unwrap());
            }
            "fisheye_r_ge_pi" => {
                let z = [0.0; 8];
                assert!(
                    generate_ray(3.2, 0.0, CameraModel::Fisheye, DistortionTier::None, &z).is_err()
                );
                assert!(!dom["ray_ok"].as_bool().unwrap());
            }
            "equisolid_backaxis" => {
                let ray = Vec3::try_new(0.0, 0.0, -1.0).unwrap();
                let z = [0.0; 8];
                assert!(
                    project_ray(ray, CameraModel::Equisolid, DistortionTier::None, &z).is_err()
                );
                assert!(!dom["project_ok"].as_bool().unwrap());
            }
            "fisheye_past90" | "equisolid_past90" | "pano_seam" | "pano_pole" => {
                let m = model(dom["model"].as_i64().unwrap());
                let u = dom["u"].as_f64().unwrap();
                let v = dom["v"].as_f64().unwrap();
                let z = [0.0; 8];
                let ray = generate_ray(u, v, m, DistortionTier::None, &z).expect("ray");
                assert!(dom["ray_ok"].as_bool().unwrap());
                let g = dom["ray"].as_array().unwrap();
                assert!(
                    close(ray.x(), g[0].as_f64().unwrap(), 1e-12)
                        && close(ray.y(), g[1].as_f64().unwrap(), 1e-12)
                        && close(ray.z(), g[2].as_f64().unwrap(), 1e-12)
                );
                // Wide fisheye/equisolid rays may have negative Z; atan2 project must succeed.
                if matches!(m, CameraModel::Fisheye | CameraModel::Equisolid) {
                    assert!(ray.z() < 0.0, "{} expects negative Z", dom["name"]);
                }
                let (pu, pv) = project_ray(ray, m, DistortionTier::None, &z).expect("project");
                assert!(dom["project_ok"].as_bool().unwrap());
                let gp = dom["project"].as_array().unwrap();
                assert!(
                    close(pu, gp[0].as_f64().unwrap(), 1e-9)
                        && close(pv, gp[1].as_f64().unwrap(), 1e-9)
                );
            }
            "fisheye_pi" | "equisolid_r2" => {
                let m = model(dom["model"].as_i64().unwrap());
                let u = dom["u"].as_f64().unwrap();
                let v = dom["v"].as_f64().unwrap();
                let z = [0.0; 8];
                assert!(generate_ray(u, v, m, DistortionTier::None, &z).is_err());
                assert!(!dom["ray_ok"].as_bool().unwrap());
            }
            other => panic!("unknown domain case {other}"),
        }
    }
}

#[test]
fn newton_residual_differs_from_valid_jacobian_bounds() {
    let doc = load();
    let fold = [-1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
    for row in doc["newton_vs_valid"].as_array().unwrap() {
        let u = row["u"].as_f64().unwrap();
        let valid = valid_distortion(u, 0.0, DistortionTier::OpenCv, &fold).unwrap();
        assert_eq!(valid, row["valid"].as_bool().unwrap(), "u={u} valid");
        let inv = undistort_lens(u, 0.0, DistortionTier::OpenCv, &fold);
        assert_eq!(
            inv.is_ok(),
            row["inverse_ok"].as_bool().unwrap(),
            "u={u} inv"
        );
        // At u=0.4: validJ passes (0.25..4) but Newton residual fails — distinct gates.
        if (u - 0.4).abs() < 1e-12 {
            assert!(valid);
            assert!(inv.is_err());
        }
    }
}

#[test]
fn panorama_seam_and_normalize_no_pole_slack() {
    let doc = load();
    let p = &doc["panorama"];
    assert!(p["full_longitude"].as_bool().unwrap());
    assert!(p["normalize_wrap"].as_bool().unwrap());
    assert!(!p["normalize_pole_slack"].as_bool().unwrap());
    let delta = p["delta_799_1"].as_array().unwrap();
    assert!((delta[0].as_f64().unwrap() + 2.0).abs() < 1e-12);
    assert!((delta[1].as_f64().unwrap()).abs() < 1e-12);

    let cam = FrameCamera::try_new(
        CameraModel::Equirectangular,
        DistortionTier::None,
        800,
        400,
        800.0 / (2.0 * std::f64::consts::PI),
        400.0 / std::f64::consts::PI,
        400.0,
        200.0,
        [0.0; 8],
    )
    .unwrap();
    assert!(cam.full_longitude());
    let (x, y) = cam.normalize_pixel(-1.0, 200.0).unwrap();
    assert!((x - 799.0).abs() < 1e-9 && (y - 200.0).abs() < 1e-12);
    let d = cam.pixel_difference((799.0, 200.0), (1.0, 200.0));
    assert!((d.0 + 2.0).abs() < 1e-12);
    // normalize_pixel has no pole slack (unlike ray_in_frame).
    assert!(cam.normalize_pixel(400.0, -0.5).is_err());
}

#[test]
fn source_and_fit_holdout_preserve_original_metadata() {
    let doc = load();
    let src = &doc["source"].as_array().unwrap()[0];
    assert_eq!(src["model"].as_i64().unwrap(), COLMAP_FOV as i64);
    assert!(src["ok"].as_bool().unwrap());
    assert!(src["params_unchanged"].as_bool().unwrap());
    let mut params = [0.0f32; 16];
    params[0] = 1000.0;
    params[1] = 900.0;
    params[2] = 800.0;
    params[3] = 600.0;
    params[4] = 0.9;
    let (px, py) = source_project(COLMAP_FOV, &params, 0.3, -0.2, 1.0).unwrap();
    let g = src["pixel"].as_array().unwrap();
    assert!(close(px, g[0].as_f64().unwrap(), 1e-9) && close(py, g[1].as_f64().unwrap(), 1e-9));

    let eucm_behind = doc["source"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["name"] == "eucm_behind")
        .unwrap();
    assert!(!eucm_behind["ok"].as_bool().unwrap());
    let mut eucm = [0.0f32; 16];
    eucm[0] = 600.0;
    eucm[1] = 600.0;
    eucm[2] = 800.0;
    eucm[3] = 600.0;
    eucm[4] = 0.6;
    eucm[5] = 1.1;
    assert!(source_project(COLMAP_EUCM, &eucm, 0.0, 0.0, -1.0).is_err());

    let fit_g = doc["fit"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["name"] == "fov_auto_grid16")
        .unwrap();
    let before = params;
    let src_fn = |x, y, z| source_project(COLMAP_FOV, &params, x, y, z);
    let r = fit_camera_auto(
        &src_fn,
        1600,
        1200,
        FitOptions {
            grid: 16,
            ..FitOptions::default()
        },
    )
    .unwrap();
    assert_eq!(params, before);
    assert_eq!(
        r.target.model.as_i32(),
        fit_g["model"].as_i64().unwrap() as i32
    );
    assert_eq!(r.samples, fit_g["samples"].as_u64().unwrap() as u32);
    assert!(close(r.max_px, fit_g["max_px"].as_f64().unwrap(), 1e-3));
    assert!(close(r.rms_px, fit_g["rms_px"].as_f64().unwrap(), 1e-3));
    assert!(r.max_px < K_EXACT_FIT_PX);
    let wrapped = FittedWithSource::from_fit(r, COLMAP_FOV, params).unwrap();
    assert_eq!(
        wrapped.source_model,
        fit_g["active_source_model"].as_i64().unwrap() as i32
    );
    assert_eq!(
        wrapped.original_source_model,
        fit_g["original_source_model"].as_i64().unwrap() as i32
    );
    assert_eq!(wrapped.source_model, -1);
    assert_eq!(wrapped.original_source_model, COLMAP_FOV);

    let empty_g = doc["fit"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["name"] == "empty_domain")
        .unwrap();
    let empty = |_x: f64, _y: f64, _z: f64| -> Result<(f64, f64), DomainError> {
        Err(DomainError::try_new(ErrorCode::ValueOutOfRange, "empty source").unwrap())
    };
    let auto = fit_camera_auto(
        &empty,
        1600,
        1200,
        FitOptions {
            grid: 16,
            ..FitOptions::default()
        },
    )
    .unwrap();
    assert!(!auto.ok);
    assert_eq!(auto.samples, empty_g["samples"].as_u64().unwrap() as u32);
    assert!((auto.target.fx - empty_g["fx"].as_f64().unwrap()).abs() < 1e-6);
}

#[test]
fn pixel_ray_uses_source_as_forward_truth() {
    // Fitted pinhole seed is deliberately inaccurate; FOV source is forward truth.
    let mut params = [0.0f32; 16];
    params[0] = 1000.0;
    params[1] = 900.0;
    params[2] = 800.0;
    params[3] = 600.0;
    params[4] = 0.9;
    let cam = FrameCamera::try_new_with_source(
        CameraModel::Pinhole,
        DistortionTier::None,
        1600,
        1200,
        900.0, // inaccurate seed
        900.0,
        800.0,
        600.0,
        [0.0; 8],
        COLMAP_FOV,
        params,
    )
    .unwrap();
    let ray = Vec3::try_new(0.3, -0.2, 1.0).unwrap();
    let (px, py) = cam.ray_pixel(ray).unwrap();
    let recovered = cam.pixel_ray(px, py).unwrap();
    let (bx, by) = cam.ray_pixel(recovered).unwrap();
    let d = cam.pixel_difference((bx, by), (px, py));
    assert!(
        d.0.hypot(d.1) < 1e-3,
        "reproject residual {}",
        d.0.hypot(d.1)
    );
    let _ = cam.projection_jacobian(ray).expect("jacobian");
}
