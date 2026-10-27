//! Holdout check: Rust camera math vs C++-generated golden (not Rust self-minted).
//!
//! Golden: `fixtures/camera/cpp-golden/camera_math.json` from
//! `tools/camera-golden` against pin `9a24f099…`.

use std::fs;
use std::path::PathBuf;

use serde_json::Value;
use spirula_core::{
    CameraModel, DistortionTier, Vec3, distort_point, generate_ray, project_ray, undistort_point,
    valid_distortion,
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

#[test]
fn cpp_camera_math_golden_holdout() {
    let raw =
        fs::read_to_string(fixture()).expect("golden present — run tools/camera-golden/build.sh");
    let doc: Value = serde_json::from_str(&raw).expect("json");
    assert_eq!(
        doc["reference_commit"].as_str().unwrap(),
        "9a24f09927944a795e7ba1bd8243a625bf419479"
    );
    let cases = doc["cases"].as_array().expect("cases");
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
            // C++ stores coeffs as float; host math promotes to double. Allow
            // 1e-9 rel/abs (same order as P1 camera golden), not binary f64 bitmatch.
            assert!(
                close(du, g_dist[0].as_f64().unwrap(), 1e-9)
                    && close(dv, g_dist[1].as_f64().unwrap(), 1e-9),
                "{} distort rust=({}, {}) golden=({}, {})",
                case["name"],
                du,
                dv,
                g_dist[0],
                g_dist[1]
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
            other => panic!("unknown domain case {other}"),
        }
    }
}
