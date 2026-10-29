//! P3-F04: Nerfstudio / Metashape / PLY focused tests.

use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant};

use spirula_core::{CancelReason, CancelToken, ErrorCode};
use spirula_io::temp_harness::TempHarness;
use spirula_io::{
    ColmapLoadConfig, DatasetFormat, DatasetProbe, load_dataset, load_dataset_budgeted,
    probe_dataset, read_ply_points,
};

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
