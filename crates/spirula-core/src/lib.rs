//! Stable domain contracts shared by the Spirula Studio rewrite.
//!
//! This crate must remain independent of Slint, `wgpu`, filesystem formats,
//! and application process state. P1-F01 owns the error catalog. P1-F02 owns
//! the training-config table and document API described in
//! `docs/phase-1/config.md`. Writers call [`TrainConfig::validate`] and
//! return [`DomainError`] instead of emitting a nonempty closed choice the
//! reader rejects. Empty strings are the `null` encoding and stay writable.
//! P1-F03 owns the cancel token and progress payloads described in
//! `docs/phase-1/cancellation.md`. P1-F04 owns the CPU math, pinhole camera,
//! similarity, and splat value checks described in `docs/phase-1/geometry.md`.
//! P1-F08 owns the versioned resource/performance report contracts in
//! `docs/phase-1/performance.md`. P3-F01 owns full lens models / rays / source
//! fit surface in `docs/phase-3/camera.md` (HOLD). Scene files and splat files
//! stay in P3-F05 / P3-F06.

mod camera;
mod camera_math;
mod camera_model;
mod cancel;
mod config;
mod config_bind;
mod config_doc;
mod config_value;
mod distortion_fit;
mod error;
mod json_doc;
mod math;
mod perf;
mod presentation;
mod progress;
mod scene;
mod source_camera;

pub use camera::{CameraPose, OpenCvDistortion, PinholeCamera};
pub use camera_math::{
    FrameCamera, distort_point, generate_ray, project_ray, undistort_lens, undistort_point,
    valid_distortion,
};
pub use camera_model::{
    CameraModel, DISTORTION_PARAMS, DistortionTier, EXACT_FIT_PX, RedistortDecision,
    demote_distortion, has_distortion, is_compiled_pair, promote_distortion, redistort_decision,
};
pub use cancel::{CancelReason, CancelToken};
pub use config::{
    ChoiceKind, FieldSpec, FieldType, TRAIN_CONFIG_FIELD_COUNT, TRAIN_CONFIG_FIELDS,
    TRAIN_CONFIG_SECTIONS, TRAIN_CONFIG_TIERS, TrainConfig, field_spec,
};
pub use config_doc::{
    LoadedTrainConfig, TRAIN_CONFIG_SCHEMA_VERSION, UnknownField, field_catalog_json,
};
pub use distortion_fit::{
    FitOptions, FitResult, FitTarget, FittedWithSource, K_EXACT_FIT_PX, SourceProject,
    fallback_fisheye_none, fit_camera, fit_camera_auto, fit_camera_auto_cancelable,
};
pub use error::{
    ContextKey, DomainError, DuplicateContextKey, EXIT_CANCELED, EXIT_FAILURE, EXIT_SUCCESS,
    EXIT_USAGE, ErrorCategory, ErrorCode, InvalidMessage, ProcessStage, UnknownContextKey,
    UnknownErrorCode, UserAction, process_exit_code,
};
pub use json_doc::{JsonKind, JsonNumber, JsonValue, parse_json};
pub use math::{Mat3, Quat, Vec2, Vec3};
pub use perf::{
    AccountedBytesTracker, AccountedMemory, DirtyStatus, EnvironmentInfo, PERF_REPORT_SCHEMA,
    PERF_REPORT_VERSION, PerfProvenance, PerfReport, PhaseTiming, SourceRevision,
    UnavailableMetric, parse_schema_version,
};
pub use presentation::{
    FrameCopyReport, PresentationPathKind, PresentedFrame, ProbeCorners, cpu_probe_corners,
    cpu_probe_rgba8, rgba8_corners, rgba8_matches_cpu_probe,
};
pub use progress::{
    OutcomeView, ProgressMetric, ProgressPublisher, ProgressSnapshot, TerminalOutcome,
};
pub use scene::{GaussianSplat, SH_C0, SceneTransform, SplatSet};
pub use source_camera::{
    COLMAP_DIVISION, COLMAP_EUCM, COLMAP_FOV, COLMAP_FULL_OPENCV,
    COLMAP_RAD_TAN_THIN_PRISM_FISHEYE, COLMAP_SIMPLE_DIVISION, MAX_PARAMS, SKEW_BASE_EQUISOLID,
    SKEW_BASE_FISHEYE, SKEW_BASE_PERSPECTIVE, SKEW_RADIAL_POLYNOMIAL, SKEW_RADIAL_RATIONAL, SKEWED,
    is_supported_source_model, project as source_project, rescale as source_rescale,
};
