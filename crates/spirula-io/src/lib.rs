//! Dataset, image, checkpoint, and export adapters.
//!
//! P1-F05 owns atomic file publish, orphan recovery, the temporary-directory
//! harness, and config filesystem save/migration. Checkpoint bundle schema and
//! completion markers stay in P5-F05.

pub mod atomic;
pub mod config_fs;
pub mod fixtures;
pub mod temp_harness;

pub use atomic::{
    PublishOptions, PublishOutcome, is_publish_temp, publish_bytes, publish_bytes_validated,
    publish_writer, recover_publish_orphans,
};
pub use config_fs::{load_train_config, migrate_train_config, save_train_config};
pub use temp_harness::TempHarness;
