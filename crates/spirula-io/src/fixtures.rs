//! Phase 0 compatibility fixture checks.
//!
//! The fixture set lives in `fixtures/` at the workspace root. `manifest.json`
//! pins every payload and golden file by size and SHA-256; the checks below
//! parse enough of each format to prove that the fixture is self-consistent and
//! that the golden values match the file contents.
//!
//! These are **fixture checks**, not the production readers. P3 replaces the
//! minimal parsers with the real dataset/camera/checkpoint readers and keeps
//! these fixtures as their acceptance inputs.

use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fmt;
use std::fs;
use std::path::{Component, Path, PathBuf};

/// One failed fixture check, with enough context to find the file.
#[derive(Debug, Clone)]
pub struct FixtureError {
    message: String,
}

impl FixtureError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for FixtureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for FixtureError {}

type Result<T> = std::result::Result<T, FixtureError>;

/// Summary of a successful fixture check.
#[derive(Debug, Clone)]
pub struct FixtureReport {
    /// Root directory that was checked.
    pub root: PathBuf,
    /// Number of manifest entries whose size and checksum matched.
    pub checked_files: usize,
    /// Format checks that ran, in order.
    pub checks: Vec<String>,
}

/// Fixture root used by the workspace tests.
///
/// `SPIRULA_FIXTURES` overrides the default; otherwise this is
/// `<workspace>/fixtures`, resolved from this crate's manifest directory.
pub fn default_fixture_root() -> PathBuf {
    if let Some(path) = std::env::var_os("SPIRULA_FIXTURES") {
        return PathBuf::from(path);
    }
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures")
}

const REQUIRED_FILES: &[&str] = &[
    "camera/single_camera.json",
    "config/catalog.json",
    "config/defaults.json",
    "datasets/tiny-colmap/cameras.txt",
    "datasets/tiny-colmap/images.txt",
    "datasets/tiny-colmap/points3D.txt",
    "datasets/tiny-colmap/images/tiny.png",
    "scene/scene_transform.json",
    "splats/tiny_splat.ply",
    "splats/seed_points.ply",
    "splats/render_splat.ply",
    "checkpoints/tiny-run/config.json",
    "checkpoints/tiny-run/step-000000001.ckpt/state.tar",
    "checkpoints/tiny-run/step-000000001.ckpt/splat.ply",
    "render/tiny_render.json",
    "render/golden.ppm",
    "render/golden.png",
    "preview/request.json",
    "preview/scene.ply",
    "golden/camera.json",
    "golden/dataset.json",
    "golden/scene.json",
    "golden/splats.json",
    "golden/checkpoint.json",
    "golden/render.json",
];

/// Verify the manifest, every checksum, and every format/golden pairing.
pub fn check_fixtures(root: &Path) -> Result<FixtureReport> {
    let manifest_path = root.join("manifest.json");
    let manifest = read_json(&manifest_path)?;
    expect_str(&manifest, "schema", "spirula-fixtures", "manifest")?;
    expect_u64(&manifest, "version", 1, "manifest")?;

    let entries = as_array(get(&manifest, "files", "manifest")?, "manifest.files")?;
    let mut checked = BTreeSet::new();
    for (index, entry) in entries.iter().enumerate() {
        let ctx = format!("manifest.files[{index}]");
        let rel = as_str(get(entry, "path", &ctx)?, &format!("{ctx}.path"))?;
        validate_relative_path(rel, &ctx)?;
        let expected_bytes = as_u64(get(entry, "bytes", &ctx)?, &format!("{ctx}.bytes"))?;
        let expected_hash = as_str(get(entry, "sha256", &ctx)?, &format!("{ctx}.sha256"))?;

        if !checked.insert(rel.to_owned()) {
            return fail(format!("{ctx}: duplicate path `{rel}`"));
        }

        let path = root.join(rel);
        let data = fs::read(&path)
            .map_err(|e| FixtureError::new(format!("reading {}: {e}", path.display())))?;
        if data.len() as u64 != expected_bytes {
            return fail(format!(
                "{}: size is {} bytes, manifest says {expected_bytes}",
                path.display(),
                data.len()
            ));
        }
        let actual_hash = sha256_hex(&data);
        if actual_hash != expected_hash {
            return fail(format!(
                "{}: checksum mismatch (manifest {expected_hash}, file {actual_hash})",
                path.display()
            ));
        }
    }

    for required in REQUIRED_FILES {
        if !checked.contains(*required) {
            return fail(format!(
                "manifest does not list required fixture `{required}`"
            ));
        }
    }

    let checks = vec![
        check_config(root)?,
        check_camera(root)?,
        check_dataset(root)?,
        check_scene(root)?,
        check_splats(root)?,
        check_checkpoint(root)?,
        check_render(root)?,
        check_preview(root)?,
    ];

    Ok(FixtureReport {
        root: root.to_path_buf(),
        checked_files: checked.len(),
        checks,
    })
}

// ---------------------------------------------------------------------------
// Camera
// ---------------------------------------------------------------------------

fn check_config(root: &Path) -> Result<String> {
    let catalog = read_json(&root.join("config/catalog.json"))?;
    expect_str(
        &catalog,
        "schema",
        "spirula-train-config-catalog",
        "config catalog",
    )?;
    expect_u64(&catalog, "version", 1, "config catalog")?;
    expect_u64(&catalog, "field_count", 251, "config catalog")?;
    let fields = as_array(
        get(&catalog, "fields", "config catalog")?,
        "config catalog.fields",
    )?;
    if fields.len() != 251 {
        return fail(format!(
            "config catalog lists {} fields, expected 251",
            fields.len()
        ));
    }
    let mut names = BTreeSet::new();
    for (index, field) in fields.iter().enumerate() {
        let context = format!("config catalog.fields[{index}]");
        let name = as_str(get(field, "name", &context)?, &format!("{context}.name"))?;
        if !names.insert(name.to_owned()) {
            return fail(format!("{context}: duplicate field `{name}`"));
        }
        for key in [
            "type",
            "section",
            "tier",
            "choices",
            "choice_kind",
            "default_json",
        ] {
            as_str(get(field, key, &context)?, &format!("{context}.{key}"))?;
        }
        get(field, "empty_is_unset", &context)?;
    }
    for required in [
        "data",
        "num_iterations",
        "scene_center",
        "outlier_threshold",
        "max_world_size",
    ] {
        if !names.contains(required) {
            return fail(format!("config catalog is missing `{required}`"));
        }
    }

    let defaults = read_text(&root.join("config/defaults.json"))?;
    if !defaults.contains("\"schema_version\":1") || !defaults.contains("Infinity") {
        return fail(
            "config defaults.json must be a current document with non-finite defaults".to_owned(),
        );
    }
    if defaults.matches("\"schema_version\"").count() != 1 {
        return fail("config defaults.json must contain schema_version once".to_owned());
    }

    Ok("config: 251-field catalog and default document".to_owned())
}

fn check_camera(root: &Path) -> Result<String> {
    let camera = read_json(&root.join("camera/single_camera.json"))?;
    let golden = read_json(&root.join("golden/camera.json"))?;
    let record = get(&camera, "camera", "camera fixture")?;

    expect_str(record, "model", "PINHOLE", "camera fixture")?;
    expect_u64(record, "camera_id", 1, "camera fixture")?;
    same_u64(
        as_u64(get(record, "width", "camera")?, "camera.width")?,
        as_u64(get(&golden, "width", "camera golden")?, "golden.width")?,
        "camera.width",
    )?;
    same_u64(
        as_u64(get(record, "height", "camera")?, "camera.height")?,
        as_u64(get(&golden, "height", "camera golden")?, "golden.height")?,
        "camera.height",
    )?;

    let intrinsics = floats(get(record, "intrinsics", "camera")?, "camera.intrinsics")?;
    close_slice(
        &intrinsics,
        &floats(
            get(&golden, "intrinsics", "camera golden")?,
            "golden.intrinsics",
        )?,
        1e-12,
        "camera.intrinsics",
    )?;

    let distortion = get(record, "distortion", "camera")?;
    let golden_distortion = get(&golden, "distortion", "camera golden")?;
    if as_str(
        get(distortion, "tier", "camera.distortion")?,
        "camera.distortion.tier",
    )? != as_str(
        get(golden_distortion, "tier", "golden.distortion")?,
        "golden.distortion.tier",
    )? {
        return fail("camera distortion tier does not match golden".to_owned());
    }
    close_slice(
        &floats(
            get(distortion, "coefficients", "camera.distortion")?,
            "camera.distortion.coefficients",
        )?,
        &floats(
            get(golden_distortion, "coefficients", "golden.distortion")?,
            "golden.distortion.coefficients",
        )?,
        1e-12,
        "camera.distortion.coefficients",
    )?;

    let samples = as_array(
        get(&camera, "projection_samples", "camera fixture")?,
        "camera.projection_samples",
    )?;
    let golden_samples = as_array(
        get(&golden, "projection_samples", "camera golden")?,
        "golden.projection_samples",
    )?;
    if samples.len() != golden_samples.len() {
        return fail("camera projection sample count does not match golden".to_owned());
    }
    for (index, (sample, expected)) in samples.iter().zip(golden_samples).enumerate() {
        let world = vec3(
            get(sample, "world", "camera sample")?,
            "camera sample world",
        )?;
        let expected_px = vec2(
            get(expected, "pixel", "camera golden sample")?,
            "camera golden pixel",
        )?;
        let actual = project_pinhole(&camera, world)
            .map_err(|e| FixtureError::new(format!("camera sample {index}: {e}")))?;
        if (actual[0] - expected_px[0]).abs() > 1e-9 || (actual[1] - expected_px[1]).abs() > 1e-9 {
            return fail(format!(
                "camera sample {index}: projected ({:.9}, {:.9}), golden ({:.9}, {:.9})",
                actual[0], actual[1], expected_px[0], expected_px[1]
            ));
        }
    }

    Ok("camera: intrinsics, distortion and projection samples".to_owned())
}

fn project_pinhole(fixture: &Value, world: [f64; 3]) -> Result<[f64; 2]> {
    let camera = get(fixture, "camera", "camera")?;
    let intrinsics = floats(get(camera, "intrinsics", "camera")?, "camera.intrinsics")?;
    if intrinsics.len() != 4 {
        return fail("camera intrinsics must have 4 values".to_owned());
    }
    let c2w = get(fixture, "c2w", "camera")?;
    let r = matrix3_from_flat(
        get(c2w, "rotation_row_major", "camera.c2w")?,
        "camera.c2w.rotation",
    )?;
    let t = vec3(
        get(c2w, "translation", "camera.c2w")?,
        "camera.c2w.translation",
    )?;

    let d = [world[0] - t[0], world[1] - t[1], world[2] - t[2]];
    // p_camera = R^T (p_world - t), with R row-major.
    let x = r[0][0] * d[0] + r[1][0] * d[1] + r[2][0] * d[2];
    let y = r[0][1] * d[0] + r[1][1] * d[1] + r[2][1] * d[2];
    let z = r[0][2] * d[0] + r[1][2] * d[1] + r[2][2] * d[2];
    if z >= 0.0 {
        return fail("point is not in front of the OpenGL camera (-Z forward)".to_owned());
    }

    let x_n = x / -z;
    let y_n = -y / -z;
    let distortion = get(camera, "distortion", "camera")?;
    let coefficients = floats(
        get(distortion, "coefficients", "camera.distortion")?,
        "camera.distortion.coefficients",
    )?;
    let k1 = coefficients.first().copied().unwrap_or(0.0);
    let k2 = coefficients.get(1).copied().unwrap_or(0.0);
    let p1 = coefficients.get(2).copied().unwrap_or(0.0);
    let p2 = coefficients.get(3).copied().unwrap_or(0.0);

    let r2 = x_n * x_n + y_n * y_n;
    let radial = 1.0 + k1 * r2 + k2 * r2 * r2;
    let x_d = x_n * radial + 2.0 * p1 * x_n * y_n + p2 * (r2 + 2.0 * x_n * x_n);
    let y_d = y_n * radial + p1 * (r2 + 2.0 * y_n * y_n) + 2.0 * p2 * x_n * y_n;

    Ok([
        intrinsics[0] * x_d + intrinsics[2],
        intrinsics[1] * y_d + intrinsics[3],
    ])
}

// ---------------------------------------------------------------------------
// COLMAP dataset and image
// ---------------------------------------------------------------------------

fn check_dataset(root: &Path) -> Result<String> {
    let golden = read_json(&root.join("golden/dataset.json"))?;
    let dataset = root.join("datasets/tiny-colmap");

    let cameras_text = read_text(&dataset.join("cameras.txt"))?;
    let camera_lines = non_comment_lines(&cameras_text);
    if camera_lines.len() != 1 {
        return fail(format!(
            "cameras.txt: expected 1 camera, found {}",
            camera_lines.len()
        ));
    }
    let fields: Vec<&str> = camera_lines[0].split_whitespace().collect();
    if fields.len() < 5 {
        return fail("cameras.txt: camera row has fewer than 5 fields".to_owned());
    }
    let golden_cameras = as_array(get(&golden, "cameras", "dataset golden")?, "golden.cameras")?;
    if golden_cameras.len() != 1 {
        return fail("golden/camera list must have exactly one camera".to_owned());
    }
    let gc = &golden_cameras[0];
    same_u64(
        fields[0].parse().unwrap_or(u64::MAX),
        as_u64(get(gc, "camera_id", "golden camera")?, "camera_id")?,
        "dataset camera id",
    )?;
    if fields[1] != as_str(get(gc, "model", "golden camera")?, "camera model")? {
        return fail(format!(
            "dataset camera model `{}` does not match golden",
            fields[1]
        ));
    }
    same_u64(
        fields[2].parse().unwrap_or(u64::MAX),
        as_u64(get(gc, "width", "golden camera")?, "camera width")?,
        "dataset camera width",
    )?;
    same_u64(
        fields[3].parse().unwrap_or(u64::MAX),
        as_u64(get(gc, "height", "golden camera")?, "camera height")?,
        "dataset camera height",
    )?;
    let params: Vec<&str> = fields[4..].to_vec();
    let golden_params = floats(get(gc, "params", "golden camera")?, "golden camera params")?;
    if params.len() != golden_params.len() {
        return fail("dataset camera parameter count does not match golden".to_owned());
    }
    for (index, (raw, expected)) in params.iter().zip(&golden_params).enumerate() {
        let value: f64 = raw
            .parse()
            .map_err(|e| FixtureError::new(format!("camera param {index}: {e}")))?;
        if (value - expected).abs() > 1e-12 {
            return fail(format!(
                "camera param {index}: file {value}, golden {expected}"
            ));
        }
    }

    // The dataset camera and the single-camera fixture describe the same lens.
    let camera_fixture = read_json(&root.join("camera/single_camera.json"))?;
    let camera_record = get(&camera_fixture, "camera", "camera fixture")?;
    close_slice(
        &golden_params,
        &floats(
            get(camera_record, "intrinsics", "camera fixture")?,
            "camera fixture intrinsics",
        )?,
        1e-12,
        "dataset/camera fixture intrinsics",
    )?;

    let images_text = read_text(&dataset.join("images.txt"))?;
    let image_lines = non_comment_lines(&images_text);
    if image_lines.len() != 1 {
        return fail(format!(
            "images.txt: expected 1 image, found {} data lines",
            image_lines.len()
        ));
    }
    let image_fields: Vec<&str> = image_lines[0].split_whitespace().collect();
    if image_fields.len() != 10 {
        return fail(format!(
            "images.txt: expected 10 fields on the image row, found {}",
            image_fields.len()
        ));
    }
    let golden_images = as_array(get(&golden, "images", "dataset golden")?, "golden.images")?;
    if golden_images.len() != 1 {
        return fail("golden image list must have exactly one image".to_owned());
    }
    let gi = &golden_images[0];
    same_u64(
        image_fields[0].parse().unwrap_or(u64::MAX),
        as_u64(get(gi, "image_id", "golden image")?, "image_id")?,
        "image id",
    )?;
    same_u64(
        image_fields[8].parse().unwrap_or(u64::MAX),
        as_u64(get(gi, "camera_id", "golden image")?, "camera_id")?,
        "image camera id",
    )?;
    if image_fields[9] != as_str(get(gi, "name", "golden image")?, "image name")? {
        return fail(format!(
            "image name `{}` does not match golden",
            image_fields[9]
        ));
    }
    let mut expected_pose = floats(get(gi, "qvec_wxyz", "golden image")?, "golden qvec")?;
    expected_pose.extend(floats(get(gi, "tvec", "golden image")?, "golden tvec")?);
    close_slice(
        &parse_floats(&image_fields[1..8], "images.txt pose")?,
        &expected_pose,
        1e-12,
        "image pose",
    )?;

    let points_text = read_text(&dataset.join("points3D.txt"))?;
    let point_lines = non_comment_lines(&points_text);
    let golden_points = as_array(
        get(&golden, "points3d", "dataset golden")?,
        "golden.points3d",
    )?;
    if point_lines.len() != golden_points.len() {
        return fail(format!(
            "points3D.txt: {} points, golden has {}",
            point_lines.len(),
            golden_points.len()
        ));
    }
    for (index, (line, expected)) in point_lines.iter().zip(golden_points).enumerate() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 8 {
            return fail(format!(
                "points3D.txt line {}: fewer than 8 fields",
                index + 1
            ));
        }
        same_u64(
            fields[0].parse().unwrap_or(u64::MAX),
            as_u64(get(expected, "point3d_id", "golden point")?, "point3d_id")?,
            &format!("point {index} id"),
        )?;
        close_slice(
            &parse_floats(&fields[1..4], "point xyz")?,
            &vec3(get(expected, "xyz", "golden point")?, "golden point xyz")?,
            1e-12,
            &format!("point {index} xyz"),
        )?;
        close_slice(
            &parse_floats(&fields[4..7], "point rgb")?,
            &floats(get(expected, "rgb", "golden point")?, "golden point rgb")?,
            1e-12,
            &format!("point {index} rgb"),
        )?;
        let error: f64 = fields[7]
            .parse()
            .map_err(|e| FixtureError::new(format!("point {index} error: {e}")))?;
        let expected_error = as_f64(
            get(expected, "error", "golden point")?,
            "golden point error",
        )?;
        if (error - expected_error).abs() > 1e-12 {
            return fail(format!(
                "point {index} error: file {error}, golden {expected_error}"
            ));
        }
    }

    let image_path = root.join("datasets/tiny-colmap/images/tiny.png");
    let image_bytes = fs::read(&image_path)
        .map_err(|e| FixtureError::new(format!("reading {}: {e}", image_path.display())))?;
    let (width, height, color_type) = png_header(&image_bytes)?;
    let golden_image = get(&golden, "image", "dataset golden")?;
    same_u64(
        width as u64,
        as_u64(get(golden_image, "width", "golden image")?, "image width")?,
        "image width",
    )?;
    same_u64(
        height as u64,
        as_u64(get(golden_image, "height", "golden image")?, "image height")?,
        "image height",
    )?;
    same_u64(
        color_type as u64,
        as_u64(
            get(golden_image, "color_type", "golden image")?,
            "image color type",
        )?,
        "image color type",
    )?;

    Ok("dataset: COLMAP text rows, golden values and tiny PNG".to_owned())
}

// ---------------------------------------------------------------------------
// Scene transform
// ---------------------------------------------------------------------------

fn check_scene(root: &Path) -> Result<String> {
    let scene = read_json(&root.join("scene/scene_transform.json"))?;
    let golden = read_json(&root.join("golden/scene.json"))?;

    expect_str(&scene, "format", "spirula-scene-transform", "scene")?;
    expect_u64(&scene, "version", 1, "scene")?;

    let train = get(&scene, "train_from_world", "scene")?;
    let world = get(&scene, "world_from_train", "scene")?;
    let train_scale = as_f64(get(train, "scale", "train block")?, "train.scale")?;
    let world_scale = as_f64(get(world, "scale", "world block")?, "world.scale")?;
    if (train_scale - as_f64(get(&golden, "scale", "golden scene")?, "golden.scale")?).abs() > 1e-12
    {
        return fail("scene train scale does not match golden".to_owned());
    }
    close_slice(
        &vec3(
            get(train, "translation", "train block")?,
            "train.translation",
        )?,
        &vec3(
            get(&golden, "translation", "golden scene")?,
            "golden.translation",
        )?,
        1e-12,
        "scene translation",
    )?;
    if (train_scale * world_scale - 1.0).abs() > 1e-12 {
        return fail("scene scales are not reciprocal".to_owned());
    }

    check_similarity_block(train, "train")?;
    check_similarity_block(world, "world")?;

    let train_r = matrix3(
        get(
            get(train, "rotation", "train block")?,
            "matrix_3x3",
            "train.rotation",
        )?,
        "train.rotation.matrix_3x3",
    )?;
    let world_r = matrix3(
        get(
            get(world, "rotation", "world block")?,
            "matrix_3x3",
            "world.rotation",
        )?,
        "world.rotation.matrix_3x3",
    )?;
    for r in 0..3 {
        for c in 0..3 {
            if (world_r[r][c] - train_r[c][r]).abs() > 1e-12 {
                return fail(format!(
                    "world_from_train.rotation[{r}][{c}] is not the transpose of train_from_world"
                ));
            }
        }
    }

    // p_train = s R p_world + t implies p_world = (1/s) R^T p_train - (1/s) R^T t.
    let train_t = vec3(
        get(train, "translation", "train block")?,
        "train.translation",
    )?;
    let world_t = vec3(
        get(world, "translation", "world block")?,
        "world.translation",
    )?;
    let expected_world_t = [
        -transpose_mul(train_r, train_t)[0] / train_scale,
        -transpose_mul(train_r, train_t)[1] / train_scale,
        -transpose_mul(train_r, train_t)[2] / train_scale,
    ];
    close_slice(
        &world_t,
        &expected_world_t,
        1e-12,
        "scene inverse translation",
    )?;

    // Translation before rotation is R^T t / scale.
    let train_pre = vec3(
        get(train, "translation_before_rotation", "train block")?,
        "train.translation_before_rotation",
    )?;
    close_slice(
        &train_pre,
        &scale_vec(transpose_mul(train_r, train_t), 1.0 / train_scale),
        1e-12,
        "train.translation_before_rotation",
    )?;
    let world_pre = vec3(
        get(world, "translation_before_rotation", "world block")?,
        "world.translation_before_rotation",
    )?;
    close_slice(
        &world_pre,
        &scale_vec(transpose_mul(world_r, world_t), 1.0 / world_scale),
        1e-12,
        "world.translation_before_rotation",
    )?;

    let centering = get(&scene, "centering", "scene")?;
    if as_str(
        get(centering, "mode", "scene.centering")?,
        "scene.centering.mode",
    )? != as_str(
        get(&golden, "centering_mode", "golden scene")?,
        "golden.centering_mode",
    )? {
        return fail("scene centering mode does not match golden".to_owned());
    }
    close_slice(
        &vec3(
            get(centering, "center_world", "scene.centering")?,
            "scene.centering.center_world",
        )?,
        &vec3(
            get(&golden, "center_world", "golden scene")?,
            "golden.center_world",
        )?,
        1e-12,
        "scene center_world",
    )?;
    close_slice(
        &vec3(
            get(centering, "center_world", "scene.centering")?,
            "scene.centering.center_world",
        )?,
        &scale_vec(train_pre, -1.0),
        1e-12,
        "scene center is minus translation_before_rotation",
    )?;

    let georeference = get(&scene, "georeference", "scene")?;
    if as_str(
        get(georeference, "frame", "scene.georeference")?,
        "scene.georeference.frame",
    )? != "enu"
    {
        return fail("scene georeference frame must be enu".to_owned());
    }
    if as_f64(
        get(georeference, "origin_lat", "scene.georeference")?,
        "origin_lat",
    )? <= 0.0
    {
        return fail("scene georeference origin_lat must be positive in this fixture".to_owned());
    }

    // Golden sample transforms.
    let samples = as_array(get(&golden, "samples", "golden scene")?, "golden.samples")?;
    if samples.is_empty() {
        return fail("golden scene must contain at least one sample".to_owned());
    }
    for (index, sample) in samples.iter().enumerate() {
        let world_point = vec3(get(sample, "world", "scene sample")?, "scene sample world")?;
        let expected = vec3(get(sample, "train", "scene sample")?, "scene sample train")?;
        let rotated = mul_mat3(train_r, world_point);
        let actual = [
            train_scale * rotated[0] + train_t[0],
            train_scale * rotated[1] + train_t[1],
            train_scale * rotated[2] + train_t[2],
        ];
        close_slice(&actual, &expected, 1e-12, &format!("scene sample {index}"))?;
    }

    Ok("scene: similarity blocks, inverse, centering, georeference and samples".to_owned())
}

fn check_similarity_block(block: &Value, label: &str) -> Result<()> {
    let rotation = get(block, "rotation", label)?;
    let r = matrix3(
        get(rotation, "matrix_3x3", label)?,
        &format!("{label}.rotation.matrix_3x3"),
    )?;

    let flat_row = floats(
        get(rotation, "matrix_3x3_flat_row_major", label)?,
        &format!("{label}.rotation.flat_row"),
    )?;
    let mut expected_flat = Vec::with_capacity(9);
    for row in r {
        expected_flat.extend_from_slice(&row);
    }
    close_slice(
        &flat_row,
        &expected_flat,
        1e-12,
        &format!("{label}.rotation.flat_row"),
    )?;

    let flat_col = floats(
        get(rotation, "matrix_3x3_flat_column_major", label)?,
        &format!("{label}.rotation.flat_col"),
    )?;
    let expected_col = [
        r[0][0], r[1][0], r[2][0], r[0][1], r[1][1], r[2][1], r[0][2], r[1][2], r[2][2],
    ];
    close_slice(
        &flat_col,
        &expected_col,
        1e-12,
        &format!("{label}.rotation.flat_col"),
    )?;

    let quat_wxyz = floats(
        get(rotation, "quaternion_wxyz", label)?,
        &format!("{label}.quaternion_wxyz"),
    )?;
    if quat_wxyz.len() != 4 {
        return fail(format!("{label}.quaternion_wxyz must have 4 values"));
    }
    let quat_xyzw = floats(
        get(rotation, "quaternion_xyzw", label)?,
        &format!("{label}.quaternion_xyzw"),
    )?;
    if quat_xyzw.len() != 4 {
        return fail(format!("{label}.quaternion_xyzw must have 4 values"));
    }
    close_slice(
        &quat_wxyz,
        &[quat_xyzw[3], quat_xyzw[0], quat_xyzw[1], quat_xyzw[2]],
        1e-12,
        &format!("{label}.quaternion spellings"),
    )?;
    let from_quat = matrix3_from_quaternion_wxyz(&quat_wxyz);
    for row in 0..3 {
        for col in 0..3 {
            if (from_quat[row][col] - r[row][col]).abs() > 1e-12 {
                return fail(format!(
                    "{label}.quaternion_wxyz does not reproduce matrix_3x3 at [{row}][{col}]"
                ));
            }
        }
    }

    let axis_angle = get(rotation, "axis_angle", label)?;
    let axis = vec3(
        get(axis_angle, "axis", label)?,
        &format!("{label}.axis_angle.axis"),
    )?;
    let angle_rad = as_f64(
        get(axis_angle, "angle_rad", label)?,
        &format!("{label}.axis_angle.angle_rad"),
    )?;
    let rotvec = vec3(
        get(rotation, "rotation_vector", label)?,
        &format!("{label}.rotation_vector"),
    )?;
    close_slice(
        &rotvec,
        &scale_vec(axis, angle_rad),
        1e-12,
        &format!("{label}.rotation_vector"),
    )?;

    let matrix4 = matrix4(
        get(block, "matrix_4x4", label)?,
        &format!("{label}.matrix_4x4"),
    )?;
    let scale = as_f64(get(block, "scale", label)?, &format!("{label}.scale"))?;
    let t = vec3(
        get(block, "translation", label)?,
        &format!("{label}.translation"),
    )?;
    for row in 0..3 {
        for col in 0..3 {
            if (matrix4[row][col] - scale * r[row][col]).abs() > 1e-12 {
                return fail(format!("{label}.matrix_4x4 rotation is inconsistent"));
            }
        }
        if (matrix4[row][3] - t[row]).abs() > 1e-12 {
            return fail(format!("{label}.matrix_4x4 translation is inconsistent"));
        }
    }
    if (matrix4[3][0]).abs() > 1e-12
        || (matrix4[3][1]).abs() > 1e-12
        || (matrix4[3][2]).abs() > 1e-12
        || (matrix4[3][3] - 1.0).abs() > 1e-12
    {
        return fail(format!("{label}.matrix_4x4 bottom row must be [0,0,0,1]"));
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Splats and point cloud
// ---------------------------------------------------------------------------

fn check_splats(root: &Path) -> Result<String> {
    let golden = read_json(&root.join("golden/splats.json"))?;
    let tiny = parse_ply(&root.join("splats/tiny_splat.ply"))?;
    let tiny_golden = get(&golden, "tiny_splat", "splats golden")?;
    same_u64(
        tiny.count as u64,
        as_u64(get(tiny_golden, "count", "tiny splat golden")?, "count")?,
        "tiny splat count",
    )?;
    if tiny.format != "binary_little_endian" {
        return fail(format!(
            "tiny_splat.ply format is `{}`, expected binary_little_endian",
            tiny.format
        ));
    }
    let expected_properties = strings(
        get(tiny_golden, "properties", "tiny splat golden")?,
        "tiny splat properties",
    )?;
    if tiny.properties != expected_properties {
        return fail("tiny_splat.ply property order does not match golden".to_owned());
    }

    let rows = binary_rows(&tiny)?;
    let first = &rows[0];
    let first_golden = get(tiny_golden, "first", "tiny splat golden")?;
    close_slice(
        &first[0..3],
        &vec3(get(first_golden, "xyz", "tiny splat golden")?, "first xyz")?,
        1e-6,
        "splat first xyz",
    )?;
    let opacity = first[9];
    let expected_opacity = as_f64(
        get(first_golden, "opacity", "tiny splat golden")?,
        "first opacity",
    )?;
    if (sigmoid(opacity) - expected_opacity).abs() > 1e-6 {
        return fail("tiny splat first opacity does not decode to the golden value".to_owned());
    }
    let color = [
        first[6] * 0.282_094_791_773_878_14 + 0.5,
        first[7] * 0.282_094_791_773_878_14 + 0.5,
        first[8] * 0.282_094_791_773_878_14 + 0.5,
    ];
    close_slice(
        &color,
        &vec3(
            get(first_golden, "color", "tiny splat golden")?,
            "first color",
        )?,
        1e-6,
        "splat first color",
    )?;
    let expected_scale_log = floats(
        get(first_golden, "scale_log", "tiny splat golden")?,
        "first scale_log",
    )?;
    close_slice(
        &first[10..13],
        &expected_scale_log,
        1e-6,
        "splat first scale log",
    )?;
    close_slice(
        &first[13..17],
        &vec4(
            get(first_golden, "rot_wxyz", "tiny splat golden")?,
            "first rot",
        )?,
        1e-6,
        "splat first rotation",
    )?;

    let bbox_min = vec3(
        get(tiny_golden, "bbox_min", "tiny splat golden")?,
        "bbox_min",
    )?;
    let bbox_max = vec3(
        get(tiny_golden, "bbox_max", "tiny splat golden")?,
        "bbox_max",
    )?;
    let (lo, hi) = bbox(&rows)?;
    close_slice(&lo, &bbox_min, 1e-6, "splat bbox min")?;
    close_slice(&hi, &bbox_max, 1e-6, "splat bbox max")?;

    let render = parse_ply(&root.join("splats/render_splat.ply"))?;
    let render_golden = get(&golden, "render_splat", "splats golden")?;
    same_u64(
        render.count as u64,
        as_u64(get(render_golden, "count", "render splat golden")?, "count")?,
        "render splat count",
    )?;
    let render_rows = binary_rows(&render)?;
    close_slice(
        &render_rows[0][0..3],
        &vec3(
            get(render_golden, "xyz", "render splat golden")?,
            "render xyz",
        )?,
        1e-6,
        "render splat xyz",
    )?;
    if (sigmoid(render_rows[0][9])
        - as_f64(
            get(render_golden, "opacity", "render splat golden")?,
            "render opacity",
        )?)
    .abs()
        > 1e-6
    {
        return fail("render splat opacity does not decode to the golden value".to_owned());
    }

    let seed = parse_ascii_ply(&root.join("splats/seed_points.ply"))?;
    let seed_golden = get(&golden, "seed_points", "splats golden")?;
    same_u64(
        seed.rows.len() as u64,
        as_u64(get(seed_golden, "count", "seed golden")?, "count")?,
        "seed point count",
    )?;
    if seed.properties
        != strings(
            get(seed_golden, "properties", "seed golden")?,
            "seed properties",
        )?
    {
        return fail("seed_points.ply property order does not match golden".to_owned());
    }
    let (seed_lo, seed_hi) = bbox(&seed.rows)?;
    close_slice(
        &seed_lo,
        &vec3(
            get(seed_golden, "bbox_min", "seed golden")?,
            "seed bbox min",
        )?,
        1e-6,
        "seed bbox min",
    )?;
    close_slice(
        &seed_hi,
        &vec3(
            get(seed_golden, "bbox_max", "seed golden")?,
            "seed bbox max",
        )?,
        1e-6,
        "seed bbox max",
    )?;

    // The checkpoint carries a copy of the inference artifact.
    let checkpoint_ply = fs::read(root.join("checkpoints/tiny-run/step-000000001.ckpt/splat.ply"))
        .map_err(|e| FixtureError::new(format!("reading checkpoint splat.ply: {e}")))?;
    let tiny_bytes = fs::read(root.join("splats/tiny_splat.ply"))
        .map_err(|e| FixtureError::new(format!("reading tiny_splat.ply: {e}")))?;
    if checkpoint_ply != tiny_bytes {
        return fail("checkpoint splat.ply is not byte-identical to tiny_splat.ply".to_owned());
    }

    Ok("splats: PLY headers, raw values, bounds and checkpoint copy".to_owned())
}

// ---------------------------------------------------------------------------
// Checkpoint
// ---------------------------------------------------------------------------

fn check_checkpoint(root: &Path) -> Result<String> {
    let golden = read_json(&root.join("golden/checkpoint.json"))?;
    let checkpoint = root.join("checkpoints/tiny-run/step-000000001.ckpt");
    let tar_bytes = fs::read(checkpoint.join("state.tar"))
        .map_err(|e| FixtureError::new(format!("reading state.tar: {e}")))?;
    let entries = parse_tar(&tar_bytes)?;
    let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
    let expected_names = strings(
        get(&golden, "entries", "checkpoint golden")?,
        "checkpoint entries",
    )?;
    if names
        != expected_names
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
    {
        return fail(format!(
            "state.tar entries {names:?} do not match golden {expected_names:?}"
        ));
    }

    let state_bytes = entries
        .iter()
        .find(|e| e.name == "state.json")
        .map(|e| &e.data)
        .ok_or_else(|| FixtureError::new("state.tar has no state.json".to_owned()))?;
    let state: Value = serde_json::from_slice(state_bytes)
        .map_err(|e| FixtureError::new(format!("parsing state.json: {e}")))?;
    let expected_state = get(&golden, "state", "checkpoint golden")?;
    for key in [
        "format_version",
        "full_resume",
        "step",
        "cur_num_splats",
        "max_num_splats",
        "num_sh",
        "sh_degree",
    ] {
        same_u64(
            state
                .get(key)
                .and_then(Value::as_u64)
                .ok_or_else(|| FixtureError::new(format!("state.json: missing `{key}`")))?,
            expected_state
                .get(key)
                .and_then(Value::as_u64)
                .ok_or_else(|| FixtureError::new(format!("golden state: missing `{key}`")))?,
            &format!("checkpoint {key}"),
        )?;
    }
    if state.get("primitive").and_then(Value::as_str)
        != expected_state.get("primitive").and_then(Value::as_str)
    {
        return fail("checkpoint primitive does not match golden".to_owned());
    }

    let means = parse_npy(
        entries
            .iter()
            .find(|e| e.name == "world.means.npy")
            .map(|e| e.data.as_slice())
            .ok_or_else(|| FixtureError::new("state.tar has no world.means.npy".to_owned()))?,
    )?;
    let expected_shape = floats(
        get(&golden, "world_means_shape", "checkpoint golden")?,
        "world means shape",
    )?
    .into_iter()
    .map(|v| v as usize)
    .collect::<Vec<_>>();
    if means.shape != expected_shape {
        return fail(format!(
            "world.means.npy shape {:?} does not match golden {expected_shape:?}",
            means.shape
        ));
    }
    close_slice(
        &means.values,
        &floats(
            get(&golden, "world_means", "checkpoint golden")?,
            "world means",
        )?,
        1e-6,
        "checkpoint world means",
    )?;

    let opacities = parse_npy(
        entries
            .iter()
            .find(|e| e.name == "world.opacities.npy")
            .map(|e| e.data.as_slice())
            .ok_or_else(|| FixtureError::new("state.tar has no world.opacities.npy".to_owned()))?,
    )?;
    close_slice(
        &opacities.values,
        &floats(
            get(&golden, "world_opacities", "checkpoint golden")?,
            "world opacities",
        )?,
        1e-6,
        "checkpoint world opacities",
    )?;

    let config = read_json(&root.join("checkpoints/tiny-run/config.json"))?;
    for key in strings(
        get(&golden, "config_keys", "checkpoint golden")?,
        "config keys",
    )? {
        if config.get(&key).is_none() {
            return fail(format!("run config.json is missing `{key}`"));
        }
    }

    Ok("checkpoint: ustar entries, state.json, NPY payloads and run config".to_owned())
}

// ---------------------------------------------------------------------------
// Preview request sample (P4-F06; load-only, no GPU)
// ---------------------------------------------------------------------------

fn check_preview(root: &Path) -> Result<String> {
    let request_path = root.join("preview/request.json");
    let parsed = crate::preview::load_preview_request(&request_path, None)
        .map_err(|e| FixtureError::new(format!("preview request load failed: {e}")))?;
    if parsed.camera.width() != 3 || parsed.camera.height() != 2 {
        return fail(format!(
            "preview fixture camera is {}x{}, expected 3x2",
            parsed.camera.width(),
            parsed.camera.height()
        ));
    }
    if parsed.splat_ply != root.join("preview/scene.ply") {
        return fail(format!(
            "preview splat_ply resolved to {}, expected preview/scene.ply",
            parsed.splat_ply.display()
        ));
    }
    let ply = fs::read(&parsed.splat_ply)
        .map_err(|e| FixtureError::new(format!("reading preview scene.ply: {e}")))?;
    if ply.len() < 4 || &ply[..4] != b"ply\n" {
        return fail("preview/scene.ply missing PLY magic".to_owned());
    }
    Ok("preview: request schema/version, relative splat path, 3x2 camera".to_owned())
}

// ---------------------------------------------------------------------------
// Render sample
// ---------------------------------------------------------------------------

fn check_render(root: &Path) -> Result<String> {
    let render = read_json(&root.join("render/tiny_render.json"))?;
    let golden = read_json(&root.join("golden/render.json"))?;
    let width = as_u64(get(&render, "width", "render fixture")?, "render.width")? as usize;
    let height = as_u64(get(&render, "height", "render fixture")?, "render.height")? as usize;
    same_u64(
        width as u64,
        as_u64(get(&golden, "width", "render golden")?, "golden.width")?,
        "render width",
    )?;
    same_u64(
        height as u64,
        as_u64(get(&golden, "height", "render golden")?, "golden.height")?,
        "render height",
    )?;

    let ppm = fs::read(root.join("render/golden.ppm"))
        .map_err(|e| FixtureError::new(format!("reading golden.ppm: {e}")))?;
    let image = parse_ppm(&ppm)?;
    if image.width != width || image.height != height {
        return fail(format!(
            "golden.ppm is {}x{}, render fixture says {width}x{height}",
            image.width, image.height
        ));
    }
    let png = fs::read(root.join("render/golden.png"))
        .map_err(|e| FixtureError::new(format!("reading golden.png: {e}")))?;
    let (png_w, png_h, _) = png_header(&png)?;
    if png_w as usize != width || png_h as usize != height {
        return fail(format!("golden.png is {png_w}x{png_h}"));
    }

    let tolerance = as_u64(
        get(&golden, "rgb_tolerance", "render golden")?,
        "rgb_tolerance",
    )? as i64;
    let samples = get(&golden, "sample", "render golden")?;
    for name in ["center", "corner", "far_corner"] {
        let sample = get(samples, name, "render golden sample")?;
        let pixel = vec2(
            get(sample, "pixel", "render sample")?,
            "render sample pixel",
        )?;
        let expected = floats(get(sample, "rgb8", "render sample")?, "render sample rgb8")?;
        let actual = image.pixel(pixel[0] as usize, pixel[1] as usize)?;
        if (actual[0] as i64 - expected[0] as i64).abs() > tolerance {
            return fail(format!(
                "golden.ppm {name} pixel is {:?}, golden says {:?}",
                actual, expected
            ));
        }
    }

    // The splat must land on the documented pixel.
    let camera = get(&render, "camera", "render fixture")?;
    let intrinsics = floats(get(camera, "intrinsics", "render camera")?, "intrinsics")?;
    let splat = parse_ply(&root.join("splats/render_splat.ply"))?;
    let splat_rows = binary_rows(&splat)?;
    let xyz = [splat_rows[0][0], splat_rows[0][1], splat_rows[0][2]];
    let projected = project_pinhole(
        &serde_json::json!({
            "camera": camera,
            "c2w": get(camera, "c2w", "render camera")?,
        }),
        xyz,
    )?;
    let center = vec2(
        get(
            get(samples, "center", "render golden sample")?,
            "pixel",
            "center",
        )?,
        "center pixel",
    )?;
    if (projected[0] - center[0]).abs() > 1e-6 || (projected[1] - center[1]).abs() > 1e-6 {
        return fail(format!(
            "render splat projects to ({:.6}, {:.6}), golden center is {:?}",
            projected[0], projected[1], center
        ));
    }

    // Recompute the documented image and compare every pixel.
    let depth = get(&golden, "depth", "render golden")?;
    let depth_expected = as_f64(get(depth, "expected", "render depth")?, "depth")?;
    if (depth_expected - 2.0).abs() > 1e-12 {
        return fail("render fixture depth must be 2.0".to_owned());
    }
    let fx = intrinsics[0];
    let scale = (splat_rows[0][10]).exp();
    let sigma = fx * scale / depth_expected;
    let opacity = sigmoid(splat_rows[0][9]);
    for y in 0..height {
        for x in 0..width {
            let r2 = (x as f64 - center[0]).powi(2) + (y as f64 - center[1]).powi(2);
            let alpha = opacity * (-0.5 * r2 / (sigma * sigma)).exp();
            let expected = (255.0 * alpha * splat_rows[0][6].mul_add(0.282_094_791_773_878_14, 0.5))
                .round() as i64;
            let actual = image.pixel(x, y)?[0] as i64;
            if (actual - expected).abs() > tolerance {
                return fail(format!(
                    "golden.ppm ({x},{y}) red is {actual}, formula gives {expected}"
                ));
            }
        }
    }

    Ok("render: golden PPM/PNG, projected center and analytic alpha".to_owned())
}

// ---------------------------------------------------------------------------
// Small parsing helpers
// ---------------------------------------------------------------------------

fn read_text(path: &Path) -> Result<String> {
    fs::read_to_string(path)
        .map_err(|e| FixtureError::new(format!("reading {}: {e}", path.display())))
}

fn read_json(path: &Path) -> Result<Value> {
    let bytes = fs::read(path)
        .map_err(|e| FixtureError::new(format!("reading {}: {e}", path.display())))?;
    serde_json::from_slice(&bytes)
        .map_err(|e| FixtureError::new(format!("parsing {}: {e}", path.display())))
}

fn get<'a>(value: &'a Value, key: &str, context: &str) -> Result<&'a Value> {
    value
        .get(key)
        .ok_or_else(|| FixtureError::new(format!("{context}: missing `{key}`")))
}

fn as_str<'a>(value: &'a Value, context: &str) -> Result<&'a str> {
    value
        .as_str()
        .ok_or_else(|| FixtureError::new(format!("{context}: expected a string")))
}

fn as_f64(value: &Value, context: &str) -> Result<f64> {
    value
        .as_f64()
        .ok_or_else(|| FixtureError::new(format!("{context}: expected a number")))
}

fn as_u64(value: &Value, context: &str) -> Result<u64> {
    value
        .as_u64()
        .ok_or_else(|| FixtureError::new(format!("{context}: expected an unsigned integer")))
}

fn as_array<'a>(value: &'a Value, context: &str) -> Result<&'a Vec<Value>> {
    value
        .as_array()
        .ok_or_else(|| FixtureError::new(format!("{context}: expected an array")))
}

fn expect_str(value: &Value, key: &str, expected: &str, context: &str) -> Result<()> {
    let actual = as_str(get(value, key, context)?, &format!("{context}.{key}"))?;
    if actual != expected {
        return fail(format!(
            "{context}.{key}: expected `{expected}`, found `{actual}`"
        ));
    }
    Ok(())
}

fn expect_u64(value: &Value, key: &str, expected: u64, context: &str) -> Result<()> {
    let actual = as_u64(get(value, key, context)?, &format!("{context}.{key}"))?;
    same_u64(actual, expected, &format!("{context}.{key}"))
}

fn same_u64(actual: u64, expected: u64, context: &str) -> Result<()> {
    if actual != expected {
        return fail(format!("{context}: found {actual}, expected {expected}"));
    }
    Ok(())
}

fn floats(value: &Value, context: &str) -> Result<Vec<f64>> {
    as_array(value, context)?
        .iter()
        .enumerate()
        .map(|(index, item)| as_f64(item, &format!("{context}[{index}]")))
        .collect()
}

fn strings(value: &Value, context: &str) -> Result<Vec<String>> {
    as_array(value, context)?
        .iter()
        .enumerate()
        .map(|(index, item)| as_str(item, &format!("{context}[{index}]")).map(str::to_owned))
        .collect()
}

fn vec2(value: &Value, context: &str) -> Result<[f64; 2]> {
    let v = floats(value, context)?;
    if v.len() != 2 {
        return fail(format!("{context}: expected 2 numbers"));
    }
    Ok([v[0], v[1]])
}

fn vec3(value: &Value, context: &str) -> Result<[f64; 3]> {
    let v = floats(value, context)?;
    if v.len() != 3 {
        return fail(format!("{context}: expected 3 numbers"));
    }
    Ok([v[0], v[1], v[2]])
}

fn vec4(value: &Value, context: &str) -> Result<[f64; 4]> {
    let v = floats(value, context)?;
    if v.len() != 4 {
        return fail(format!("{context}: expected 4 numbers"));
    }
    Ok([v[0], v[1], v[2], v[3]])
}

fn matrix3(value: &Value, context: &str) -> Result<[[f64; 3]; 3]> {
    let rows = as_array(value, context)?;
    if rows.len() != 3 {
        return fail(format!("{context}: expected 3 rows"));
    }
    let mut out = [[0.0; 3]; 3];
    for (r, row) in rows.iter().enumerate() {
        let values = floats(row, &format!("{context}[{r}]"))?;
        if values.len() != 3 {
            return fail(format!("{context}[{r}]: expected 3 columns"));
        }
        out[r] = [values[0], values[1], values[2]];
    }
    Ok(out)
}

fn matrix3_from_flat(value: &Value, context: &str) -> Result<[[f64; 3]; 3]> {
    let v = floats(value, context)?;
    if v.len() != 9 {
        return fail(format!("{context}: expected 9 numbers"));
    }
    Ok([[v[0], v[1], v[2]], [v[3], v[4], v[5]], [v[6], v[7], v[8]]])
}

fn matrix4(value: &Value, context: &str) -> Result<[[f64; 4]; 4]> {
    let rows = as_array(value, context)?;
    if rows.len() != 4 {
        return fail(format!("{context}: expected 4 rows"));
    }
    let mut out = [[0.0; 4]; 4];
    for (r, row) in rows.iter().enumerate() {
        let values = floats(row, &format!("{context}[{r}]"))?;
        if values.len() != 4 {
            return fail(format!("{context}[{r}]: expected 4 columns"));
        }
        out[r] = [values[0], values[1], values[2], values[3]];
    }
    Ok(out)
}

fn parse_floats(fields: &[&str], context: &str) -> Result<Vec<f64>> {
    fields
        .iter()
        .enumerate()
        .map(|(index, raw)| {
            raw.parse::<f64>()
                .map_err(|e| FixtureError::new(format!("{context}[{index}]: {e}")))
        })
        .collect()
}

fn close_slice(actual: &[f64], expected: &[f64], tolerance: f64, context: &str) -> Result<()> {
    if actual.len() != expected.len() {
        return fail(format!(
            "{context}: length {} does not match expected {}",
            actual.len(),
            expected.len()
        ));
    }
    for (index, (a, e)) in actual.iter().zip(expected).enumerate() {
        if !a.is_finite() || !e.is_finite() {
            return fail(format!("{context}[{index}]: non-finite value"));
        }
        if (a - e).abs() > tolerance {
            return fail(format!(
                "{context}[{index}]: found {a:.12}, expected {e:.12}"
            ));
        }
    }
    Ok(())
}

fn scale_vec(v: [f64; 3], scale: f64) -> [f64; 3] {
    [v[0] * scale, v[1] * scale, v[2] * scale]
}

fn transpose_mul(r: [[f64; 3]; 3], v: [f64; 3]) -> [f64; 3] {
    [
        r[0][0] * v[0] + r[1][0] * v[1] + r[2][0] * v[2],
        r[0][1] * v[0] + r[1][1] * v[1] + r[2][1] * v[2],
        r[0][2] * v[0] + r[1][2] * v[1] + r[2][2] * v[2],
    ]
}

fn mul_mat3(r: [[f64; 3]; 3], v: [f64; 3]) -> [f64; 3] {
    [
        r[0][0] * v[0] + r[0][1] * v[1] + r[0][2] * v[2],
        r[1][0] * v[0] + r[1][1] * v[1] + r[1][2] * v[2],
        r[2][0] * v[0] + r[2][1] * v[1] + r[2][2] * v[2],
    ]
}

fn matrix3_from_quaternion_wxyz(q: &[f64]) -> [[f64; 3]; 3] {
    let (w, x, y, z) = (q[0], q[1], q[2], q[3]);
    [
        [
            1.0 - 2.0 * (y * y + z * z),
            2.0 * (x * y - w * z),
            2.0 * (x * z + w * y),
        ],
        [
            2.0 * (x * y + w * z),
            1.0 - 2.0 * (x * x + z * z),
            2.0 * (y * z - w * x),
        ],
        [
            2.0 * (x * z - w * y),
            2.0 * (y * z + w * x),
            1.0 - 2.0 * (x * x + y * y),
        ],
    ]
}

fn sigmoid(value: f64) -> f64 {
    1.0 / (1.0 + (-value).exp())
}

fn validate_relative_path(path: &str, context: &str) -> Result<()> {
    if path.is_empty() {
        return fail(format!("{context}: empty path"));
    }
    let parsed = Path::new(path);
    if parsed.is_absolute() {
        return fail(format!("{context}: `{path}` must be relative"));
    }
    for component in parsed.components() {
        match component {
            Component::Normal(_) => {}
            _ => {
                return fail(format!(
                    "{context}: `{path}` must not contain `..` or a root"
                ));
            }
        }
    }
    Ok(())
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(64);
    for byte in digest {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

fn non_comment_lines(text: &str) -> Vec<&str> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .collect()
}

fn fail<T>(message: String) -> Result<T> {
    Err(FixtureError::new(message))
}

// ---------------------------------------------------------------------------
// PLY
// ---------------------------------------------------------------------------

struct Ply {
    format: String,
    count: usize,
    properties: Vec<String>,
    types: Vec<String>,
    data_offset: usize,
    bytes: Vec<u8>,
}

struct AsciiPly {
    properties: Vec<String>,
    rows: Vec<Vec<f64>>,
}

fn parse_ply(path: &Path) -> Result<Ply> {
    let bytes = fs::read(path)
        .map_err(|e| FixtureError::new(format!("reading {}: {e}", path.display())))?;
    let header = ply_header(&bytes, path)?;
    Ok(Ply {
        format: header.format,
        count: header.count,
        properties: header.properties,
        types: header.types,
        data_offset: header.data_offset,
        bytes,
    })
}

fn parse_ascii_ply(path: &Path) -> Result<AsciiPly> {
    let bytes = fs::read(path)
        .map_err(|e| FixtureError::new(format!("reading {}: {e}", path.display())))?;
    let header = ply_header(&bytes, path)?;
    if header.format != "ascii" {
        return fail(format!("{}: expected an ASCII PLY", path.display()));
    }
    let text = std::str::from_utf8(&bytes[header.data_offset..])
        .map_err(|e| FixtureError::new(format!("{}: {e}", path.display())))?;
    let mut rows = Vec::new();
    for (index, line) in text.lines().filter(|l| !l.trim().is_empty()).enumerate() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() != header.properties.len() {
            return fail(format!(
                "{}: row {index} has {} fields, expected {}",
                path.display(),
                fields.len(),
                header.properties.len()
            ));
        }
        let values = parse_floats(&fields, &format!("{}: row {index}", path.display()))?;
        rows.push(values);
    }
    if rows.len() != header.count {
        return fail(format!(
            "{}: found {} rows, header says {}",
            path.display(),
            rows.len(),
            header.count
        ));
    }
    Ok(AsciiPly {
        properties: header.properties,
        rows,
    })
}

struct PlyHeader {
    format: String,
    count: usize,
    properties: Vec<String>,
    types: Vec<String>,
    data_offset: usize,
}

fn ply_header(bytes: &[u8], path: &Path) -> Result<PlyHeader> {
    let mut offset = 0;
    let mut format = None;
    let mut count = None;
    let mut properties = Vec::new();
    let mut types = Vec::new();
    loop {
        let end = bytes[offset..]
            .iter()
            .position(|b| *b == b'\n')
            .map(|p| offset + p)
            .ok_or_else(|| {
                FixtureError::new(format!("{}: unterminated PLY header", path.display()))
            })?;
        let line = std::str::from_utf8(&bytes[offset..end])
            .map_err(|e| FixtureError::new(format!("{}: {e}", path.display())))?
            .trim();
        offset = end + 1;
        if line == "end_header" {
            return Ok(PlyHeader {
                format: format.ok_or_else(|| {
                    FixtureError::new(format!("{}: PLY header has no format", path.display()))
                })?,
                count: count.ok_or_else(|| {
                    FixtureError::new(format!(
                        "{}: PLY header has no vertex element",
                        path.display()
                    ))
                })?,
                properties,
                types,
                data_offset: offset,
            });
        }
        if let Some(rest) = line.strip_prefix("format ") {
            format = Some(rest.split_whitespace().next().unwrap_or(rest).to_owned());
        } else if let Some(rest) = line.strip_prefix("element vertex ") {
            count = Some(rest.trim().parse::<usize>().map_err(|e| {
                FixtureError::new(format!("{}: bad vertex count: {e}", path.display()))
            })?);
        } else if let Some(rest) = line.strip_prefix("property ") {
            let mut parts = rest.split_whitespace();
            let kind = parts.next();
            let name = parts.next();
            let kind =
                kind.ok_or_else(|| FixtureError::new(format!("{}: bad property", path.display())))?;
            if !matches!(
                kind,
                "float" | "double" | "uchar" | "uint8" | "int" | "uint"
            ) {
                return fail(format!(
                    "{}: unsupported property type `{kind}`",
                    path.display()
                ));
            }
            types.push(kind.to_owned());
            properties.push(
                name.ok_or_else(|| {
                    FixtureError::new(format!("{}: property has no name", path.display()))
                })?
                .to_owned(),
            );
        }
    }
}

fn binary_rows(ply: &Ply) -> Result<Vec<Vec<f64>>> {
    if ply.format != "binary_little_endian" {
        return fail(format!(
            "PLY format `{}` is not binary_little_endian",
            ply.format
        ));
    }
    if let Some(kind) = ply.types.iter().find(|kind| kind.as_str() != "float") {
        return fail(format!("binary PLY property type `{kind}` is not float32"));
    }
    let stride = ply.properties.len() * 4;
    let expected = ply.count * stride;
    let data = &ply.bytes[ply.data_offset..];
    if data.len() != expected {
        return fail(format!(
            "binary PLY payload is {} bytes, expected {expected}",
            data.len()
        ));
    }
    let mut rows = Vec::with_capacity(ply.count);
    for row in data.chunks_exact(stride) {
        let mut values = Vec::with_capacity(ply.properties.len());
        for chunk in row.as_chunks::<4>().0 {
            values.push(f32::from_le_bytes(*chunk) as f64);
        }
        rows.push(values);
    }
    Ok(rows)
}

fn bbox(rows: &[Vec<f64>]) -> Result<([f64; 3], [f64; 3])> {
    if rows.is_empty() {
        return fail("cannot compute a bounding box of an empty point set".to_owned());
    }
    let mut lo = [f64::INFINITY; 3];
    let mut hi = [f64::NEG_INFINITY; 3];
    for row in rows {
        for axis in 0..3 {
            lo[axis] = lo[axis].min(row[axis]);
            hi[axis] = hi[axis].max(row[axis]);
        }
    }
    Ok((lo, hi))
}

// ---------------------------------------------------------------------------
// PNG / PPM
// ---------------------------------------------------------------------------

fn png_header(bytes: &[u8]) -> Result<(u32, u32, u8)> {
    const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    if bytes.len() < 33 || bytes.get(0..8) != Some(SIGNATURE.as_slice()) {
        return fail("not a PNG file".to_owned());
    }
    if bytes.get(12..16) != Some(b"IHDR".as_slice()) {
        return fail("PNG does not start with IHDR".to_owned());
    }
    let width = u32::from_be_bytes(bytes[16..20].try_into().unwrap());
    let height = u32::from_be_bytes(bytes[20..24].try_into().unwrap());
    let color_type = bytes[25];
    Ok((width, height, color_type))
}

struct Ppm {
    width: usize,
    height: usize,
    pixels: Vec<[u8; 3]>,
}

impl Ppm {
    fn pixel(&self, x: usize, y: usize) -> Result<[u8; 3]> {
        if x >= self.width || y >= self.height {
            return fail(format!(
                "pixel ({x},{y}) is outside {}x{}",
                self.width, self.height
            ));
        }
        Ok(self.pixels[y * self.width + x])
    }
}

fn parse_ppm(bytes: &[u8]) -> Result<Ppm> {
    let text = std::str::from_utf8(bytes)
        .map_err(|e| FixtureError::new(format!("golden.ppm is not UTF-8: {e}")))?;
    let mut tokens = text.split_whitespace();
    if tokens.next() != Some("P3") {
        return fail("golden.ppm must be an ASCII P3 PPM".to_owned());
    }
    let width: usize = tokens
        .next()
        .ok_or_else(|| FixtureError::new("golden.ppm: missing width".to_owned()))?
        .parse()
        .map_err(|e| FixtureError::new(format!("golden.ppm: {e}")))?;
    let height: usize = tokens
        .next()
        .ok_or_else(|| FixtureError::new("golden.ppm: missing height".to_owned()))?
        .parse()
        .map_err(|e| FixtureError::new(format!("golden.ppm: {e}")))?;
    let max: u32 = tokens
        .next()
        .ok_or_else(|| FixtureError::new("golden.ppm: missing max value".to_owned()))?
        .parse()
        .map_err(|e| FixtureError::new(format!("golden.ppm: {e}")))?;
    if max != 255 {
        return fail(format!("golden.ppm max value is {max}, expected 255"));
    }
    let mut values = Vec::with_capacity(width * height * 3);
    for token in tokens {
        values.push(
            token
                .parse::<u8>()
                .map_err(|e| FixtureError::new(format!("golden.ppm: {e}")))?,
        );
    }
    if values.len() != width * height * 3 {
        return fail(format!(
            "golden.ppm has {} samples, expected {}",
            values.len(),
            width * height * 3
        ));
    }
    let pixels = values.as_chunks::<3>().0.to_vec();
    Ok(Ppm {
        width,
        height,
        pixels,
    })
}

// ---------------------------------------------------------------------------
// Checkpoint container helpers (ustar + NPY)
// ---------------------------------------------------------------------------

struct TarEntry {
    name: String,
    data: Vec<u8>,
}

fn parse_tar(bytes: &[u8]) -> Result<Vec<TarEntry>> {
    let mut entries = Vec::new();
    let mut offset = 0;
    while offset + 512 <= bytes.len() {
        let header = &bytes[offset..offset + 512];
        if header.iter().all(|b| *b == 0) {
            break;
        }
        let name = tar_string(&header[0..100]);
        let size_raw = tar_string(&header[124..136]);
        let size = usize::from_str_radix(size_raw.trim(), 8)
            .map_err(|e| FixtureError::new(format!("tar entry `{name}`: bad size: {e}")))?;
        let data_start = offset + 512;
        let data_end = data_start
            .checked_add(size)
            .ok_or_else(|| FixtureError::new("tar entry size overflow".to_owned()))?;
        if data_end > bytes.len() {
            return fail(format!("tar entry `{name}` runs past the end of the file"));
        }
        entries.push(TarEntry {
            name,
            data: bytes[data_start..data_end].to_vec(),
        });
        offset = data_start + size.div_ceil(512) * 512;
    }
    if entries.is_empty() {
        return fail("state.tar contains no entries".to_owned());
    }
    Ok(entries)
}

fn tar_string(field: &[u8]) -> String {
    let end = field.iter().position(|b| *b == 0).unwrap_or(field.len());
    String::from_utf8_lossy(&field[..end]).trim().to_owned()
}

struct Npy {
    shape: Vec<usize>,
    values: Vec<f64>,
}

fn parse_npy(bytes: &[u8]) -> Result<Npy> {
    if bytes.len() < 10 || !bytes.starts_with(b"\x93NUMPY") {
        return fail("NPY: bad magic".to_owned());
    }
    let major = bytes[6];
    if major != 1 {
        return fail(format!("NPY: expected version 1, found {major}"));
    }
    let header_len = u16::from_le_bytes([bytes[8], bytes[9]]) as usize;
    let header_start = 10;
    let header_end = header_start + header_len;
    if header_end > bytes.len() {
        return fail("NPY: header runs past the end of the file".to_owned());
    }
    let header = std::str::from_utf8(&bytes[header_start..header_end])
        .map_err(|e| FixtureError::new(format!("NPY header: {e}")))?;

    let descr = between(header, "'descr': '", "'")
        .ok_or_else(|| FixtureError::new("NPY: missing descr".to_owned()))?;
    if descr != "<f4" {
        return fail(format!("NPY: expected descr '<f4', found '{descr}'"));
    }
    if !header.contains("'fortran_order': False") {
        return fail("NPY: only C-order arrays are supported".to_owned());
    }
    let shape_text = between(header, "'shape': (", ")")
        .ok_or_else(|| FixtureError::new("NPY: missing shape".to_owned()))?;
    let mut shape = Vec::new();
    for part in shape_text.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        shape.push(
            part.parse::<usize>()
                .map_err(|e| FixtureError::new(format!("NPY shape: {e}")))?,
        );
    }
    let count = shape.iter().product::<usize>();
    let payload = &bytes[header_end..];
    if payload.len() != count * 4 {
        return fail(format!(
            "NPY payload is {} bytes, shape {shape:?} needs {}",
            payload.len(),
            count * 4
        ));
    }
    let values = payload
        .as_chunks::<4>()
        .0
        .iter()
        .map(|chunk| f32::from_le_bytes(*chunk) as f64)
        .collect();
    Ok(Npy { shape, values })
}

fn between<'a>(haystack: &'a str, start: &str, end: &str) -> Option<&'a str> {
    let from = haystack.find(start)? + start.len();
    let to = haystack[from..].find(end)? + from;
    Some(&haystack[from..to])
}
