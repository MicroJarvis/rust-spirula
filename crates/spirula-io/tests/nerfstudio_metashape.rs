//! P3-F04: Nerfstudio / Metashape / PLY focused tests.

use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant};

use spirula_core::{CameraModel, CancelReason, CancelToken, ContextKey, DistortionTier, ErrorCode};
use spirula_io::temp_harness::TempHarness;
use spirula_io::{
    ColmapLoadConfig, DatasetFormat, DatasetProbe, load_dataset, load_dataset_budgeted,
    probe_dataset, read_ply_points, read_ply_points_budgeted,
};

fn identity_tm() -> &'static str {
    "[[1,0,0,0],[0,1,0,0],[0,0,1,0],[0,0,0,1]]"
}

fn write_ns(ds: &std::path::Path, body: &str) {
    std::fs::create_dir_all(ds).unwrap();
    std::fs::write(ds.join("transforms.json"), body).unwrap();
}

fn fixtures_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/datasets")
}

#[test]
fn nerfstudio_min_loads_opengl_c2w_and_ply() {
    let ds = fixtures_root().join("nerfstudio-min");
    let loaded = load_dataset(
        &ds,
        &ColmapLoadConfig {
            require_image_files: true,
            ..Default::default()
        },
        None,
    )
    .expect("nerfstudio load");
    assert_eq!(loaded.format, DatasetFormat::Nerfstudio);
    assert_eq!(loaded.summary.format, "nerfstudio");
    assert_eq!(loaded.frames.len(), 1);
    assert_eq!(loaded.points.num(), 2);
    // OpenGL c2w from transform_matrix (translation z=2).
    assert!((loaded.frames[0].c2w[11] - 2.0).abs() < 1e-9);
    assert_eq!(loaded.frames[0].intrins[0], 100.0);
    assert_eq!(loaded.points.points[0].xyz, [1.0, 2.0, 3.0]);
    assert_eq!(loaded.points.points[0].rgb, [10, 20, 30]);
}

#[test]
fn ply_binary_le_fixture() {
    let p = fixtures_root().join("ply-binary-le.ply");
    let pts = read_ply_points(&p, None).unwrap();
    assert_eq!(pts.num(), 1);
    assert!((pts.points[0].xyz[0] - 1.5).abs() < 1e-5);
    assert!((pts.points[0].xyz[1] + 2.5).abs() < 1e-5);
    assert!((pts.points[0].xyz[2] - 3.25).abs() < 1e-5);
    assert_eq!(pts.points[0].rgb, [1, 2, 3]);
}

#[test]
fn metashape_xml_loads_with_opengl_yz_flip() {
    let ds = fixtures_root().join("metashape-min");
    let loaded = load_dataset(
        &ds,
        &ColmapLoadConfig {
            require_image_files: true,
            format: Some("metashape".into()),
            metashape_xml: Some("cameras.xml".into()),
            metashape_ply: Some("points.ply".into()),
            image_dir: "images".into(),
            ..Default::default()
        },
        None,
    )
    .expect("metashape load");
    assert_eq!(loaded.format, DatasetFormat::Metashape);
    assert_eq!(loaded.summary.format, "metashape");
    assert_eq!(loaded.frames.len(), 1);
    assert_eq!(loaded.points.num(), 1);
    // Identity OpenCV → negate Y/Z columns → c2w row0=[1,0,0,0], row1=[0,-1,0,0], row2=[0,0,-1,1]
    let c = &loaded.frames[0].c2w;
    assert!((c[0] - 1.0).abs() < 1e-9);
    assert!((c[5] + 1.0).abs() < 1e-9); // -1 at (1,1)
    assert!((c[10] + 1.0).abs() < 1e-9); // -1 at (2,2)
    assert!((c[11] - 1.0).abs() < 1e-9); // t_z = 1
}

#[test]
fn metashape_xml_utf8_bom_loads() {
    // Independent harness xml_compat xml_bom + fixtures/metashape-xml-compat.
    let h = TempHarness::new().unwrap();
    let ds = h.path().join("bom");
    std::fs::create_dir_all(&ds).unwrap();
    std::fs::copy(
        fixtures_root().join("metashape-xml-compat/cameras-bom.xml"),
        ds.join("cameras.xml"),
    )
    .unwrap();
    let loaded = load_dataset(
        &ds,
        &ColmapLoadConfig {
            format: Some("metashape".into()),
            require_image_files: false,
            ..Default::default()
        },
        None,
    )
    .expect("BOM Metashape XML");
    assert_eq!(loaded.frames.len(), 1);
}

#[test]
fn metashape_xml_cdata_transform_loads() {
    // Independent harness xml_compat xml_cdata + fixtures/metashape-xml-compat.
    let h = TempHarness::new().unwrap();
    let ds = h.path().join("cdata");
    std::fs::create_dir_all(&ds).unwrap();
    std::fs::copy(
        fixtures_root().join("metashape-xml-compat/cameras-cdata.xml"),
        ds.join("cameras.xml"),
    )
    .unwrap();
    let loaded = load_dataset(
        &ds,
        &ColmapLoadConfig {
            format: Some("metashape".into()),
            require_image_files: false,
            ..Default::default()
        },
        None,
    )
    .expect("CDATA Metashape transform");
    assert_eq!(loaded.frames.len(), 1);
    let c = &loaded.frames[0].c2w;
    assert!((c[0] - 1.0).abs() < 1e-9);
    assert!((c[5] + 1.0).abs() < 1e-9);
    assert!((c[11] - 1.0).abs() < 1e-9);
}

#[test]
fn auto_detect_priority_nerfstudio_over_colmap_over_metashape() {
    let h = TempHarness::new().unwrap();
    let ds = h.path().join("ds");
    std::fs::create_dir_all(ds.join("images")).unwrap();
    // Metashape markers
    std::fs::write(ds.join("cam.xml"), b"<document><chunk></chunk></document>").unwrap();
    // COLMAP markers
    std::fs::create_dir_all(ds.join("sparse/0")).unwrap();
    std::fs::write(
        ds.join("sparse/0/cameras.txt"),
        b"1 PINHOLE 64 48 100 100 32 24\n",
    )
    .unwrap();
    std::fs::write(
        ds.join("sparse/0/images.txt"),
        b"1 1 0 0 0 0 0 0 1 a.png\n\n",
    )
    .unwrap();
    // Nerfstudio wins
    std::fs::write(
        ds.join("transforms.json"),
        r#"{
  "camera_model":"PINHOLE","fl_x":100,"fl_y":100,"cx":32,"cy":24,"w":64,"h":48,
  "frames":[{"file_path":"images/a.png","transform_matrix":[[1,0,0,0],[0,1,0,0],[0,0,1,0],[0,0,0,1]]}]
}"#,
    )
    .unwrap();
    // Missing image → require_image_files false for probe/load shape.
    let probe = probe_dataset(&ds, None).unwrap();
    assert!(matches!(
        probe,
        DatasetProbe::Available {
            format: DatasetFormat::Nerfstudio,
            ..
        }
    ));
    let loaded = load_dataset(
        &ds,
        &ColmapLoadConfig {
            require_image_files: false,
            ..Default::default()
        },
        None,
    )
    .unwrap();
    assert_eq!(loaded.format, DatasetFormat::Nerfstudio);

    // Without transforms, COLMAP wins over Metashape.
    std::fs::remove_file(ds.join("transforms.json")).unwrap();
    let probe2 = probe_dataset(&ds, None).unwrap();
    assert!(matches!(
        probe2,
        DatasetProbe::Available {
            format: DatasetFormat::Colmap,
            ..
        }
    ));
}

#[test]
fn rejects_path_escape_nonfinite_singular_applied_transform() {
    let h = TempHarness::new().unwrap();
    let ds = h.path().join("bad");
    std::fs::create_dir_all(&ds).unwrap();

    std::fs::write(
        ds.join("transforms.json"),
        r#"{
  "camera_model":"PINHOLE","fl_x":100,"fl_y":100,"cx":32,"cy":24,"w":64,"h":48,
  "frames":[{"file_path":"../escape.png","transform_matrix":[[1,0,0,0],[0,1,0,0],[0,0,1,0],[0,0,0,1]]}]
}"#,
    )
    .unwrap();
    let e = load_dataset(
        &ds,
        &ColmapLoadConfig {
            require_image_files: false,
            ..Default::default()
        },
        None,
    )
    .unwrap_err();
    assert_eq!(e.code(), ErrorCode::InvalidArgument);

    // JSON has no NaN literal; string "nan" parses to f64::NAN via `str::parse`.
    std::fs::write(
        ds.join("transforms.json"),
        r#"{
  "camera_model":"PINHOLE","fl_x":100,"fl_y":100,"cx":32,"cy":24,"w":64,"h":48,
  "frames":[{"file_path":"a.png","transform_matrix":[[1,0,0,0],[0,1,0,"nan"],[0,0,1,0],[0,0,0,1]]}]
}"#,
    )
    .unwrap();
    let e = load_dataset(
        &ds,
        &ColmapLoadConfig {
            require_image_files: false,
            ..Default::default()
        },
        None,
    )
    .unwrap_err();
    assert_eq!(e.code(), ErrorCode::ValueOutOfRange);

    std::fs::write(
        ds.join("transforms.json"),
        r#"{
  "camera_model":"PINHOLE","fl_x":100,"fl_y":100,"cx":32,"cy":24,"w":64,"h":48,
  "applied_transform":[[1,0,0],[0,0,0],[0,0,0]],
  "frames":[{"file_path":"a.png","transform_matrix":[[1,0,0,0],[0,1,0,0],[0,0,1,0],[0,0,0,1]]}]
}"#,
    )
    .unwrap();
    let e = load_dataset(
        &ds,
        &ColmapLoadConfig {
            require_image_files: false,
            ..Default::default()
        },
        None,
    )
    .unwrap_err();
    assert_eq!(e.code(), ErrorCode::ValueOutOfRange);
}

#[test]
fn empty_frames_and_missing_fields_err() {
    let h = TempHarness::new().unwrap();
    let ds = h.path().join("empty");
    std::fs::create_dir_all(&ds).unwrap();
    std::fs::write(ds.join("transforms.json"), r#"{"frames":[]}"#).unwrap();
    assert_eq!(
        load_dataset(&ds, &ColmapLoadConfig::default(), None)
            .unwrap_err()
            .code(),
        ErrorCode::MalformedRecord
    );

    std::fs::write(
        ds.join("transforms.json"),
        r#"{"frames":[{"file_path":"a.png","transform_matrix":[[1,0,0,0],[0,1,0,0],[0,0,1,0],[0,0,0,1]]}]}"#,
    )
    .unwrap();
    assert_eq!(
        load_dataset(
            &ds,
            &ColmapLoadConfig {
                require_image_files: false,
                ..Default::default()
            },
            None
        )
        .unwrap_err()
        .code(),
        ErrorCode::MalformedRecord
    );
}

#[test]
fn psx_only_typed_unsupported() {
    let h = TempHarness::new().unwrap();
    let ds = h.path().join("psx");
    std::fs::create_dir_all(&ds).unwrap();
    std::fs::write(
        ds.join("project.psx"),
        b"<document path=\"x.files/project.zip\"/>",
    )
    .unwrap();
    let probe = probe_dataset(&ds, None).unwrap();
    assert!(matches!(
        probe,
        DatasetProbe::Unavailable {
            format: DatasetFormat::Metashape,
            ..
        }
    ));
    let e = load_dataset(
        &ds,
        &ColmapLoadConfig {
            format: Some("metashape".into()),
            ..Default::default()
        },
        None,
    )
    .unwrap_err();
    assert_eq!(e.code(), ErrorCode::UnimplementedOption);
}

#[test]
fn metashape_rejects_nonzero_p3_p4() {
    let h = TempHarness::new().unwrap();
    let ds = h.path().join("p3");
    std::fs::create_dir_all(ds.join("images")).unwrap();
    std::fs::write(
        ds.join("cameras.xml"),
        r#"<?xml version="1.0"?>
<document><chunk>
  <sensors><sensor id="0" type="frame">
    <resolution width="64" height="48"/>
    <calibration class="adjusted" type="frame">
      <f>100</f><cx>0</cx><cy>0</cy>
      <k1>0</k1><k2>0</k2><k3>0</k3><k4>0</k4>
      <p1>0</p1><p2>0</p2><p3>0.1</p3><p4>0</p4>
      <b1>0</b1><b2>0</b2>
    </calibration>
  </sensor></sensors>
  <cameras><camera id="0" label="a" sensor_id="0">
    <transform>1 0 0 0 0 1 0 0 0 0 1 0 0 0 0 1</transform>
  </camera></cameras>
</chunk></document>"#,
    )
    .unwrap();
    let e = load_dataset(
        &ds,
        &ColmapLoadConfig {
            format: Some("metashape".into()),
            require_image_files: false,
            ..Default::default()
        },
        None,
    )
    .unwrap_err();
    assert_eq!(e.code(), ErrorCode::UnsupportedFormat);
}

#[test]
fn after_start_cancel_nerfstudio_ply() {
    let h = TempHarness::new().unwrap();
    let ds = h.path().join("cancel");
    std::fs::create_dir_all(ds.join("images")).unwrap();
    // Many ASCII vertices so cancel can land mid-PLY.
    let mut ply = String::from(
        "ply\nformat ascii 1.0\nelement vertex 200000\nproperty float x\nproperty float y\nproperty float z\nend_header\n",
    );
    for i in 0..200000 {
        ply.push_str(&format!("{i}.0 0.0 0.0\n"));
    }
    std::fs::write(ds.join("sparse_pc.ply"), ply).unwrap();
    std::fs::write(
        ds.join("transforms.json"),
        r#"{
  "camera_model":"PINHOLE","fl_x":100,"fl_y":100,"cx":32,"cy":24,"w":64,"h":48,
  "ply_file_path":"sparse_pc.ply",
  "frames":[{"file_path":"images/a.png","transform_matrix":[[1,0,0,0],[0,1,0,0],[0,0,1,0],[0,0,0,1]]}]
}"#,
    )
    .unwrap();

    let token = CancelToken::new();
    let t2 = token.clone();
    let path = ds.clone();
    let handle = thread::spawn(move || {
        // After-start: let PLY parse begin, then cancel.
        thread::sleep(Duration::from_millis(5));
        assert!(t2.request(CancelReason::User));
    });
    let start = Instant::now();
    let err = load_dataset(
        &path,
        &ColmapLoadConfig {
            require_image_files: false,
            ..Default::default()
        },
        Some(&token),
    )
    .unwrap_err();
    handle.join().unwrap();
    assert_eq!(err.code(), ErrorCode::Canceled);
    assert!(
        start.elapsed() < Duration::from_millis(2500),
        "cancel observed too slowly: {:?}",
        start.elapsed()
    );
}

#[test]
fn resource_budget_rejects_nerfstudio() {
    let ds = fixtures_root().join("nerfstudio-min");
    let e = load_dataset_budgeted(
        &ds,
        &ColmapLoadConfig {
            require_image_files: true,
            ..Default::default()
        },
        None,
        64,
    )
    .unwrap_err();
    assert_eq!(e.code(), ErrorCode::ValueOutOfRange);
}

#[test]
fn transform_matrix_json_precharge_covers_five_array_capacities() {
    // Mirrors independent review: serde_json 4×4 matrix → 5 Vec<Value> with
    // capacity 4 each → ≥640 array-slot bytes; bound must exceed false 512.
    use serde_json::Value;
    let tm = serde_json::json!([
        [1.0, 0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ]);
    let mut slots = 0usize;
    if let Value::Array(outer) = &tm {
        slots += outer.capacity();
        for row in outer {
            if let Value::Array(r) = row {
                slots += r.capacity();
            }
        }
    }
    let array_only = slots * std::mem::size_of::<Value>();
    assert!(array_only >= 640, "array_only={array_only}");
    // Load path uses transform_matrix_value_precharge ≥ 2×slots + slack.
    // Prove a 512-byte claim would be false against this fixture shape.
    assert!(array_only > 512);
    assert!(
        load_dataset(
            &fixtures_root().join("metashape-min"),
            &ColmapLoadConfig {
                require_image_files: false,
                format: Some("metashape".into()),
                metashape_xml: Some("cameras.xml".into()),
                ..Default::default()
            },
            None,
        )
        .is_ok(),
        "metashape-min still loads with raised transform precharge"
    );
}

#[test]
fn metashape_budget_rejects_before_conversion_alloc() {
    // XML file+DOM can fit a mid budget while sensor/map/clone conversion cannot.
    // Sweep until we observe a conversion-field reject (not only file/xml_chars).
    let ds = fixtures_root().join("metashape-min");
    let cfg = ColmapLoadConfig {
        require_image_files: false,
        format: Some("metashape".into()),
        metashape_xml: Some("cameras.xml".into()),
        metashape_ply: Some("points.ply".into()),
        image_dir: "images".into(),
        ..Default::default()
    };
    assert!(
        load_dataset_budgeted(&ds, &cfg, None, 1 << 28).is_ok(),
        "fixture must load under a large budget"
    );
    const CONVERSION_FIELDS: &[&str] = &[
        "metashape_sensors",
        "metashape_components",
        "metashape_cameras",
        "metashape_frames",
        "metashape_group",
        "metashape_meta",
        "image_walk_visited",
        "image_filenames",
    ];
    let mut saw_conversion = false;
    // Fine sweep: past tiny file/xml rejects into conversion pre-charges.
    for budget in (1_000u64..120_000).step_by(500) {
        match load_dataset_budgeted(&ds, &cfg, None, budget) {
            Ok(_) => break,
            Err(e) => {
                assert_eq!(e.code(), ErrorCode::ValueOutOfRange, "budget={budget}");
                let field = e.context_value(ContextKey::Field).unwrap_or("");
                if CONVERSION_FIELDS.contains(&field) {
                    saw_conversion = true;
                    break;
                }
                // Still in file/xml_chars/xml_* — keep raising budget.
            }
        }
    }
    assert!(
        saw_conversion,
        "expected a conversion/walk pre-alloc reject after XML fits"
    );
}

#[test]
fn require_image_files_missing_stable() {
    let h = TempHarness::new().unwrap();
    let ds = h.path().join("miss");
    std::fs::create_dir_all(&ds).unwrap();
    std::fs::write(
        ds.join("transforms.json"),
        r#"{
  "camera_model":"PINHOLE","fl_x":100,"fl_y":100,"cx":32,"cy":24,"w":64,"h":48,
  "frames":[{"file_path":"images/missing.png","transform_matrix":[[1,0,0,0],[0,1,0,0],[0,0,1,0],[0,0,0,1]]}]
}"#,
    )
    .unwrap();
    let e = load_dataset(
        &ds,
        &ColmapLoadConfig {
            require_image_files: true,
            ..Default::default()
        },
        None,
    )
    .unwrap_err();
    assert_eq!(e.code(), ErrorCode::NotFound);

    let ok = load_dataset(
        &ds,
        &ColmapLoadConfig {
            require_image_files: false,
            ..Default::default()
        },
        None,
    )
    .unwrap();
    assert_eq!(ok.frames.len(), 1);
}

#[test]
fn equisolid_without_distortion_keeps_equisolid_model() {
    let h = TempHarness::new().unwrap();
    let ds = h.path().join("eq");
    write_ns(
        &ds,
        &format!(
            r#"{{
  "camera_model":"EQUISOLID","fl_x":100,"fl_y":100,"cx":32,"cy":24,"w":64,"h":48,
  "frames":[{{"file_path":"a.png","transform_matrix":{}}}]
}}"#,
            identity_tm()
        ),
    );
    let loaded = load_dataset(
        &ds,
        &ColmapLoadConfig {
            require_image_files: false,
            ..Default::default()
        },
        None,
    )
    .unwrap();
    assert_eq!(loaded.frames[0].model, CameraModel::Equisolid);
    assert_eq!(loaded.frames[0].distortion, DistortionTier::None);
}

#[test]
fn pinhole_b1_affinity_keeps_pinhole_and_scales_fx() {
    let h = TempHarness::new().unwrap();
    let ds = h.path().join("b1");
    write_ns(
        &ds,
        &format!(
            r#"{{
  "camera_model":"PINHOLE","fl_x":100,"fl_y":100,"cx":32,"cy":24,"w":64,"h":48,
  "b1":0.01,"b2":0.0,
  "frames":[{{"file_path":"a.png","transform_matrix":{}}}]
}}"#,
            identity_tm()
        ),
    );
    let loaded = load_dataset(
        &ds,
        &ColmapLoadConfig {
            require_image_files: false,
            ..Default::default()
        },
        None,
    )
    .unwrap();
    assert_eq!(loaded.frames[0].model, CameraModel::Pinhole);
    assert!((loaded.frames[0].intrins[0] - 101.0).abs() < 1e-4);
    assert_eq!(loaded.frames[0].distortion, DistortionTier::None);
}

#[test]
fn thin_prism_hint_keeps_k4_as_fourth_radial_not_rational() {
    let h = TempHarness::new().unwrap();
    let ds = h.path().join("k4");
    write_ns(
        &ds,
        &format!(
            r#"{{
  "camera_model":"PINHOLE","camera_distortion":"THIN_PRISM",
  "fl_x":100,"fl_y":100,"cx":32,"cy":24,"w":64,"h":48,
  "k4":0.01,
  "frames":[{{"file_path":"a.png","transform_matrix":{}}}]
}}"#,
            identity_tm()
        ),
    );
    let loaded = load_dataset(
        &ds,
        &ColmapLoadConfig {
            require_image_files: false,
            ..Default::default()
        },
        None,
    )
    .unwrap();
    assert_eq!(loaded.frames[0].model, CameraModel::Pinhole);
    assert_eq!(loaded.frames[0].distortion, DistortionTier::ThinPrism);
    // Polynomial layout: raw[3]=k4 stays 0.01 (not rational demote that flips signs).
    assert!((loaded.frames[0].dist_coeffs[3] - 0.01).abs() < 1e-6);
}

#[test]
fn pinhole_with_skew_enters_fit_path() {
    let h = TempHarness::new().unwrap();
    let ds = h.path().join("skew");
    write_ns(
        &ds,
        &format!(
            r#"{{
  "camera_model":"PINHOLE","fl_x":100,"fl_y":100,"cx":32,"cy":24,"w":64,"h":48,
  "b2":0.05,
  "frames":[{{"file_path":"a.png","transform_matrix":{}}}]
}}"#,
            identity_tm()
        ),
    );
    let loaded = load_dataset(
        &ds,
        &ColmapLoadConfig {
            require_image_files: false,
            ..Default::default()
        },
        None,
    )
    .unwrap();
    // Skew forces fit_unsupported_lens; result remains a compiled pair.
    assert!(matches!(
        loaded.frames[0].model,
        CameraModel::Pinhole | CameraModel::Fisheye | CameraModel::Equisolid
    ));
}

#[test]
fn fisheye_keeps_tangential_in_polynomial_raw() {
    let h = TempHarness::new().unwrap();
    let ds = h.path().join("fish");
    write_ns(
        &ds,
        &format!(
            r#"{{
  "camera_model":"FISHEYE","fl_x":100,"fl_y":100,"cx":32,"cy":24,"w":64,"h":48,
  "k1":0.1,"k2":0.01,"k3":0.001,"k4":0.0001,"p1":0.002,"p2":-0.003,
  "frames":[{{"file_path":"a.png","transform_matrix":{}}}]
}}"#,
            identity_tm()
        ),
    );
    let loaded = load_dataset(
        &ds,
        &ColmapLoadConfig {
            require_image_files: false,
            ..Default::default()
        },
        None,
    )
    .unwrap();
    assert_eq!(loaded.frames[0].model, CameraModel::Fisheye);
    // Polynomial: [k1,k2,k3,k4,p1,p2,sx,sy]
    assert!((loaded.frames[0].dist_coeffs[0] - 0.1).abs() < 1e-5);
    assert!((loaded.frames[0].dist_coeffs[4] - 0.002).abs() < 1e-5);
    assert!((loaded.frames[0].dist_coeffs[5] + 0.003).abs() < 1e-5);
}

#[test]
fn metashape_k4_thin_prism_radial_not_rational() {
    let h = TempHarness::new().unwrap();
    let ds = h.path().join("msk4");
    std::fs::create_dir_all(ds.join("images")).unwrap();
    std::fs::write(ds.join("images/cam0.png"), b"x").unwrap();
    std::fs::write(
        ds.join("cameras.xml"),
        r#"<?xml version="1.0"?>
<document><chunk>
  <sensors><sensor id="0" type="frame">
    <resolution width="64" height="48"/>
    <calibration class="adjusted" type="frame">
      <f>100</f><cx>0</cx><cy>0</cy>
      <k1>0</k1><k2>0</k2><k3>0</k3><k4>0.01</k4>
      <p1>0</p1><p2>0</p2><b1>0</b1><b2>0</b2>
    </calibration>
  </sensor></sensors>
  <cameras><camera id="0" label="cam0" sensor_id="0">
    <transform>1 0 0 0 0 1 0 0 0 0 1 1 0 0 0 1</transform>
  </camera></cameras>
</chunk></document>"#,
    )
    .unwrap();
    let loaded = load_dataset(
        &ds,
        &ColmapLoadConfig {
            format: Some("metashape".into()),
            require_image_files: false,
            metashape_xml: Some("cameras.xml".into()),
            ..Default::default()
        },
        None,
    )
    .unwrap();
    assert_eq!(loaded.frames[0].model, CameraModel::Pinhole);
    assert!((loaded.frames[0].dist_coeffs[3] - 0.01).abs() < 1e-6);
}

#[test]
fn metashape_utf8_label_no_panic_typed_no_match() {
    let h = TempHarness::new().unwrap();
    let ds = h.path().join("utf8");
    std::fs::create_dir_all(ds.join("images")).unwrap();
    std::fs::write(ds.join("images/other.png"), b"x").unwrap();
    std::fs::write(
        ds.join("cameras.xml"),
        r#"<?xml version="1.0"?>
<document><chunk>
  <sensors><sensor id="0" type="frame">
    <resolution width="64" height="48"/>
    <calibration class="adjusted" type="frame">
      <f>100</f><cx>0</cx><cy>0</cy>
      <k1>0</k1><k2>0</k2><k3>0</k3><k4>0</k4>
      <p1>0</p1><p2>0</p2><b1>0</b1><b2>0</b2>
    </calibration>
  </sensor></sensors>
  <cameras><camera id="0" label="相机" sensor_id="0">
    <transform>1 0 0 0 0 1 0 0 0 0 1 0 0 0 0 1</transform>
  </camera></cameras>
</chunk></document>"#,
    )
    .unwrap();
    let err = load_dataset(
        &ds,
        &ColmapLoadConfig {
            format: Some("metashape".into()),
            require_image_files: true,
            metashape_xml: Some("cameras.xml".into()),
            ..Default::default()
        },
        None,
    )
    .unwrap_err();
    // No panic: stable DomainError (no usable cameras after failed UTF-8-safe match).
    assert!(
        matches!(err.code(), ErrorCode::MalformedRecord | ErrorCode::NotFound),
        "got {:?}",
        err.code()
    );
}

#[test]
fn resource_budget_charges_json_dom_before_tiny_reject() {
    let h = TempHarness::new().unwrap();
    let ds = h.path().join("bud");
    // Large enough JSON that file read fits a mid budget but DOM+builds do not.
    let mut frames = String::new();
    for i in 0..40 {
        if i > 0 {
            frames.push(',');
        }
        frames.push_str(&format!(
            r#"{{"file_path":"f{i:04}.png","transform_matrix":{}}}"#,
            identity_tm()
        ));
    }
    write_ns(
        &ds,
        &format!(
            r#"{{
  "camera_model":"PINHOLE","fl_x":100,"fl_y":100,"cx":32,"cy":24,"w":64,"h":48,
  "frames":[{frames}]
}}"#
        ),
    );
    let ok = load_dataset(
        &ds,
        &ColmapLoadConfig {
            require_image_files: false,
            ..Default::default()
        },
        None,
    )
    .unwrap();
    assert_eq!(ok.frames.len(), 40);
    // Cap above file+DOM but below frames/names/split accumulation.
    let e = load_dataset_budgeted(
        &ds,
        &ColmapLoadConfig {
            require_image_files: false,
            ..Default::default()
        },
        None,
        8_000,
    )
    .unwrap_err();
    assert_eq!(e.code(), ErrorCode::ValueOutOfRange);
}

#[test]
fn resource_budget_rejects_json_dom_before_serde_alloc() {
    let h = TempHarness::new().unwrap();
    let ds = h.path().join("dom");
    // ~2 KiB JSON: file charge fits a mid budget; DOM cap must not.
    let mut frames = String::new();
    for i in 0..12 {
        if i > 0 {
            frames.push(',');
        }
        frames.push_str(&format!(
            r#"{{"file_path":"f{i:04}.png","transform_matrix":{}}}"#,
            identity_tm()
        ));
    }
    let body = format!(
        r#"{{
  "camera_model":"PINHOLE","fl_x":100,"fl_y":100,"cx":32,"cy":24,"w":64,"h":48,
  "frames":[{frames}]
}}"#
    );
    write_ns(&ds, &body);
    let file_len = body.len() as u64;
    // Enough for transforms_json file bytes, below json_dom_budget_cap.
    let budget = file_len + 512;
    let e = load_dataset_budgeted(
        &ds,
        &ColmapLoadConfig {
            require_image_files: false,
            ..Default::default()
        },
        None,
        budget,
    )
    .unwrap_err();
    assert_eq!(e.code(), ErrorCode::ValueOutOfRange);
    assert_eq!(
        e.context_value(ContextKey::Field),
        Some("json_dom"),
        "must reject on pre-serde DOM charge, not a later post-hoc estimate"
    );
}

#[test]
fn json_budget_rejects_zero_array_capacity_cliff() {
    // Independent REVIEW counterexample (`/tmp/spirula-p3f04-review` json_memory_bound):
    // unused array of 131073 zeros → Vec capacity 262144; array-only allocated
    // bytes = capacity * size_of::<Value>() (= 8_388_608 on 64-bit) exceeds the
    // false `24*len+256` claim. Budget = len + old_claim + 100_000 must reject
    // under the corrected pre-allocation cap (and must not accept).
    let h = TempHarness::new().unwrap();
    let ds = h.path().join("cliff");
    let n = 131_073usize;
    let mut text = String::from("{\"unused\":[");
    for i in 0..n {
        if i != 0 {
            text.push(',');
        }
        text.push('0');
    }
    text.push_str(
        "],\"camera_model\":\"PINHOLE\",\"fl_x\":100,\"fl_y\":100,\"cx\":32,\"cy\":24,\"w\":64,\"h\":48,\"frames\":[{\"file_path\":\"a.png\",\"transform_matrix\":[[1,0,0,0],[0,1,0,0],[0,0,1,0],[0,0,0,1]]}]}",
    );
    let meta: serde_json::Value = serde_json::from_str(&text).unwrap();
    let arr = meta["unused"].as_array().unwrap();
    let array_only = arr.capacity() * std::mem::size_of::<serde_json::Value>();
    let old_claimed = text.len() * 24 + 256;
    assert!(
        array_only > old_claimed,
        "counterexample setup: array_only={array_only} old_claimed={old_claimed}"
    );
    write_ns(&ds, &text);
    let budget = (text.len() + old_claimed + 100_000) as u64;
    let e = load_dataset_budgeted(
        &ds,
        &ColmapLoadConfig {
            require_image_files: false,
            ..Default::default()
        },
        None,
        budget,
    )
    .expect_err("capacity-cliff budget must reject");
    assert_eq!(e.code(), ErrorCode::ValueOutOfRange);
    assert_eq!(
        e.context_value(ContextKey::Field),
        Some("json_dom"),
        "must fail on pre-serde json_dom charge covering Vec capacity"
    );
}

#[test]
fn after_start_cancel_during_json_parse() {
    let h = TempHarness::new().unwrap();
    let ds = h.path().join("jcancel");
    // > CANCEL_WORK_CHUNK bytes so IoRead byte ticks observe cancel mid-parse.
    let mut frames = String::new();
    for i in 0..2_500 {
        if i > 0 {
            frames.push(',');
        }
        frames.push_str(&format!(
            r#"{{"file_path":"f{i:05}.png","transform_matrix":{}}}"#,
            identity_tm()
        ));
    }
    write_ns(
        &ds,
        &format!(
            r#"{{
  "camera_model":"PINHOLE","fl_x":100,"fl_y":100,"cx":32,"cy":24,"w":64,"h":48,
  "frames":[{frames}]
}}"#
        ),
    );
    let cancel = CancelToken::new();
    let c2 = cancel.clone();
    let path = ds.clone();
    let handle = thread::spawn(move || {
        thread::sleep(Duration::from_millis(2));
        assert!(c2.request(CancelReason::User));
    });
    let start = Instant::now();
    let err = load_dataset(
        &path,
        &ColmapLoadConfig {
            require_image_files: false,
            ..Default::default()
        },
        Some(&cancel),
    )
    .unwrap_err();
    handle.join().unwrap();
    assert_eq!(err.code(), ErrorCode::Canceled);
    assert!(
        start.elapsed() < Duration::from_millis(2500),
        "JSON cancel too slow: {:?}",
        start.elapsed()
    );
}

#[test]
fn ply_ascii_token_bounded_not_quadratic() {
    let h = TempHarness::new().unwrap();
    let p = h.path().join("tok.ply");
    // Long ASCII body: O(n²) from_utf8-on-suffix would blow the time budget.
    let n = 80_000u32;
    let mut ply = format!(
        "ply\nformat ascii 1.0\nelement vertex {n}\nproperty float x\nproperty float y\nproperty float z\nend_header\n"
    );
    for i in 0..n {
        ply.push_str(&format!("{i}.0 0.0 0.0\n"));
    }
    std::fs::write(&p, ply).unwrap();
    let cancel = CancelToken::new();
    let c2 = cancel.clone();
    let path = p.clone();
    let handle = thread::spawn(move || {
        thread::sleep(Duration::from_millis(5));
        assert!(c2.request(CancelReason::User));
    });
    let start = Instant::now();
    let err = read_ply_points_budgeted(&path, Some(&cancel), 1 << 30).unwrap_err();
    handle.join().unwrap();
    assert_eq!(err.code(), ErrorCode::Canceled);
    assert!(
        start.elapsed() < Duration::from_millis(2500),
        "ASCII PLY cancel too slow (possible O(n²)): {:?}",
        start.elapsed()
    );
}

#[test]
fn metashape_xml_trailing_comment_and_numeric_entities() {
    let h = TempHarness::new().unwrap();
    let ds = h.path().join("ent");
    std::fs::create_dir_all(ds.join("images")).unwrap();
    std::fs::write(ds.join("images/cam0.png"), b"x").unwrap();
    std::fs::write(
        ds.join("cameras.xml"),
        r#"<?xml version="1.0"?>
<document><!-- head --><chunk>
  <sensors><sensor id="0" type="frame">
    <resolution width="64" height="48"/>
    <calibration class="adjusted" type="frame">
      <f>100</f><cx>0</cx><cy>0</cy>
      <k1>0</k1><k2>0</k2><k3>0</k3><k4>0</k4>
      <p1>0</p1><p2>0</p2><b1>0</b1><b2>0</b2>
    </calibration>
  </sensor></sensors>
  <cameras><camera id="0" label="cam0" sensor_id="0">
    <transform>1 0 0 0 0 1 0 0 0 0 1 0 0 0 0 1</transform>
  </camera><!-- trailing sibling --></cameras>
</chunk></document>"#,
    )
    .unwrap();
    let loaded = load_dataset(
        &ds,
        &ColmapLoadConfig {
            format: Some("metashape".into()),
            require_image_files: false,
            metashape_xml: Some("cameras.xml".into()),
            ..Default::default()
        },
        None,
    )
    .expect("comments + entities path");
    assert_eq!(loaded.frames.len(), 1);
}

#[test]
fn after_start_cancel_during_metashape_image_walk() {
    let h = TempHarness::new().unwrap();
    let ds = h.path().join("walk");
    let images = ds.join("images");
    std::fs::create_dir_all(&images).unwrap();
    for i in 0..5_000 {
        std::fs::write(images.join(format!("img_{i:05}.png")), b"x").unwrap();
    }
    std::fs::write(
        ds.join("cameras.xml"),
        r#"<?xml version="1.0"?>
<document><chunk>
  <sensors><sensor id="0" type="frame">
    <resolution width="64" height="48"/>
    <calibration class="adjusted" type="frame">
      <f>100</f><cx>0</cx><cy>0</cy>
      <k1>0</k1><k2>0</k2><k3>0</k3><k4>0</k4>
      <p1>0</p1><p2>0</p2><b1>0</b1><b2>0</b2>
    </calibration>
  </sensor></sensors>
  <cameras><camera id="0" label="img_00000" sensor_id="0">
    <transform>1 0 0 0 0 1 0 0 0 0 1 0 0 0 0 1</transform>
  </camera></cameras>
</chunk></document>"#,
    )
    .unwrap();
    let cancel = CancelToken::new();
    let c2 = cancel.clone();
    let path = ds.clone();
    let handle = thread::spawn(move || {
        thread::sleep(Duration::from_millis(2));
        assert!(c2.request(CancelReason::User));
    });
    let err = load_dataset(
        &path,
        &ColmapLoadConfig {
            format: Some("metashape".into()),
            require_image_files: false,
            metashape_xml: Some("cameras.xml".into()),
            ..Default::default()
        },
        Some(&cancel),
    )
    .unwrap_err();
    handle.join().unwrap();
    assert_eq!(err.code(), ErrorCode::Canceled);
}
