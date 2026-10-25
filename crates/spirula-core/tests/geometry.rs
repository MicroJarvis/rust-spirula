use std::path::PathBuf;

use spirula_core::{
    CameraPose, ErrorCode, GaussianSplat, JsonKind, JsonNumber, JsonValue, Mat3, OpenCvDistortion,
    PinholeCamera, Quat, SceneTransform, SplatSet, Vec3, parse_json,
};

const TOL: f64 = 1e-9;

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures")
}

fn read_json(path: &str) -> JsonValue {
    parse_json(&std::fs::read_to_string(fixtures().join(path)).unwrap()).unwrap()
}

fn field<'a>(value: &'a JsonValue, key: &str) -> &'a JsonValue {
    match &value.kind {
        JsonKind::Object(entries) => &entries.iter().find(|(name, _)| name == key).unwrap().1,
        other => panic!("expected object, got {other:?}"),
    }
}

fn number(value: &JsonValue) -> f64 {
    match &value.kind {
        JsonKind::Number(JsonNumber::Finite(value)) => *value,
        other => panic!("expected finite number, got {other:?}"),
    }
}

fn numbers(value: &JsonValue) -> Vec<f64> {
    match &value.kind {
        JsonKind::Array(items) => items.iter().map(number).collect(),
        other => panic!("expected array, got {other:?}"),
    }
}

fn vec3(values: &[f64]) -> Vec3 {
    Vec3::try_new(values[0], values[1], values[2]).unwrap()
}

fn close(actual: f64, expected: f64) {
    assert!(
        (actual - expected).abs() <= TOL,
        "{actual} differs from {expected}"
    );
}

fn close3(actual: Vec3, expected: &[f64]) {
    close(actual.x(), expected[0]);
    close(actual.y(), expected[1]);
    close(actual.z(), expected[2]);
}

fn close9(actual: [f64; 9], expected: &[f64]) {
    for (actual, expected) in actual.iter().zip(expected) {
        close(*actual, *expected);
    }
}

#[test]
fn inverse_composition_quaternion_and_row_column() {
    let scene = read_json("scene/scene_transform.json");
    let train = field(&scene, "train_from_world");
    let rotation = field(field(train, "rotation"), "matrix_3x3_flat_row_major");
    let column = field(field(train, "rotation"), "matrix_3x3_flat_column_major");
    let row = Mat3::try_from_row_major(numbers(rotation).try_into().unwrap()).unwrap();
    let from_column = Mat3::try_from_column_major(numbers(column).try_into().unwrap()).unwrap();
    close9(from_column.row_major(), &row.row_major());
    close9(row.column_major(), &numbers(column));

    let wxyz = numbers(field(field(train, "rotation"), "quaternion_wxyz"));
    let xyzw = numbers(field(field(train, "rotation"), "quaternion_xyzw"));
    let quat = Quat::try_from_wxyz(wxyz[0], wxyz[1], wxyz[2], wxyz[3]).unwrap();
    for (actual, expected) in quat.xyzw().iter().zip(&xyzw) {
        close(*actual, *expected);
    }
    for (actual, expected) in quat.wxyz().iter().zip(&wxyz) {
        close(*actual, *expected);
    }
    close9(quat.to_rotation().row_major(), &row.row_major());
    close9(
        Quat::try_from_wxyz(-wxyz[0], -wxyz[1], -wxyz[2], -wxyz[3])
            .unwrap()
            .to_rotation()
            .row_major(),
        &row.row_major(),
    );

    let turned = quat * quat;
    let inverse = row.try_inverse().unwrap();
    close9((inverse * row).row_major(), &Mat3::identity().row_major());
    close9(turned.to_rotation().row_major(), &(row * row).row_major());
    close3(
        quat.rotate(Vec3::try_new(1.0, 0.0, 0.0).unwrap()),
        &[0.0, 1.0, 0.0],
    );

    let singular = Mat3::try_from_row_major([0.0; 9]).unwrap();
    assert_eq!(
        singular.try_inverse().unwrap_err().code(),
        ErrorCode::ValueOutOfRange
    );
    assert_eq!(
        Mat3::try_from_row_major([1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, -1.0])
            .unwrap()
            .try_rotation()
            .unwrap_err()
            .code(),
        ErrorCode::InvalidArgument
    );
    assert!(Quat::try_from_wxyz(0.0, 0.0, 0.0, 0.0).is_err());
    assert!(Vec3::try_new(f64::NAN, 0.0, 0.0).is_err());
    assert!(Mat3::try_from_row_major([f64::INFINITY; 9]).is_err());
}

#[test]
fn golden_projection_pose_and_behind_camera() {
    let camera = read_json("camera/single_camera.json");
    let golden = read_json("golden/camera.json");
    let record = field(&camera, "camera");
    let intrinsics = numbers(field(record, "intrinsics"));
    let coefficients = numbers(field(field(record, "distortion"), "coefficients"));
    let c2w = field(&camera, "c2w");
    let rotation = Mat3::try_from_row_major(
        numbers(field(c2w, "rotation_row_major"))
            .try_into()
            .unwrap(),
    )
    .unwrap();
    let translation = vec3(&numbers(field(c2w, "translation")));
    let pinhole = PinholeCamera::try_new(
        64,
        48,
        intrinsics[0],
        intrinsics[1],
        intrinsics[2],
        intrinsics[3],
        OpenCvDistortion::try_new(
            coefficients[0],
            coefficients[1],
            coefficients[2],
            coefficients[3],
        )
        .unwrap(),
        CameraPose::try_new(rotation, translation).unwrap(),
    )
    .unwrap();

    let samples = match &field(&golden, "projection_samples").kind {
        JsonKind::Array(items) => items,
        other => panic!("expected samples, got {other:?}"),
    };
    for sample in samples {
        let world = vec3(&numbers(field(sample, "world")));
        let pixel = numbers(field(sample, "pixel"));
        let projected = pinhole.project_world(world).unwrap();
        close(projected.x(), pixel[0]);
        close(projected.y(), pixel[1]);
    }

    let moved =
        CameraPose::try_new(Mat3::identity(), Vec3::try_new(1.0, -2.0, 0.5).unwrap()).unwrap();
    let moved_camera = PinholeCamera::try_new(
        64,
        48,
        intrinsics[0],
        intrinsics[1],
        intrinsics[2],
        intrinsics[3],
        OpenCvDistortion::try_new(
            coefficients[0],
            coefficients[1],
            coefficients[2],
            coefficients[3],
        )
        .unwrap(),
        moved,
    )
    .unwrap();
    let origin_sample = vec3(&numbers(field(&samples[0], "world")));
    let shifted = origin_sample + moved.translation();
    let projected = moved_camera.project_world(shifted).unwrap();
    let expected = numbers(field(&samples[0], "pixel"));
    close(projected.x(), expected[0]);
    close(projected.y(), expected[1]);

    let yaw = Mat3::try_from_row_major([0.0, 0.0, 1.0, 0.0, 1.0, 0.0, -1.0, 0.0, 0.0]).unwrap();
    let turned = PinholeCamera::try_new(
        64,
        48,
        intrinsics[0],
        intrinsics[1],
        intrinsics[2],
        intrinsics[3],
        OpenCvDistortion::try_new(0.0, 0.0, 0.0, 0.0).unwrap(),
        CameraPose::try_new(yaw, Vec3::try_new(0.0, 0.0, 0.0).unwrap()).unwrap(),
    )
    .unwrap();
    let center = turned
        .project_world(Vec3::try_new(-2.0, 0.0, 0.0).unwrap())
        .unwrap();
    close(center.x(), intrinsics[2]);
    close(center.y(), intrinsics[3]);

    let behind = pinhole
        .project_world(Vec3::try_new(0.0, 0.0, 2.0).unwrap())
        .unwrap_err();
    assert_eq!(behind.code(), ErrorCode::ValueOutOfRange);
    assert_eq!(
        behind.context_value(spirula_core::ContextKey::Field),
        Some("camera_z")
    );
    assert_eq!(
        behind.context_value(spirula_core::ContextKey::Expected),
        Some("< 0")
    );

    assert_eq!(
        PinholeCamera::try_new(
            0,
            48,
            100.0,
            100.0,
            32.0,
            24.0,
            OpenCvDistortion::try_new(0.0, 0.0, 0.0, 0.0).unwrap(),
            CameraPose::identity(),
        )
        .unwrap_err()
        .code(),
        ErrorCode::ValueOutOfRange
    );
    assert!(
        PinholeCamera::try_new(
            64,
            48,
            0.0,
            100.0,
            32.0,
            24.0,
            OpenCvDistortion::try_new(0.0, 0.0, 0.0, 0.0).unwrap(),
            CameraPose::identity(),
        )
        .is_err()
    );
    assert!(OpenCvDistortion::try_new(f64::NAN, 0.0, 0.0, 0.0).is_err());
    assert!(
        CameraPose::try_new(
            Mat3::try_from_row_major([2.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0]).unwrap(),
            Vec3::try_new(0.0, 0.0, 0.0).unwrap(),
        )
        .is_err()
    );
}

#[test]
fn golden_scene_and_splat_invariants() {
    let scene_file = read_json("scene/scene_transform.json");
    let golden = read_json("golden/scene.json");
    let train = field(&scene_file, "train_from_world");
    let rotation = Mat3::try_from_row_major(
        numbers(field(field(train, "rotation"), "matrix_3x3_flat_row_major"))
            .try_into()
            .unwrap(),
    )
    .unwrap();
    let transform = SceneTransform::try_new(
        rotation,
        vec3(&numbers(field(train, "translation"))),
        number(field(train, "scale")),
    )
    .unwrap();
    let samples = match &field(&golden, "samples").kind {
        JsonKind::Array(items) => items,
        other => panic!("expected samples, got {other:?}"),
    };
    let inverse = transform.inverse().unwrap();
    for sample in samples {
        let world = vec3(&numbers(field(sample, "world")));
        let expected = numbers(field(sample, "train"));
        close3(transform.apply(world).unwrap(), &expected);
        close3(inverse.apply(vec3(&expected)).unwrap(), &world.to_array());
    }
    close3(
        transform.translation_before_rotation(),
        &numbers(field(train, "translation_before_rotation")),
    );
    let center = numbers(field(&golden, "center_world"));
    let before = transform.translation_before_rotation();
    close(before.x(), -center[0]);
    close(before.y(), -center[1]);
    close(before.z(), -center[2]);
    assert_eq!(
        SceneTransform::try_new(rotation, transform.translation(), 0.0)
            .unwrap_err()
            .code(),
        ErrorCode::ValueOutOfRange
    );
    assert!(SceneTransform::try_new(rotation, transform.translation(), -0.5).is_err());

    let splats = read_json("golden/splats.json");
    let first = field(field(&splats, "tiny_splat"), "first");
    let scale_log = numbers(field(first, "scale_log"));
    let rot = numbers(field(first, "rot_wxyz"));
    let color = [1.0, 0.0, 0.0];
    let c0 = spirula_core::SH_C0;
    let dc = Vec3::try_new(
        (color[0] - 0.5) / c0,
        (color[1] - 0.5) / c0,
        (color[2] - 0.5) / c0,
    )
    .unwrap();
    let logit = (0.8_f64 / 0.2).ln();
    let splat = GaussianSplat::try_new(
        vec3(&numbers(field(first, "xyz"))),
        Quat::try_from_wxyz(rot[0], rot[1], rot[2], rot[3]).unwrap(),
        vec3(&scale_log),
        logit,
        dc,
    )
    .unwrap();
    assert_eq!(splat.rotation().wxyz(), [rot[0], rot[1], rot[2], rot[3]]);
    close(splat.opacity(), number(field(first, "opacity")));
    close3(splat.scales(), &[0.1, 0.1, 0.1]);
    close3(splat.base_color(), &color);
    close3(splat.scale_log(), &scale_log);

    let bbox_min = numbers(field(field(&splats, "tiny_splat"), "bbox_min"));
    let bbox_max = numbers(field(field(&splats, "tiny_splat"), "bbox_max"));
    let set = SplatSet::try_from_splats(vec![
        GaussianSplat::try_new(
            vec3(&bbox_min),
            Quat::identity(),
            vec3(&scale_log),
            logit,
            dc,
        )
        .unwrap(),
        splat,
        GaussianSplat::try_new(
            vec3(&bbox_max),
            Quat::identity(),
            vec3(&scale_log),
            logit,
            dc,
        )
        .unwrap(),
    ])
    .unwrap();
    let (min, max) = set.bounds();
    close3(min, &bbox_min);
    close3(max, &bbox_max);
    assert_eq!(
        SplatSet::try_from_splats(Vec::new()).unwrap_err().code(),
        ErrorCode::InvalidArgument
    );
    assert!(
        GaussianSplat::try_new(
            splat.mean(),
            splat.rotation(),
            Vec3::try_new(1.0e9, 0.0, 0.0).unwrap(),
            logit,
            dc,
        )
        .is_err()
    );
    assert!(
        GaussianSplat::try_new(
            splat.mean(),
            splat.rotation(),
            splat.scale_log(),
            f64::NAN,
            dc
        )
        .is_err()
    );
}
