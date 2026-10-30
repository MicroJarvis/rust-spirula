//! Dataset, image, checkpoint, and export adapters.
//!
//! P1-F05 owns atomic file publish, orphan recovery, the temporary-directory
//! harness, and config filesystem save/migration. P1-F08 owns performance
//! report persistence (JSON + atomic publish) and content hashing helpers.
//! P3-F02 owns image decode (PNG/JPEG/BMP/TGA/TIFF/EXR), EXIF orientation,
//! mask normalize (flip/offset), and depth/normal aux maps.
//! P3-F03 owns COLMAP text/binary dataset reading (not `fixtures` checkers).
//! P3-F04 owns Nerfstudio / Metashape adapters + shared PLY vertex reader.
//! P3-F05 owns scene-transform / region JSON, scene-center apply, and normalize
//! metadata on [`ParsedDataset`]. Checkpoint bundle schema stays in P5-F05.

pub mod atomic;
pub mod camera_catalog;
pub mod cancel_checkpoint;
pub mod colmap;
pub mod config_fs;
pub mod fixtures;
pub mod hash;
pub mod image_depth;
pub mod image_edt;
pub mod image_mask;
pub mod image_normal;
pub mod image_orient;
pub mod image_probe;
pub mod metashape;
pub mod nerfstudio;
pub mod perf;
pub mod ply;
pub mod region_io;
pub mod sample_workload;
pub mod scene_apply;
pub mod scene_transform;
pub mod temp_harness;

pub use camera_catalog::{CameraCatalogEntry, parse_catalog_bytes, roundtrip_entry};
pub use cancel_checkpoint::{CANCEL_CHECK_INTERVAL, CANCEL_WORK_CHUNK, CancelCheckpoint};
pub use colmap::{
    ColmapLoadConfig, DatasetFormat, DatasetLoadSummary, DatasetProbe, ParsedDataset,
    load_colmap_dataset, load_colmap_dataset_budgeted, load_dataset, load_dataset_budgeted,
    probe_dataset,
};
pub use metashape::{load_metashape_dataset, load_metashape_dataset_budgeted};
pub use nerfstudio::{load_nerfstudio_dataset, load_nerfstudio_dataset_budgeted};
pub use ply::{read_ply_points, read_ply_points_budgeted};
pub use region_io::{
    parse_region_bytes, read_region_json, region_from_value, region_to_json, region_to_value,
    write_region_json,
};
pub use scene_apply::apply_scene_transform_to_dataset;
pub use scene_apply::{
    AppliedTransform, SceneApplyConfig, apply_scene_to_dataset, compute_normalized_transform,
};
pub use scene_transform::{
    GeoReference, SCENE_TRANSFORM_FORMAT, SCENE_TRANSFORM_VERSION, SceneTransformDoc,
    parse_scene_transform_bytes, read_scene_transform_json, scene_transform_to_json,
    write_scene_transform_json,
};

pub use atomic::{
    PublishLease, PublishOptions, PublishOutcome, acquire_publish_lease, is_publish_temp,
    parent_dir, publish_bytes, publish_bytes_if_absent, publish_bytes_validated,
    publish_bytes_validated_on_lease, publish_writer, publish_writer_on_lease,
    recover_publish_orphans,
};
pub use config_fs::{load_train_config, migrate_train_config, save_train_config};
pub use hash::{sha256_file, sha256_hex};
pub use image_depth::{cpu_bilinear_resize_u16, decode_depth16_bytes, normalize_depth16};
pub use image_edt::{
    apply_mask_boundary_offset_frac, apply_mask_boundary_offset_frac_cancelable,
    apply_mask_boundary_offset_px, apply_mask_boundary_offset_px_cancelable,
};
pub use image_mask::{
    MaskNormalizeOptions, MaskSourceKind, cpu_resize_u8, decode_mask_image_bytes,
    decode_mask_image_bytes_hinted, extract_alpha_plane, extract_nonzero_plane, nearest_resize_u8,
    normalize_mask, normalize_mask_cancelable, stbi_luma_u8,
};
pub use image_normal::{
    cpu_bilinear_resize_rgb8, decode_normal_rgb8_bytes, normalize_normal_rgb8, turn_normal_vectors,
};
pub use image_orient::{
    ApplyPolicy, ExifOrientation, ExifOrientationMode, ExifTransform, MAX_ENCODED_BYTES,
    MAX_IMAGE_PIXELS, apply_orientation, apply_orientation_cancelable, exif_transform,
    orient_pixels, orient_pixels_cancelable, oriented_size, read_exif_orientations,
    read_jpeg_exif_orientation, read_path_exif_orientation,
};
pub use image_probe::{
    DecodedRgb, decode_rgb_auto, decode_rgb_auto_bytes, decode_rgb8, decode_rgb8_bytes,
    decode_rgb8_bytes_hinted, ensure_pixel_budget, probe_image_size, probe_image_size_bytes,
    read_image_bytes_capped,
};
pub use perf::{
    load_perf_report, parse_perf_report_bytes, parse_perf_report_str, provenance_hashes,
    report_to_json, save_perf_report,
};
pub use sample_workload::{
    SAMPLE_ACCOUNTED_SOURCE, SAMPLE_CHUNK_BYTES, SAMPLE_ITERATIONS, SampleConfig, WorkloadResult,
    canonical_sample_config_json, canonical_sample_fixture, expected_workload_checksum,
    parse_sample_config, run_measured_workload,
};
pub use temp_harness::TempHarness;
