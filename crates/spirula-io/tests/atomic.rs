use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Barrier};
use std::thread;

use spirula_core::{
    CancelReason, CancelToken, ContextKey, ErrorCode, LoadedTrainConfig, TrainConfig,
};
use spirula_io::{
    PublishOptions, TempHarness, is_publish_temp, load_train_config, migrate_train_config,
    parent_dir, publish_bytes, publish_bytes_validated, publish_writer, recover_publish_orphans,
    save_train_config,
};

fn backup_path(path: &Path) -> PathBuf {
    let mut backup = path.as_os_str().to_owned();
    backup.push(".bak");
    PathBuf::from(backup)
}

#[test]
fn publish_creates_and_replaces_without_delete_window() {
    let harness = TempHarness::new().unwrap();
    let path = harness.prepare_file("data.bin").unwrap();
    fs::write(&path, b"old").unwrap();

    let outcome = publish_bytes(&path, b"new-bytes", PublishOptions::default()).unwrap();
    assert_eq!(outcome.bytes_written, 9);
    assert_eq!(fs::read(&path).unwrap(), b"new-bytes");
    assert!(recover_publish_orphans(&path).unwrap().is_empty());
}

#[test]
fn bare_relative_destination_normalizes_empty_parent_to_dot() {
    let harness = TempHarness::new().unwrap();
    let previous = std::env::current_dir().unwrap();
    std::env::set_current_dir(harness.path()).unwrap();
    let relative = Path::new("config.json");
    assert_eq!(parent_dir(relative), Path::new("."));
    fs::write(relative, b"old").unwrap();
    let outcome = publish_bytes(
        relative,
        b"fresh",
        PublishOptions::default().with_sync_directory(true),
    );
    std::env::set_current_dir(previous).unwrap();
    outcome.unwrap();
    assert_eq!(
        fs::read(harness.child("config.json").unwrap()).unwrap(),
        b"fresh"
    );
}

#[test]
fn validation_failure_leaves_destination_intact() {
    let harness = TempHarness::new().unwrap();
    let path = harness.prepare_file("dest.txt").unwrap();
    fs::write(&path, b"keep-me").unwrap();

    let error = publish_bytes_validated(&path, b"bad", PublishOptions::default(), |_| {
        Err(
            spirula_core::DomainError::try_new(ErrorCode::IntegrityMismatch, "payload rejected")
                .unwrap(),
        )
    })
    .unwrap_err();
    assert_eq!(error.code(), ErrorCode::IntegrityMismatch);
    assert_eq!(fs::read(&path).unwrap(), b"keep-me");
    assert!(
        harness
            .path()
            .read_dir()
            .unwrap()
            .filter_map(Result::ok)
            .all(|entry| !is_publish_temp(&path, &entry.path()))
    );
}

#[test]
fn interrupted_write_cleans_temp_and_keeps_destination() {
    let harness = TempHarness::new().unwrap();
    let path = harness.prepare_file("dest.bin").unwrap();
    fs::write(&path, b"stable").unwrap();

    let error = publish_writer(&path, PublishOptions::default(), |file| {
        file.write_all(b"partial").unwrap();
        Err(
            spirula_core::DomainError::try_new(ErrorCode::WriteFailed, "simulated interrupt")
                .unwrap(),
        )
    })
    .unwrap_err();
    assert_eq!(error.code(), ErrorCode::WriteFailed);
    assert_eq!(fs::read(&path).unwrap(), b"stable");
    assert!(recover_publish_orphans(&path).unwrap().is_empty());
}

#[test]
fn length_mismatch_is_integrity_failure_before_replace() {
    let harness = TempHarness::new().unwrap();
    let path = harness.prepare_file("dest.bin").unwrap();
    fs::write(&path, b"stable").unwrap();

    let error = publish_writer(&path, PublishOptions::default(), |file| {
        file.write_all(b"abc").unwrap();
        Ok(99)
    })
    .unwrap_err();
    assert_eq!(error.code(), ErrorCode::IntegrityMismatch);
    assert_eq!(
        error.context_value(ContextKey::Path),
        Some(path.to_str().unwrap())
    );
    assert_eq!(fs::read(&path).unwrap(), b"stable");
}

#[test]
fn cancellation_aborts_before_replace() {
    let harness = TempHarness::new().unwrap();
    let path = harness.prepare_file("dest.bin").unwrap();
    fs::write(&path, b"stable").unwrap();
    let token = CancelToken::new();
    token.request(CancelReason::User);

    let error = publish_bytes(
        &path,
        b"should-not-land",
        PublishOptions::default().with_cancel(&token),
    )
    .unwrap_err();
    assert_eq!(error.code(), ErrorCode::Canceled);
    assert_eq!(fs::read(&path).unwrap(), b"stable");
    assert!(recover_publish_orphans(&path).unwrap().is_empty());
}

#[test]
fn concurrent_publishers_leave_one_complete_payload() {
    let harness = TempHarness::new().unwrap();
    let path = Arc::new(harness.prepare_file("race.bin").unwrap());
    fs::write(path.as_ref(), b"seed").unwrap();
    let barrier = Arc::new(Barrier::new(8));
    let mut handles = Vec::new();
    for index in 0..8 {
        let path = Arc::clone(&path);
        let barrier = Arc::clone(&barrier);
        handles.push(thread::spawn(move || {
            let payload = format!("payload-{index:04}-{}", "x".repeat(256));
            barrier.wait();
            loop {
                match publish_bytes(path.as_ref(), payload.as_bytes(), PublishOptions::default()) {
                    Ok(_) => break,
                    Err(error) if error.code() == ErrorCode::CommandInFlight => continue,
                    Err(error) => panic!("unexpected publish failure: {error:?}"),
                }
            }
            payload
        }));
    }
    let written: Vec<String> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    let final_bytes = fs::read(path.as_ref()).unwrap();
    let final_text = String::from_utf8(final_bytes).unwrap();
    assert!(
        written.iter().any(|payload| payload == &final_text),
        "final file must equal one complete publisher payload, got {final_text}"
    );
    assert!(recover_publish_orphans(path.as_ref()).unwrap().is_empty());
}

#[test]
fn orphan_recovery_uses_strict_names_and_skips_live_writer() {
    let harness = TempHarness::new().unwrap();
    let path = harness.prepare_file("keep.json").unwrap();
    fs::write(&path, b"{\"ok\":true}").unwrap();

    let strict_orphan = path.with_file_name(format!(
        "{}.spirula-tmp.9.9",
        path.file_name().unwrap().to_string_lossy()
    ));
    fs::write(&strict_orphan, b"trash").unwrap();
    let decoy = path.with_file_name(format!(
        "{}.spirula-tmp.orphan",
        path.file_name().unwrap().to_string_lossy()
    ));
    fs::write(&decoy, b"user-file").unwrap();
    let neighbor = harness.child("keep.json.neighbor").unwrap();
    fs::write(&neighbor, b"other").unwrap();

    assert!(is_publish_temp(&path, &strict_orphan));
    assert!(!is_publish_temp(&path, &decoy));

    let paused = Arc::new(Barrier::new(2));
    let resume = Arc::new(Barrier::new(2));
    let writer_path = path.clone();
    let paused_writer = Arc::clone(&paused);
    let resume_writer = Arc::clone(&resume);
    let writer = thread::spawn(move || {
        publish_writer(&writer_path, PublishOptions::default(), |file| {
            file.write_all(b"live-temp").unwrap();
            paused_writer.wait();
            resume_writer.wait();
            Err(
                spirula_core::DomainError::try_new(ErrorCode::WriteFailed, "paused writer abort")
                    .unwrap(),
            )
        })
    });

    paused.wait();
    let blocked = recover_publish_orphans(&path).unwrap_err();
    assert_eq!(blocked.code(), ErrorCode::CommandInFlight);
    assert!(strict_orphan.exists());
    assert!(
        path.parent()
            .unwrap()
            .read_dir()
            .unwrap()
            .filter_map(Result::ok)
            .any(|entry| is_publish_temp(&path, &entry.path()) && entry.path() != strict_orphan)
    );

    resume.wait();
    assert!(writer.join().unwrap().is_err());

    let removed = recover_publish_orphans(&path).unwrap();
    assert_eq!(removed, vec![strict_orphan.clone()]);
    assert!(!strict_orphan.exists());
    assert!(decoy.exists());
    assert!(path.exists());
    assert!(neighbor.exists());
}

#[test]
fn errors_carry_path_and_source() {
    let harness = TempHarness::new().unwrap();
    let missing_parent = harness.child("no-such-dir/file.txt").unwrap();
    let error = publish_bytes(&missing_parent, b"x", PublishOptions::default()).unwrap_err();
    assert_eq!(error.code(), ErrorCode::NotFound);
    assert!(error.context_value(ContextKey::Path).is_some());
    assert!(std::error::Error::source(&error).is_some());
}

#[test]
fn config_save_and_migration_preserve_backup() {
    let harness = TempHarness::new().unwrap();
    let path = harness.prepare_file("config.json").unwrap();
    let legacy = "{\n  \"max_steps\": 12\n}\n";
    fs::write(&path, legacy).unwrap();

    let mut loaded = load_train_config(&path).unwrap();
    assert_eq!(loaded.schema_version(), None);
    loaded.config.max_steps = Some(99);
    save_train_config(&path, &loaded, PublishOptions::default()).unwrap();
    let saved = fs::read_to_string(&path).unwrap();
    assert!(saved.contains("\"max_steps\": 99") || saved.contains("\"max_steps\":99"));
    assert!(!saved.contains("schema_version"));

    let loaded = load_train_config(&path).unwrap();
    migrate_train_config(&path, &loaded, PublishOptions::default()).unwrap();
    let backup = backup_path(&path);
    assert_eq!(fs::read_to_string(&backup).unwrap(), saved);
    let migrated = load_train_config(&path).unwrap();
    assert_eq!(migrated.schema_version(), Some(1));
    assert!(
        fs::read_to_string(&path)
            .unwrap()
            .contains("schema_version")
    );
}

#[test]
fn migration_keeps_existing_backup_and_rejects_stale_loaded() {
    let harness = TempHarness::new().unwrap();
    let path = harness.prepare_file("config.json").unwrap();
    let legacy = "{\n  \"max_steps\": 12\n}\n";
    fs::write(&path, legacy).unwrap();
    let loaded = load_train_config(&path).unwrap();
    migrate_train_config(&path, &loaded, PublishOptions::default()).unwrap();
    let backup = backup_path(&path);
    let first_backup = fs::read_to_string(&backup).unwrap();
    assert_eq!(first_backup, legacy);

    fs::write(&backup, b"do-not-clobber").unwrap();
    let again = load_train_config(&path).unwrap();
    migrate_train_config(&path, &again, PublishOptions::default()).unwrap();
    assert_eq!(fs::read(&backup).unwrap(), b"do-not-clobber");

    let stale = load_train_config(&path).unwrap();
    fs::write(&path, b"{\n  \"max_steps\": 1\n}\n").unwrap();
    let error = migrate_train_config(&path, &stale, PublishOptions::default()).unwrap_err();
    assert_eq!(error.code(), ErrorCode::InvalidState);
    assert_eq!(fs::read(&backup).unwrap(), b"do-not-clobber");
    assert_eq!(fs::read(&path).unwrap(), b"{\n  \"max_steps\": 1\n}\n");
}

#[test]
fn migration_validates_before_backup_side_effects() {
    let harness = TempHarness::new().unwrap();
    let path = harness.prepare_file("config.json").unwrap();
    let legacy = "{\n  \"max_steps\": 12\n}\n";
    fs::write(&path, legacy).unwrap();
    let mut loaded = load_train_config(&path).unwrap();
    loaded.config.center_method = "not-a-choice".to_owned();
    let error = migrate_train_config(&path, &loaded, PublishOptions::default()).unwrap_err();
    assert_ne!(error.code(), ErrorCode::WriteFailed);
    assert!(!backup_path(&path).exists());
    assert_eq!(fs::read_to_string(&path).unwrap(), legacy);
}

#[test]
fn concurrent_migrations_preserve_first_backup() {
    let harness = TempHarness::new().unwrap();
    let path = Arc::new(harness.prepare_file("config.json").unwrap());
    let legacy = "{\n  \"max_steps\": 12\n}\n";
    fs::write(path.as_ref(), legacy).unwrap();
    let barrier = Arc::new(Barrier::new(4));
    let mut handles = Vec::new();
    for _ in 0..4 {
        let path = Arc::clone(&path);
        let barrier = Arc::clone(&barrier);
        handles.push(thread::spawn(move || {
            let loaded = load_train_config(path.as_ref()).unwrap();
            barrier.wait();
            loop {
                match migrate_train_config(path.as_ref(), &loaded, PublishOptions::default()) {
                    Ok(outcome) => break Ok(outcome),
                    Err(error) if error.code() == ErrorCode::CommandInFlight => continue,
                    Err(error) if error.code() == ErrorCode::InvalidState => break Err(error),
                    Err(error) => break Err(error),
                }
            }
        }));
    }
    let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    assert!(
        results.iter().any(Result::is_ok),
        "at least one migrator must publish; got {results:?}"
    );
    let backup = fs::read_to_string(backup_path(path.as_ref())).unwrap();
    assert_eq!(backup, legacy);
    let migrated = load_train_config(path.as_ref()).unwrap();
    assert_eq!(migrated.schema_version(), Some(1));
}

#[test]
fn config_corruption_rejected_before_replace() {
    let harness = TempHarness::new().unwrap();
    let path = harness.prepare_file("config.json").unwrap();
    fs::write(&path, b"{\"max_steps\": 1}\n").unwrap();
    let error = publish_bytes_validated(&path, b"{not-json", PublishOptions::default(), |bytes| {
        let text = std::str::from_utf8(bytes).unwrap();
        LoadedTrainConfig::parse(text).map(|_| ())
    })
    .unwrap_err();
    assert_ne!(error.code(), ErrorCode::WriteFailed);
    assert_eq!(fs::read(&path).unwrap(), b"{\"max_steps\": 1}\n");
}

#[test]
fn temp_harness_rejects_escape_and_cleans_only_its_root() {
    let first = TempHarness::new().unwrap();
    let second = TempHarness::new().unwrap();
    assert_ne!(first.path(), second.path());
    assert!(first.child("../escape").is_err());
    assert!(first.child("/absolute").is_err());
    let file = first.create_file("nested/a.txt").unwrap();
    assert!(file.starts_with(first.path()));
    let first_root = first.path().to_path_buf();
    let second_root = second.path().to_path_buf();
    drop(first);
    assert!(!first_root.exists());
    assert!(second_root.exists());
    drop(second);
    assert!(!second_root.exists());
}

#[test]
fn current_config_roundtrip_through_filesystem() {
    let harness = TempHarness::new().unwrap();
    let path = harness.prepare_file("fresh.json").unwrap();
    let text = TrainConfig::default().to_current_json().unwrap();
    publish_bytes_validated(&path, text.as_bytes(), PublishOptions::default(), |bytes| {
        LoadedTrainConfig::parse(std::str::from_utf8(bytes).unwrap()).map(|_| ())
    })
    .unwrap();
    let loaded = load_train_config(&path).unwrap();
    assert_eq!(loaded.schema_version(), Some(1));
    assert_eq!(loaded.to_current_json().unwrap(), text);
}
