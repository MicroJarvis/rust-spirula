//! Dataset, image, checkpoint, and export adapters.
//!
//! P1-F05 owns atomic file publish, orphan recovery, the temporary-directory
//! harness, and config filesystem save/migration. P1-F08 owns performance
//! report persistence (JSON + atomic publish) and content hashing helpers.
//! Checkpoint bundle schema and completion markers stay in P5-F05.

pub mod atomic;
pub mod camera_catalog;
pub mod config_fs;
pub mod fixtures;
pub mod hash;
pub mod perf;
pub mod sample_workload;
pub mod temp_harness;

pub use camera_catalog::{CameraCatalogEntry, parse_catalog_bytes, roundtrip_entry};

pub use atomic::{
    PublishLease, PublishOptions, PublishOutcome, acquire_publish_lease, is_publish_temp,
    parent_dir, publish_bytes, publish_bytes_if_absent, publish_bytes_validated,
    publish_bytes_validated_on_lease, publish_writer, publish_writer_on_lease,
    recover_publish_orphans,
};
pub use config_fs::{load_train_config, migrate_train_config, save_train_config};
pub use hash::{sha256_file, sha256_hex};
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
