//! Dataset, image, checkpoint, and export adapters.
//!
//! P1-F05 owns atomic file publish, orphan recovery, the temporary-directory
//! harness, and config filesystem save/migration. P1-F08 owns performance
//! report persistence (JSON + atomic publish) and content hashing helpers.
//! P3-F02 owns image size/EXIF orientation and mask normalize (PNG/JPEG).
//! Checkpoint bundle schema and completion markers stay in P5-F05.

pub mod atomic;
pub mod config_fs;
pub mod fixtures;
pub mod hash;
pub mod image_mask;
pub mod image_orient;
pub mod image_probe;
pub mod perf;
pub mod sample_workload;
pub mod temp_harness;

pub use atomic::{
    PublishLease, PublishOptions, PublishOutcome, acquire_publish_lease, is_publish_temp,
    parent_dir, publish_bytes, publish_bytes_if_absent, publish_bytes_validated,
    publish_bytes_validated_on_lease, publish_writer, publish_writer_on_lease,
    recover_publish_orphans,
};
pub use config_fs::{load_train_config, migrate_train_config, save_train_config};
pub use hash::{sha256_file, sha256_hex};
pub use image_mask::{
    MaskNormalizeOptions, MaskSourceKind, decode_mask_image_bytes, nearest_resize_u8,
    normalize_mask,
};
pub use image_orient::{
    ApplyPolicy, ExifOrientation, ExifOrientationMode, ExifTransform, MAX_IMAGE_PIXELS,
    apply_orientation, exif_transform, orient_pixels, oriented_size, read_exif_orientations,
    read_jpeg_exif_orientation, read_path_exif_orientation,
};
pub use image_probe::{decode_rgb8, decode_rgb8_bytes, probe_image_size, probe_image_size_bytes};
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
