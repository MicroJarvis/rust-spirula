//! Nerfstudio `transforms.json` reader (P3-F04).
//!
//! OpenGL c2w from `transform_matrix`; path-safe `file_path`; optional PLY via
//! [`crate::ply`]. Scene center / normalize deferred to P3-F05 — poses stay as
//! written after format conversion (including baked `applied_transform`).
//! Singular `applied_transform` still rejected.
//!
//! Pin: `NerfstudioParser.cpp` @ `9a24f099…`.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

use serde_json::Value;
use spirula_core::{
    CameraModel, CancelToken, DISTORTION_PARAMS, DistortionTier, ErrorCode, FitOptions,
    K_EXACT_FIT_PX, MAX_PARAMS, SKEW_BASE_EQUISOLID, SKEW_BASE_FISHEYE, SKEW_BASE_PERSPECTIVE,
    SKEW_RADIAL_POLYNOMIAL, SKEW_RADIAL_RATIONAL, SKEWED, demote_distortion, fit_camera,
    fit_camera_auto_cancelable, source_project,
};

use crate::cancel_checkpoint::CANCEL_WORK_CHUNK;
use crate::colmap::budget::{
    MAX_CAMERAS, MAX_COLMAP_FILE_BYTES, MAX_IMAGES, MAX_LOAD_MEMORY_BYTES, ParseCancel, ParseMem,
    ensure_count, err, err_path, try_reserve_elems,
};
use crate::colmap::load::validate_image_name;
use crate::colmap::split::compute_splits;
use crate::colmap::types::RedistortSource;
use crate::colmap::{
    ColmapCamera, ColmapLoadConfig, ColmapPoints3D, DatasetFormat, DatasetLoadSummary, LoadedFrame,
    ParsedDataset, SplitIndices,
};
use crate::ply::read_ply_points_mem;

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

/// Baked NS frame intrinsics (pin `NerfstudioParser.cpp:650-745`).
struct NsBaked {
    model: CameraModel,
    distortion: DistortionTier,
    fx: f32,
    fy: f32,
    cx: f32,
    cy: f32,
    dist: [f32; DISTORTION_PARAMS],
    redistort: RedistortSource,
}

fn finite_f32(v: f64, field: &str) -> Result<f32, spirula_core::DomainError> {
    if !v.is_finite() {
        return Err(err(
            ErrorCode::ValueOutOfRange,
            field,
            format!("NerfstudioParser: non-finite {field}"),
        ));
    }
    Ok(v as f32)
}

fn positive_f32(v: f64, field: &str) -> Result<f32, spirula_core::DomainError> {
    let f = finite_f32(v, field)?;
    if f <= 0.0 {
        return Err(err(
            ErrorCode::ValueOutOfRange,
            field,
            format!("NerfstudioParser: non-positive {field}"),
        ));
    }
    Ok(f)
}

fn hint_is_known(hint: &str) -> bool {
    // Empty string is NOT known (pin: `camera_distortion_from_name` fails).
    hint == "RATIONAL" || DistortionTier::from_name(hint).is_ok()
}

fn skew_base(model: CameraModel) -> f32 {
    match model {
        CameraModel::Fisheye => SKEW_BASE_FISHEYE,
        CameraModel::Equisolid => SKEW_BASE_EQUISOLID,
        CameraModel::Pinhole | CameraModel::Equirectangular => SKEW_BASE_PERSPECTIVE,
    }
}

struct FitLensArgs<'a> {
    model: CameraModel,
    rational: bool,
    fx: f64,
    fy: f64,
    cx: f64,
    cy: f64,
    skew_px: f64,
    raw: &'a [f32; DISTORTION_PARAMS],
    width: u32,
    height: u32,
    cancel: Option<&'a CancelToken>,
}

/// Fit skewed / rational lenses onto a compiled tier (pin `fit_unsupported_lens`).
fn fit_unsupported_lens(args: FitLensArgs<'_>) -> Result<NsBaked, spirula_core::DomainError> {
    let FitLensArgs {
        model,
        rational,
        fx,
        fy,
        cx,
        cy,
        skew_px,
        raw,
        width,
        height,
        cancel,
    } = args;
    let mut src = RedistortSource {
        source_model: SKEWED,
        params: [0.0; MAX_PARAMS],
        fit_max_px: 0.0,
    };
    src.params[0] = finite_f32(fx, "fx")?;
    src.params[1] = finite_f32(fy, "fy")?;
    src.params[2] = finite_f32(cx, "cx")?;
    src.params[3] = finite_f32(cy, "cy")?;
    src.params[4] = finite_f32(skew_px, "skew_px")?;
    for (i, c) in raw.iter().enumerate() {
        src.params[5 + i] = *c;
    }
    src.params[13] = skew_base(model);
    src.params[14] = if rational {
        SKEW_RADIAL_RATIONAL
    } else {
        SKEW_RADIAL_POLYNOMIAL
    };

    let project = |x: f64, y: f64, z: f64| -> Result<(f64, f64), spirula_core::DomainError> {
        source_project(SKEWED, &src.params, x, y, z)
    };

    // Skew keeps declared model; rational uses auto (pin C++).
    let mut fit = if rational {
        fit_camera_auto_cancelable(&project, width, height, FitOptions::default(), cancel)?
    } else {
        fit_camera(&project, width, height, model, FitOptions::default())?
    };
    if !fit.invertible || fit.samples == 0 {
        fit = fit_camera_auto_cancelable(&project, width, height, FitOptions::default(), cancel)?;
    }

    let mut coeffs = [0.0f64; DISTORTION_PARAMS];
    coeffs.copy_from_slice(&fit.target.coeffs);
    let (tier, demoted) = demote_distortion(fit.target.distortion, &coeffs)?;
    let mut dist = [0.0f32; DISTORTION_PARAMS];
    for (i, v) in demoted.iter().enumerate() {
        dist[i] = finite_f32(*v, "dist")?;
    }

    let mut redistort = RedistortSource::default();
    if fit.max_px >= K_EXACT_FIT_PX {
        redistort = src;
        redistort.fit_max_px = finite_f32(fit.max_px, "fit_max_px")?;
    }

    Ok(NsBaked {
        model: fit.target.model,
        distortion: tier,
        fx: positive_f32(fit.target.fx, "fx")?,
        fy: positive_f32(fit.target.fy, "fy")?,
        cx: finite_f32(fit.target.cx, "cx")?,
        cy: finite_f32(fit.target.cy, "cy")?,
        dist,
        redistort,
    })
}

struct BakeLensArgs<'a> {
    model_name: &'a str,
    fx: f64,
    fy: f64,
    cx: f64,
    cy: f64,
    w: f64,
    h: f64,
    rd: &'a RawDist,
    hint: &'a str,
    thin_prism_shaped: bool,
    cancel: Option<&'a CancelToken>,
}

/// Pin `NerfstudioParser.cpp:650-745` — preserve declared projection model;
/// do not remap through fake COLMAP model names.
fn bake_nerfstudio_lens(args: BakeLensArgs<'_>) -> Result<NsBaked, spirula_core::DomainError> {
    let BakeLensArgs {
        model_name,
        mut fx,
        fy,
        cx,
        cy,
        w,
        h,
        rd,
        hint,
        thin_prism_shaped,
        cancel,
    } = args;
    let model = CameraModel::from_name(model_name).map_err(|_| {
        err(
            ErrorCode::UnsupportedFormat,
            "camera_model",
            format!("NerfstudioParser: unsupported camera model {model_name}"),
        )
    })?;
    if !(w.is_finite() && h.is_finite()) || w <= 0.0 || h <= 0.0 {
        return Err(err(
            ErrorCode::ValueOutOfRange,
            "resolution",
            "NerfstudioParser: non-positive width/height",
        ));
    }
    let width = w as u32;
    let height = h as u32;
    if width == 0 || height == 0 {
        return Err(err(
            ErrorCode::ValueOutOfRange,
            "resolution",
            "NerfstudioParser: non-positive width/height",
        ));
    }

    // Affinity folds into fx; skew is off-diagonal (handled by fit when nonzero).
    let skew_px = rd.b2 * fx;
    if rd.b1 != 0.0 {
        fx *= 1.0 + rd.b1;
    }

    let hint_rational = hint == "RATIONAL";
    let hint_known = hint_is_known(hint);
    // C++: fisheye = (model != PINHOLE). OPENCV/PINHOLE names → Pinhole.
    let fisheye = model != CameraModel::Pinhole;
    let rational = (hint_rational
        || (!hint_known
            && !fisheye
            && (rd.k5 != 0.0 || rd.k6 != 0.0 || (rd.k4 != 0.0 && !thin_prism_shaped))))
        && (rd.k4 != 0.0 || rd.k5 != 0.0 || rd.k6 != 0.0);

    let mut raw = [0.0f32; DISTORTION_PARAMS];
    if rational {
        raw[0] = finite_f32(rd.k1, "k1")?;
        raw[1] = finite_f32(rd.k2, "k2")?;
        raw[2] = finite_f32(rd.k3, "k3")?;
        raw[3] = finite_f32(rd.k4, "k4")?;
        raw[4] = finite_f32(rd.k5, "k5")?;
        raw[5] = finite_f32(rd.k6, "k6")?;
        raw[6] = finite_f32(rd.p1, "p1")?;
        raw[7] = finite_f32(rd.p2, "p2")?;
    } else {
        raw[0] = finite_f32(rd.k1, "k1")?;
        raw[1] = finite_f32(rd.k2, "k2")?;
        raw[2] = finite_f32(rd.k3, "k3")?;
        raw[3] = finite_f32(rd.k4, "k4")?;
        raw[4] = finite_f32(rd.p1, "p1")?;
        raw[5] = finite_f32(rd.p2, "p2")?;
        raw[6] = finite_f32(rd.sx1, "sx1")?;
        raw[7] = finite_f32(rd.sy1, "sy1")?;
    }

    if model == CameraModel::Equirectangular {
        // Canonical panorama intrinsics; no lens / skew.
        let fx_e = w / (2.0 * std::f64::consts::PI);
        let fy_e = h / std::f64::consts::PI;
        return Ok(NsBaked {
            model: CameraModel::Equirectangular,
            distortion: DistortionTier::None,
            fx: positive_f32(fx_e, "fx")?,
            fy: positive_f32(fy_e, "fy")?,
            cx: finite_f32(w / 2.0, "cx")?,
            cy: finite_f32(h / 2.0, "cy")?,
            dist: [0.0; DISTORTION_PARAMS],
            redistort: RedistortSource::default(),
        });
    }

    if skew_px != 0.0 || rational {
        return fit_unsupported_lens(FitLensArgs {
            model,
            rational,
            fx,
            fy,
            cx,
            cy,
            skew_px,
            raw: &raw,
            width,
            height,
            cancel,
        });
    }

    // Direct raw path: keep declared model; demote tier from ThinPrism.
    let mut coeffs = [0.0f64; DISTORTION_PARAMS];
    for (i, c) in raw.iter().enumerate() {
        coeffs[i] = f64::from(*c);
    }
    let (tier, demoted) = demote_distortion(DistortionTier::ThinPrism, &coeffs)?;
    let mut dist = [0.0f32; DISTORTION_PARAMS];
    for (i, v) in demoted.iter().enumerate() {
        dist[i] = finite_f32(*v, "dist")?;
    }
    Ok(NsBaked {
        model,
        distortion: tier,
        fx: positive_f32(fx, "fx")?,
        fy: positive_f32(fy, "fy")?,
        cx: finite_f32(cx, "cx")?,
        cy: finite_f32(cy, "cy")?,
        dist,
        redistort: RedistortSource::default(),
    })
}

fn model_colmap_name(model: CameraModel) -> &'static str {
    match model {
        CameraModel::Pinhole => "PINHOLE",
        CameraModel::Fisheye => "FISHEYE",
        CameraModel::Equisolid => "EQUISOLID",
        CameraModel::Equirectangular => "EQUIRECTANGULAR",
    }
}

struct FrameBuild {
    /// Index into the original `frames` JSON array (avoids cloning the Value).
    frame_idx: usize,
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
    let meta = parse_json_dom_budgeted(&text, &mut mem, cancel)?;

    let mut loaded = load_nerfstudio_meta_value_mem(&meta, dataset_dir, cfg, cancel, &mut mem)?;
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
    let mut mem = ParseMem::with_cap(memory_budget_bytes);
    // Caller-owned meta DOM: charge a conservative live-size estimate.
    mem.charge(estimate_json_bytes(meta), "json_dom")?;
    load_nerfstudio_meta_value_mem(meta, dataset_dir, cfg, cancel, &mut mem)
}

pub(crate) fn estimate_json_bytes(v: &Value) -> u64 {
    match v {
        Value::Null | Value::Bool(_) => 8,
        Value::Number(_) => 16,
        Value::String(s) => s.len() as u64 + 16,
        Value::Array(a) => a
            .iter()
            .map(estimate_json_bytes)
            .fold(16u64, |acc, n| acc.saturating_add(n).saturating_add(8)),
        Value::Object(m) => m.iter().fold(16u64, |acc, (k, val)| {
            acc.saturating_add(k.len() as u64)
                .saturating_add(16)
                .saturating_add(estimate_json_bytes(val))
        }),
    }
}

/// Conservative upper bound on live `serde_json::Value` size for a UTF-8 JSON
/// document of `text_len` bytes.
///
/// Charge **before** DOM materialization. Independent counterexample: an unused
/// array of `131073` zeros (`text_len≈262329`) yields `Vec` capacity `262144`
/// and array-only bytes `capacity * size_of::<Value>() = 8_388_608`, which
/// exceeds any `24 * len + 256` claim. Bound therefore assumes:
/// - ≤1 `Value` slot per input byte with **2×** capacity rounding
///   (`2 * size_of::<Value>() * len`);
/// - another `size_of::<Value>() * len` for object/map entry Values / nesting;
/// - `len` bytes for decoded string/key payloads (≤ source UTF-8);
/// - fixed deserializer scratch.
///
/// File bytes are charged separately by [`read_capped_text`].
pub(crate) fn json_dom_budget_cap(text_len: usize) -> u64 {
    let n = text_len as u64;
    let value = std::mem::size_of::<serde_json::Value>() as u64;
    let array_slots = n.saturating_mul(value.saturating_mul(2));
    let containers = n.saturating_mul(value);
    let payloads = n;
    let scratch = 4096u64;
    array_slots
        .saturating_add(containers)
        .saturating_add(payloads)
        .saturating_add(scratch)
}

/// Charge [`json_dom_budget_cap`] **before** allocating the DOM, then parse via
/// `serde_json::from_reader` with per-byte cancel ticks (IoRead pulls one byte
/// at a time, so after-start cancel is observed during parse — not only
/// post-parse polling).
fn parse_json_dom_budgeted(
    text: &str,
    mem: &mut ParseMem,
    cancel: Option<&CancelToken>,
) -> Result<Value, spirula_core::DomainError> {
    mem.charge(json_dom_budget_cap(text.len()), "json_dom")?;
    if let Some(c) = cancel {
        c.check()?;
    }

    struct CancelByteReader<'a> {
        data: &'a [u8],
        pos: usize,
        cancel: Option<&'a CancelToken>,
        pending: usize,
        canceled: Option<spirula_core::DomainError>,
    }

    impl Read for CancelByteReader<'_> {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            if self.pos >= self.data.len() {
                return Ok(0);
            }
            // One byte per read matches serde_json IoRead::bytes() so cancel
            // ticks advance in lockstep with the deserializer.
            let n = 1.min(buf.len());
            buf[0] = self.data[self.pos];
            self.pos += 1;
            self.pending = self.pending.saturating_add(n);
            while self.pending >= CANCEL_WORK_CHUNK {
                self.pending -= CANCEL_WORK_CHUNK;
                if let Some(token) = self.cancel
                    && let Err(e) = token.check()
                {
                    self.canceled = Some(e);
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::Interrupted,
                        "parse canceled",
                    ));
                }
            }
            Ok(n)
        }
    }

    let mut reader = CancelByteReader {
        data: text.as_bytes(),
        pos: 0,
        cancel,
        pending: 0,
        canceled: None,
    };
    match serde_json::from_reader(&mut reader) {
        Ok(v) => Ok(v),
        Err(_) => {
            if let Some(e) = reader.canceled {
                return Err(e);
            }
            Err(err(
                ErrorCode::MalformedRecord,
                "transforms_json",
                "NerfstudioParser: JSON parse error",
            ))
        }
    }
}

pub(crate) fn load_nerfstudio_meta_value_mem(
    meta: &Value,
    dataset_dir: &Path,
    cfg: &ColmapLoadConfig,
    cancel: Option<&CancelToken>,
    mem: &mut ParseMem,
) -> Result<ParsedDataset, spirula_core::DomainError> {
    let mut cp = ParseCancel::new(cancel);
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
    try_reserve_elems(&mut builds, jframes.len(), mem, "builds")?;
    let mut missing = Vec::new();
    for (frame_idx, fr) in jframes.iter().enumerate() {
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
            let s = abs.display().to_string();
            mem.charge_str_bytes(s.len(), "missing")?;
            missing.push(s);
            continue;
        }
        let sort_key = path_sort_key(&abs);
        mem.charge_str_bytes(sort_key.len(), "sort_key")?;
        mem.charge_str_bytes(abs.as_os_str().len(), "path")?;
        mem.charge_str_bytes(fp.len(), "file_path")?;
        mem.charge_elems::<FrameBuild>(1, "builds")?;
        builds.push(FrameBuild {
            frame_idx,
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

    // Seed points — charge into the same cumulative budget (not a disposable sub-cap).
    let points = if let Some(ply_rel) = meta.get("ply_file_path").and_then(|v| v.as_str()) {
        safe_rel_path(ply_rel)?;
        let ply_path = dataset_dir.join(ply_rel);
        if cfg.require_image_files || ply_path.is_file() {
            read_ply_points_mem(&ply_path, cancel, mem)?
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
            read_ply_points_mem(&p, cancel, mem)?
        } else {
            ColmapPoints3D::default()
        }
    };
    cp.poll_now()?;

    ensure_count(builds.len() as u64, MAX_CAMERAS, "cameras")?;
    let mut cameras = BTreeMap::new();
    let mut names = Vec::new();
    try_reserve_elems(&mut names, builds.len(), mem, "names")?;
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
    try_reserve_elems(&mut frames, plan.emit.len(), mem, "frames")?;
    for (emit_i, &bi) in plan.emit.iter().enumerate() {
        cp.tick(1)?;
        let b = &builds[bi];
        let fr = &jframes[b.frame_idx];
        let fx = frame_or_meta_num(fr, meta, "fl_x", true)?;
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
        let hint = frame_or_meta_str(fr, meta, "camera_distortion").unwrap_or("");
        let thin_prism_shaped = frame_or_meta_has(fr, meta, "b1")
            || frame_or_meta_has(fr, meta, "b2")
            || frame_or_meta_has(fr, meta, "sx1")
            || frame_or_meta_has(fr, meta, "sy1");

        let baked = bake_nerfstudio_lens(BakeLensArgs {
            model_name,
            fx,
            fy,
            cx,
            cy,
            w,
            h,
            rd: &rd,
            hint,
            thin_prism_shaped,
            cancel,
        })?;

        let cam_id = (emit_i as i32) + 1;
        let width_u = w as u64;
        let height_u = h as u64;
        let cam = ColmapCamera {
            camera_id: cam_id,
            model: model_colmap_name(baked.model).into(),
            width: width_u,
            height: height_u,
            params: if baked.model == CameraModel::Equirectangular {
                vec![w, h]
            } else {
                vec![
                    f64::from(baked.fx),
                    f64::from(baked.fy),
                    f64::from(baked.cx),
                    f64::from(baked.cy),
                ]
            },
        };
        mem.charge_btree_entries::<(i32, ColmapCamera)>(1, "cameras")?;
        mem.charge_str_bytes(cam.model.len(), "cameras")?;
        mem.charge_elems::<f64>(cam.params.len(), "cameras")?;
        cameras.insert(cam_id, cam);

        let width = u32::try_from(width_u).map_err(|_| {
            err(
                ErrorCode::ValueOutOfRange,
                "width",
                format!("width {width_u} exceeds u32"),
            )
        })?;
        let height = u32::try_from(height_u).map_err(|_| {
            err(
                ErrorCode::ValueOutOfRange,
                "height",
                format!("height {height_u} exceeds u32"),
            )
        })?;

        let c2w = read_c2w12(fr)?;
        mem.charge_str_bytes(b.file_path.len(), "frame_name")?;
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
            redistort: baked.redistort,
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
        center_mode: String::new(),
        center: [0.0; 3],
        center_json: [0.0; 3],
        raw_to_file: [0.0; 16],
        train_frame_scale: 0.0,
        train_to_normalized: [0.0; 16],
        normalized_rotation: [0.0; 9],
    }
    .with_identity_scene())
}
