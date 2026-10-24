//! Stable domain errors from the P0-F05 catalog.
//!
//! Codes, categories and default retryability come from
//! `docs/phase-0/runtime-contracts.md` section 1. This module does not define
//! a second catalog. P1-F01 decisions that fill unspecified details live in
//! section 1.4 of that document.

use std::error::Error as StdError;
use std::fmt;

/// Closed catalog of `SP-<AREA>-<NNN>` codes. A code is never reused.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ErrorCode {
    /// `SP-INP-001`: invalid argument, option or value.
    InvalidArgument,
    /// `SP-INP-002`: unknown CLI flag or command.
    UnknownCommand,
    /// `SP-INP-003`: value outside the accepted range or choices.
    ValueOutOfRange,
    /// `SP-INP-004`: recognized but not implemented option.
    UnimplementedOption,
    /// `SP-IO-001`: file or directory does not exist.
    NotFound,
    /// `SP-IO-002`: permission denied.
    PermissionDenied,
    /// `SP-IO-003`: read failed (transient I/O).
    ReadFailed,
    /// `SP-IO-004`: write failed (transient I/O).
    WriteFailed,
    /// `SP-IO-005`: image or map could not be decoded.
    DecodeFailed,
    /// `SP-FMT-001`: format is not supported.
    UnsupportedFormat,
    /// `SP-FMT-002`: malformed record or field.
    MalformedRecord,
    /// `SP-FMT-003`: unsupported schema or format version.
    UnsupportedVersion,
    /// `SP-FMT-004`: checksum or integrity mismatch.
    IntegrityMismatch,
    /// `SP-SESS-001`: command is invalid in the current state.
    InvalidState,
    /// `SP-SESS-002`: session is not ready for the command.
    SessionNotReady,
    /// `SP-SESS-003`: a mutating command is already in flight.
    CommandInFlight,
    /// `SP-CAN-001`: cooperative cancellation completed.
    Canceled,
    /// `SP-GPU-001`: no adapter satisfies the request.
    NoAdapter,
    /// `SP-GPU-002`: device lost or stalled; recreate required.
    DeviceLost,
    /// `SP-GPU-003`: submission or wait failed.
    SubmitFailed,
    /// `SP-GPU-004`: device allocation failed.
    AllocationFailed,
    /// `SP-GPU-005`: shader compile or pipeline layout failed.
    ShaderFailed,
    /// `SP-CKPT-001`: checkpoint is not resumable (inference-only).
    CheckpointNotResumable,
    /// `SP-CKPT-002`: checkpoint is corrupt.
    CheckpointCorrupt,
    /// `SP-CKPT-003`: checkpoint is incomplete (partial write).
    CheckpointIncomplete,
    /// `SP-INT-001`: invariant violation or a panic caught at a worker boundary.
    Internal,
}

/// Stable category stored beside a code. Several codes share one category.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ErrorCategory {
    Input,
    NotFound,
    Permission,
    Io,
    DataDecode,
    Unsupported,
    InvalidData,
    Conflict,
    Canceled,
    GpuUnavailable,
    GpuDeviceLost,
    GpuSubmit,
    Resource,
    Internal,
}

/// Optional prescribed follow-up. Absence means the catalog does not prescribe one.
///
/// Variants are append-only. See runtime-contracts.md section 1.4.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum UserAction {
    CorrectInput,
    CheckPermissions,
    Retry,
    RecreateDevice,
    UseCompleteCheckpoint,
    ReportDefect,
}

/// Stable context keys. Unknown keys are rejected; values stay opaque strings.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ContextKey {
    Path,
    Format,
    Field,
    Expected,
    Actual,
    Line,
    Step,
    Device,
    Resource,
    Checkpoint,
    Command,
    Codec,
}

/// Whether a process already owns a session. Selects exit 2 versus exit 1.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ProcessStage {
    /// Usage and invalid input reported before a session exists.
    BeforeSession,
    /// A session exists, so input failures use the generic failure exit.
    InSession,
}

/// Process exit codes from runtime-contracts.md section 1.3.
pub const EXIT_SUCCESS: u8 = 0;
pub const EXIT_FAILURE: u8 = 1;
pub const EXIT_USAGE: u8 = 2;
pub const EXIT_CANCELED: u8 = 130;

/// `ErrorCode::parse` rejected a string that is not in the catalog.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnknownErrorCode {
    raw: String,
}

/// `ContextKey::parse` rejected a string that is not a stable key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnknownContextKey {
    raw: String,
}

/// A context map rejected a second value for the same key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DuplicateContextKey {
    key: ContextKey,
}

/// `DomainError::try_new` rejected an empty or whitespace-only message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InvalidMessage {
    message: String,
}

/// Typed failure that crosses a public API or event boundary.
///
/// `source` stays on the local error chain and is never part of
/// [`DomainError::to_event_json`].
pub struct DomainError {
    code: ErrorCode,
    message: String,
    context: Vec<(ContextKey, String)>,
    retryable_override: Option<bool>,
    user_action: Option<UserAction>,
    source: Option<Box<dyn StdError + Send + Sync + 'static>>,
}

impl ErrorCode {
    /// Catalog order from runtime-contracts.md section 1.2.
    pub const ALL: [ErrorCode; 26] = [
        ErrorCode::InvalidArgument,
        ErrorCode::UnknownCommand,
        ErrorCode::ValueOutOfRange,
        ErrorCode::UnimplementedOption,
        ErrorCode::NotFound,
        ErrorCode::PermissionDenied,
        ErrorCode::ReadFailed,
        ErrorCode::WriteFailed,
        ErrorCode::DecodeFailed,
        ErrorCode::UnsupportedFormat,
        ErrorCode::MalformedRecord,
        ErrorCode::UnsupportedVersion,
        ErrorCode::IntegrityMismatch,
        ErrorCode::InvalidState,
        ErrorCode::SessionNotReady,
        ErrorCode::CommandInFlight,
        ErrorCode::Canceled,
        ErrorCode::NoAdapter,
        ErrorCode::DeviceLost,
        ErrorCode::SubmitFailed,
        ErrorCode::AllocationFailed,
        ErrorCode::ShaderFailed,
        ErrorCode::CheckpointNotResumable,
        ErrorCode::CheckpointCorrupt,
        ErrorCode::CheckpointIncomplete,
        ErrorCode::Internal,
    ];

    /// Stable `SP-<AREA>-<NNN>` text. Never reused for a different meaning.
    pub const fn as_str(self) -> &'static str {
        match self {
            ErrorCode::InvalidArgument => "SP-INP-001",
            ErrorCode::UnknownCommand => "SP-INP-002",
            ErrorCode::ValueOutOfRange => "SP-INP-003",
            ErrorCode::UnimplementedOption => "SP-INP-004",
            ErrorCode::NotFound => "SP-IO-001",
            ErrorCode::PermissionDenied => "SP-IO-002",
            ErrorCode::ReadFailed => "SP-IO-003",
            ErrorCode::WriteFailed => "SP-IO-004",
            ErrorCode::DecodeFailed => "SP-IO-005",
            ErrorCode::UnsupportedFormat => "SP-FMT-001",
            ErrorCode::MalformedRecord => "SP-FMT-002",
            ErrorCode::UnsupportedVersion => "SP-FMT-003",
            ErrorCode::IntegrityMismatch => "SP-FMT-004",
            ErrorCode::InvalidState => "SP-SESS-001",
            ErrorCode::SessionNotReady => "SP-SESS-002",
            ErrorCode::CommandInFlight => "SP-SESS-003",
            ErrorCode::Canceled => "SP-CAN-001",
            ErrorCode::NoAdapter => "SP-GPU-001",
            ErrorCode::DeviceLost => "SP-GPU-002",
            ErrorCode::SubmitFailed => "SP-GPU-003",
            ErrorCode::AllocationFailed => "SP-GPU-004",
            ErrorCode::ShaderFailed => "SP-GPU-005",
            ErrorCode::CheckpointNotResumable => "SP-CKPT-001",
            ErrorCode::CheckpointCorrupt => "SP-CKPT-002",
            ErrorCode::CheckpointIncomplete => "SP-CKPT-003",
            ErrorCode::Internal => "SP-INT-001",
        }
    }

    pub const fn category(self) -> ErrorCategory {
        match self {
            ErrorCode::InvalidArgument
            | ErrorCode::UnknownCommand
            | ErrorCode::ValueOutOfRange
            | ErrorCode::UnimplementedOption => ErrorCategory::Input,
            ErrorCode::NotFound => ErrorCategory::NotFound,
            ErrorCode::PermissionDenied => ErrorCategory::Permission,
            ErrorCode::ReadFailed | ErrorCode::WriteFailed => ErrorCategory::Io,
            ErrorCode::DecodeFailed => ErrorCategory::DataDecode,
            ErrorCode::UnsupportedFormat
            | ErrorCode::UnsupportedVersion
            | ErrorCode::ShaderFailed
            | ErrorCode::CheckpointNotResumable => ErrorCategory::Unsupported,
            ErrorCode::MalformedRecord
            | ErrorCode::IntegrityMismatch
            | ErrorCode::CheckpointCorrupt
            | ErrorCode::CheckpointIncomplete => ErrorCategory::InvalidData,
            ErrorCode::InvalidState | ErrorCode::SessionNotReady | ErrorCode::CommandInFlight => {
                ErrorCategory::Conflict
            }
            ErrorCode::Canceled => ErrorCategory::Canceled,
            ErrorCode::NoAdapter => ErrorCategory::GpuUnavailable,
            ErrorCode::DeviceLost => ErrorCategory::GpuDeviceLost,
            ErrorCode::SubmitFailed => ErrorCategory::GpuSubmit,
            ErrorCode::AllocationFailed => ErrorCategory::Resource,
            ErrorCode::Internal => ErrorCategory::Internal,
        }
    }

    /// Catalog default. An operation may refine this on [`DomainError`].
    pub const fn retryable(self) -> bool {
        matches!(
            self,
            ErrorCode::ReadFailed
                | ErrorCode::WriteFailed
                | ErrorCode::DecodeFailed
                | ErrorCode::CommandInFlight
                | ErrorCode::SubmitFailed
        )
    }

    /// English catalog meaning. Not a substitute for a caller-supplied message.
    pub const fn meaning(self) -> &'static str {
        match self {
            ErrorCode::InvalidArgument => "invalid argument, option or value",
            ErrorCode::UnknownCommand => "unknown CLI flag or command",
            ErrorCode::ValueOutOfRange => "value outside the accepted range / choices",
            ErrorCode::UnimplementedOption => "recognized but not implemented option",
            ErrorCode::NotFound => "file or directory does not exist",
            ErrorCode::PermissionDenied => "permission denied",
            ErrorCode::ReadFailed => "read failed (transient I/O)",
            ErrorCode::WriteFailed => "write failed (transient I/O)",
            ErrorCode::DecodeFailed => "image/map could not be decoded",
            ErrorCode::UnsupportedFormat => "format is not supported",
            ErrorCode::MalformedRecord => "malformed record/field",
            ErrorCode::UnsupportedVersion => "unsupported schema/format version",
            ErrorCode::IntegrityMismatch => "checksum/integrity mismatch",
            ErrorCode::InvalidState => "command is invalid in the current state",
            ErrorCode::SessionNotReady => "session is not ready for the command",
            ErrorCode::CommandInFlight => "a mutating command is already in flight",
            ErrorCode::Canceled => "cooperative cancellation completed",
            ErrorCode::NoAdapter => "no adapter satisfies the request",
            ErrorCode::DeviceLost => "device lost or stalled; recreate required",
            ErrorCode::SubmitFailed => "submission or wait failed",
            ErrorCode::AllocationFailed => "device allocation failed",
            ErrorCode::ShaderFailed => "shader compile or pipeline layout failed",
            ErrorCode::CheckpointNotResumable => "checkpoint is not resumable (inference-only)",
            ErrorCode::CheckpointCorrupt => "checkpoint is corrupt",
            ErrorCode::CheckpointIncomplete => "checkpoint is incomplete (partial write)",
            ErrorCode::Internal => "invariant violation / caught panic",
        }
    }

    /// Suggested action for this code, if the catalog meaning implies one.
    ///
    /// Callers may store a different action with [`DomainError::with_user_action`].
    pub const fn suggested_user_action(self) -> Option<UserAction> {
        match self {
            ErrorCode::InvalidArgument
            | ErrorCode::UnknownCommand
            | ErrorCode::ValueOutOfRange
            | ErrorCode::UnimplementedOption => Some(UserAction::CorrectInput),
            ErrorCode::PermissionDenied => Some(UserAction::CheckPermissions),
            ErrorCode::ReadFailed
            | ErrorCode::WriteFailed
            | ErrorCode::DecodeFailed
            | ErrorCode::CommandInFlight
            | ErrorCode::SubmitFailed => Some(UserAction::Retry),
            ErrorCode::DeviceLost => Some(UserAction::RecreateDevice),
            ErrorCode::CheckpointCorrupt | ErrorCode::CheckpointIncomplete => {
                Some(UserAction::UseCompleteCheckpoint)
            }
            ErrorCode::Internal => Some(UserAction::ReportDefect),
            ErrorCode::NotFound
            | ErrorCode::UnsupportedFormat
            | ErrorCode::MalformedRecord
            | ErrorCode::UnsupportedVersion
            | ErrorCode::IntegrityMismatch
            | ErrorCode::InvalidState
            | ErrorCode::SessionNotReady
            | ErrorCode::Canceled
            | ErrorCode::NoAdapter
            | ErrorCode::AllocationFailed
            | ErrorCode::ShaderFailed
            | ErrorCode::CheckpointNotResumable => None,
        }
    }

    pub fn parse(text: &str) -> Result<Self, UnknownErrorCode> {
        Self::ALL
            .into_iter()
            .find(|code| code.as_str() == text)
            .ok_or_else(|| UnknownErrorCode {
                raw: text.to_owned(),
            })
    }
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl std::str::FromStr for ErrorCode {
    type Err = UnknownErrorCode;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Self::parse(text)
    }
}

impl ErrorCategory {
    pub const ALL: [ErrorCategory; 14] = [
        ErrorCategory::Input,
        ErrorCategory::NotFound,
        ErrorCategory::Permission,
        ErrorCategory::Io,
        ErrorCategory::DataDecode,
        ErrorCategory::Unsupported,
        ErrorCategory::InvalidData,
        ErrorCategory::Conflict,
        ErrorCategory::Canceled,
        ErrorCategory::GpuUnavailable,
        ErrorCategory::GpuDeviceLost,
        ErrorCategory::GpuSubmit,
        ErrorCategory::Resource,
        ErrorCategory::Internal,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            ErrorCategory::Input => "input",
            ErrorCategory::NotFound => "not_found",
            ErrorCategory::Permission => "permission",
            ErrorCategory::Io => "io",
            ErrorCategory::DataDecode => "data_decode",
            ErrorCategory::Unsupported => "unsupported",
            ErrorCategory::InvalidData => "invalid_data",
            ErrorCategory::Conflict => "conflict",
            ErrorCategory::Canceled => "canceled",
            ErrorCategory::GpuUnavailable => "gpu_unavailable",
            ErrorCategory::GpuDeviceLost => "gpu_device_lost",
            ErrorCategory::GpuSubmit => "gpu_submit",
            ErrorCategory::Resource => "resource",
            ErrorCategory::Internal => "internal",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|category| category.as_str() == text)
    }
}

impl fmt::Display for ErrorCategory {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl UserAction {
    pub const fn as_str(self) -> &'static str {
        match self {
            UserAction::CorrectInput => "correct_input",
            UserAction::CheckPermissions => "check_permissions",
            UserAction::Retry => "retry",
            UserAction::RecreateDevice => "recreate_device",
            UserAction::UseCompleteCheckpoint => "use_complete_checkpoint",
            UserAction::ReportDefect => "report_defect",
        }
    }
}

impl fmt::Display for UserAction {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl ContextKey {
    pub const ALL: [ContextKey; 12] = [
        ContextKey::Path,
        ContextKey::Format,
        ContextKey::Field,
        ContextKey::Expected,
        ContextKey::Actual,
        ContextKey::Line,
        ContextKey::Step,
        ContextKey::Device,
        ContextKey::Resource,
        ContextKey::Checkpoint,
        ContextKey::Command,
        ContextKey::Codec,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            ContextKey::Path => "path",
            ContextKey::Format => "format",
            ContextKey::Field => "field",
            ContextKey::Expected => "expected",
            ContextKey::Actual => "actual",
            ContextKey::Line => "line",
            ContextKey::Step => "step",
            ContextKey::Device => "device",
            ContextKey::Resource => "resource",
            ContextKey::Checkpoint => "checkpoint",
            ContextKey::Command => "command",
            ContextKey::Codec => "codec",
        }
    }

    pub fn parse(text: &str) -> Result<Self, UnknownContextKey> {
        Self::ALL
            .into_iter()
            .find(|key| key.as_str() == text)
            .ok_or_else(|| UnknownContextKey {
                raw: text.to_owned(),
            })
    }
}

impl fmt::Display for ContextKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl UnknownErrorCode {
    pub fn raw(&self) -> &str {
        &self.raw
    }
}

impl fmt::Display for UnknownErrorCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "unknown error code `{}`", self.raw)
    }
}

impl StdError for UnknownErrorCode {}

impl UnknownContextKey {
    pub fn raw(&self) -> &str {
        &self.raw
    }
}

impl fmt::Display for UnknownContextKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "unknown context key `{}`", self.raw)
    }
}

impl StdError for UnknownContextKey {}

impl DuplicateContextKey {
    pub const fn key(self) -> ContextKey {
        self.key
    }
}

impl fmt::Display for DuplicateContextKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "duplicate context key `{}`", self.key)
    }
}

impl StdError for DuplicateContextKey {}

impl InvalidMessage {
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for InvalidMessage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "error message must contain non-whitespace text, got `{}`",
            self.message
        )
    }
}

impl StdError for InvalidMessage {}

impl DomainError {
    /// Build an error. `message` is human-readable and must not be empty.
    pub fn try_new(code: ErrorCode, message: impl Into<String>) -> Result<Self, InvalidMessage> {
        let message = message.into();
        if message.trim().is_empty() {
            return Err(InvalidMessage { message });
        }
        Ok(Self {
            code,
            message,
            context: Vec::new(),
            retryable_override: None,
            user_action: None,
            source: None,
        })
    }

    pub const fn code(&self) -> ErrorCode {
        self.code
    }

    pub const fn category(&self) -> ErrorCategory {
        self.code.category()
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    /// Catalog default, unless an operation refined it.
    pub fn retryable(&self) -> bool {
        self.retryable_override.unwrap_or(self.code.retryable())
    }

    pub const fn catalog_retryable(&self) -> bool {
        self.code.retryable()
    }

    pub const fn user_action(&self) -> Option<UserAction> {
        self.user_action
    }

    pub fn context(&self) -> &[(ContextKey, String)] {
        &self.context
    }

    pub fn context_value(&self, key: ContextKey) -> Option<&str> {
        self.context
            .iter()
            .find(|(candidate, _)| *candidate == key)
            .map(|(_, value)| value.as_str())
    }

    /// Insert one stable context entry. Duplicate keys are rejected.
    ///
    /// An empty value is allowed: the contract treats values as optional.
    pub fn insert_context(
        &mut self,
        key: ContextKey,
        value: impl Into<String>,
    ) -> Result<(), DuplicateContextKey> {
        if self.context.iter().any(|(candidate, _)| *candidate == key) {
            return Err(DuplicateContextKey { key });
        }
        self.context.push((key, value.into()));
        Ok(())
    }

    pub fn with_context(
        mut self,
        key: ContextKey,
        value: impl Into<String>,
    ) -> Result<Self, DuplicateContextKey> {
        self.insert_context(key, value)?;
        Ok(self)
    }

    /// Operation-level refinement. Does not change [`ErrorCode::retryable`].
    pub fn with_retryable(mut self, retryable: bool) -> Self {
        self.retryable_override = Some(retryable);
        self
    }

    pub fn with_user_action(mut self, action: UserAction) -> Self {
        self.user_action = Some(action);
        self
    }

    /// Attach a local cause. It is not copied into the event JSON.
    pub fn with_source(mut self, source: impl StdError + Send + Sync + 'static) -> Self {
        self.source = Some(Box::new(source));
        self
    }

    /// One stderr line. `show_code` is the `SPIRULA_ERROR_CODES=1` form.
    ///
    /// Core does not read the environment variable; the CLI adapter does.
    pub fn cli_stderr_line(&self, show_code: bool) -> String {
        if show_code {
            format!("error: {} (code: {})", self.message, self.code.as_str())
        } else {
            format!("error: {}", self.message)
        }
    }

    /// Structured event object. `source` is omitted.
    ///
    /// Key order is `code`, `category`, `message`, `retryable`, `context`,
    /// then `user_action` only when set. Context keys follow [`ContextKey::ALL`].
    pub fn to_event_json(&self) -> String {
        let mut json = String::new();
        json.push('{');
        push_json_key(&mut json, "code");
        push_json_string(&mut json, self.code.as_str());
        json.push(',');
        push_json_key(&mut json, "category");
        push_json_string(&mut json, self.category().as_str());
        json.push(',');
        push_json_key(&mut json, "message");
        push_json_string(&mut json, &self.message);
        json.push(',');
        push_json_key(&mut json, "retryable");
        json.push_str(if self.retryable() { "true" } else { "false" });
        json.push(',');
        push_json_key(&mut json, "context");
        json.push('{');
        let mut first = true;
        for key in ContextKey::ALL {
            let Some(value) = self.context_value(key) else {
                continue;
            };
            if !first {
                json.push(',');
            }
            first = false;
            push_json_key(&mut json, key.as_str());
            push_json_string(&mut json, value);
        }
        json.push('}');
        if let Some(action) = self.user_action {
            json.push(',');
            push_json_key(&mut json, "user_action");
            push_json_string(&mut json, action.as_str());
        }
        json.push('}');
        json
    }
}

impl fmt::Debug for DomainError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DomainError")
            .field("code", &self.code)
            .field("message", &self.message)
            .field("context", &self.context)
            .field("retryable", &self.retryable())
            .field("user_action", &self.user_action)
            .field("source", &self.source.as_ref().map(|_| "present"))
            .finish()
    }
}

impl fmt::Display for DomainError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl StdError for DomainError {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        self.source
            .as_ref()
            .map(|source| source.as_ref() as &(dyn StdError + 'static))
    }
}

/// Exit code for a failure. Success is [`EXIT_SUCCESS`] and is not an error.
///
/// Cooperative cancellation and SIGINT both use [`EXIT_CANCELED`]. Input
/// failures use [`EXIT_USAGE`] only while no session exists.
pub fn process_exit_code(error: &DomainError, stage: ProcessStage) -> u8 {
    if error.category() == ErrorCategory::Canceled {
        return EXIT_CANCELED;
    }
    if stage == ProcessStage::BeforeSession && error.category() == ErrorCategory::Input {
        return EXIT_USAGE;
    }
    EXIT_FAILURE
}

fn push_json_key(json: &mut String, key: &str) {
    push_json_string(json, key);
    json.push(':');
}

fn push_json_string(json: &mut String, value: &str) {
    json.push('"');
    for character in value.chars() {
        match character {
            '"' => json.push_str("\\\""),
            '\\' => json.push_str("\\\\"),
            '\n' => json.push_str("\\n"),
            '\r' => json.push_str("\\r"),
            '\t' => json.push_str("\\t"),
            control if control.is_control() => {
                json.push_str(&format!("\\u{value:04x}", value = u32::from(control)));
            }
            other => json.push(other),
        }
    }
    json.push('"');
}
