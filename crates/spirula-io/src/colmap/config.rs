//! Load configuration for dataset reading (P3-F03/F04 subset of DatasetParserConfig).

use super::split::SplitConfig;

/// Options for [`super::load::load_colmap_dataset`] / [`crate::load_dataset`].
#[derive(Clone, Debug)]
pub struct ColmapLoadConfig {
    /// Reconstruction dir relative to dataset dir. Empty / `None` = auto-detect.
    pub recon_dir: Option<String>,
    pub image_dir: String,
    /// When true, frames whose image file is missing are skipped; all missing → error.
    pub require_image_files: bool,
    pub eval_mode: String,
    pub train_split_fraction: f32,
    pub eval_interval: i32,
    /// `"train"` | `"eval"` | `"all"`.
    pub split: String,
    pub validation_fraction: f32,
    /// Explicit format: `"colmap"` | `"nerfstudio"` | `"metashape"`. Empty = auto.
    pub format: Option<String>,
    /// Optional Metashape camera-export XML path (relative to dataset or absolute).
    pub metashape_xml: Option<String>,
    /// Optional Metashape PLY path.
    pub metashape_ply: Option<String>,
    /// Optional Metashape `.psx` path (typed unsupported without zip runtime).
    pub metashape_psx: Option<String>,
    /// Scene-center mode: `none` / named / `auto` (P3-F05). Default `none`.
    pub scene_center: String,
    /// Auto threshold in scene radii (C++ `center_auto_threshold`). Default 20.
    pub scene_center_threshold: f32,
    /// Explicit center override; when set, `scene_center` must not be `auto`.
    pub scene_center_explicit: Option<[f64; 3]>,
    /// When true (default), undo applied_transform (NS), apply center, fill normalize.
    pub apply_scene: bool,
}

impl Default for ColmapLoadConfig {
    fn default() -> Self {
        Self {
            recon_dir: None,
            image_dir: "images".into(),
            require_image_files: true,
            eval_mode: "all".into(),
            train_split_fraction: 0.9,
            eval_interval: 8,
            split: "train".into(),
            validation_fraction: 0.0,
            format: None,
            metashape_xml: None,
            metashape_ply: None,
            metashape_psx: None,
            scene_center: "none".into(),
            scene_center_threshold: 20.0,
            scene_center_explicit: None,
            apply_scene: true,
        }
    }
}

impl ColmapLoadConfig {
    pub fn split_config(&self) -> SplitConfig {
        SplitConfig {
            eval_mode: self.eval_mode.clone(),
            train_split_fraction: self.train_split_fraction,
            eval_interval: self.eval_interval,
            split: self.split.clone(),
            validation_fraction: self.validation_fraction,
        }
    }
}
