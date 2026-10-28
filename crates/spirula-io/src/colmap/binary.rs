//! Little-endian COLMAP binary readers (`cameras.bin` / `images.bin` / `points3D.bin`).

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use spirula_core::{CancelToken, ErrorCode};

use super::budget::{
    MAX_CAMERAS, MAX_COLMAP_FILE_BYTES, MAX_IMAGES, MAX_OBSERVATIONS, MAX_POINTS, MAX_TRACK_ELEMS,
    ParseCancel, checked_mul_u64, ensure_count, err, err_path,
};
use super::models::require_model_by_id;
use super::types::{
    ColmapCamera, ColmapImage, ColmapObservation, ColmapPoint3D, ColmapPoints3D, ColmapTrackElem,
};

struct BinReader {
    data: Vec<u8>,
    pos: usize,
    path: String,
}

impl BinReader {
    fn open(path: &Path) -> Result<Self, spirula_core::DomainError> {
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
        if meta.len() > MAX_COLMAP_FILE_BYTES {
            return Err(err(
                ErrorCode::ValueOutOfRange,
                "file_size",
                format!(
                    "COLMAP file {} exceeds budget {MAX_COLMAP_FILE_BYTES} bytes",
                    path.display()
                ),
            ));
        }
        let data = fs::read(path).map_err(|e| {
            err_path(
                ErrorCode::ReadFailed,
                &path.display().to_string(),
                format!("cannot read: {e}"),
            )
        })?;
        Ok(Self {
            data,
            pos: 0,
            path: path.display().to_string(),
        })
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

    fn cstring(&mut self) -> Result<String, spirula_core::DomainError> {
        let start = self.pos;
        while self.pos < self.data.len() && self.data[self.pos] != 0 {
            self.pos += 1;
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
    let mut r = BinReader::open(path)?;
    let n = r.u64()?;
    ensure_count(n, MAX_CAMERAS, "cameras")?;
    let mut cameras = BTreeMap::new();
    let mut cp = ParseCancel::new(cancel);
    for _ in 0..n {
        cp.tick(1)?;
        let camera_id = r.i32()?;
        let model_id = r.i32()?;
        let width = r.u64()?;
        let height = r.u64()?;
        let info = require_model_by_id(model_id)?;
        let mut params = Vec::with_capacity(info.num_params);
        for _ in 0..info.num_params {
            params.push(r.f64()?);
        }
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
    let mut r = BinReader::open(path)?;
    let n = r.u64()?;
    ensure_count(n, MAX_IMAGES, "images")?;
    let mut images = BTreeMap::new();
    let mut obs_budget = 0u64;
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
        let name = r.cstring()?;
        if name.is_empty() {
            return Err(err_path(
                ErrorCode::MalformedRecord,
                &r.path,
                "empty image name",
            ));
        }
        let num_points2d = r.u64()?;
        obs_budget = obs_budget.saturating_add(num_points2d);
        ensure_count(obs_budget, MAX_OBSERVATIONS, "observations")?;
        // Each observation: 2×f64 + u64 point3D_id.
        let mut observations = Vec::with_capacity(usize::try_from(num_points2d).unwrap_or(0));
        for _ in 0..num_points2d {
            cp.tick(1)?;
            let x = r.f64()?;
            let y = r.f64()?;
            let point3d_id = r.u64()? as i64;
            // COLMAP uses u64; invalid sentinel is often all-bits / max — keep as i64 bit pattern.
            observations.push(ColmapObservation { x, y, point3d_id });
        }
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
    let mut r = BinReader::open(path)?;
    let n = r.u64()?;
    ensure_count(n, MAX_POINTS, "points3D")?;
    let mut points = Vec::with_capacity(usize::try_from(n).unwrap_or(0));
    let mut track_budget = 0u64;
    let mut cp = ParseCancel::new(cancel);
    for _ in 0..n {
        cp.tick(1)?;
        let point3d_id = r.u64()?;
        let xyz = [r.f64()?, r.f64()?, r.f64()?];
        let rgb = [r.u8()?, r.u8()?, r.u8()?];
        let error = r.f64()?;
        let track_len = r.u64()?;
        track_budget = track_budget.saturating_add(track_len);
        ensure_count(track_budget, MAX_TRACK_ELEMS, "track")?;
        // Pre-check remaining bytes for track: track_len * 2 * i32.
        let track_bytes = checked_mul_u64(track_len, 8, "track")?;
        if (r.remaining() as u64) < track_bytes {
            return Err(err_path(
                ErrorCode::MalformedRecord,
                &r.path,
                "truncated file",
            ));
        }
        let mut track = Vec::with_capacity(usize::try_from(track_len).unwrap_or(0));
        for _ in 0..track_len {
            cp.tick(1)?;
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
