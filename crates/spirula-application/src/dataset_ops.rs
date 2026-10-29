//! Application entry for P3-F03 COLMAP dataset load.

use std::path::Path;

use spirula_core::{CancelToken, DomainError, ErrorCode};
use spirula_io::{ColmapLoadConfig, DatasetLoadSummary, ParsedDataset, load_colmap_dataset};

use crate::session::ApplicationService;

/// Worker-facing load outcome.
#[derive(Debug)]
pub(crate) enum DatasetWorkResult {
    Ready(DatasetLoadSummary),
    Failed(DomainError),
    Canceled,
}

fn canceled(cancel: Option<&CancelToken>) -> bool {
    cancel.is_some_and(|c| c.is_cancelled())
}

pub(crate) fn run_load_colmap_cancelable(
    path: &Path,
    recon_dir: Option<&str>,
    eval_mode: &str,
    split: &str,
    cancel: Option<&CancelToken>,
) -> DatasetWorkResult {
    if canceled(cancel) {
        return DatasetWorkResult::Canceled;
    }
    let mut cfg = ColmapLoadConfig::default();
    if let Some(r) = recon_dir.filter(|s| !s.is_empty()) {
        cfg.recon_dir = Some(r.to_owned());
    }
    cfg.eval_mode = eval_mode.to_owned();
    cfg.split = split.to_owned();
    match load_colmap_dataset(path, &cfg, cancel) {
        Ok(ds) => {
            if canceled(cancel) {
                DatasetWorkResult::Canceled
            } else {
                DatasetWorkResult::Ready(ds.summary)
            }
        }
        Err(e) if e.code() == ErrorCode::Canceled => DatasetWorkResult::Canceled,
        Err(e) => DatasetWorkResult::Failed(e),
    }
}

impl ApplicationService {
    /// Sync COLMAP load through the Idle|Ready gate (same owner path as admitted command).
    pub fn load_colmap_dataset_file(
        &self,
        path: &Path,
        recon_dir: Option<&str>,
        eval_mode: &str,
        split: &str,
    ) -> Result<ParsedDataset, DomainError> {
        self.require_idle_or_ready()?;
        let mut cfg = ColmapLoadConfig::default();
        if let Some(r) = recon_dir.filter(|s| !s.is_empty()) {
            cfg.recon_dir = Some(r.to_owned());
        }
        cfg.eval_mode = eval_mode.to_owned();
        cfg.split = split.to_owned();
        load_colmap_dataset(path, &cfg, None)
    }
}
