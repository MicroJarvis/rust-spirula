//! Little-endian COLMAP binary readers (`cameras.bin` / `images.bin` / `points3D.bin`).

use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::Read;
use std::path::Path;

use spirula_core::{CancelToken, ErrorCode};

use super::budget::{
    MAX_CAMERAS, MAX_COLMAP_FILE_BYTES, MAX_IMAGES, MAX_OBSERVATIONS, MAX_POINTS, MAX_TRACK_ELEMS,
    MIN_POINT3D_RECORD_BYTES, OBSERVATION_RECORD_BYTES, ParseCancel, ParseMem, TRACK_ELEM_BYTES,
    checked_mul_u64, ensure_count, ensure_remaining, err, err_path, try_reserve_elems,
};
use super::models::require_model_by_id;
use super::types::{
    ColmapCamera, ColmapImage, ColmapObservation, ColmapPoint3D, ColmapPoints3D, ColmapTrackElem,
};

/// Chunk size for cancel-aware binary table reads (avoids whole-file `fs::read`
/// blocking cancel for up to [`MAX_COLMAP_FILE_BYTES`]).
const BIN_READ_CHUNK: usize = 256 * 1024;

struct BinReader {
    data: Vec<u8>,
    pos: usize,
    path: String,
}

impl BinReader {
    fn open(path: &Path, cancel: Option<&CancelToken>) -> Result<Self, spirula_core::DomainError> {
        let meta = std::fs::metadata(path).map_err(|e| {
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
        let mut file = File::open(path).map_err(|e| {
            err_path(
                ErrorCode::ReadFailed,
                &path.display().to_string(),
                format!("cannot open: {e}"),
            )
        })?;
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
        cp.poll_now()?;
        let mut buf = [0u8; BIN_READ_CHUNK];
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
            data.extend_from_slice(&buf[..n]);
            cp.tick(n)?;
        }
        Ok(Self {
            data,
            pos: 0,
            path: path.display().to_string(),
        })
    }

    fn byte_len(&self) -> u64 {
        self.data.len() as u64
    }

    fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }

    fn take(&mut self, n: usize) -> Result<&[u8], spirula_core::DomainError> {
        if self.remaining() < n {
            return Err(err_path(
                ErrorCode::MalformedRecord,
                &self.path,
                "truncated file",
            ));
        }
        let start = self.pos;
        self.pos += n;
        Ok(&self.data[start..self.pos])
    }

    fn u8(&mut self) -> Result<u8, spirula_core::DomainError> {
        Ok(self.take(1)?[0])
    }

    fn i32(&mut self) -> Result<i32, spirula_core::DomainError> {
        let b = self.take(4)?;
        Ok(i32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn u64(&mut self) -> Result<u64, spirula_core::DomainError> {
        let b = self.take(8)?;
        Ok(u64::from_le_bytes([
            b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
        ]))
    }

    fn f64(&mut self) -> Result<f64, spirula_core::DomainError> {
        let b = self.take(8)?;
        let v = f64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]);
        if !v.is_finite() {
            return Err(err_path(
                ErrorCode::ValueOutOfRange,
                &self.path,
                "nonfinite binary float",
            ));
        }
        Ok(v)
    }

    fn cstring(&mut self, cp: &mut ParseCancel<'_>) -> Result<String, spirula_core::DomainError> {
        let start = self.pos;
        while self.pos < self.data.len() && self.data[self.pos] != 0 {
            self.pos += 1;
            // Bound cancel observation during long names.
            if (self.pos - start).is_multiple_of(4096) {
                cp.tick(4096)?;
            }
        }
        if self.pos >= self.data.len() {
            return Err(err_path(
                ErrorCode::MalformedRecord,
                &self.path,
                "truncated cstring",
            ));
        }
        let s = std::str::from_utf8(&self.data[start..self.pos]).map_err(|_| {
            err_path(
                ErrorCode::MalformedRecord,
                &self.path,
                "image name is not UTF-8",
            )
        })?;
        self.pos += 1; // NUL
        Ok(s.to_owned())
    }
}

pub fn read_cameras_binary(
    path: &Path,
    cancel: Option<&CancelToken>,
) -> Result<BTreeMap<i32, ColmapCamera>, spirula_core::DomainError> {
    let mut r = BinReader::open(path, cancel)?;
    let n = r.u64()?;
    ensure_count(n, MAX_CAMERAS, "cameras")?;
    let mut cameras = BTreeMap::new();
    let mut mem = ParseMem::new();
    mem.charge(r.byte_len(), "file")?;
    let mut cp = ParseCancel::new(cancel);
    for _ in 0..n {
        cp.tick(1)?;
        let camera_id = r.i32()?;
        let model_id = r.i32()?;
        let width = r.u64()?;
        let height = r.u64()?;
        let info = require_model_by_id(model_id)?;
        let mut params = Vec::new();
        try_reserve_elems(&mut params, info.num_params, &mut mem, "params")?;
        for _ in 0..info.num_params {
            params.push(r.f64()?);
        }
        if cameras.contains_key(&camera_id) {
            return Err(err(
                ErrorCode::MalformedRecord,
                "camera_id",
                format!("duplicate camera_id {camera_id}"),
            ));
        }
        mem.charge_elems::<ColmapCamera>(1, "cameras")?;
        mem.charge_str_bytes(info.name.len(), "cameras")?;
        cameras.insert(
            camera_id,
            ColmapCamera {
                camera_id,
                model: info.name.to_owned(),
                width,
                height,
                params,
            },
        );
    }
    cp.poll_now()?;
    Ok(cameras)
}

pub fn read_images_binary(
    path: &Path,
    cancel: Option<&CancelToken>,
) -> Result<BTreeMap<i32, ColmapImage>, spirula_core::DomainError> {
    let mut r = BinReader::open(path, cancel)?;
    let n = r.u64()?;
    ensure_count(n, MAX_IMAGES, "images")?;
    let mut images = BTreeMap::new();
    let mut obs_budget = 0u64;
    let mut mem = ParseMem::new();
    mem.charge(r.byte_len(), "file")?;
    let mut cp = ParseCancel::new(cancel);
    for _ in 0..n {
        cp.tick(1)?;
        let image_id = r.i32()?;
        let mut qvec = [0.0; 4];
        for q in &mut qvec {
            *q = r.f64()?;
        }
        let mut tvec = [0.0; 3];
        for t in &mut tvec {
            *t = r.f64()?;
        }
        let camera_id = r.i32()?;
        let name = r.cstring(&mut cp)?;
        if name.is_empty() {
            return Err(err_path(
                ErrorCode::MalformedRecord,
                &r.path,
                "empty image name",
            ));
        }
        mem.charge_str_bytes(name.len(), "images")?;
        let num_points2d = r.u64()?;
        obs_budget = obs_budget.saturating_add(num_points2d);
        ensure_count(obs_budget, MAX_OBSERVATIONS, "observations")?;
        let need = checked_mul_u64(num_points2d, OBSERVATION_RECORD_BYTES, "observations")?;
        ensure_remaining(r.remaining() as u64, need, "observations")?;
        let n_obs = usize::try_from(num_points2d).map_err(|_| {
            err(
                ErrorCode::ValueOutOfRange,
                "observations",
                format!("observation count {num_points2d} exceeds usize"),
            )
        })?;
        let mut observations = Vec::new();
        try_reserve_elems(&mut observations, n_obs, &mut mem, "observations")?;
        for i in 0..num_points2d {
            cp.tick(1)?;
            if i > 0 && i % 4096 == 0 {
                cp.poll_now()?;
            }
            let x = r.f64()?;
            let y = r.f64()?;
            let point3d_id = r.u64()? as i64;
            observations.push(ColmapObservation { x, y, point3d_id });
        }
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
    Ok(images)
}

pub fn read_points3d_binary(
    path: &Path,
    cancel: Option<&CancelToken>,
) -> Result<ColmapPoints3D, spirula_core::DomainError> {
    let mut r = BinReader::open(path, cancel)?;
    let n = r.u64()?;
    ensure_count(n, MAX_POINTS, "points3D")?;
    // Reject multi-GB reserve from a tiny file declaring huge `n`.
    let need = checked_mul_u64(n, MIN_POINT3D_RECORD_BYTES, "points3D")?;
    ensure_remaining(r.remaining() as u64, need, "points3D")?;
    let mut points = Vec::new();
    let mut mem = ParseMem::new();
    mem.charge(r.byte_len(), "file")?;
    let n_usize = usize::try_from(n).map_err(|_| {
        err(
            ErrorCode::ValueOutOfRange,
            "points3D",
            format!("point count {n} exceeds usize"),
        )
    })?;
    try_reserve_elems(&mut points, n_usize, &mut mem, "points3D")?;
    let mut seen_ids = BTreeSet::new();
    let mut track_budget = 0u64;
    let mut cp = ParseCancel::new(cancel);
    for _ in 0..n {
        cp.tick(1)?;
        let point3d_id = r.u64()?;
        if !seen_ids.insert(point3d_id) {
            return Err(err(
                ErrorCode::MalformedRecord,
                "point3d_id",
                format!("duplicate point3D_id {point3d_id}"),
            ));
        }
        let xyz = [r.f64()?, r.f64()?, r.f64()?];
        let rgb = [r.u8()?, r.u8()?, r.u8()?];
        let error = r.f64()?;
        let track_len = r.u64()?;
        track_budget = track_budget.saturating_add(track_len);
        ensure_count(track_budget, MAX_TRACK_ELEMS, "track")?;
        let track_bytes = checked_mul_u64(track_len, TRACK_ELEM_BYTES, "track")?;
        ensure_remaining(r.remaining() as u64, track_bytes, "track")?;
        let n_track = usize::try_from(track_len).map_err(|_| {
            err(
                ErrorCode::ValueOutOfRange,
                "track",
                format!("track length {track_len} exceeds usize"),
            )
        })?;
        let mut track = Vec::new();
        try_reserve_elems(&mut track, n_track, &mut mem, "track")?;
        for i in 0..track_len {
            cp.tick(1)?;
            if i > 0 && i % 4096 == 0 {
                cp.poll_now()?;
            }
            track.push(ColmapTrackElem {
                image_id: r.i32()?,
                point2d_idx: r.i32()?,
            });
        }
        points.push(ColmapPoint3D {
            point3d_id,
            xyz,
            rgb,
            error,
            track,
        });
    }
    cp.poll_now()?;
    Ok(ColmapPoints3D { points })
}
