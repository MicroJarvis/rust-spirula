//! COLMAP text table readers (`cameras.txt` / `images.txt` / `points3D.txt`).

use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::Read;
use std::path::Path;

use spirula_core::{CancelToken, ErrorCode};

use super::budget::{
    MAX_CAMERAS, MAX_COLMAP_FILE_BYTES, MAX_IMAGES, MAX_OBSERVATIONS, MAX_POINTS, MAX_TRACK_ELEMS,
    ParseCancel, ParseMem, ensure_count, err, err_path, try_reserve_elems,
};
use super::models::{require_exact_param_count, require_model_by_name};
use super::types::{
    ColmapCamera, ColmapImage, ColmapObservation, ColmapPoint3D, ColmapPoints3D, ColmapTrackElem,
};

/// Chunk size for cancel-aware capped text table reads (avoids whole-file
/// `fs::read` blocking cancel for up to [`MAX_COLMAP_FILE_BYTES`]).
const TEXT_READ_CHUNK: usize = 256 * 1024;

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

/// Metadata-gated, budget-charged, cancel-checkpointed table read.
///
/// Does **not** call `fs::read`: bytes are streamed in [`TEXT_READ_CHUNK`]
/// slices so an already-canceled (or mid-read canceled) token is observed
/// before/while filling the Vec, while still enforcing
/// [`MAX_COLMAP_FILE_BYTES`] and cumulative [`ParseMem`] charges.
fn read_capped(
    path: &Path,
    mem: &mut ParseMem,
    cancel: Option<&CancelToken>,
) -> Result<Vec<u8>, spirula_core::DomainError> {
    let meta = std::fs::metadata(path).map_err(|e| open_err(path, e))?;
    let len = meta.len();
    if len > MAX_COLMAP_FILE_BYTES {
        return Err(err(
            ErrorCode::ValueOutOfRange,
            "file_size",
            format!(
                "COLMAP file {} exceeds budget {MAX_COLMAP_FILE_BYTES} bytes",
                path.display()
            ),
        ));
    }
    // Charge metadata length before allocating so a huge file fails on budget,
    // not only after a successful read.
    mem.charge(len, "file")?;
    let mut charged = len;

    let mut file = File::open(path).map_err(|e| open_err(path, e))?;
    let mut data = Vec::new();
    if len > 0 {
        data.try_reserve(len as usize).map_err(|_| {
            err(
                ErrorCode::ValueOutOfRange,
                "file",
                format!(
                    "COLMAP file {} reservation of {len} bytes failed",
                    path.display()
                ),
            )
        })?;
    }

    let mut cp = ParseCancel::new(cancel);
    // Observe cancel before the first blocking read.
    cp.poll_now()?;

    let mut buf = [0u8; TEXT_READ_CHUNK];
    let mut read_total = 0u64;
    loop {
        let n = file.read(&mut buf).map_err(|e| {
            err_path(
                ErrorCode::ReadFailed,
                &path.display().to_string(),
                format!("cannot read: {e}"),
            )
        })?;
        if n == 0 {
            break;
        }
        read_total = read_total.saturating_add(n as u64);
        // TOCTOU: reject growth past the metadata-gated budget during stream.
        if read_total > MAX_COLMAP_FILE_BYTES {
            return Err(err(
                ErrorCode::ValueOutOfRange,
                "file_size",
                format!(
                    "COLMAP file {} grew past budget {MAX_COLMAP_FILE_BYTES} bytes during read",
                    path.display()
                ),
            ));
        }
        if read_total > charged {
            mem.charge(read_total - charged, "file")?;
            charged = read_total;
        }
        data.extend_from_slice(&buf[..n]);
        cp.tick(n)?;
    }
    Ok(data)
}

fn trim_line(s: &str) -> &str {
    s.trim_matches(|c: char| c == ' ' || c == '\t' || c == '\r')
}

/// Iterate non-comment COLMAP text lines; caller handles POINTS2D raw lines.
struct TextLines<'a> {
    data: &'a str,
    pos: usize,
}

impl<'a> TextLines<'a> {
    fn new(data: &'a str) -> Self {
        Self { data, pos: 0 }
    }

    fn next_data_line(&mut self) -> Option<&'a str> {
        while let Some(raw) = self.next_raw_line() {
            let t = trim_line(raw);
            if t.is_empty() || t.starts_with('#') {
                continue;
            }
            return Some(t);
        }
        None
    }

    fn next_raw_line(&mut self) -> Option<&'a str> {
        if self.pos >= self.data.len() {
            return None;
        }
        let rest = &self.data[self.pos..];
        let (line, adv) = match rest.find('\n') {
            Some(i) => (&rest[..i], i + 1),
            None => (rest, rest.len()),
        };
        self.pos += adv;
        Some(line.strip_suffix('\r').unwrap_or(line))
    }
}

fn parse_i32(tok: &str, field: &str) -> Result<i32, spirula_core::DomainError> {
    tok.parse::<i32>().map_err(|_| {
        err(
            ErrorCode::MalformedRecord,
            field,
            format!("invalid i32 `{tok}`"),
        )
    })
}

fn parse_u64(tok: &str, field: &str) -> Result<u64, spirula_core::DomainError> {
    tok.parse::<u64>().map_err(|_| {
        err(
            ErrorCode::MalformedRecord,
            field,
            format!("invalid u64 `{tok}`"),
        )
    })
}

fn parse_f64(tok: &str, field: &str) -> Result<f64, spirula_core::DomainError> {
    let v: f64 = tok.parse().map_err(|_| {
        err(
            ErrorCode::MalformedRecord,
            field,
            format!("invalid f64 `{tok}`"),
        )
    })?;
    if !v.is_finite() {
        return Err(err(
            ErrorCode::ValueOutOfRange,
            field,
            format!("nonfinite value `{tok}`"),
        ));
    }
    Ok(v)
}

fn parse_u8(tok: &str, field: &str) -> Result<u8, spirula_core::DomainError> {
    let v = parse_i32(tok, field)?;
    if !(0..=255).contains(&v) {
        return Err(err(
            ErrorCode::ValueOutOfRange,
            field,
            format!("rgb channel out of range: {v}"),
        ));
    }
    Ok(v as u8)
}

/// Read `cameras.txt` into an id-keyed map. Duplicate `camera_id` → MalformedRecord.
pub fn read_cameras_text(
    path: &Path,
    cancel: Option<&CancelToken>,
) -> Result<BTreeMap<i32, ColmapCamera>, spirula_core::DomainError> {
    let mut mem = ParseMem::new();
    let bytes = read_capped(path, &mut mem, cancel)?;
    let text = std::str::from_utf8(&bytes).map_err(|_| {
        err_path(
            ErrorCode::MalformedRecord,
            &path.display().to_string(),
            "cameras.txt is not UTF-8",
        )
    })?;
    let mut lines = TextLines::new(text);
    let mut cameras = BTreeMap::new();
    let mut cp = ParseCancel::new(cancel);
    while let Some(line) = lines.next_data_line() {
        cp.tick(1)?;
        let mut it = line.split_whitespace();
        let id_s = it.next().ok_or_else(|| {
            err(
                ErrorCode::MalformedRecord,
                "cameras.txt",
                "malformed cameras.txt line",
            )
        })?;
        let model = it
            .next()
            .ok_or_else(|| {
                err(
                    ErrorCode::MalformedRecord,
                    "cameras.txt",
                    "malformed cameras.txt line",
                )
            })?
            .to_owned();
        if model.is_empty() {
            return Err(err(
                ErrorCode::MalformedRecord,
                "cameras.txt",
                "malformed cameras.txt line",
            ));
        }
        mem.charge_str_bytes(model.len(), "cameras")?;
        let info = require_model_by_name(&model)?;
        let width = parse_u64(
            it.next().ok_or_else(|| {
                err(
                    ErrorCode::MalformedRecord,
                    "cameras.txt",
                    "malformed cameras.txt line",
                )
            })?,
            "width",
        )?;
        let height = parse_u64(
            it.next().ok_or_else(|| {
                err(
                    ErrorCode::MalformedRecord,
                    "cameras.txt",
                    "malformed cameras.txt line",
                )
            })?,
            "height",
        )?;
        // Exact arity via iterator — charge/reserve before any param push; no
        // intermediate `Vec<&str>` of unbounded leftover tokens.
        let mut params = Vec::new();
        try_reserve_elems(&mut params, info.num_params, &mut mem, "params")?;
        for _ in 0..info.num_params {
            let tok = it.next().ok_or_else(|| {
                err(
                    ErrorCode::MalformedRecord,
                    "params",
                    format!(
                        "camera model {} needs {} parameters",
                        info.name, info.num_params
                    ),
                )
            })?;
            params.push(parse_f64(tok, "params")?);
        }
        if it.next().is_some() {
            return Err(err(
                ErrorCode::MalformedRecord,
                "params",
                format!(
                    "camera model {} has trailing tokens beyond {} parameters",
                    info.name, info.num_params
                ),
            ));
        }
        require_exact_param_count(info, &params)?;
        let camera_id = parse_i32(id_s, "camera_id")?;
        ensure_count(cameras.len() as u64 + 1, MAX_CAMERAS, "cameras")?;
        if cameras.contains_key(&camera_id) {
            return Err(err(
                ErrorCode::MalformedRecord,
                "camera_id",
                format!("duplicate camera_id {camera_id}"),
            ));
        }
        mem.charge_elems::<ColmapCamera>(1, "cameras")?;
        cameras.insert(
            camera_id,
            ColmapCamera {
                camera_id,
                model,
                width,
                height,
                params,
            },
        );
    }
    cp.poll_now()?;
    let _ = mem.used();
    Ok(cameras)
}

fn parse_observations(
    line: &str,
    budget: &mut u64,
    mem: &mut ParseMem,
    cp: &mut ParseCancel<'_>,
) -> Result<Vec<ColmapObservation>, spirula_core::DomainError> {
    // Count tokens without allocating `Vec<&str>`; charge/reserve before parse.
    let n_toks = line.split_whitespace().count();
    if n_toks == 0 {
        return Ok(Vec::new());
    }
    if !n_toks.is_multiple_of(3) {
        return Err(err(
            ErrorCode::MalformedRecord,
            "points2d",
            format!("POINTS2D line length {n_toks} is not a multiple of 3"),
        ));
    }
    let n = n_toks / 3;
    *budget = budget.saturating_add(n as u64);
    ensure_count(*budget, MAX_OBSERVATIONS, "observations")?;
    let mut out = Vec::new();
    try_reserve_elems(&mut out, n, mem, "observations")?;
    let mut it = line.split_whitespace();
    for i in 0..n {
        cp.tick(1)?;
        // Extra poll on long lines so after-start cancel wins before chunk ceiling.
        if i > 0 && i % 4096 == 0 {
            cp.poll_now()?;
        }
        let x_s = it.next().expect("token count");
        let y_s = it.next().expect("token count");
        let id_s = it.next().expect("token count");
        let x = parse_f64(x_s, "obs_x")?;
        let y = parse_f64(y_s, "obs_y")?;
        let point3d_id: i64 = id_s.parse().map_err(|_| {
            err(
                ErrorCode::MalformedRecord,
                "point3d_id",
                format!("invalid point3D_id `{id_s}`"),
            )
        })?;
        out.push(ColmapObservation { x, y, point3d_id });
    }
    Ok(out)
}

/// Read `images.txt`. Each image is a header line + POINTS2D raw line.
pub fn read_images_text(
    path: &Path,
    cancel: Option<&CancelToken>,
) -> Result<BTreeMap<i32, ColmapImage>, spirula_core::DomainError> {
    let mut mem = ParseMem::new();
    let bytes = read_capped(path, &mut mem, cancel)?;
    let text = std::str::from_utf8(&bytes).map_err(|_| {
        err_path(
            ErrorCode::MalformedRecord,
            &path.display().to_string(),
            "images.txt is not UTF-8",
        )
    })?;
    let mut lines = TextLines::new(text);
    let mut images = BTreeMap::new();
    let mut obs_budget = 0u64;
    let mut cp = ParseCancel::new(cancel);
    while let Some(line) = lines.next_data_line() {
        cp.tick(1)?;
        let mut it = line.split_whitespace();
        let image_id = parse_i32(
            it.next().ok_or_else(|| {
                err(
                    ErrorCode::MalformedRecord,
                    "images.txt",
                    "malformed images.txt line",
                )
            })?,
            "image_id",
        )?;
        let mut qvec = [0.0; 4];
        for q in &mut qvec {
            *q = parse_f64(
                it.next().ok_or_else(|| {
                    err(
                        ErrorCode::MalformedRecord,
                        "images.txt",
                        "malformed images.txt line",
                    )
                })?,
                "qvec",
            )?;
        }
        let mut tvec = [0.0; 3];
        for t in &mut tvec {
            *t = parse_f64(
                it.next().ok_or_else(|| {
                    err(
                        ErrorCode::MalformedRecord,
                        "images.txt",
                        "malformed images.txt line",
                    )
                })?,
                "tvec",
            )?;
        }
        let camera_id = parse_i32(
            it.next().ok_or_else(|| {
                err(
                    ErrorCode::MalformedRecord,
                    "images.txt",
                    "malformed images.txt line",
                )
            })?,
            "camera_id",
        )?;
        let name = it
            .next()
            .ok_or_else(|| {
                err(
                    ErrorCode::MalformedRecord,
                    "images.txt",
                    "malformed images.txt line",
                )
            })?
            .to_owned();
        if name.is_empty() {
            return Err(err(
                ErrorCode::MalformedRecord,
                "images.txt",
                "malformed images.txt line",
            ));
        }
        mem.charge_str_bytes(name.len(), "images")?;
        // POINTS2D line — may be empty; always consume one raw line (pin).
        let obs_line = lines.next_raw_line().unwrap_or("");
        let observations =
            parse_observations(trim_line(obs_line), &mut obs_budget, &mut mem, &mut cp)?;
        ensure_count(images.len() as u64 + 1, MAX_IMAGES, "images")?;
        if images.contains_key(&image_id) {
            return Err(err(
                ErrorCode::MalformedRecord,
                "image_id",
                format!("duplicate image_id {image_id}"),
            ));
        }
        mem.charge_elems::<ColmapImage>(1, "images")?;
        images.insert(
            image_id,
            ColmapImage {
                image_id,
                qvec,
                tvec,
                camera_id,
                name,
                observations,
            },
        );
    }
    cp.poll_now()?;
    let _ = mem.used();
    Ok(images)
}

/// Read `points3D.txt` including track association (reference skips track; we keep it).
pub fn read_points3d_text(
    path: &Path,
    cancel: Option<&CancelToken>,
) -> Result<ColmapPoints3D, spirula_core::DomainError> {
    let mut mem = ParseMem::new();
    let bytes = read_capped(path, &mut mem, cancel)?;
    let text = std::str::from_utf8(&bytes).map_err(|_| {
        err_path(
            ErrorCode::MalformedRecord,
            &path.display().to_string(),
            "points3D.txt is not UTF-8",
        )
    })?;
    let mut lines = TextLines::new(text);
    let mut points = Vec::new();
    let mut seen_ids = BTreeSet::new();
    let mut track_budget = 0u64;
    let mut cp = ParseCancel::new(cancel);
    while let Some(line) = lines.next_data_line() {
        cp.tick(1)?;
        let mut it = line.split_whitespace();
        let point3d_id = parse_u64(
            it.next().ok_or_else(|| {
                err(
                    ErrorCode::MalformedRecord,
                    "points3D.txt",
                    "malformed points3D.txt line",
                )
            })?,
            "point3d_id",
        )?;
        if !seen_ids.insert(point3d_id) {
            return Err(err(
                ErrorCode::MalformedRecord,
                "point3d_id",
                format!("duplicate point3D_id {point3d_id}"),
            ));
        }
        let xyz = [
            parse_f64(
                it.next().ok_or_else(|| {
                    err(
                        ErrorCode::MalformedRecord,
                        "points3D.txt",
                        "malformed points3D.txt line",
                    )
                })?,
                "xyz",
            )?,
            parse_f64(
                it.next().ok_or_else(|| {
                    err(
                        ErrorCode::MalformedRecord,
                        "points3D.txt",
                        "malformed points3D.txt line",
                    )
                })?,
                "xyz",
            )?,
            parse_f64(
                it.next().ok_or_else(|| {
                    err(
                        ErrorCode::MalformedRecord,
                        "points3D.txt",
                        "malformed points3D.txt line",
                    )
                })?,
                "xyz",
            )?,
        ];
        let rgb = [
            parse_u8(
                it.next().ok_or_else(|| {
                    err(
                        ErrorCode::MalformedRecord,
                        "points3D.txt",
                        "malformed points3D.txt line",
                    )
                })?,
                "rgb",
            )?,
            parse_u8(
                it.next().ok_or_else(|| {
                    err(
                        ErrorCode::MalformedRecord,
                        "points3D.txt",
                        "malformed points3D.txt line",
                    )
                })?,
                "rgb",
            )?,
            parse_u8(
                it.next().ok_or_else(|| {
                    err(
                        ErrorCode::MalformedRecord,
                        "points3D.txt",
                        "malformed points3D.txt line",
                    )
                })?,
                "rgb",
            )?,
        ];
        let error = match it.next() {
            Some(t) => parse_f64(t, "error")?,
            None => 0.0,
        };
        // Count remaining track tokens without `Vec<&str>`; budget before reserve.
        let n_toks = it.clone().count();
        if !n_toks.is_multiple_of(2) {
            return Err(err(
                ErrorCode::MalformedRecord,
                "track",
                format!("track length {n_toks} is not even"),
            ));
        }
        let n = n_toks / 2;
        track_budget = track_budget.saturating_add(n as u64);
        ensure_count(track_budget, MAX_TRACK_ELEMS, "track")?;
        let mut track = Vec::new();
        try_reserve_elems(&mut track, n, &mut mem, "track")?;
        for i in 0..n {
            cp.tick(1)?;
            if i > 0 && i % 4096 == 0 {
                cp.poll_now()?;
            }
            let img_s = it.next().expect("token count");
            let idx_s = it.next().expect("token count");
            track.push(ColmapTrackElem {
                image_id: parse_i32(img_s, "track_image_id")?,
                point2d_idx: parse_i32(idx_s, "track_point2d_idx")?,
            });
        }
        ensure_count(points.len() as u64 + 1, MAX_POINTS, "points3D")?;
        mem.charge_elems::<ColmapPoint3D>(1, "points3D")?;
        points.push(ColmapPoint3D {
            point3d_id,
            xyz,
            rgb,
            error,
            track,
        });
    }
    cp.poll_now()?;
    let _ = mem.used();
    Ok(ColmapPoints3D { points })
}
