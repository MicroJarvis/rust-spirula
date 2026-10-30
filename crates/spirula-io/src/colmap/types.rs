//! COLMAP records and loaded dataset surface for P3-F03.

use std::collections::BTreeMap;
use std::path::PathBuf;

use spirula_core::{CameraModel, DISTORTION_PARAMS, DistortionTier, MAX_PARAMS};

/// Raw COLMAP camera record (text or binary).
#[derive(Clone, Debug, PartialEq)]
pub struct ColmapCamera {
    pub camera_id: i32,
    pub model: String,
    pub width: u64,
    pub height: u64,
    pub params: Vec<f64>,
}

/// One 2D observation on an image (`x`, `y`, optional `point3D_id`; `-1` = none).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ColmapObservation {
    pub x: f64,
    pub y: f64,
    pub point3d_id: i64,
}

/// Raw COLMAP image / frame record.
#[derive(Clone, Debug, PartialEq)]
pub struct ColmapImage {
    pub image_id: i32,
    /// World→camera quaternion `(w, x, y, z)`.
    pub qvec: [f64; 4],
    pub tvec: [f64; 3],
    pub camera_id: i32,
    pub name: String,
    pub observations: Vec<ColmapObservation>,
}

/// One track element on a 3D point (`image_id`, `point2D_idx`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ColmapTrackElem {
    pub image_id: i32,
    pub point2d_idx: i32,
}

/// One 3D point with optional track association.
#[derive(Clone, Debug, PartialEq)]
pub struct ColmapPoint3D {
    pub point3d_id: u64,
    pub xyz: [f64; 3],
    pub rgb: [u8; 3],
    pub error: f64,
    pub track: Vec<ColmapTrackElem>,
}

/// Points cloud + track table.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ColmapPoints3D {
    pub points: Vec<ColmapPoint3D>,
}

impl ColmapPoints3D {
    pub fn num(&self) -> usize {
        self.points.len()
    }

    pub fn xyz_flat(&self) -> Vec<f64> {
        let mut out = Vec::with_capacity(self.points.len() * 3);
        for p in &self.points {
            out.extend_from_slice(&p.xyz);
        }
        out
    }

    pub fn rgb_flat(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.points.len() * 3);
        for p in &self.points {
            out.extend_from_slice(&p.rgb);
        }
        out
    }
}

/// Redistort source retained when a COLMAP lens was fitted (P3-F01).
#[derive(Clone, Debug, PartialEq)]
pub struct RedistortSource {
    /// `-1` = exact fit / no redistort.
    pub source_model: i32,
    pub params: [f32; MAX_PARAMS],
    pub fit_max_px: f32,
}

impl Default for RedistortSource {
    fn default() -> Self {
        Self {
            source_model: -1,
            params: [0.0; MAX_PARAMS],
            fit_max_px: 0.0,
        }
    }
}

/// Baked intrinsics for one COLMAP camera id (after demote / optional fit).
#[derive(Clone, Debug, PartialEq)]
pub struct BakedIntrinsics {
    pub model: CameraModel,
    pub distortion: DistortionTier,
    pub fx: f32,
    pub fy: f32,
    pub cx: f32,
    pub cy: f32,
    pub dist: [f32; DISTORTION_PARAMS],
    pub source: RedistortSource,
}

impl Default for BakedIntrinsics {
    fn default() -> Self {
        Self {
            model: CameraModel::Pinhole,
            distortion: DistortionTier::None,
            fx: 0.0,
            fy: 0.0,
            cx: 0.0,
            cy: 0.0,
            dist: [0.0; DISTORTION_PARAMS],
            source: RedistortSource::default(),
        }
    }
}

/// One loaded frame after path resolve + filename sort + optional split.
#[derive(Clone, Debug, PartialEq)]
pub struct LoadedFrame {
    pub image_id: i32,
    pub camera_id: i32,
    pub name: String,
    pub path: PathBuf,
    /// OpenGL camera-to-world 3×4 row-major (`colmap_to_c2w`).
    pub c2w: [f64; 12],
    pub width: u32,
    pub height: u32,
    pub intrins: [f32; 4],
    pub model: CameraModel,
    pub distortion: DistortionTier,
    pub dist_coeffs: [f32; DISTORTION_PARAMS],
    pub redistort: RedistortSource,
    pub observations: Vec<ColmapObservation>,
}

/// Train / eval / validation index lists over [`ParsedDataset::frames`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SplitIndices {
    pub train: Vec<usize>,
    pub eval: Vec<usize>,
    pub validation: Vec<usize>,
}

/// Detected dataset format for probe / auto-detect.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DatasetFormat {
    Colmap,
    Nerfstudio,
    Metashape,
}

impl DatasetFormat {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Colmap => "colmap",
            Self::Nerfstudio => "nerfstudio",
            Self::Metashape => "metashape",
        }
    }
}

/// Probe outcome before a full parse.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DatasetProbe {
    /// COLMAP reconstruction available at `recon_dir` (relative or absolute).
    Available {
        format: DatasetFormat,
        recon_dir: PathBuf,
    },
    /// Format recognized but not loadable (e.g. Metashape `.psx` without zip).
    Unavailable {
        format: DatasetFormat,
        reason: String,
    },
    /// No supported markers.
    NotADataset { hint: String },
}

/// Compact summary published on `DatasetLoaded` (no pixel buffers).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DatasetLoadSummary {
    pub path: String,
    pub format: String,
    pub recon_dir: String,
    pub num_cameras: u64,
    pub num_images: u64,
    pub num_points: u64,
    pub num_train: u64,
    pub num_eval: u64,
    pub num_validation: u64,
}

/// Full parse result (P3-F03) with optional scene-center / normalize (P3-F05).
#[derive(Clone, Debug, PartialEq)]
pub struct ParsedDataset {
    pub dataset_dir: PathBuf,
    pub recon_dir: PathBuf,
    pub format: DatasetFormat,
    pub cameras: BTreeMap<i32, ColmapCamera>,
    pub frames: Vec<LoadedFrame>,
    pub points: ColmapPoints3D,
    pub split: SplitIndices,
    pub summary: DatasetLoadSummary,
    /// Resolved centering mode name (`none`, `point-median`, …).
    pub center_mode: String,
    /// Center subtracted from world-frame poses/points.
    pub center: [f64; 3],
    /// Same center expressed in the file / `applied_transform` frame (`A c + b`).
    pub center_json: [f64; 3],
    /// Raw → file affine 4×4 (`applied_transform`; identity for COLMAP).
    pub raw_to_file: [f64; 16],
    /// `1 / scale_factor` of the would-be normalized frame.
    pub train_frame_scale: f32,
    /// `inv(T_n_from_train)` row-major 4×4 (viewer remap).
    pub train_to_normalized: [f32; 16],
    /// Up→+Z rotation inside normalize, row-major 3×3.
    pub normalized_rotation: [f32; 9],
}

impl ParsedDataset {
    /// Identity scene metadata (pre-centering).
    pub fn with_identity_scene(mut self) -> Self {
        self.center_mode = "none".into();
        self.center = [0.0; 3];
        self.center_json = [0.0; 3];
        self.raw_to_file = [
            1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
        ];
        self.train_frame_scale = 1.0;
        self.train_to_normalized = [
            1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
        ];
        self.normalized_rotation = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0];
        self
    }
}

/// Per-file encoding preference after probing a reconstruction dir.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ColmapFileFmt {
    #[default]
    None,
    Bin,
    Text,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ColmapModelFmt {
    pub cameras: ColmapFileFmt,
    pub images: ColmapFileFmt,
    pub points3d: ColmapFileFmt,
}

impl ColmapModelFmt {
    pub fn has_recon(self) -> bool {
        self.cameras != ColmapFileFmt::None && self.images != ColmapFileFmt::None
    }
}
