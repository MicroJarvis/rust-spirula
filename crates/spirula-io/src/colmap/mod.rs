//! COLMAP dataset reading (P3-F03).
//!
//! Production parsers for text + little-endian binary `cameras` / `images` /
//! `points3D`, directory probing, OpenGL c2w conversion, train/eval/validation
//! splits, and 18-model bake/fit via P3-F01. Scene center/normalize stays in
//! P3-F05; Nerfstudio/Metashape parsers stay in P3-F04 (typed unavailable).
//!
//! Pin: `9a24f09927944a795e7ba1bd8243a625bf419479`
//! (`ColmapParser.cpp`, `DatasetCommon.cpp`, `DatasetParser.h`).

mod bake;
mod binary;
mod budget;
mod config;
mod load;
mod models;
mod pose;
mod probe;
mod split;
mod text;
mod types;

pub use budget::{
    MAX_CAMERAS, MAX_COLMAP_FILE_BYTES, MAX_IMAGES, MAX_OBSERVATIONS, MAX_POINTS, MAX_TRACK_ELEMS,
};
pub use config::ColmapLoadConfig;
pub use load::{load_colmap_dataset, load_dataset};
pub use models::{
    COLMAP_MODELS, ColmapModelInfo, model_by_id, model_id_by_name, require_exact_param_count,
    require_model_by_name,
};
pub use pose::{colmap_to_c2w, qvec2rotmat};
pub use probe::{colmap_model_fmt, colmap_model_num_images, find_colmap_recon, probe_dataset};
pub use split::{SplitConfig, assign_val_split, compute_splits, linspace_indices, train_subset};
pub use types::{
    BakedIntrinsics, ColmapCamera, ColmapFileFmt, ColmapImage, ColmapModelFmt, ColmapObservation,
    ColmapPoint3D, ColmapPoints3D, ColmapTrackElem, DatasetFormat, DatasetLoadSummary,
    DatasetProbe, LoadedFrame, ParsedDataset, RedistortSource, SplitIndices,
};

// Re-export low-level readers for tests / tooling (not the fixture checker).
pub use bake::{bake_and_fit, bake_colmap_intrinsics};
pub use binary::{read_cameras_binary, read_images_binary, read_points3d_binary};
pub use text::{read_cameras_text, read_images_text, read_points3d_text};
