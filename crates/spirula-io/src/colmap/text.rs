//! COLMAP text table readers (`cameras.txt` / `images.txt` / `points3D.txt`).

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
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

fn read_capped(path: &Path, mem: &mut ParseMem) -> Result<Vec<u8>, spirula_core::DomainError> {
    let meta = fs::metadata(path).map_err(|e| {
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
    })?;
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
    let data = fs::read(path).map_err(|e| {
        err_path(
            ErrorCode::ReadFailed,
            &path.display().to_string(),
            format!("cannot read: {e}"),
        )
    })?;
    // TOCTOU: reject growth past the metadata-gated budget after read.
    if data.len() as u64 > MAX_COLMAP_FILE_BYTES {
        return Err(err(
            ErrorCode::ValueOutOfRange,
            "file_size",
            format!(
                "COLMAP file {} grew past budget {MAX_COLMAP_FILE_BYTES} bytes during read",
                path.display()
            ),
        ));
    }
    if (data.len() as u64) > len {
        mem.charge((data.len() as u64) - len, "file")?;
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
    let bytes = read_capped(path, &mut mem)?;
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
        let mut params = Vec::new();
        let toks: Vec<&str> = it.collect();
        mem.charge_elems::<&str>(toks.len(), "params")?;
        try_reserve_elems(&mut params, toks.len(), &mut mem, "params")?;
        for tok in toks {
            params.push(parse_f64(tok, "params")?);
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
    // Charge token Vec growth into the shared cumulative budget (not reset
    // per line — that previously missed Vec/clone growth across images).
    let toks: Vec<&str> = line.split_whitespace().collect();
    mem.charge_elems::<&str>(toks.len(), "observations")?;
    if toks.is_empty() {
        return Ok(Vec::new());
    }
    if !toks.len().is_multiple_of(3) {
        return Err(err(
            ErrorCode::MalformedRecord,
            "points2d",
            format!("POINTS2D line length {} is not a multiple of 3", toks.len()),
        ));
    }
    let n = (toks.len() / 3) as u64;
    *budget = budget.saturating_add(n);
    ensure_count(*budget, MAX_OBSERVATIONS, "observations")?;
    let mut out = Vec::new();
    try_reserve_elems(&mut out, toks.len() / 3, mem, "observations")?;
    for (i, chunk) in toks.as_chunks::<3>().0.iter().enumerate() {
        cp.tick(1)?;
        // Extra poll on long lines so after-start cancel wins before chunk ceiling.
        if i > 0 && i % 4096 == 0 {
            cp.poll_now()?;
        }
        let x = parse_f64(chunk[0], "obs_x")?;
        let y = parse_f64(chunk[1], "obs_y")?;
        let point3d_id: i64 = chunk[2].parse().map_err(|_| {
            err(
                ErrorCode::MalformedRecord,
                "point3d_id",
                format!("invalid point3D_id `{}`", chunk[2]),
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
    let bytes = read_capped(path, &mut mem)?;
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
    let bytes = read_capped(path, &mut mem)?;
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
        let rest: Vec<&str> = it.collect();
        mem.charge_elems::<&str>(rest.len(), "track")?;
        if !rest.len().is_multiple_of(2) {
            return Err(err(
                ErrorCode::MalformedRecord,
                "track",
                format!("track length {} is not even", rest.len()),
            ));
        }
        let n = (rest.len() / 2) as u64;
        track_budget = track_budget.saturating_add(n);
        ensure_count(track_budget, MAX_TRACK_ELEMS, "track")?;
        let mut track = Vec::new();
        try_reserve_elems(&mut track, rest.len() / 2, &mut mem, "track")?;
        for (i, chunk) in rest.as_chunks::<2>().0.iter().enumerate() {
            cp.tick(1)?;
            if i > 0 && i % 4096 == 0 {
                cp.poll_now()?;
            }
            track.push(ColmapTrackElem {
                image_id: parse_i32(chunk[0], "track_image_id")?,
                point2d_idx: parse_i32(chunk[1], "track_point2d_idx")?,
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
