//! Load configuration for COLMAP dataset reading (P3-F03 subset of DatasetParserConfig).

use super::split::SplitConfig;

/// Options for [`super::load::load_colmap_dataset`].
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
