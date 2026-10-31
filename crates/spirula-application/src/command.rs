//! Command payloads admitted by the P1-F07 session owner.

use crate::bound::check_label;
use crate::ids::CommandId;
use crate::limits::SessionLimits;
use spirula_core::{ContextKey, DomainError, ErrorCode};

/// Caller-facing command before admission assigns an id.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Command {
    pub payload: CommandPayload,
}

impl Command {
    pub fn new(payload: CommandPayload) -> Self {
        Self { payload }
    }
}

/// Session command surface. Probe* stay fake; P3-F02/P3-F03 paths are real IO.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CommandPayload {
    /// Fake dataset load. `label` is a non-empty probe tag, not a path read.
    ProbeLoad {
        label: String,
    },
    /// Fake multi-step work. Does not claim training capability.
    ProbeRun {
        steps: u64,
    },
    /// Real COLMAP dataset load via `spirula-io` (P3-F03). Idle|Ready|Failed.
    LoadDataset {
        path: String,
        /// Empty → auto-detect reconstruction dir.
        recon_dir: String,
        eval_mode: String,
        /// `train` | `eval` | `all`.
        split: String,
    },
    /// Real PNG/JPEG orient bake via `spirula-io` (P3-F02). Idle|Ready only.
    PrepareOrientedImage {
        path: String,
        /// `None` → read JPEG EXIF (non-JPEG → identity).
        orientation: Option<u8>,
        turns_only: bool,
    },
    /// Real mask normalize via `spirula-io` (P3-F02). Idle|Ready only.
    PrepareNormalizedMask {
        path: String,
        dst_w: u32,
        dst_h: u32,
        orientation: u8,
        turns_only: bool,
        flip: bool,
        /// `false` → NonZero file mask; `true` → AlphaThreshold128.
        alpha: bool,
        /// `mask_boundary_offset` as milli-fraction (`frac = milli / 1000`).
        boundary_offset_milli: i32,
    },
    /// Real headless GPU preview via render facade + atomic PNG (P4-F06).
    /// Idle|Ready only. Paths are bounded strings — no image payload in history.
    Preview {
        request_path: String,
        output_path: String,
    },
    Pause,
    Resume,
    Stop {
        save: bool,
    },
    Shutdown,
    /// Clear `Failed` back to `Idle`.
    Reset,
}

impl CommandPayload {
    pub(crate) fn name(&self) -> &'static str {
        match self {
            Self::ProbeLoad { .. } => "probe_load",
            Self::ProbeRun { .. } => "probe_run",
            Self::LoadDataset { .. } => "load_dataset",
            Self::PrepareOrientedImage { .. } => "prepare_oriented_image",
            Self::PrepareNormalizedMask { .. } => "prepare_normalized_mask",
            Self::Preview { .. } => "preview",
            Self::Pause => "pause",
            Self::Resume => "resume",
            Self::Stop { .. } => "stop",
            Self::Shutdown => "shutdown",
            Self::Reset => "reset",
        }
    }

    pub(crate) fn is_stop(&self) -> bool {
        matches!(self, Self::Stop { .. })
    }

    pub(crate) fn is_shutdown(&self) -> bool {
        matches!(self, Self::Shutdown)
    }

    pub(crate) fn is_mutating_probe(&self) -> bool {
        matches!(
            self,
            Self::ProbeLoad { .. }
                | Self::ProbeRun { .. }
                | Self::LoadDataset { .. }
                | Self::PrepareOrientedImage { .. }
                | Self::PrepareNormalizedMask { .. }
                | Self::Preview { .. }
        )
    }

    pub(crate) fn validate(&self, limits: &SessionLimits) -> Result<(), DomainError> {
        match self {
            Self::ProbeLoad { label } => check_label(label, limits),
            Self::ProbeRun { steps } => {
                if *steps == 0 {
                    return Err(DomainError::try_new(
                        ErrorCode::ValueOutOfRange,
                        "probe run steps must be > 0",
                    )
                    .expect("message")
                    .with_context(ContextKey::Field, "steps")
                    .expect("field")
                    .with_context(ContextKey::Expected, "> 0")
                    .expect("expected")
                    .with_context(ContextKey::Actual, "0")
                    .expect("actual"));
                }
                Ok(())
            }
            Self::LoadDataset {
                path,
                recon_dir,
                eval_mode,
                split,
            } => {
                crate::bound::check_image_path(path)?;
                crate::bound::check_recon_dir(recon_dir)?;
                crate::bound::check_load_dataset_enum("eval_mode", eval_mode)?;
                crate::bound::check_load_dataset_enum("split", split)?;
                match eval_mode.as_str() {
                    "all" | "fraction" | "interval" | "filename" => {}
                    _ => {
                        return Err(DomainError::try_new(
                            ErrorCode::InvalidArgument,
                            "unknown eval_mode (expected all|fraction|interval|filename)",
                        )
                        .expect("message")
                        .with_context(ContextKey::Field, "eval_mode")
                        .expect("field"));
                    }
                }
                match split.as_str() {
                    "train" | "eval" | "all" => {}
                    _ => {
                        return Err(DomainError::try_new(
                            ErrorCode::InvalidArgument,
                            "unknown split (expected train|eval|all)",
                        )
                        .expect("message")
                        .with_context(ContextKey::Field, "split")
                        .expect("field"));
                    }
                }
                let _ = limits;
                Ok(())
            }
            Self::PrepareOrientedImage {
                path, orientation, ..
            } => {
                crate::bound::check_image_path(path)?;
                if let Some(o) = orientation {
                    spirula_io::ExifOrientation::from_u8(*o).map(|_| ())?;
                }
                Ok(())
            }
            Self::PrepareNormalizedMask {
                path,
                dst_w,
                dst_h,
                orientation,
                ..
            } => {
                crate::bound::check_image_path(path)?;
                spirula_io::ExifOrientation::from_u8(*orientation).map(|_| ())?;
                spirula_io::ensure_pixel_budget(*dst_w, *dst_h)?;
                Ok(())
            }
            Self::Preview {
                request_path,
                output_path,
            } => {
                crate::bound::check_image_path(request_path)?;
                crate::bound::check_image_path(output_path)?;
                Ok(())
            }
            _ => Ok(()),
        }
    }

    /// Approximate retained size for history byte accounting.
    pub(crate) fn approx_bytes(&self) -> usize {
        match self {
            Self::ProbeLoad { label } => 16 + label.len(),
            Self::ProbeRun { .. } => 16,
            Self::LoadDataset {
                path,
                recon_dir,
                eval_mode,
                split,
            } => 32 + path.len() + recon_dir.len() + eval_mode.len() + split.len(),
            Self::PrepareOrientedImage { path, .. } => 24 + path.len(),
            Self::PrepareNormalizedMask { path, .. } => 40 + path.len(),
            Self::Preview {
                request_path,
                output_path,
            } => 32 + request_path.len() + output_path.len(),
            Self::Stop { .. } => 12,
            Self::Pause | Self::Resume | Self::Shutdown | Self::Reset => 8,
        }
    }
}

/// Immediate admission result. Callers never block on work.
#[derive(Debug)]
pub enum SubmitOutcome {
    Accepted { id: CommandId },
    Rejected { error: DomainError },
}
