use spirula_core::{
    ContextKey, DomainError, EXIT_CANCELED, EXIT_FAILURE, EXIT_SUCCESS, EXIT_USAGE, ErrorCategory,
    ErrorCode, ProcessStage, UserAction, process_exit_code,
};
use std::error::Error;
use std::fmt;

const CATALOG: &[(&str, &str, bool, &str)] = &[
    (
        "SP-INP-001",
        "input",
        false,
        "invalid argument, option or value",
    ),
    ("SP-INP-002", "input", false, "unknown CLI flag or command"),
    (
        "SP-INP-003",
        "input",
        false,
        "value outside the accepted range / choices",
    ),
    (
        "SP-INP-004",
        "input",
        false,
        "recognized but not implemented option",
    ),
    (
        "SP-IO-001",
        "not_found",
        false,
        "file or directory does not exist",
    ),
    ("SP-IO-002", "permission", false, "permission denied"),
    ("SP-IO-003", "io", true, "read failed (transient I/O)"),
    ("SP-IO-004", "io", true, "write failed (transient I/O)"),
    (
        "SP-IO-005",
        "data_decode",
        true,
        "image/map could not be decoded",
    ),
    (
        "SP-FMT-001",
        "unsupported",
        false,
        "format is not supported",
    ),
    (
        "SP-FMT-002",
        "invalid_data",
        false,
        "malformed record/field",
    ),
    (
        "SP-FMT-003",
        "unsupported",
        false,
        "unsupported schema/format version",
    ),
    (
        "SP-FMT-004",
        "invalid_data",
        false,
        "checksum/integrity mismatch",
    ),
    (
        "SP-SESS-001",
        "conflict",
        false,
        "command is invalid in the current state",
    ),
    (
        "SP-SESS-002",
        "conflict",
        false,
        "session is not ready for the command",
    ),
    (
        "SP-SESS-003",
        "conflict",
        true,
        "a mutating command is already in flight",
    ),
    (
        "SP-CAN-001",
        "canceled",
        false,
        "cooperative cancellation completed",
    ),
    (
        "SP-GPU-001",
        "gpu_unavailable",
        false,
        "no adapter satisfies the request",
    ),
    (
        "SP-GPU-002",
        "gpu_device_lost",
        false,
        "device lost or stalled; recreate required",
    ),
    (
        "SP-GPU-003",
        "gpu_submit",
        true,
        "submission or wait failed",
    ),
    ("SP-GPU-004", "resource", false, "device allocation failed"),
    (
        "SP-GPU-005",
        "unsupported",
        false,
        "shader compile or pipeline layout failed",
    ),
    (
        "SP-CKPT-001",
        "unsupported",
        false,
        "checkpoint is not resumable (inference-only)",
    ),
    (
        "SP-CKPT-002",
        "invalid_data",
        false,
        "checkpoint is corrupt",
    ),
    (
        "SP-CKPT-003",
        "invalid_data",
        false,
        "checkpoint is incomplete (partial write)",
    ),
    (
        "SP-INT-001",
        "internal",
        false,
        "invariant violation / caught panic",
    ),
];

#[test]
fn catalog_matches_runtime_contract() {
    assert_eq!(ErrorCode::ALL.len(), 26);
    assert_eq!(CATALOG.len(), ErrorCode::ALL.len());

    let mut seen = std::collections::BTreeSet::new();
    for (code, (text, category, retryable, meaning)) in ErrorCode::ALL.iter().zip(CATALOG) {
        assert!(seen.insert(*text), "duplicate code {text}");
        assert_eq!(code.as_str(), *text);
        assert_eq!(code.to_string(), *text);
        assert_eq!(code.category().as_str(), *category);
        assert_eq!(code.retryable(), *retryable);
        assert_eq!(code.meaning(), *meaning);
        assert_eq!(ErrorCode::parse(text).unwrap(), *code);
        assert_eq!(text.parse::<ErrorCode>().unwrap(), *code);
        assert_eq!(ErrorCategory::parse(category).unwrap().as_str(), *category);
    }
}

#[test]
fn unknown_codes_and_keys_are_rejected() {
    for text in ["SP-INP-005", "SP-NOPE-001", "sp-inp-001", "SP-IO-001 ", ""] {
        let error = ErrorCode::parse(text).unwrap_err();
        assert_eq!(error.raw(), text);
        assert!(error.to_string().contains(text));
    }

    let error = ContextKey::parse("filename").unwrap_err();
    assert_eq!(error.raw(), "filename");
    assert!(error.to_string().contains("filename"));
    assert!(ContextKey::parse("path").is_ok());
}

#[test]
fn errors_match_on_code_category_and_display_message() {
    let error = DomainError::try_new(ErrorCode::DeviceLost, "device lost").unwrap();

    match error.code() {
        ErrorCode::DeviceLost => {}
        other => panic!("unexpected code {other}"),
    }
    match error.category() {
        ErrorCategory::GpuDeviceLost => {}
        other => panic!("unexpected category {other}"),
    }
    assert_eq!(error.to_string(), "device lost");
    assert!(!error.to_string().contains("SP-GPU-002"));
    assert!(!error.retryable());
    assert_eq!(error.user_action(), None);
    assert!(error.context().is_empty());
}

#[test]
fn empty_messages_and_duplicate_context_are_rejected() {
    for message in ["", " ", "\n\t"] {
        let error = DomainError::try_new(ErrorCode::InvalidArgument, message).unwrap_err();
        assert_eq!(error.message(), message);
    }

    let mut error = DomainError::try_new(ErrorCode::MalformedRecord, "bad field").unwrap();
    error.insert_context(ContextKey::Field, "focal").unwrap();
    let duplicate = error
        .insert_context(ContextKey::Field, "other")
        .unwrap_err();
    assert_eq!(duplicate.key(), ContextKey::Field);
    assert_eq!(error.context_value(ContextKey::Field), Some("focal"));

    let error = error
        .with_context(ContextKey::Expected, "")
        .unwrap()
        .with_context(ContextKey::Line, "4")
        .unwrap();
    assert_eq!(error.context_value(ContextKey::Expected), Some(""));
    assert_eq!(error.context().len(), 3);
}

#[test]
fn retryable_refinement_does_not_change_the_catalog() {
    let error = DomainError::try_new(ErrorCode::ReadFailed, "read timed out")
        .unwrap()
        .with_retryable(false);
    assert!(error.catalog_retryable());
    assert!(error.code().retryable());
    assert!(!error.retryable());

    let still_retryable = DomainError::try_new(ErrorCode::SubmitFailed, "queue timeout").unwrap();
    assert!(still_retryable.retryable());
    assert!(!ErrorCode::DeviceLost.retryable());
}

#[test]
fn source_stays_local_and_event_json_uses_stable_fields() {
    #[derive(Debug)]
    struct IoCause;

    impl fmt::Display for IoCause {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("os said busy")
        }
    }

    impl Error for IoCause {}

    let error = DomainError::try_new(ErrorCode::DecodeFailed, "png \"a\"\nfailed")
        .unwrap()
        .with_context(ContextKey::Path, "frame.png")
        .unwrap()
        .with_context(ContextKey::Format, "png")
        .unwrap()
        .with_context(ContextKey::Codec, "png\\raw")
        .unwrap()
        .with_user_action(UserAction::Retry)
        .with_source(IoCause);

    assert_eq!(
        error.source().map(ToString::to_string).as_deref(),
        Some("os said busy")
    );
    let json = error.to_event_json();
    assert_eq!(
        json,
        concat!(
            r#"{"code":"SP-IO-005","category":"data_decode","message":"png \"a\"\nfailed","#,
            r#""retryable":true,"context":{"path":"frame.png","format":"png","codec":"png\\raw"},"#,
            r#""user_action":"retry"}"#
        )
    );
    assert!(!json.contains("os said busy"));
    assert!(!json.contains("source"));
}

#[test]
fn cli_text_and_exit_table_follow_the_contract() {
    let lost = DomainError::try_new(ErrorCode::DeviceLost, "device lost").unwrap();
    assert_eq!(lost.cli_stderr_line(false), "error: device lost");
    assert_eq!(
        lost.cli_stderr_line(true),
        "error: device lost (code: SP-GPU-002)"
    );
    assert_eq!(
        process_exit_code(&lost, ProcessStage::InSession),
        EXIT_FAILURE
    );

    let usage = DomainError::try_new(ErrorCode::UnknownCommand, "unknown flag --foo").unwrap();
    assert_eq!(
        process_exit_code(&usage, ProcessStage::BeforeSession),
        EXIT_USAGE
    );
    assert_eq!(
        process_exit_code(&usage, ProcessStage::InSession),
        EXIT_FAILURE
    );

    let canceled = DomainError::try_new(ErrorCode::Canceled, "canceled by user").unwrap();
    assert_eq!(
        process_exit_code(&canceled, ProcessStage::BeforeSession),
        EXIT_CANCELED
    );
    assert_eq!(
        process_exit_code(&canceled, ProcessStage::InSession),
        EXIT_CANCELED
    );
    assert_eq!(EXIT_SUCCESS, 0);
    assert_ne!(EXIT_CANCELED, EXIT_FAILURE);
}

#[test]
fn cli_stderr_line_escapes_line_separators_without_changing_message_or_json() {
    let message = "top\nbottom\r\nnel\u{0085}ls\u{2028}ps\u{2029}end";
    let error = DomainError::try_new(ErrorCode::InvalidArgument, message).unwrap();

    assert_eq!(error.message(), message);
    assert_eq!(error.to_string(), message);

    let plain = error.cli_stderr_line(false);
    let coded = error.cli_stderr_line(true);
    assert_eq!(
        plain,
        "error: top\\nbottom\\r\\nnel\\u0085ls\\u2028ps\\u2029end"
    );
    assert_eq!(
        coded,
        "error: top\\nbottom\\r\\nnel\\u0085ls\\u2028ps\\u2029end (code: SP-INP-001)"
    );
    for line in [&plain, &coded] {
        assert!(
            !line.chars().any(|character| {
                matches!(
                    character,
                    '\n' | '\r' | '\u{0085}' | '\u{2028}' | '\u{2029}'
                )
            }),
            "cli line broke into more than one physical line: {line:?}"
        );
    }

    let json = error.to_event_json();
    assert!(json.contains(r#""message":"top\nbottom\r\nnel\u0085ls"#));
    assert!(json.contains('\u{2028}'));
    assert!(json.contains('\u{2029}'));
    assert!(!json.contains(r#"\u2028"#));
    assert!(!json.contains(r#"\u2029"#));
}

#[test]
fn cli_stderr_line_escapes_vertical_tab_and_form_feed() {
    let message = "above\u{000b}mid\u{000c}below";
    let error = DomainError::try_new(ErrorCode::ReadFailed, message).unwrap();

    assert_eq!(error.message(), message);
    assert_eq!(error.to_string(), message);

    let plain = error.cli_stderr_line(false);
    let coded = error.cli_stderr_line(true);
    assert_eq!(plain, "error: above\\vmid\\fbelow");
    assert_eq!(coded, "error: above\\vmid\\fbelow (code: SP-IO-003)");
    for line in [&plain, &coded] {
        assert!(
            !line
                .chars()
                .any(|character| matches!(character, '\u{000b}' | '\u{000c}')),
            "cli line broke into more than one physical line: {line:?}"
        );
    }

    let json = error.to_event_json();
    assert!(json.contains(r#""message":"above\u000bmid\u000cbelow""#));
    assert!(!json.contains('\u{000b}'));
    assert!(!json.contains('\u{000c}'));
}

#[test]
fn suggested_actions_cover_only_clear_catalog_meanings() {
    assert_eq!(
        ErrorCode::ValueOutOfRange.suggested_user_action(),
        Some(UserAction::CorrectInput)
    );
    assert_eq!(
        ErrorCode::PermissionDenied.suggested_user_action(),
        Some(UserAction::CheckPermissions)
    );
    assert_eq!(
        ErrorCode::WriteFailed.suggested_user_action(),
        Some(UserAction::Retry)
    );
    assert_eq!(
        ErrorCode::DeviceLost.suggested_user_action(),
        Some(UserAction::RecreateDevice)
    );
    assert_eq!(
        ErrorCode::CheckpointIncomplete.suggested_user_action(),
        Some(UserAction::UseCompleteCheckpoint)
    );
    assert_eq!(
        ErrorCode::Internal.suggested_user_action(),
        Some(UserAction::ReportDefect)
    );
    assert_eq!(ErrorCode::NotFound.suggested_user_action(), None);
    assert_eq!(ErrorCode::ShaderFailed.suggested_user_action(), None);
    assert_eq!(ErrorCode::Canceled.suggested_user_action(), None);

    let stored = DomainError::try_new(ErrorCode::NotFound, "missing scene")
        .unwrap()
        .with_user_action(UserAction::CorrectInput);
    assert_eq!(stored.user_action(), Some(UserAction::CorrectInput));
    assert!(
        stored
            .to_event_json()
            .contains("\"user_action\":\"correct_input\"")
    );
}

#[test]
fn context_json_uses_catalog_key_order_not_insertion_order() {
    let error = DomainError::try_new(ErrorCode::IntegrityMismatch, "checksum mismatch")
        .unwrap()
        .with_context(ContextKey::Actual, "abc")
        .unwrap()
        .with_context(ContextKey::Checkpoint, "step-3")
        .unwrap()
        .with_context(ContextKey::Expected, "def")
        .unwrap();

    assert_eq!(
        error.to_event_json(),
        concat!(
            r#"{"code":"SP-FMT-004","category":"invalid_data","message":"checksum mismatch","#,
            r#""retryable":false,"context":{"expected":"def","actual":"abc","checkpoint":"step-3"}}"#
        )
    );
}
