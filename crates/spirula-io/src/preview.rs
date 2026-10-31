//! P4-F06: versioned preview request load, RGB8 PNG encode, atomic publish.
//!
//! Descriptor bytes are bounded by a fixed hardcap **before** parse. The
//! configured `max_file_bytes` stays independent (default 32 MiB for scene
//! files); the descriptor effective limit is
//! `min(max_file_bytes, PREVIEW_REQUEST_HARDCAP_BYTES)`. Scene PLY reads use
//! `max_file_bytes` as a separate on-disk cap from the host parse budget.
//! Unknown JSON keys are typed-rejected (no silent ignore of distortion /
//! primitive / sh_degree / color_space). Relative paths resolve against the
//! request file parent. Product PNG is opaque RGB8.

use std::fs::File;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

use image::ColorType;
use serde_json::Value;
use spirula_core::{
    CameraPose, CancelToken, ContextKey, DomainError, ErrorCode, Mat3, OpenCvDistortion,
    PinholeCamera, SplatSet, Vec3,
};

use crate::atomic::{PublishOptions, PublishOutcome, publish_bytes_validated};
use crate::ply::read_ply_splats_budgeted_with_file_cap;

pub const PREVIEW_REQUEST_SCHEMA: &str = "spirula-preview-request";
pub const PREVIEW_REQUEST_VERSION: u32 = 1;

/// Fixed descriptor hardcap (read bound). User `max_file_bytes` cannot raise it.
pub const PREVIEW_REQUEST_HARDCAP_BYTES: u64 = 1 << 20; // 1 MiB

const DEFAULT_MAX_PIXELS: u64 = 480_000;
const DEFAULT_MAX_HOST_BYTES: u64 = 256 << 20;
const DEFAULT_MAX_GPU_BYTES: u64 = 256 << 20;
const DEFAULT_MAX_FILE_BYTES: u64 = 32 << 20;
const DEFAULT_MAX_IMAGE_BYTES: u64 = 12 << 20;
const DEFAULT_ALPHA_CUTOFF: f64 = 0.01;
const DEFAULT_EARLY_OUT: f64 = 1e-4;

const ROOT_KEYS: &[&str] = &[
    "schema",
    "version",
    "splat_ply",
    "camera",
    "background",
    "thresholds",
    "budgets",
];
const CAMERA_KEYS: &[&str] = &["model", "width", "height", "fx", "fy", "cx", "cy", "c2w"];
const C2W_KEYS: &[&str] = &["rotation_row_major", "translation"];
const THRESHOLD_KEYS: &[&str] = &["alpha_cutoff", "early_out_transmittance"];
const BUDGET_KEYS: &[&str] = &[
    "max_pixels",
    "max_host_bytes",
    "max_gpu_bytes",
    "max_file_bytes",
    "max_image_bytes",
];

fn err(code: ErrorCode, field: &str, message: impl Into<String>) -> DomainError {
    DomainError::try_new(code, message)
        .expect("message")
        .with_context(ContextKey::Field, field)
        .expect("field")
}

fn err_path(code: ErrorCode, path: &Path, message: impl Into<String>) -> DomainError {
    DomainError::try_new(code, message)
        .expect("message")
        .with_context(ContextKey::Path, path.display().to_string())
        .expect("path")
}

fn reject_unknown(
    obj: &serde_json::Map<String, Value>,
    allowed: &[&str],
    where_: &str,
) -> Result<(), DomainError> {
    for key in obj.keys() {
        if !allowed.contains(&key.as_str()) {
            return Err(err(
                ErrorCode::MalformedRecord,
                where_,
                format!("unknown field `{key}` in {where_}"),
            ));
        }
    }
    Ok(())
}

/// Resource budgets for one preview call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PreviewBudgets {
    pub max_pixels: u64,
    pub max_host_bytes: u64,
    pub max_gpu_bytes: u64,
    pub max_file_bytes: u64,
    pub max_image_bytes: u64,
}

impl Default for PreviewBudgets {
    fn default() -> Self {
        Self {
            max_pixels: DEFAULT_MAX_PIXELS,
            max_host_bytes: DEFAULT_MAX_HOST_BYTES,
            max_gpu_bytes: DEFAULT_MAX_GPU_BYTES,
            max_file_bytes: DEFAULT_MAX_FILE_BYTES,
            max_image_bytes: DEFAULT_MAX_IMAGE_BYTES,
        }
    }
}

/// Validated preview request (paths resolved).
#[derive(Clone, Debug)]
pub struct PreviewRequestV1 {
    pub request_path: PathBuf,
    pub splat_ply: PathBuf,
    pub camera: PinholeCamera,
    pub background: [f64; 3],
    pub alpha_cutoff: f64,
    pub early_out_transmittance: f64,
    pub budgets: PreviewBudgets,
}

/// Loaded scene ready for render (PLY already decoded).
#[derive(Debug)]
pub struct LoadedPreviewScene {
    pub request: PreviewRequestV1,
    pub splats: SplatSet,
    pub wall_load_ns: u128,
}

fn require_object<'a>(
    v: &'a Value,
    field: &str,
) -> Result<&'a serde_json::Map<String, Value>, DomainError> {
    v.as_object().ok_or_else(|| {
        err(
            ErrorCode::MalformedRecord,
            field,
            format!("{field} must be object"),
        )
    })
}

fn get<'a>(
    obj: &'a serde_json::Map<String, Value>,
    key: &str,
    field: &str,
) -> Result<&'a Value, DomainError> {
    obj.get(key)
        .ok_or_else(|| err(ErrorCode::MalformedRecord, field, format!("missing {key}")))
}

fn as_u64(v: &Value, field: &str) -> Result<u64, DomainError> {
    v.as_u64().ok_or_else(|| {
        err(
            ErrorCode::MalformedRecord,
            field,
            format!("{field} must be u64"),
        )
    })
}

fn as_f64(v: &Value, field: &str) -> Result<f64, DomainError> {
    let x = v.as_f64().ok_or_else(|| {
        err(
            ErrorCode::MalformedRecord,
            field,
            format!("{field} must be number"),
        )
    })?;
    if !x.is_finite() {
        return Err(err(
            ErrorCode::ValueOutOfRange,
            field,
            format!("{field} nonfinite"),
        ));
    }
    Ok(x)
}

fn as_str<'a>(v: &'a Value, field: &str) -> Result<&'a str, DomainError> {
    v.as_str().ok_or_else(|| {
        err(
            ErrorCode::MalformedRecord,
            field,
            format!("{field} must be string"),
        )
    })
}

fn read_f64_n(v: &Value, n: usize, field: &str) -> Result<Vec<f64>, DomainError> {
    let a = v.as_array().ok_or_else(|| {
        err(
            ErrorCode::MalformedRecord,
            field,
            format!("{field} must be array"),
        )
    })?;
    if a.len() != n {
        return Err(err(
            ErrorCode::MalformedRecord,
            field,
            format!("{field} length {} != {n}", a.len()),
        ));
    }
    let mut out = Vec::with_capacity(n);
    for (i, cell) in a.iter().enumerate() {
        out.push(as_f64(cell, &format!("{field}[{i}]"))?);
    }
    Ok(out)
}

fn resolve_path(base: &Path, raw: &str) -> Result<PathBuf, DomainError> {
    if raw.is_empty() {
        return Err(err(
            ErrorCode::InvalidArgument,
            "path",
            "path must be non-empty",
        ));
    }
    let p = Path::new(raw);
    if p.is_absolute() {
        Ok(p.to_path_buf())
    } else {
        Ok(base.join(p))
    }
}

fn read_capped(
    path: &Path,
    hardcap: u64,
    cancel: Option<&CancelToken>,
) -> Result<Vec<u8>, DomainError> {
    if let Some(c) = cancel {
        c.check()?;
    }
    let mut file = File::open(path).map_err(|e| {
        err_path(
            ErrorCode::ReadFailed,
            path,
            format!("open preview request: {e}"),
        )
    })?;
    let meta = file.metadata().map_err(|e| {
        err_path(
            ErrorCode::ReadFailed,
            path,
            format!("stat preview request: {e}"),
        )
    })?;
    if !meta.is_file() {
        return Err(err_path(
            ErrorCode::InvalidArgument,
            path,
            "preview request must be a regular file",
        ));
    }
    // Soft hint only; growth after stat is caught by hardcap+1 sentinel below.
    if meta.len() > hardcap {
        return Err(err_path(
            ErrorCode::ValueOutOfRange,
            path,
            format!("preview request exceeds hardcap {hardcap} bytes"),
        ));
    }
    let mut buf = Vec::new();
    let reserve = (hardcap as usize).saturating_add(1);
    buf.try_reserve_exact(reserve.min(meta.len() as usize + 1))
        .map_err(|_| {
            err_path(
                ErrorCode::AllocationFailed,
                path,
                "preview request allocation failed",
            )
        })?;
    // hardcap+1 sentinel: if the file grew past hardcap, we observe > hardcap bytes.
    Read::take(Read::by_ref(&mut file), hardcap.saturating_add(1))
        .read_to_end(&mut buf)
        .map_err(|e| {
            err_path(
                ErrorCode::ReadFailed,
                path,
                format!("read preview request: {e}"),
            )
        })?;
    if buf.len() as u64 > hardcap {
        return Err(err_path(
            ErrorCode::ValueOutOfRange,
            path,
            format!("preview request exceeds hardcap {hardcap} bytes"),
        ));
    }
    if let Some(c) = cancel {
        c.check()?;
    }
    Ok(buf)
}

fn parse_camera(cam: &Value, budgets: PreviewBudgets) -> Result<PinholeCamera, DomainError> {
    let obj = require_object(cam, "camera")?;
    reject_unknown(obj, CAMERA_KEYS, "camera")?;
    let model = as_str(get(obj, "model", "camera.model")?, "camera.model")?;
    if model != "PINHOLE" {
        return Err(err(
            ErrorCode::UnimplementedOption,
            "camera.model",
            format!("unsupported camera model {model}"),
        ));
    }
    let width = as_u64(get(obj, "width", "camera.width")?, "camera.width")?;
    let height = as_u64(get(obj, "height", "camera.height")?, "camera.height")?;
    if width == 0 || height == 0 || width > u64::from(u32::MAX) || height > u64::from(u32::MAX) {
        return Err(err(
            ErrorCode::ValueOutOfRange,
            "camera.size",
            "width/height must be in 1..=u32::MAX",
        ));
    }
    let pixels = width.checked_mul(height).ok_or_else(|| {
        err(
            ErrorCode::ValueOutOfRange,
            "camera.size",
            "width*height overflow",
        )
    })?;
    if pixels > budgets.max_pixels {
        return Err(err(
            ErrorCode::ValueOutOfRange,
            "max_pixels",
            format!("pixels {pixels} exceeds budget {}", budgets.max_pixels),
        ));
    }
    let fx = as_f64(get(obj, "fx", "camera.fx")?, "camera.fx")?;
    let fy = as_f64(get(obj, "fy", "camera.fy")?, "camera.fy")?;
    let cx = as_f64(get(obj, "cx", "camera.cx")?, "camera.cx")?;
    let cy = as_f64(get(obj, "cy", "camera.cy")?, "camera.cy")?;
    if fx <= 0.0 || fy <= 0.0 {
        return Err(err(
            ErrorCode::ValueOutOfRange,
            "camera.intrinsics",
            "fx/fy must be > 0",
        ));
    }
    let c2w = require_object(get(obj, "c2w", "camera.c2w")?, "camera.c2w")?;
    reject_unknown(c2w, C2W_KEYS, "camera.c2w")?;
    let rot = read_f64_n(
        get(c2w, "rotation_row_major", "camera.c2w.rotation")?,
        9,
        "camera.c2w.rotation_row_major",
    )?;
    let mut r = [0.0; 9];
    r.copy_from_slice(&rot);
    let translation = read_f64_n(
        get(c2w, "translation", "camera.c2w.translation")?,
        3,
        "camera.c2w.translation",
    )?;
    // Finite matrix first, then CameraPose::try_new → try_rotation (orthonormal).
    let mat = Mat3::try_from_row_major(r)
        .map_err(|e| err(e.code(), "camera.c2w.rotation", e.message().to_owned()))?;
    let pose = CameraPose::try_new(
        mat,
        Vec3::try_new(translation[0], translation[1], translation[2])?,
    )?;
    PinholeCamera::try_new(
        width as u32,
        height as u32,
        fx,
        fy,
        cx,
        cy,
        OpenCvDistortion::try_new(0.0, 0.0, 0.0, 0.0)?,
        pose,
    )
}

fn parse_budgets(root: &serde_json::Map<String, Value>) -> Result<PreviewBudgets, DomainError> {
    let mut budgets = PreviewBudgets::default();
    let Some(v) = root.get("budgets") else {
        return Ok(budgets);
    };
    let obj = require_object(v, "budgets")?;
    reject_unknown(obj, BUDGET_KEYS, "budgets")?;
    if let Some(x) = obj.get("max_pixels") {
        budgets.max_pixels = as_u64(x, "budgets.max_pixels")?;
    }
    if let Some(x) = obj.get("max_host_bytes") {
        budgets.max_host_bytes = as_u64(x, "budgets.max_host_bytes")?;
    }
    if let Some(x) = obj.get("max_gpu_bytes") {
        budgets.max_gpu_bytes = as_u64(x, "budgets.max_gpu_bytes")?;
    }
    if let Some(x) = obj.get("max_file_bytes") {
        budgets.max_file_bytes = as_u64(x, "budgets.max_file_bytes")?;
    }
    if let Some(x) = obj.get("max_image_bytes") {
        budgets.max_image_bytes = as_u64(x, "budgets.max_image_bytes")?;
    }
    if budgets.max_pixels == 0
        || budgets.max_host_bytes == 0
        || budgets.max_gpu_bytes == 0
        || budgets.max_file_bytes == 0
        || budgets.max_image_bytes == 0
    {
        return Err(err(
            ErrorCode::ValueOutOfRange,
            "budgets",
            "budgets must be > 0",
        ));
    }
    // Keep max_file_bytes as the scene/file budget (default 32 MiB). Descriptor
    // hardcap is applied only at the request-byte check below — do not collapse
    // the two dimensions here.
    Ok(budgets)
}

/// Load and validate a versioned preview request JSON.
pub fn load_preview_request(
    path: &Path,
    cancel: Option<&CancelToken>,
) -> Result<PreviewRequestV1, DomainError> {
    let bytes = read_capped(path, PREVIEW_REQUEST_HARDCAP_BYTES, cancel)?;
    let root: Value = serde_json::from_slice(&bytes).map_err(|e| {
        err_path(
            ErrorCode::MalformedRecord,
            path,
            format!("preview request JSON: {e}"),
        )
    })?;
    let obj = require_object(&root, "preview_request")?;
    reject_unknown(obj, ROOT_KEYS, "preview_request")?;
    let schema = as_str(get(obj, "schema", "schema")?, "schema")?;
    if schema != PREVIEW_REQUEST_SCHEMA {
        return Err(err(
            ErrorCode::UnsupportedVersion,
            "schema",
            format!("expected {PREVIEW_REQUEST_SCHEMA}, got {schema}"),
        ));
    }
    let version = as_u64(get(obj, "version", "version")?, "version")?;
    if version != u64::from(PREVIEW_REQUEST_VERSION) {
        return Err(err(
            ErrorCode::UnsupportedVersion,
            "version",
            format!("expected {PREVIEW_REQUEST_VERSION}, got {version}"),
        ));
    }
    let budgets = parse_budgets(obj)?;
    // Descriptor-only ceiling: min(configured file cap, fixed 1 MiB hardcap).
    let descriptor_cap = budgets.max_file_bytes.min(PREVIEW_REQUEST_HARDCAP_BYTES);
    if bytes.len() as u64 > descriptor_cap {
        return Err(err_path(
            ErrorCode::ValueOutOfRange,
            path,
            format!(
                "preview request {} exceeds descriptor cap {descriptor_cap}",
                bytes.len(),
            ),
        ));
    }
    let base = path.parent().unwrap_or_else(|| Path::new("."));
    let splat_raw = as_str(get(obj, "splat_ply", "splat_ply")?, "splat_ply")?;
    let splat_ply = resolve_path(base, splat_raw)?;
    let camera = parse_camera(get(obj, "camera", "camera")?, budgets)?;
    let bg = read_f64_n(get(obj, "background", "background")?, 3, "background")?;
    let background = [bg[0], bg[1], bg[2]];
    let mut alpha_cutoff = DEFAULT_ALPHA_CUTOFF;
    let mut early_out = DEFAULT_EARLY_OUT;
    if let Some(th) = obj.get("thresholds") {
        let tobj = require_object(th, "thresholds")?;
        reject_unknown(tobj, THRESHOLD_KEYS, "thresholds")?;
        if let Some(v) = tobj.get("alpha_cutoff") {
            alpha_cutoff = as_f64(v, "thresholds.alpha_cutoff")?;
        }
        if let Some(v) = tobj.get("early_out_transmittance") {
            early_out = as_f64(v, "thresholds.early_out_transmittance")?;
        }
    }
    if !(alpha_cutoff.is_finite() && alpha_cutoff > 0.0 && alpha_cutoff <= 1.0) {
        return Err(err(
            ErrorCode::ValueOutOfRange,
            "alpha_cutoff",
            "alpha_cutoff must be in (0,1]",
        ));
    }
    if !(early_out.is_finite() && early_out > 0.0 && early_out <= 1.0) {
        return Err(err(
            ErrorCode::ValueOutOfRange,
            "early_out_transmittance",
            "early_out must be in (0,1]",
        ));
    }
    Ok(PreviewRequestV1 {
        request_path: path.to_path_buf(),
        splat_ply,
        camera,
        background,
        alpha_cutoff,
        early_out_transmittance: early_out,
        budgets,
    })
}

/// Load request + budgeted PLY splats.
pub fn load_preview_scene(
    path: &Path,
    cancel: Option<&CancelToken>,
) -> Result<LoadedPreviewScene, DomainError> {
    let wall = Instant::now();
    let request = load_preview_request(path, cancel)?;
    if let Some(c) = cancel {
        c.check()?;
    }
    // File cap and host parse budget are independent dimensions.
    let splats = read_ply_splats_budgeted_with_file_cap(
        &request.splat_ply,
        cancel,
        request.budgets.max_host_bytes,
        request.budgets.max_file_bytes,
    )?;
    if let Some(c) = cancel {
        c.check()?;
    }
    Ok(LoadedPreviewScene {
        request,
        splats,
        wall_load_ns: wall.elapsed().as_nanos(),
    })
}

/// Fixed-capacity encoded PNG sink: allocates exactly `max` bytes once.
/// Bounds are on **capacity** (`buf.len()`), not only the written `len`.
struct CappedWriter {
    buf: Box<[u8]>,
    len: usize,
}

impl CappedWriter {
    fn try_new(max: usize) -> Result<Self, DomainError> {
        let mut storage = Vec::new();
        storage.try_reserve_exact(max).map_err(|_| {
            err(
                ErrorCode::AllocationFailed,
                "png",
                "png encoder buffer allocation failed",
            )
        })?;
        storage.resize(max, 0);
        Ok(Self {
            buf: storage.into_boxed_slice(),
            len: 0,
        })
    }

    /// Returns encoded bytes. Caller must already have reserved host for the
    /// fixed `max` capacity plus this copy (see [`preview_encode_host_need`]).
    fn into_encoded(self) -> Vec<u8> {
        self.buf[..self.len].to_vec()
    }
}

/// png 0.18.1 `StreamWriter` row scratch: prev_buf + curr_buf + filtered_buf
/// each of length `width*3` (see encoder.rs ~1415–1417).
const PNG_STREAM_ROW_BUFS: u64 = 3;
/// Default IDAT chunk buffer used by `stream_writer_with_size` (png DEFAULT_BUFFER_LENGTH=4KiB).
const PNG_STREAM_CHUNK_CLAIM: u64 = 4 << 10;
/// png 0.18.1 `StreamWriter` + `Compression::Fast` uses `fdeflate::Compressor`
/// (encoder.rs Wrapper::FDeflate), not flate2. Conservative compressor-state
/// claim (not RSS).
const PNG_FDEFLATE_COMPRESSOR_CLAIM: u64 = 64 << 10;
/// png 0.18.1 `UnfilteringBuffer::GROWTH_BYTES` (8 KiB resize steps).
const PNG_UNFILTER_GROWTH_BYTES: u64 = 8 << 10;
/// png 0.18.1 `UnfilteringBuffer::new` initial capacity cap (`min(..., 128 KiB)`).
const PNG_UNFILTER_PREALLOC_CAP: u64 = 128 << 10;
/// fdeflate Huffman tables still live on the PNG zlib path:
/// `litlen Box<[u32;4096]> + dist Box<[u32;512]>` = 18432 B. (Separate from
/// fdeflate's convenience `output.resize(+32KiB)` API — PNG keeps lookback in
/// `UnfilteringBuffer::data_stream`.)
const FDEFLATE_HUFFMAN_TABLES_BYTES: u64 = 18_432;
/// Decoder/reader/chunk state claim (fixed; not RSS).
const PNG_READER_OVERHEAD_BYTES: u64 = 16 << 10;

/// Conservative auditable PNG **decode** scratch for **canonical product**
/// 8-bit RGB PNGs (IHDR+IDAT+IEND only — no ancillary). Not RSS.
///
/// png 0.18.1 `UnfilteringBuffer::new` preallocates
/// `min(align(raw_row * min(height,128)), 128KiB)`, then
/// `with_unfilled_buffer` grows by 8 KiB with Vec geometric capacity. Bound
/// the buffer as `max(2 * (raw_frame + 8KiB), 128KiB)` and add one raw-row
/// prev scratch + Huffman tables + reader overhead.
///
/// This is **not** a total budget for arbitrary PNGs with text/zTXt/iTXt:
/// image 0.25.10 sets `set_ignore_text_chunk(false)`, so decompressed
/// metadata is **not** ignored and is **not** bounded by the compressed
/// container cap in [`decode_preview_png_rgb8`].
pub fn preview_png_decode_scratch_bytes(width: u32, height: u32) -> Result<u64, DomainError> {
    // raw_row = filter byte + width*3 samples (8-bit RGB).
    let raw_row = u64::from(width)
        .checked_mul(3)
        .and_then(|v| v.checked_add(1))
        .ok_or_else(|| err(ErrorCode::ValueOutOfRange, "size", "raw row overflow"))?;
    let raw_frame = raw_row
        .checked_mul(u64::from(height))
        .ok_or_else(|| err(ErrorCode::ValueOutOfRange, "size", "raw frame overflow"))?;
    let grown = raw_frame
        .checked_add(PNG_UNFILTER_GROWTH_BYTES)
        .and_then(|v| v.checked_mul(2))
        .ok_or_else(|| {
            err(
                ErrorCode::ValueOutOfRange,
                "max_host_bytes",
                "PNG unfilter capacity overflow",
            )
        })?;
    let unfilter_buf = grown.max(PNG_UNFILTER_PREALLOC_CAP);
    unfilter_buf
        .checked_add(raw_row) // PrevRow::Scratch / scratch_buffer ≤ one raw row
        .and_then(|v| v.checked_add(FDEFLATE_HUFFMAN_TABLES_BYTES))
        .and_then(|v| v.checked_add(PNG_READER_OVERHEAD_BYTES))
        .ok_or_else(|| {
            err(
                ErrorCode::ValueOutOfRange,
                "max_host_bytes",
                "PNG decode scratch overflow",
            )
        })
}

/// Checked simultaneous host reservation for streaming encode + one PNG validation decode.
///
/// Encode path (png 0.18.1 `StreamWriter` + `Compression::Fast` →
/// `fdeflate::Compressor` into outer [`CappedWriter`]): no full internal
/// `zlib_encoded: Vec`. Peak counts source RGB8 + fixed encoded capacity +
/// encoded copy + 3×row scratch + chunk buffer + fdeflate compressor claim.
///
/// Publish path (source RGB8 dropped): encoded capacity + decode RGB8 +
/// product-PNG decoder scratch ([`preview_png_decode_scratch_bytes`]).
/// Does **not** claim RSS / bus-moved bytes.
pub fn preview_encode_host_need(
    width: u32,
    height: u32,
    rgb8_len: u64,
    max_image_bytes: u64,
) -> Result<u64, DomainError> {
    let row = u64::from(width)
        .checked_mul(3)
        .ok_or_else(|| err(ErrorCode::ValueOutOfRange, "size", "row byte overflow"))?;
    let row_scratch = row.checked_mul(PNG_STREAM_ROW_BUFS).ok_or_else(|| {
        err(
            ErrorCode::ValueOutOfRange,
            "max_host_bytes",
            "PNG row scratch overflow",
        )
    })?;
    let decode_scratch = preview_png_decode_scratch_bytes(width, height)?;
    let encode_peak = rgb8_len
        .checked_add(max_image_bytes)
        .and_then(|v| v.checked_add(max_image_bytes))
        .and_then(|v| v.checked_add(row_scratch))
        .and_then(|v| v.checked_add(PNG_STREAM_CHUNK_CLAIM))
        .and_then(|v| v.checked_add(PNG_FDEFLATE_COMPRESSOR_CLAIM));
    let publish_peak = max_image_bytes
        .checked_add(rgb8_len)
        .and_then(|v| v.checked_add(decode_scratch));
    match (encode_peak, publish_peak) {
        (Some(a), Some(b)) => Ok(a.max(b)),
        _ => Err(err(
            ErrorCode::ValueOutOfRange,
            "max_host_bytes",
            "encode/publish host reservation overflow",
        )),
    }
}

impl Write for CappedWriter {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        let next = self
            .len
            .checked_add(data.len())
            .ok_or_else(|| io::Error::other("png size overflow"))?;
        if next > self.buf.len() {
            return Err(io::Error::other("png exceeds max_image_bytes"));
        }
        self.buf[self.len..next].copy_from_slice(data);
        self.len = next;
        Ok(data.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Encode opaque RGB8 into PNG bytes under `max_image_bytes`.
///
/// Uses png 0.18.1 **streaming** `StreamWriter` into a fixed-capacity
/// [`CappedWriter`] so zlib output cannot allocate a full unbounded
/// `zlib_encoded` Vec before the outer cap rejects (unlike
/// `Writer::write_image_data`). Filter is `NoFilter`; compression `Fast`.
/// Callers that share a whole-call `max_host_bytes` must check
/// [`preview_encode_host_need`] first and drop unneeded scene buffers.
pub fn encode_preview_png_rgb8(
    width: u32,
    height: u32,
    rgb8: &[u8],
    max_image_bytes: u64,
) -> Result<Vec<u8>, DomainError> {
    if width == 0 || height == 0 {
        return Err(err(
            ErrorCode::ValueOutOfRange,
            "size",
            "width/height must be > 0",
        ));
    }
    if max_image_bytes == 0 {
        return Err(err(
            ErrorCode::ValueOutOfRange,
            "max_image_bytes",
            "max_image_bytes must be > 0",
        ));
    }
    let expected = (u64::from(width))
        .checked_mul(u64::from(height))
        .and_then(|p| p.checked_mul(3))
        .ok_or_else(|| {
            err(
                ErrorCode::ValueOutOfRange,
                "rgb8",
                "width*height*3 overflow",
            )
        })?;
    if expected != rgb8.len() as u64 {
        return Err(err(
            ErrorCode::InvalidArgument,
            "rgb8",
            format!("rgb8 len {} != expected {expected}", rgb8.len()),
        ));
    }
    let max = usize::try_from(max_image_bytes).map_err(|_| {
        err(
            ErrorCode::ValueOutOfRange,
            "max_image_bytes",
            "max_image_bytes exceeds usize",
        )
    })?;
    let mut out = CappedWriter::try_new(max)?;
    {
        let mut encoder = png::Encoder::new(&mut out, width, height);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_compression(png::Compression::Fast);
        encoder.set_filter(png::Filter::NoFilter);
        let mut writer = encoder.write_header().map_err(|e| {
            err(
                ErrorCode::WriteFailed,
                "png",
                format!("PNG header failed: {e}"),
            )
        })?;
        // Stream zlib through CappedWriter (chunk size = PNG_STREAM_CHUNK_CLAIM).
        let mut stream = writer
            .stream_writer_with_size(PNG_STREAM_CHUNK_CLAIM as usize)
            .map_err(|e| {
                err(
                    ErrorCode::WriteFailed,
                    "png",
                    format!("PNG stream writer failed: {e}"),
                )
            })?;
        stream.write_all(rgb8).map_err(|e| {
            let msg = e.to_string();
            if msg.contains("max_image_bytes") {
                err(
                    ErrorCode::ValueOutOfRange,
                    "max_image_bytes",
                    format!("encoded PNG exceeds max_image_bytes {max_image_bytes}"),
                )
            } else {
                err(
                    ErrorCode::WriteFailed,
                    "png",
                    format!("PNG encode failed: {e}"),
                )
            }
        })?;
        stream.finish().map_err(|e| {
            let msg = e.to_string();
            if msg.contains("max_image_bytes") {
                err(
                    ErrorCode::ValueOutOfRange,
                    "max_image_bytes",
                    format!("encoded PNG exceeds max_image_bytes {max_image_bytes}"),
                )
            } else {
                err(
                    ErrorCode::WriteFailed,
                    "png",
                    format!("PNG finish failed: {e}"),
                )
            }
        })?;
    }
    let bytes = out.into_encoded();
    if bytes.len() as u64 > max_image_bytes {
        return Err(err(
            ErrorCode::ValueOutOfRange,
            "max_image_bytes",
            format!(
                "encoded PNG {} exceeds max_image_bytes {max_image_bytes}",
                bytes.len()
            ),
        ));
    }
    Ok(bytes)
}

/// Decoded RGB8 hardcap for publish validation (IHDR checked before decode).
const PREVIEW_PNG_DECODE_MAX_PIXELS: u64 = DEFAULT_MAX_PIXELS;
const PREVIEW_PNG_DECODE_MAX_BYTES: u64 = DEFAULT_MAX_IMAGE_BYTES;

/// Parse PNG signature + IHDR before any decoder allocation.
///
/// Requires 8-bit RGB (`bit_depth=8`, `color_type=2`) and extent within the
/// preview decode hardcaps. Used by publish validation and the package isolate
/// gate (rejects BMP / truncated IHDR stubs before full decode).
pub fn parse_png_ihdr_rgb8(png_bytes: &[u8]) -> Result<(u32, u32), DomainError> {
    if png_bytes.is_empty() {
        return Err(err(ErrorCode::InvalidArgument, "png", "empty PNG payload"));
    }
    // signature(8) + len(4) + type(4) + IHDR(13) + crc(4) = 33
    if png_bytes.len() < 33 || &png_bytes[..8] != b"\x89PNG\r\n\x1a\n" {
        return Err(err(
            ErrorCode::MalformedRecord,
            "png",
            "payload is not a PNG (signature/IHDR mismatch)",
        ));
    }
    let chunk_len = u32::from_be_bytes(png_bytes[8..12].try_into().expect("4"));
    if chunk_len != 13 || &png_bytes[12..16] != b"IHDR" {
        return Err(err(
            ErrorCode::MalformedRecord,
            "png",
            "PNG first chunk must be IHDR",
        ));
    }
    let width = u32::from_be_bytes(png_bytes[16..20].try_into().expect("4"));
    let height = u32::from_be_bytes(png_bytes[20..24].try_into().expect("4"));
    let bit_depth = png_bytes[24];
    let color_type = png_bytes[25];
    if width == 0 || height == 0 {
        return Err(err(
            ErrorCode::ValueOutOfRange,
            "png",
            "PNG width/height must be > 0",
        ));
    }
    // 2 = truecolor RGB; bit depth 8 only for product preview.
    if color_type != 2 || bit_depth != 8 {
        return Err(err(
            ErrorCode::MalformedRecord,
            "png",
            format!(
                "preview PNG must be 8-bit RGB (color_type=2), got type={color_type} depth={bit_depth}"
            ),
        ));
    }
    let pixels = u64::from(width)
        .checked_mul(u64::from(height))
        .ok_or_else(|| {
            err(
                ErrorCode::ValueOutOfRange,
                "png",
                "PNG width*height overflow",
            )
        })?;
    if pixels > PREVIEW_PNG_DECODE_MAX_PIXELS {
        return Err(err(
            ErrorCode::ValueOutOfRange,
            "png",
            format!("PNG pixels {pixels} exceed decode hardcap {PREVIEW_PNG_DECODE_MAX_PIXELS}"),
        ));
    }
    let decoded_bytes = pixels.checked_mul(3).ok_or_else(|| {
        err(
            ErrorCode::ValueOutOfRange,
            "png",
            "decoded RGB8 byte overflow",
        )
    })?;
    if decoded_bytes > PREVIEW_PNG_DECODE_MAX_BYTES {
        return Err(err(
            ErrorCode::ValueOutOfRange,
            "png",
            format!(
                "decoded RGB8 {decoded_bytes} exceeds decode hardcap {PREVIEW_PNG_DECODE_MAX_BYTES}"
            ),
        ));
    }
    Ok((width, height))
}

/// Decode a complete opaque RGB8 preview PNG: signature/IHDR 8-bit RGB first,
/// then explicit PNG-only decode (no auto format; JPEG/BMP rejected).
///
/// Caps: compressed **container** length ([`PREVIEW_PNG_DECODE_MAX_BYTES`]) and
/// IHDR extent/hardcaps. Does **not** promise a total host budget for arbitrary
/// ancillary/text metadata — image 0.25.10 PNG decoding uses
/// `set_ignore_text_chunk(false)`, so decompressed text is allocated outside
/// the product scratch formula in [`preview_png_decode_scratch_bytes`].
/// Product encode emits IHDR+IDAT+IEND only; publish validation covers that
/// path. Callers that must accept untrusted PNGs need an explicit ancillary
/// policy beyond this helper.
pub fn decode_preview_png_rgb8(png_bytes: &[u8]) -> Result<(u32, u32, Vec<u8>), DomainError> {
    if png_bytes.len() as u64 > PREVIEW_PNG_DECODE_MAX_BYTES {
        return Err(err(
            ErrorCode::ValueOutOfRange,
            "png",
            format!(
                "PNG container {} exceeds decode hardcap {PREVIEW_PNG_DECODE_MAX_BYTES}",
                png_bytes.len()
            ),
        ));
    }
    let (ihdr_w, ihdr_h) = parse_png_ihdr_rgb8(png_bytes)?;
    let decoded =
        image::load_from_memory_with_format(png_bytes, image::ImageFormat::Png).map_err(|e| {
            err(
                ErrorCode::MalformedRecord,
                "png",
                format!("invalid PNG payload: {e}"),
            )
        })?;
    if decoded.color() != ColorType::Rgb8 {
        return Err(err(
            ErrorCode::MalformedRecord,
            "png",
            format!("preview PNG must be RGB8, got {:?}", decoded.color()),
        ));
    }
    if decoded.width() != ihdr_w || decoded.height() != ihdr_h {
        return Err(err(
            ErrorCode::MalformedRecord,
            "png",
            "PNG decoder dimensions disagree with IHDR",
        ));
    }
    // Consuming conversion — `to_rgb8()` would clone an already-Rgb8 frame.
    Ok((ihdr_w, ihdr_h, decoded.into_rgb8().into_raw()))
}

/// Validate a complete opaque RGB8 PNG once (IHDR + full decode).
fn validate_complete_preview_png(png_bytes: &[u8]) -> Result<(), DomainError> {
    let _ = decode_preview_png_rgb8(png_bytes)?;
    Ok(())
}

/// Atomically publish PNG bytes. On `postpublish_directory_sync`, the
/// destination **already holds** the new file (durability risk, not untouched).
///
/// `cancel` is wired into [`PublishOptions`] so the atomic helper’s final
/// pre-rename check observes mid-publish cancel. Complete PNG validation runs
/// **once** before the lease (no second full decode in the publish callback).
pub fn publish_preview_png(
    destination: &Path,
    png_bytes: &[u8],
    cancel: Option<&CancelToken>,
) -> Result<PublishOutcome, DomainError> {
    validate_complete_preview_png(png_bytes)?;
    let mut options = PublishOptions::default();
    if let Some(token) = cancel {
        options = options.with_cancel(token);
    }
    // Cheap identity check only — full decode already done once above.
    publish_bytes_validated(destination, png_bytes, options, |bytes| {
        if bytes.len() < 8 || &bytes[..8] != b"\x89PNG\r\n\x1a\n" {
            return Err(err(
                ErrorCode::MalformedRecord,
                "png",
                "PNG signature lost before publish",
            ));
        }
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::temp_harness::TempHarness;
    use spirula_core::CancelReason;
    use std::fs;

    #[test]
    fn encode_roundtrip_dimensions() {
        let rgb = vec![10u8, 20, 30, 40, 50, 60];
        let png = encode_preview_png_rgb8(2, 1, &rgb, 1 << 20).unwrap();
        let decoded = image::load_from_memory(&png).unwrap().to_rgb8();
        assert_eq!(decoded.width(), 2);
        assert_eq!(decoded.height(), 1);
        assert_eq!(decoded.as_raw(), &rgb);
        // Product encode: signature + IHDR 8-bit RGB + full decode.
        let (w, h, out) = decode_preview_png_rgb8(&png).unwrap();
        assert_eq!((w, h), (2, 1));
        assert_eq!(out, rgb);
    }

    #[test]
    fn decode_scratch_is_whole_frame_bound_not_48kib() {
        // Tiny frames still pay the 128 KiB UnfilteringBuffer prealloc floor.
        let tiny = preview_png_decode_scratch_bytes(2, 1).unwrap();
        assert!(
            tiny >= PNG_UNFILTER_PREALLOC_CAP + FDEFLATE_HUFFMAN_TABLES_BYTES,
            "tiny scratch={tiny}"
        );
        assert!(tiny > 48 << 10, "48 KiB under-claims png UnfilteringBuffer");
        // 800×600: 2*(raw_frame+8KiB) dominates the 128 KiB floor.
        let raw_frame = (800u64 * 3 + 1) * 600;
        let expected_floor =
            (2 * (raw_frame + PNG_UNFILTER_GROWTH_BYTES)).max(PNG_UNFILTER_PREALLOC_CAP);
        let wide = preview_png_decode_scratch_bytes(800, 600).unwrap();
        assert!(
            wide >= expected_floor + FDEFLATE_HUFFMAN_TABLES_BYTES,
            "800x600 scratch={wide} floor={expected_floor}"
        );
    }

    #[test]
    fn unknown_root_field_rejected() {
        let harness = TempHarness::new().unwrap();
        let path = harness.path().join("req.json");
        fs::write(
            &path,
            r#"{
              "schema":"spirula-preview-request","version":1,
              "splat_ply":"x.ply",
              "camera":{"model":"PINHOLE","width":1,"height":1,"fx":1,"fy":1,"cx":0.5,"cy":0.5,
                "c2w":{"rotation_row_major":[1,0,0,0,1,0,0,0,1],"translation":[0,0,0]}},
              "background":[0,0,0],
              "sh_degree":3
            }"#,
        )
        .unwrap();
        let err = load_preview_request(&path, None).unwrap_err();
        assert_eq!(err.code(), ErrorCode::MalformedRecord);
    }

    #[test]
    fn publish_preserves_prior_on_cancel_and_garbage() {
        let harness = TempHarness::new().unwrap();
        let dest = harness.path().join("out.png");
        let old = encode_preview_png_rgb8(1, 1, &[1, 2, 3], 1 << 20).unwrap();
        fs::write(&dest, &old).unwrap();
        let token = CancelToken::new();
        token.request(CancelReason::User);
        let valid = encode_preview_png_rgb8(1, 1, &[4, 5, 6], 1 << 20).unwrap();
        let err = publish_preview_png(&dest, &valid, Some(&token)).unwrap_err();
        assert_eq!(err.code(), ErrorCode::Canceled);
        assert_eq!(fs::read(&dest).unwrap(), old);
        assert!(publish_preview_png(&dest, b"\x89PNG\r\n\x1a\nnot PNG chunks", None).is_err());
        assert_eq!(fs::read(&dest).unwrap(), old);
    }
}
