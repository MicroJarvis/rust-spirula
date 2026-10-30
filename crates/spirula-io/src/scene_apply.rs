//! Apply scene center / applied_transform undo / normalize metadata (P3-F05).
//!
//! Pin: `NerfstudioParser.cpp` applied_transform undo + centering;
//! `ColmapParser.cpp` centering; `DatasetCommon.cpp` `compute_normalized_transform`.

use spirula_core::{
    CenterMode, ContextKey, DomainError, ErrorCode, Mat3, ResolvedCenter, SceneTransform, Vec3,
    resolve_scene_center,
};

use crate::colmap::types::{ColmapPoints3D, LoadedFrame, ParsedDataset};

fn err(code: ErrorCode, field: &str, message: impl Into<String>) -> DomainError {
    DomainError::try_new(code, message)
        .expect("message")
        .with_context(ContextKey::Field, field)
        .expect("field")
}

/// 3×4 affine `p' = A p + b` from Nerfstudio `applied_transform` (3×3 or 3×4).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AppliedTransform {
    pub a: [[f64; 3]; 3],
    pub b: [f64; 3],
}

impl AppliedTransform {
    pub fn identity() -> Self {
        Self {
            a: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            b: [0.0, 0.0, 0.0],
        }
    }

    /// Parse Nerfstudio `applied_transform` JSON array (3×3 or 3×4).
    pub fn from_json_value(at: &serde_json::Value) -> Result<Self, DomainError> {
        let rows = at.as_array().ok_or_else(|| {
            err(
                ErrorCode::MalformedRecord,
                "applied_transform",
                "applied_transform must be a 3×3/3×4 array",
            )
        })?;
        if rows.len() < 3 {
            return Err(err(
                ErrorCode::MalformedRecord,
                "applied_transform",
                "applied_transform must have ≥3 rows",
            ));
        }
        let mut a = [[0.0; 3]; 3];
        let mut b = [0.0; 3];
        for (r, row) in rows.iter().take(3).enumerate() {
            let cols = row.as_array().ok_or_else(|| {
                err(
                    ErrorCode::MalformedRecord,
                    "applied_transform",
                    "applied_transform row must be an array",
                )
            })?;
            if cols.len() < 3 {
                return Err(err(
                    ErrorCode::MalformedRecord,
                    "applied_transform",
                    "applied_transform row too short",
                ));
            }
            for (c, cell) in cols.iter().take(3).enumerate() {
                let v = cell.as_f64().ok_or_else(|| {
                    err(
                        ErrorCode::MalformedRecord,
                        "applied_transform",
                        "applied_transform non-numeric",
                    )
                })?;
                if !v.is_finite() {
                    return Err(err(
                        ErrorCode::ValueOutOfRange,
                        "applied_transform",
                        "applied_transform non-finite",
                    ));
                }
                a[r][c] = v;
            }
            if cols.len() >= 4 {
                let v = cols[3].as_f64().ok_or_else(|| {
                    err(
                        ErrorCode::MalformedRecord,
                        "applied_transform",
                        "applied_transform translation non-numeric",
                    )
                })?;
                if !v.is_finite() {
                    return Err(err(
                        ErrorCode::ValueOutOfRange,
                        "applied_transform",
                        "applied_transform translation non-finite",
                    ));
                }
                b[r] = v;
            }
        }
        Ok(Self { a, b })
    }

    pub fn is_identity(self) -> bool {
        for r in 0..3 {
            for c in 0..3 {
                let expect = if r == c { 1.0 } else { 0.0 };
                if self.a[r][c] != expect {
                    return false;
                }
            }
            if self.b[r] != 0.0 {
                return false;
            }
        }
        true
    }

    pub fn inverse(self) -> Result<Self, DomainError> {
        let m = self.a;
        let det = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
            - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
            + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
        if !det.is_finite() || det.abs() < 1e-20 {
            return Err(err(
                ErrorCode::ValueOutOfRange,
                "applied_transform",
                "singular applied_transform",
            ));
        }
        let inv = [
            [
                (m[1][1] * m[2][2] - m[1][2] * m[2][1]) / det,
                (m[0][2] * m[2][1] - m[0][1] * m[2][2]) / det,
                (m[0][1] * m[1][2] - m[0][2] * m[1][1]) / det,
            ],
            [
                (m[1][2] * m[2][0] - m[1][0] * m[2][2]) / det,
                (m[0][0] * m[2][2] - m[0][2] * m[2][0]) / det,
                (m[0][2] * m[1][0] - m[0][0] * m[1][2]) / det,
            ],
            [
                (m[1][0] * m[2][1] - m[1][1] * m[2][0]) / det,
                (m[0][1] * m[2][0] - m[0][0] * m[2][1]) / det,
                (m[0][0] * m[1][1] - m[0][1] * m[1][0]) / det,
            ],
        ];
        let mut bi = [0.0; 3];
        for r in 0..3 {
            bi[r] = -(inv[r][0] * self.b[0] + inv[r][1] * self.b[1] + inv[r][2] * self.b[2]);
        }
        Ok(Self { a: inv, b: bi })
    }

    pub fn apply_point(self, p: [f64; 3]) -> [f64; 3] {
        [
            self.a[0][0] * p[0] + self.a[0][1] * p[1] + self.a[0][2] * p[2] + self.b[0],
            self.a[1][0] * p[0] + self.a[1][1] * p[1] + self.a[1][2] * p[2] + self.b[1],
            self.a[2][0] * p[0] + self.a[2][1] * p[1] + self.a[2][2] * p[2] + self.b[2],
        ]
    }

    /// `c2w' = T @ c2w` for OpenGL 3×4 row-major (`c2w` bottom row implicit).
    pub fn apply_c2w(self, c2w: &[f64; 12]) -> [f64; 12] {
        let mut out = [0.0; 12];
        for r in 0..3 {
            for c in 0..4 {
                out[r * 4 + c] = self.a[r][0] * c2w[c]
                    + self.a[r][1] * c2w[4 + c]
                    + self.a[r][2] * c2w[8 + c]
                    + if c == 3 { self.b[r] } else { 0.0 };
            }
        }
        out
    }
}

/// Options for [`apply_scene_to_dataset`].
#[derive(Clone, Debug)]
pub struct SceneApplyConfig {
    /// `"none"` / named mode / `"auto"`. Default `"none"` (C++ DatasetParserConfig).
    pub center_mode: String,
    pub center_auto_threshold: f32,
    /// Explicit center overrides resolve when set (`center_mode` must not be `auto`).
    pub center: Option<[f64; 3]>,
    /// Nerfstudio `applied_transform` (identity when absent).
    pub applied_transform: AppliedTransform,
    /// Compute `train_to_normalized` / `train_frame_scale` metadata.
    pub compute_normalize: bool,
}

impl Default for SceneApplyConfig {
    fn default() -> Self {
        Self {
            center_mode: "none".into(),
            center_auto_threshold: 20.0,
            center: None,
            applied_transform: AppliedTransform::identity(),
            compute_normalize: true,
        }
    }
}

fn collect_c2w(frames: &[LoadedFrame]) -> Vec<[f64; 12]> {
    frames.iter().map(|f| f.c2w).collect()
}

fn collect_points(points: &ColmapPoints3D) -> Vec<[f64; 3]> {
    points.points.iter().map(|p| p.xyz).collect()
}

fn parse_center(
    cfg: &SceneApplyConfig,
    c2w: &[[f64; 12]],
    points: &[[f64; 3]],
) -> Result<ResolvedCenter, DomainError> {
    if let Some(c) = cfg.center {
        if cfg.center_mode == "auto" {
            return Err(err(
                ErrorCode::InvalidArgument,
                "scene_center",
                "explicit center cannot pair with mode auto",
            ));
        }
        let mode = CenterMode::from_name(&cfg.center_mode)?;
        let center = Vec3::try_new(c[0], c[1], c[2])?;
        return Ok(ResolvedCenter { mode, center });
    }
    resolve_scene_center(&cfg.center_mode, cfg.center_auto_threshold, c2w, points)
}

/// Up → +Z align + unit-cube scale. Returns `(scale_factor, T_n_from_camera[16], R_align[9])`.
///
/// Pin: `DatasetCommon.cpp::compute_normalized_transform` (no EXIF leveling here).
pub fn compute_normalized_transform(
    c2w: &[[f64; 12]],
) -> Result<(f64, [f64; 16], [f64; 9]), DomainError> {
    let mut t_out = [0.0; 16];
    t_out[0] = 1.0;
    t_out[5] = 1.0;
    t_out[10] = 1.0;
    t_out[15] = 1.0;
    let mut r_out = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0];
    let n = c2w.len();
    if n == 0 {
        return Ok((1.0, t_out, r_out));
    }
    let mut up = [0.0; 3];
    let mut center = [0.0; 3];
    for m in c2w {
        // Default OpenGL up = +Y column of c2w.
        for r in 0..3 {
            up[r] += m[r * 4 + 1];
            center[r] += m[r * 4 + 3];
        }
    }
    let nf = n as f64;
    let un = (up[0] * up[0] + up[1] * up[1] + up[2] * up[2])
        .sqrt()
        .max(1e-12);
    for d in 0..3 {
        up[d] /= un;
        center[d] /= nf;
    }
    // Rodrigues: rotate `up` onto +Z. axis = up × z = (up.y, -up.x, 0)
    let mut axis = [up[1], -up[0], 0.0];
    let s = (axis[0] * axis[0] + axis[1] * axis[1]).sqrt();
    let c = up[2];
    let mut r = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    if s > 1e-12 {
        for a in &mut axis {
            *a /= s;
        }
        let c1 = 1.0 - c;
        // Pin: DatasetCommon.cpp Rodrigues layout.
        r = [
            [
                c + axis[0] * axis[0] * c1,
                axis[0] * axis[1] * c1 - axis[2] * s,
                axis[1] * s,
            ],
            [
                axis[0] * axis[1] * c1 + axis[2] * s,
                c + axis[1] * axis[1] * c1,
                -axis[0] * s,
            ],
            [-axis[1] * s, axis[0] * s, c],
        ];
    } else if c < 0.0 {
        r[1][1] = -1.0;
        r[2][2] = -1.0;
    }
    for (row_i, row) in r.iter().enumerate() {
        for (col_i, val) in row.iter().enumerate() {
            r_out[row_i * 3 + col_i] = *val;
        }
    }
    let mut max_abs: f64 = 0.0;
    for m in c2w {
        let d = [m[3] - center[0], m[7] - center[1], m[11] - center[2]];
        for row in &r {
            let q = row[0] * d[0] + row[1] * d[1] + row[2] * d[2];
            max_abs = max_abs.max(q.abs());
        }
    }
    let scale_factor = if max_abs > 1e-9 { 1.0 / max_abs } else { 1.0 };
    for row in 0..3 {
        let mut t = 0.0;
        for col in 0..3 {
            t_out[row * 4 + col] = r[row][col] * scale_factor;
            t -= r[row][col] * center[col];
        }
        t_out[row * 4 + 3] = t * scale_factor;
    }
    Ok((scale_factor, t_out, r_out))
}

fn invert_affine4(m: &[f64; 16]) -> Result<[f64; 16], DomainError> {
    // Invert 3×3 R/s block + translation for affine last row [0,0,0,1].
    let r = Mat3::try_from_row_major([m[0], m[1], m[2], m[4], m[5], m[6], m[8], m[9], m[10]])?;
    let r_inv = r.try_inverse()?;
    let t = Vec3::try_new(m[3], m[7], m[11])?;
    let t_inv = r_inv.try_mul_vec(t)?.try_scale(-1.0)?;
    let rm = r_inv.row_major();
    let ta = t_inv.to_array();
    Ok([
        rm[0], rm[1], rm[2], ta[0], rm[3], rm[4], rm[5], ta[1], rm[6], rm[7], rm[8], ta[2], 0.0,
        0.0, 0.0, 1.0,
    ])
}

/// Undo applied_transform, resolve/apply scene center, fill normalize metadata.
pub fn apply_scene_to_dataset(
    ds: &mut ParsedDataset,
    cfg: &SceneApplyConfig,
) -> Result<SceneTransform, DomainError> {
    let applied = cfg.applied_transform;
    let undo = if applied.is_identity() {
        AppliedTransform::identity()
    } else {
        applied.inverse()?
    };

    if !undo.is_identity() {
        for frame in &mut ds.frames {
            frame.c2w = undo.apply_c2w(&frame.c2w);
        }
        for p in &mut ds.points.points {
            p.xyz = undo.apply_point(p.xyz);
        }
    }

    let c2w = collect_c2w(&ds.frames);
    let pts = collect_points(&ds.points);
    let resolved = parse_center(cfg, &c2w, &pts)?;
    let center = resolved.center.to_array();

    // center_json = A @ center + b in the transforms.json / file frame.
    let center_json = applied.apply_point(center);

    for frame in &mut ds.frames {
        frame.c2w[3] -= center[0];
        frame.c2w[7] -= center[1];
        frame.c2w[11] -= center[2];
    }
    for p in &mut ds.points.points {
        p.xyz[0] -= center[0];
        p.xyz[1] -= center[1];
        p.xyz[2] -= center[2];
    }

    ds.center_mode = resolved.mode.as_str().to_owned();
    ds.center = center;
    ds.center_json = center_json;
    ds.raw_to_file = [
        applied.a[0][0],
        applied.a[0][1],
        applied.a[0][2],
        applied.b[0],
        applied.a[1][0],
        applied.a[1][1],
        applied.a[1][2],
        applied.b[1],
        applied.a[2][0],
        applied.a[2][1],
        applied.a[2][2],
        applied.b[2],
        0.0,
        0.0,
        0.0,
        1.0,
    ];

    if cfg.compute_normalize {
        // Normalize over file-frame poses (C++ uses c2w_all after center_json subtract).
        // Reconstruct file-frame centered poses ≈ applied @ (world_centered).
        let mut file_c2w = Vec::with_capacity(ds.frames.len());
        for frame in &ds.frames {
            // After undo+center, frame is world-centered. File-centered pose is
            // A @ world_centered (no translation b):
            // (A p + b) - (A c + b) = A (p - c).
            let linear = AppliedTransform {
                a: applied.a,
                b: [0.0; 3],
            };
            file_c2w.push(linear.apply_c2w(&frame.c2w));
        }
        let (scale_factor, t_n, r_align) = compute_normalized_transform(&file_c2w)?;
        let t_inv = invert_affine4(&t_n)?;
        ds.train_frame_scale = if scale_factor != 0.0 {
            (1.0 / scale_factor) as f32
        } else {
            1.0
        };
        ds.train_to_normalized = t_inv.map(|v| v as f32);
        ds.normalized_rotation = r_align.map(|v| v as f32);
    }

    // Active train_from_world for pure centering: R=I, s=1, t=-center.
    SceneTransform::from_center(resolved.center)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::colmap::types::RedistortSource;
    use crate::colmap::types::{
        ColmapPoint3D, ColmapPoints3D, DatasetFormat, DatasetLoadSummary, LoadedFrame, SplitIndices,
    };
    use spirula_core::{CameraModel, DISTORTION_PARAMS, DistortionTier};
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    fn empty_frame(c2w: [f64; 12]) -> LoadedFrame {
        LoadedFrame {
            image_id: 1,
            camera_id: 1,
            name: "a.jpg".into(),
            path: PathBuf::from("a.jpg"),
            c2w,
            width: 1,
            height: 1,
            intrins: [1.0; 4],
            model: CameraModel::Pinhole,
            distortion: DistortionTier::None,
            dist_coeffs: [0.0; DISTORTION_PARAMS],
            redistort: RedistortSource::default(),
            observations: vec![],
        }
    }

    #[test]
    fn centering_subtracts_from_poses_and_points() {
        let mut ds = ParsedDataset {
            dataset_dir: PathBuf::from("."),
            recon_dir: PathBuf::from("."),
            format: DatasetFormat::Colmap,
            cameras: BTreeMap::new(),
            frames: vec![empty_frame([
                1.0, 0.0, 0.0, 10.0, 0.0, 1.0, 0.0, 20.0, 0.0, 0.0, 1.0, 30.0,
            ])],
            points: ColmapPoints3D {
                points: vec![ColmapPoint3D {
                    point3d_id: 1,
                    xyz: [10.0, 20.0, 30.0],
                    rgb: [0; 3],
                    error: 0.0,
                    track: vec![],
                }],
            },
            split: SplitIndices::default(),
            summary: DatasetLoadSummary {
                path: ".".into(),
                format: "colmap".into(),
                recon_dir: ".".into(),
                num_cameras: 0,
                num_images: 1,
                num_points: 1,
                num_train: 0,
                num_eval: 0,
                num_validation: 0,
            },
            center_mode: "none".into(),
            center: [0.0; 3],
            center_json: [0.0; 3],
            raw_to_file: [
                1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
            ],
            train_frame_scale: 1.0,
            train_to_normalized: [
                1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
            ],
            normalized_rotation: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
        };
        let cfg = SceneApplyConfig {
            center_mode: "point-mean".into(),
            compute_normalize: false,
            ..SceneApplyConfig::default()
        };
        apply_scene_to_dataset(&mut ds, &cfg).unwrap();
        assert_eq!(ds.center, [10.0, 20.0, 30.0]);
        assert_eq!(ds.frames[0].c2w[3], 0.0);
        assert_eq!(ds.points.points[0].xyz, [0.0, 0.0, 0.0]);
    }

    #[test]
    fn applied_transform_undo_then_center() {
        let at = AppliedTransform {
            a: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            b: [10.0, 20.0, 30.0],
        };
        // Pose/point already in applied frame: world = (0,0,0) → file (10,20,30).
        let mut ds = ParsedDataset {
            dataset_dir: PathBuf::from("."),
            recon_dir: PathBuf::from("."),
            format: DatasetFormat::Nerfstudio,
            cameras: BTreeMap::new(),
            frames: vec![empty_frame([
                1.0, 0.0, 0.0, 10.0, 0.0, 1.0, 0.0, 20.0, 0.0, 0.0, 1.0, 30.0,
            ])],
            points: ColmapPoints3D {
                points: vec![ColmapPoint3D {
                    point3d_id: 1,
                    xyz: [10.0, 20.0, 30.0],
                    rgb: [0; 3],
                    error: 0.0,
                    track: vec![],
                }],
            },
            split: SplitIndices::default(),
            summary: DatasetLoadSummary {
                path: ".".into(),
                format: "nerfstudio".into(),
                recon_dir: ".".into(),
                num_cameras: 0,
                num_images: 1,
                num_points: 1,
                num_train: 0,
                num_eval: 0,
                num_validation: 0,
            },
            center_mode: "none".into(),
            center: [0.0; 3],
            center_json: [0.0; 3],
            raw_to_file: [
                1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
            ],
            train_frame_scale: 1.0,
            train_to_normalized: [
                1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
            ],
            normalized_rotation: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
        };
        let cfg = SceneApplyConfig {
            center_mode: "none".into(),
            applied_transform: at,
            compute_normalize: false,
            ..SceneApplyConfig::default()
        };
        apply_scene_to_dataset(&mut ds, &cfg).unwrap();
        // Undo brings world origin; center none keeps it.
        assert!((ds.frames[0].c2w[3]).abs() < 1e-12);
        assert_eq!(ds.points.points[0].xyz, [0.0, 0.0, 0.0]);
        assert_eq!(ds.center_json, [10.0, 20.0, 30.0]); // A@0+b with center 0? center is 0
    }
}
