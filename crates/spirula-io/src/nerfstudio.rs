//! Nerfstudio `transforms.json` reader (P3-F04).
//!
//! OpenGL c2w from `transform_matrix`; path-safe `file_path`; optional PLY via
//! [`crate::ply`]. Scene center / normalize deferred to P3-F05 — poses stay as
//! written after format conversion (including baked `applied_transform`).
//! Singular `applied_transform` still rejected.
//!
//! Pin: `NerfstudioParser.cpp` @ `9a24f099…`.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use serde_json::Value;
use spirula_core::{CancelToken, ErrorCode};

use crate::colmap::bake::bake_and_fit_cancelable;
use crate::colmap::budget::{
    MAX_CAMERAS, MAX_COLMAP_FILE_BYTES, MAX_IMAGES, MAX_LOAD_MEMORY_BYTES, ParseCancel, ParseMem,
    ensure_count, err, err_path, try_reserve_elems,
};
use crate::colmap::load::validate_image_name;
use crate::colmap::split::compute_splits;
use crate::colmap::{
    ColmapCamera, ColmapLoadConfig, ColmapPoints3D, DatasetFormat, DatasetLoadSummary, LoadedFrame,
    ParsedDataset, SplitIndices,
};
use crate::ply::read_ply_points_budgeted;

const READ_CHUNK: usize = 256 * 1024;

fn open_err(path: &Path, e: std::io::Error) -> spirula_core::DomainError {
    let code = if e.kind() == std::io::ErrorKind::NotFound {
        ErrorCode::NotFound
    } else if e.kind() == std::io::ErrorKind::PermissionDenied {
        ErrorCode::PermissionDenied
    } else {
        ErrorCode::ReadFailed
    };
    err_path(
        code,
        &path.display().to_string(),
        format!("cannot open: {e}"),
    )
}

fn read_capped_text(
    path: &Path,
    mem: &mut ParseMem,
    cancel: Option<&CancelToken>,
) -> Result<String, spirula_core::DomainError> {
    let meta = std::fs::metadata(path).map_err(|e| open_err(path, e))?;
    let len = meta.len();
    if len > MAX_COLMAP_FILE_BYTES {
        return Err(err(
            ErrorCode::ValueOutOfRange,
            "file_size",
            format!(
                "transforms.json {} exceeds budget {MAX_COLMAP_FILE_BYTES} bytes",
                path.display()
            ),
        ));
    }
    mem.charge(len, "transforms_json")?;
    let mut charged = len;
    let mut file = std::fs::File::open(path).map_err(|e| open_err(path, e))?;
    let mut data = Vec::new();
    if len > 0 {
        data.try_reserve(len as usize).map_err(|_| {
            err(
                ErrorCode::ValueOutOfRange,
                "transforms_json",
                format!("transforms.json reservation of {len} bytes failed"),
            )
        })?;
    }
    let mut cp = ParseCancel::new(cancel);
    cp.poll_now()?;
    let mut buf = [0u8; READ_CHUNK];
    let mut read_total = 0u64;
    loop {
        use std::io::Read;
        let n = file.read(&mut buf).map_err(|e| {
            err_path(
                ErrorCode::ReadFailed,
                &path.display().to_string(),
                format!("cannot read transforms.json: {e}"),
            )
        })?;
        if n == 0 {
            break;
        }
        read_total = read_total.saturating_add(n as u64);
        if read_total > MAX_COLMAP_FILE_BYTES {
            return Err(err(
                ErrorCode::ValueOutOfRange,
                "file_size",
                "transforms.json grew past budget during read",
            ));
        }
        if read_total > charged {
            mem.charge(read_total - charged, "transforms_json")?;
            charged = read_total;
        }
        data.extend_from_slice(&buf[..n]);
        cp.tick(n)?;
    }
    String::from_utf8(data).map_err(|_| {
        err(
            ErrorCode::MalformedRecord,
            "transforms_json",
            "transforms.json is not valid UTF-8",
        )
    })
}

fn path_sort_key(path: &Path) -> String {
    path.components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

fn json_num(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.parse().ok(),
        _ => None,
    }
}

fn frame_or_meta_num(
    frame: &Value,
    meta: &Value,
    key: &str,
    required: bool,
) -> Result<f64, spirula_core::DomainError> {
    if let Some(v) = frame.get(key).and_then(json_num) {
        return Ok(v);
    }
    if let Some(v) = meta.get(key).and_then(json_num) {
        return Ok(v);
    }
    if required {
        return Err(err(
            ErrorCode::MalformedRecord,
            key,
            format!("NerfstudioParser: missing '{key}' in frame and file header"),
        ));
    }
    Ok(0.0)
}

fn frame_or_meta_str<'a>(frame: &'a Value, meta: &'a Value, key: &str) -> Option<&'a str> {
    frame
        .get(key)
        .and_then(|v| v.as_str())
        .or_else(|| meta.get(key).and_then(|v| v.as_str()))
}

fn frame_or_meta_has(frame: &Value, meta: &Value, key: &str) -> bool {
    frame.get(key).is_some() || meta.get(key).is_some()
}

/// Validate `applied_transform` 3×3 is invertible when present (P3-F05 owns undo).
fn check_applied_transform(meta: &Value) -> Result<(), spirula_core::DomainError> {
    let Some(at) = meta.get("applied_transform") else {
        return Ok(());
    };
    let rows = at.as_array().ok_or_else(|| {
        err(
            ErrorCode::MalformedRecord,
            "applied_transform",
            "NerfstudioParser: applied_transform must be a 3×3/3×4 array",
        )
    })?;
    if rows.len() < 3 {
        return Err(err(
            ErrorCode::MalformedRecord,
            "applied_transform",
            "NerfstudioParser: applied_transform must have ≥3 rows",
        ));
    }
    let mut m = [[0.0f64; 3]; 3];
    for (r, row) in rows.iter().take(3).enumerate() {
        let cols = row.as_array().ok_or_else(|| {
            err(
                ErrorCode::MalformedRecord,
                "applied_transform",
                "NerfstudioParser: applied_transform row must be an array",
            )
        })?;
        if cols.len() < 3 {
            return Err(err(
                ErrorCode::MalformedRecord,
                "applied_transform",
                "NerfstudioParser: applied_transform row too short",
            ));
        }
        for (c, cell) in cols.iter().take(3).enumerate() {
            let v = json_num(cell).ok_or_else(|| {
                err(
                    ErrorCode::MalformedRecord,
                    "applied_transform",
                    "NerfstudioParser: applied_transform non-numeric",
                )
            })?;
            if !v.is_finite() {
                return Err(err(
                    ErrorCode::ValueOutOfRange,
                    "applied_transform",
                    "NerfstudioParser: applied_transform non-finite",
                ));
            }
            m[r][c] = v;
        }
    }
    let det = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
        - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
        + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
    if det.abs() < 1e-20 {
        return Err(err(
            ErrorCode::ValueOutOfRange,
            "applied_transform",
            "NerfstudioParser: singular applied_transform",
        ));
    }
    Ok(())
}

fn read_c2w12(frame: &Value) -> Result<[f64; 12], spirula_core::DomainError> {
    let tm = frame.get("transform_matrix").ok_or_else(|| {
        err(
            ErrorCode::MalformedRecord,
            "transform_matrix",
            "NerfstudioParser: bad transform_matrix",
        )
    })?;
    let rows = tm.as_array().ok_or_else(|| {
        err(
            ErrorCode::MalformedRecord,
            "transform_matrix",
            "NerfstudioParser: bad transform_matrix",
        )
    })?;
    if rows.len() < 3 {
        return Err(err(
            ErrorCode::MalformedRecord,
            "transform_matrix",
            "NerfstudioParser: bad transform_matrix",
        ));
    }
    let mut out = [0.0f64; 12];
    for r in 0..3 {
        let row = rows[r].as_array().ok_or_else(|| {
            err(
                ErrorCode::MalformedRecord,
                "transform_matrix",
                "NerfstudioParser: bad transform_matrix row",
            )
        })?;
        if row.len() < 4 {
            return Err(err(
                ErrorCode::MalformedRecord,
                "transform_matrix",
                "NerfstudioParser: bad transform_matrix row",
            ));
        }
        for c in 0..4 {
            let v = json_num(&row[c]).ok_or_else(|| {
                err(
                    ErrorCode::MalformedRecord,
                    "transform_matrix",
                    "NerfstudioParser: non-numeric transform_matrix",
                )
            })?;
            if !v.is_finite() {
                return Err(err(
                    ErrorCode::ValueOutOfRange,
                    "transform_matrix",
                    "NerfstudioParser: non-finite transform_matrix",
                ));
            }
            out[r * 4 + c] = v;
        }
    }
    Ok(out)
}

fn safe_rel_path(file_path: &str) -> Result<(), spirula_core::DomainError> {
    validate_image_name(file_path)?;
    // Extra: reject empty path segments that survive Component::Normal.
    let p = Path::new(file_path);
    for c in p.components() {
        match c {
            Component::Normal(s) if s.is_empty() => {
                return Err(err(
                    ErrorCode::InvalidArgument,
                    "file_path",
                    format!("illegal image path `{file_path}`"),
                ));
            }
            _ => {}
        }
    }
    Ok(())
}

struct BuildCam<'a> {
    camera_id: i32,
    model_name: &'a str,
    width: u64,
    height: u64,
    fx: f64,
    fy: f64,
    cx: f64,
    cy: f64,
    rd: &'a RawDist,
    rational: bool,
    thin_prism_shaped: bool,
    skew_px: f64,
}

fn build_colmap_camera(args: BuildCam<'_>) -> Result<ColmapCamera, spirula_core::DomainError> {
    let BuildCam {
        camera_id,
        model_name,
        width,
        height,
        fx,
        fy,
        cx,
        cy,
        rd,
        rational,
        thin_prism_shaped,
        skew_px,
    } = args;
    // Map NS distortion into a COLMAP model that bake_and_fit understands.
    let (model, params) = if model_name == "EQUIRECTANGULAR" {
        // COLMAP EQUIRECTANGULAR stores [width, height] (validated against resolution).
        (
            "EQUIRECTANGULAR".to_owned(),
            vec![width as f64, height as f64],
        )
    } else if rational {
        (
            "FULL_OPENCV".to_owned(),
            vec![
                fx, fy, cx, cy, rd.k1, rd.k2, rd.p1, rd.p2, rd.k3, rd.k4, rd.k5, rd.k6,
            ],
        )
    } else if skew_px != 0.0
        || rd.sx1 != 0.0
        || rd.sy1 != 0.0
        || (thin_prism_shaped && rd.b1 != 0.0)
    {
        (
            "THIN_PRISM_FISHEYE".to_owned(),
            vec![
                fx, fy, cx, cy, rd.k1, rd.k2, rd.k3, rd.k4, rd.p1, rd.p2, rd.sx1, rd.sy1,
            ],
        )
    } else if matches!(
        model_name,
        "FISHEYE"
            | "SIMPLE_FISHEYE"
            | "OPENCV_FISHEYE"
            | "SIMPLE_RADIAL_FISHEYE"
            | "RADIAL_FISHEYE"
            | "THIN_PRISM_FISHEYE"
            | "EQUISOLID"
    ) {
        (
            "OPENCV_FISHEYE".to_owned(),
            vec![fx, fy, cx, cy, rd.k1, rd.k2, rd.k3, rd.k4],
        )
    } else if rd.k1 == 0.0
        && rd.k2 == 0.0
        && rd.p1 == 0.0
        && rd.p2 == 0.0
        && rd.k3 == 0.0
        && rd.k4 == 0.0
    {
        ("PINHOLE".to_owned(), vec![fx, fy, cx, cy])
    } else if rd.k3 == 0.0 && rd.k4 == 0.0 && rd.k5 == 0.0 && rd.k6 == 0.0 {
        (
            "OPENCV".to_owned(),
            vec![fx, fy, cx, cy, rd.k1, rd.k2, rd.p1, rd.p2],
        )
    } else {
        (
            "FULL_OPENCV".to_owned(),
            vec![
                fx, fy, cx, cy, rd.k1, rd.k2, rd.p1, rd.p2, rd.k3, rd.k4, rd.k5, rd.k6,
            ],
        )
    };

    crate::colmap::require_model_by_name(&model).map_err(|e| {
        err(
            ErrorCode::UnsupportedFormat,
            "camera_model",
            format!("NerfstudioParser: unsupported camera model {model_name} ({e})"),
        )
    })?;
    for (i, p) in params.iter().enumerate() {
        if !p.is_finite() {
            return Err(err(
                ErrorCode::ValueOutOfRange,
                "params",
                format!("NerfstudioParser: non-finite camera param[{i}]"),
            ));
        }
    }
    if width == 0 || height == 0 {
        return Err(err(
            ErrorCode::ValueOutOfRange,
            "resolution",
            "NerfstudioParser: non-positive width/height",
        ));
    }
    Ok(ColmapCamera {
        camera_id,
        model,
        width,
        height,
        params,
    })
}

#[derive(Clone, Copy, Default)]
struct RawDist {
    k1: f64,
    k2: f64,
    k3: f64,
    k4: f64,
    k5: f64,
    k6: f64,
    p1: f64,
    p2: f64,
    sx1: f64,
    sy1: f64,
    b1: f64,
    b2: f64,
}

struct FrameBuild {
    frame: Value,
    path: PathBuf,
    sort_key: String,
    file_path: String,
}

/// Load a Nerfstudio dataset from `dataset_dir/transforms.json`.
pub fn load_nerfstudio_dataset(
    dataset_dir: &Path,
    cfg: &ColmapLoadConfig,
    cancel: Option<&CancelToken>,
) -> Result<ParsedDataset, spirula_core::DomainError> {
    load_nerfstudio_dataset_budgeted(dataset_dir, cfg, cancel, MAX_LOAD_MEMORY_BYTES)
}

/// Same as [`load_nerfstudio_dataset`] with an explicit memory budget.
pub fn load_nerfstudio_dataset_budgeted(
    dataset_dir: &Path,
    cfg: &ColmapLoadConfig,
    cancel: Option<&CancelToken>,
    memory_budget_bytes: u64,
) -> Result<ParsedDataset, spirula_core::DomainError> {
    let mut mem = ParseMem::with_cap(memory_budget_bytes);
    let mut cp = ParseCancel::new(cancel);
    cp.poll_now()?;

    if !dataset_dir.is_dir() {
        return Err(err_path(
            if dataset_dir.exists() {
                ErrorCode::InvalidArgument
            } else {
                ErrorCode::NotFound
            },
            &dataset_dir.display().to_string(),
            if dataset_dir.exists() {
                "dataset path is not a directory"
            } else {
                "dataset path does not exist"
            },
        ));
    }

    let transforms_path = dataset_dir.join("transforms.json");
    if !transforms_path.is_file() {
        return Err(err_path(
            ErrorCode::NotFound,
            &transforms_path.display().to_string(),
            "NerfstudioParser: transforms.json does not exist",
        ));
    }

    let text = read_capped_text(&transforms_path, &mut mem, cancel)?;
    cp.poll_now()?;
    let meta: Value = serde_json::from_str(&text).map_err(|e| {
        err(
            ErrorCode::MalformedRecord,
            "transforms_json",
            format!("NerfstudioParser: JSON parse error: {e}"),
        )
    })?;

    let mut loaded = load_nerfstudio_meta_value(
        &meta,
        dataset_dir,
        cfg,
        cancel,
        memory_budget_bytes.saturating_sub(mem.used()),
    )?;
    loaded.recon_dir = transforms_path.clone();
    loaded.summary.recon_dir = transforms_path.display().to_string();
    Ok(loaded)
}

/// Shared back-end: consume an already-built transforms.json-shaped meta
/// (Nerfstudio file or Metashape conversion).
pub fn load_nerfstudio_meta_value(
    meta: &Value,
    dataset_dir: &Path,
    cfg: &ColmapLoadConfig,
    cancel: Option<&CancelToken>,
    memory_budget_bytes: u64,
) -> Result<ParsedDataset, spirula_core::DomainError> {
    let mut cp = ParseCancel::new(cancel);
    let mut mem = ParseMem::with_cap(memory_budget_bytes);
    cp.poll_now()?;

    check_applied_transform(meta)?;

    let jframes = meta
        .get("frames")
        .and_then(|v| v.as_array())
        .ok_or_else(|| {
            err(
                ErrorCode::MalformedRecord,
                "frames",
                "NerfstudioParser: no frames in transforms.json",
            )
        })?;
    if jframes.is_empty() {
        return Err(err(
            ErrorCode::MalformedRecord,
            "frames",
            "NerfstudioParser: no frames in transforms.json",
        ));
    }

    let mut builds = Vec::new();
    let mut missing = Vec::new();
    for fr in jframes {
        cp.tick(1)?;
        let Some(fp) = fr.get("file_path").and_then(|v| v.as_str()) else {
            continue;
        };
        if Path::new(fp)
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("webp"))
        {
            continue;
        }
        safe_rel_path(fp)?;
        let abs = dataset_dir.join(fp);
        if cfg.require_image_files && !abs.is_file() {
            missing.push(abs.display().to_string());
            continue;
        }
        let sort_key = path_sort_key(&abs);
        mem.charge_str_bytes(sort_key.len(), "sort_key")?;
        mem.charge_str_bytes(abs.as_os_str().len(), "path")?;
        builds.push(FrameBuild {
            frame: fr.clone(),
            path: abs,
            sort_key,
            file_path: fp.to_owned(),
        });
    }
    if builds.is_empty() {
        if !missing.is_empty() {
            return Err(err_path(
                ErrorCode::NotFound,
                &missing[0],
                format!(
                    "NerfstudioParser: no usable frames (first missing: {}); check --image-dir / paths",
                    missing[0]
                ),
            ));
        }
        return Err(err(
            ErrorCode::MalformedRecord,
            "frames",
            "NerfstudioParser: no usable frames",
        ));
    }
    builds.sort_by(|a, b| a.sort_key.cmp(&b.sort_key));
    ensure_count(builds.len() as u64, MAX_IMAGES, "images")?;

    // Seed points (optional).
    let points = if let Some(ply_rel) = meta.get("ply_file_path").and_then(|v| v.as_str()) {
        safe_rel_path(ply_rel)?;
        let ply_path = dataset_dir.join(ply_rel);
        if cfg.require_image_files || ply_path.is_file() {
            read_ply_points_budgeted(
                &ply_path,
                cancel,
                memory_budget_bytes.saturating_sub(mem.used()),
            )?
        } else {
            ColmapPoints3D::default()
        }
    } else {
        let mut found = None;
        for cand in ["sparse_pc.ply", "pointcloud.ply"] {
            let p = dataset_dir.join(cand);
            if p.is_file() {
                found = Some(p);
                break;
            }
        }
        if let Some(p) = found {
            read_ply_points_budgeted(&p, cancel, memory_budget_bytes.saturating_sub(mem.used()))?
        } else {
            ColmapPoints3D::default()
        }
    };
    cp.poll_now()?;

    // One synthetic camera id per distinct baked intrinsics key would be ideal;
    // for correctness assign one camera per frame (NS often repeats intrinsics).
    ensure_count(builds.len() as u64, MAX_CAMERAS, "cameras")?;
    let mut cameras = BTreeMap::new();
    let mut names = Vec::new();
    try_reserve_elems(&mut names, builds.len(), &mut mem, "names")?;
    for b in &builds {
        cp.tick(1)?;
        let s = b.path.display().to_string();
        mem.charge_str_bytes(s.len(), "names")?;
        names.push(s);
    }
    let plan = compute_splits(&names, &cfg.split_config())?;
    mem.charge_elems::<usize>(plan.emit.len(), "split_emit")?;
    mem.charge_elems::<usize>(plan.train.len(), "split_train")?;
    mem.charge_elems::<usize>(plan.eval.len(), "split_eval")?;
    mem.charge_elems::<usize>(plan.validation.len(), "split_validation")?;

    let mut frames = Vec::new();
    try_reserve_elems(&mut frames, plan.emit.len(), &mut mem, "frames")?;
    for (emit_i, &bi) in plan.emit.iter().enumerate() {
        cp.tick(1)?;
        let b = &builds[bi];
        let fr = &b.frame;
        let mut fx = frame_or_meta_num(fr, meta, "fl_x", true)?;
        let fy = frame_or_meta_num(fr, meta, "fl_y", true)?;
        let cx = frame_or_meta_num(fr, meta, "cx", true)?;
        let cy = frame_or_meta_num(fr, meta, "cy", true)?;
        let w = frame_or_meta_num(fr, meta, "w", true)?;
        let h = frame_or_meta_num(fr, meta, "h", true)?;
        if ![fx, fy, cx, cy, w, h].iter().all(|v| v.is_finite()) {
            return Err(err(
                ErrorCode::ValueOutOfRange,
                "intrinsics",
                "NerfstudioParser: non-finite camera field",
            ));
        }
        let model_name = frame_or_meta_str(fr, meta, "camera_model").unwrap_or("OPENCV");
        // Validate known COLMAP/NS model name early.
        spirula_core::CameraModel::from_name(model_name).map_err(|_| {
            err(
                ErrorCode::UnsupportedFormat,
                "camera_model",
                format!("NerfstudioParser: unsupported camera model {model_name}"),
            )
        })?;

        let rd = RawDist {
            k1: frame_or_meta_num(fr, meta, "k1", false)?,
            k2: frame_or_meta_num(fr, meta, "k2", false)?,
            k3: frame_or_meta_num(fr, meta, "k3", false)?,
            k4: frame_or_meta_num(fr, meta, "k4", false)?,
            k5: frame_or_meta_num(fr, meta, "k5", false)?,
            k6: frame_or_meta_num(fr, meta, "k6", false)?,
            p1: frame_or_meta_num(fr, meta, "p1", false)?,
            p2: frame_or_meta_num(fr, meta, "p2", false)?,
            sx1: frame_or_meta_num(fr, meta, "sx1", false)?,
            sy1: frame_or_meta_num(fr, meta, "sy1", false)?,
            b1: frame_or_meta_num(fr, meta, "b1", false)?,
            b2: frame_or_meta_num(fr, meta, "b2", false)?,
        };
        let skew_px = rd.b2 * fx;
        if rd.b1 != 0.0 {
            fx *= 1.0 + rd.b1;
        }

        let hint = frame_or_meta_str(fr, meta, "camera_distortion").unwrap_or("");
        let hint_rational = hint == "RATIONAL";
        let hint_known =
            hint_rational || hint.is_empty() || matches!(hint, "THIN_PRISM" | "OPENCV" | "NONE");
        let thin_prism_shaped = frame_or_meta_has(fr, meta, "b1")
            || frame_or_meta_has(fr, meta, "b2")
            || frame_or_meta_has(fr, meta, "sx1")
            || frame_or_meta_has(fr, meta, "sy1");
        let pinhole_model = matches!(
            model_name,
            "PINHOLE" | "SIMPLE_PINHOLE" | "SIMPLE_RADIAL" | "RADIAL" | "OPENCV" | "FULL_OPENCV"
        );
        let fisheye = !pinhole_model && model_name != "EQUIRECTANGULAR";
        let rational = (hint_rational
            || (!hint_known
                && !fisheye
                && (rd.k5 != 0.0 || rd.k6 != 0.0 || (rd.k4 != 0.0 && !thin_prism_shaped))))
            && (rd.k4 != 0.0 || rd.k5 != 0.0 || rd.k6 != 0.0);

        let cam_id = (emit_i as i32) + 1;
        let width_u = w as u64;
        let height_u = h as u64;
        let cam = build_colmap_camera(BuildCam {
            camera_id: cam_id,
            model_name,
            width: width_u,
            height: height_u,
            fx,
            fy,
            cx,
            cy,
            rd: &rd,
            rational,
            thin_prism_shaped,
            skew_px,
        })?;
        let baked = bake_and_fit_cancelable(&cam, cancel)?;
        let width = u32::try_from(cam.width).map_err(|_| {
            err(
                ErrorCode::ValueOutOfRange,
                "width",
                format!("width {} exceeds u32", cam.width),
            )
        })?;
        let height = u32::try_from(cam.height).map_err(|_| {
            err(
                ErrorCode::ValueOutOfRange,
                "height",
                format!("height {} exceeds u32", cam.height),
            )
        })?;
        cameras.insert(cam_id, cam);

        let c2w = read_c2w12(fr)?;
        frames.push(LoadedFrame {
            image_id: cam_id,
            camera_id: cam_id,
            name: b.file_path.clone(),
            path: b.path.clone(),
            c2w,
            width,
            height,
            intrins: [baked.fx, baked.fy, baked.cx, baked.cy],
            model: baked.model,
            distortion: baked.distortion,
            dist_coeffs: baked.dist,
            redistort: baked.source,
            observations: Vec::new(),
        });
    }

    // Remap split indices: plan already indexes emit domain == frames.
    let split = SplitIndices {
        train: plan.train,
        eval: plan.eval,
        validation: plan.validation,
    };

    let summary = DatasetLoadSummary {
        path: dataset_dir.display().to_string(),
        format: DatasetFormat::Nerfstudio.as_str().into(),
        recon_dir: dataset_dir.display().to_string(),
        num_cameras: cameras.len() as u64,
        num_images: frames.len() as u64,
        num_points: points.num() as u64,
        num_train: split.train.len() as u64,
        num_eval: split.eval.len() as u64,
        num_validation: split.validation.len() as u64,
    };

    Ok(ParsedDataset {
        dataset_dir: dataset_dir.to_path_buf(),
        recon_dir: dataset_dir.to_path_buf(),
        format: DatasetFormat::Nerfstudio,
        cameras,
        frames,
        points,
        split,
        summary,
    })
}
