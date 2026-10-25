use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::{Duration, Instant};

use spirula_core::{
    CancelReason, CancelToken, ContextKey, ErrorCode, LoadedTrainConfig, TrainConfig,
};
use spirula_io::{
    PublishOptions, TempHarness, acquire_publish_lease, is_publish_temp, load_train_config,
    migrate_train_config, parent_dir, publish_bytes, publish_bytes_if_absent,
    publish_bytes_validated, publish_writer, publish_writer_on_lease, recover_publish_orphans,
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
    let canonical = fs::canonicalize(&path).unwrap();
    assert_eq!(
        error.context_value(ContextKey::Path),
        Some(canonical.to_str().unwrap())
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
    assert_eq!(removed.len(), 1);
    assert_eq!(removed[0].file_name(), strict_orphan.file_name());
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

#[cfg(windows)]
#[test]
fn temp_harness_rejects_drive_relative_prefix_on_windows() {
    let harness = TempHarness::new().unwrap();
    // `C:relative` is not absolute, but Component::Prefix must still be rejected.
    assert!(harness.child(r"C:relative").is_err());
    assert!(harness.prepare_file(r"C:relative").is_err());
}

#[cfg(unix)]
#[test]
fn temp_harness_rejects_symlink_escape_on_unix() {
    use std::os::unix::fs::symlink;

    let harness = TempHarness::new().unwrap();
    let outside = TempHarness::new().unwrap();
    let victim = outside.create_file("secret.txt").unwrap();
    fs::write(&victim, b"keep").unwrap();
    symlink(outside.path(), harness.path().join("escape")).unwrap();

    assert!(harness.prepare_file("escape/secret.txt").is_err());
    assert!(harness.create_file("escape/pwned.txt").is_err());
    assert_eq!(fs::read(&victim).unwrap(), b"keep");
    assert!(!outside.path().join("pwned.txt").exists());

    symlink(&victim, harness.path().join("alias.txt")).unwrap();
    assert!(harness.create_file("alias.txt").is_err());
    assert_eq!(fs::read(&victim).unwrap(), b"keep");
}

#[cfg(unix)]
#[test]
fn publish_temp_matching_preserves_os_string_identity() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;
    use std::path::PathBuf;

    // Lossy UTF-8 collapses 0xFF and 0xFE; native OsStr matching must not.
    let name_a = OsString::from_vec(vec![b'a', 0xff]);
    let name_b = OsString::from_vec(vec![b'a', 0xfe]);
    assert_eq!(name_a.to_string_lossy(), name_b.to_string_lossy());
    assert_ne!(name_a, name_b);

    let path_a = PathBuf::from(&name_a);
    let path_b = PathBuf::from(&name_b);
    let mut orphan_a = name_a.clone();
    orphan_a.push(".spirula-tmp.1.1");
    let orphan_a = PathBuf::from(orphan_a);

    assert!(is_publish_temp(&path_a, &orphan_a));
    assert!(!is_publish_temp(&path_b, &orphan_a));
}

#[cfg(all(unix, target_os = "linux"))]
#[test]
fn recover_preserves_native_os_string_temp_identity_on_linux() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    let harness = TempHarness::new().unwrap();
    let name_a = OsString::from_vec(vec![b'a', 0xff]);
    let name_b = OsString::from_vec(vec![b'a', 0xfe]);

    let path_a = harness.path().join(&name_a);
    let path_b = harness.path().join(&name_b);
    fs::write(&path_a, b"A").unwrap();
    fs::write(&path_b, b"B").unwrap();

    let mut orphan_a = name_a.clone();
    orphan_a.push(".spirula-tmp.1.1");
    let orphan_a = harness.path().join(orphan_a);
    fs::write(&orphan_a, b"temp-A").unwrap();

    assert!(is_publish_temp(&path_a, &orphan_a));
    assert!(!is_publish_temp(&path_b, &orphan_a));

    let removed = recover_publish_orphans(&path_b).unwrap();
    assert!(removed.is_empty());
    assert!(orphan_a.exists());

    let removed = recover_publish_orphans(&path_a).unwrap();
    assert_eq!(removed, vec![orphan_a.clone()]);
    assert!(!orphan_a.exists());
    assert_eq!(fs::read(&path_a).unwrap(), b"A");
    assert_eq!(fs::read(&path_b).unwrap(), b"B");
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

#[test]
fn load_rejects_invalid_utf8_as_malformed_record() {
    let harness = TempHarness::new().unwrap();
    let path = harness.prepare_file("bad.json").unwrap();
    fs::write(&path, [0x7b, 0xff, 0x7d]).unwrap();
    let error = load_train_config(&path).unwrap_err();
    assert_eq!(error.code(), ErrorCode::MalformedRecord);
    assert!(!error.retryable());
    assert_eq!(
        error.context_value(ContextKey::Path),
        Some(path.to_str().unwrap())
    );
}

#[test]
fn wrong_lease_rejects_publish_to_other_destination() {
    let harness = TempHarness::new().unwrap();
    let path_a = harness.prepare_file("a.bin").unwrap();
    let path_b = harness.prepare_file("b.bin").unwrap();
    fs::write(&path_a, b"A").unwrap();
    fs::write(&path_b, b"B").unwrap();

    let lease_a = acquire_publish_lease(&path_a).unwrap();
    let _lease_b = acquire_publish_lease(&path_b).unwrap();

    let error = publish_writer_on_lease(&path_b, PublishOptions::default(), &lease_a, |file| {
        file.write_all(b"hijack").unwrap();
        Ok(6)
    })
    .unwrap_err();
    assert_eq!(error.code(), ErrorCode::InvalidArgument);
    assert_eq!(
        error.context_value(ContextKey::Actual),
        Some("lease_destination_mismatch")
    );
    assert_eq!(fs::read(&path_a).unwrap(), b"A");
    assert_eq!(fs::read(&path_b).unwrap(), b"B");

    // Bound methods always publish the lease destination, never a smuggled path.
    lease_a
        .publish_bytes_validated(b"A2", PublishOptions::default(), |_| Ok(()))
        .unwrap();
    assert_eq!(fs::read(&path_a).unwrap(), b"A2");
    assert_eq!(fs::read(&path_b).unwrap(), b"B");
}

#[test]
fn paused_no_clobber_publication_blocks_recovery() {
    let harness = TempHarness::new().unwrap();
    let config = harness.prepare_file("config.json").unwrap();
    let backup = backup_path(&config);
    fs::write(&config, b"{\"max_steps\": 1}\n").unwrap();

    let paused = Arc::new(Barrier::new(2));
    let resume = Arc::new(Barrier::new(2));
    let backup_for_writer = backup.clone();
    let paused_writer = Arc::clone(&paused);
    let resume_writer = Arc::clone(&resume);
    let writer = thread::spawn(move || {
        // Same lease + temp lifecycle as publish_bytes_if_absent; pause after
        // the live temp exists so recovery can observe the held lease.
        let lease = acquire_publish_lease(&backup_for_writer).unwrap();
        lease.publish_writer(PublishOptions::default(), |file| {
            file.write_all(b"live-backup-temp").unwrap();
            paused_writer.wait();
            resume_writer.wait();
            Err(
                spirula_core::DomainError::try_new(ErrorCode::WriteFailed, "paused backup temp")
                    .unwrap(),
            )
        })
    });

    // Config lease alone must not cover backup temps (distinct lock identity).
    let config_lease = acquire_publish_lease(&config).unwrap();
    paused.wait();

    let blocked = recover_publish_orphans(&backup).unwrap_err();
    assert_eq!(blocked.code(), ErrorCode::CommandInFlight);
    assert!(
        backup
            .parent()
            .unwrap()
            .read_dir()
            .unwrap()
            .filter_map(Result::ok)
            .any(|entry| is_publish_temp(&backup, &entry.path())),
        "live backup temp must survive while the backup lease is held"
    );

    drop(config_lease);
    assert!(recover_publish_orphans(&config).unwrap().is_empty());
    assert!(
        backup
            .parent()
            .unwrap()
            .read_dir()
            .unwrap()
            .filter_map(Result::ok)
            .any(|entry| is_publish_temp(&backup, &entry.path())),
        "recovering config orphans must not delete backup destination temps"
    );

    resume.wait();
    assert!(writer.join().unwrap().is_err());
    assert!(recover_publish_orphans(&backup).unwrap().is_empty());
    assert!(!backup.exists());
}

#[test]
fn publish_bytes_if_absent_lease_blocks_recovery_and_no_clobbers() {
    let harness = TempHarness::new().unwrap();
    // prepare_file only ensures ancestors; destination must stay absent.
    let path = harness.prepare_file("fresh.bak").unwrap();
    assert!(!path.exists());

    let paused = Arc::new(Barrier::new(2));
    let resume = Arc::new(Barrier::new(2));
    let writer_path = path.clone();
    let paused_gate = Arc::clone(&paused);
    let resume_gate = Arc::clone(&resume);
    let writer = thread::spawn(move || {
        let lease = acquire_publish_lease(&writer_path).unwrap();
        paused_gate.wait();
        let result = lease.publish_bytes_if_absent(b"created", PublishOptions::default());
        resume_gate.wait();
        result
    });

    paused.wait();
    let blocked = recover_publish_orphans(&path).unwrap_err();
    assert_eq!(blocked.code(), ErrorCode::CommandInFlight);
    assert!(!path.exists());
    resume.wait();
    let outcome = writer.join().unwrap().unwrap().unwrap();
    assert_eq!(outcome.bytes_written, 7);
    assert_eq!(fs::read(&path).unwrap(), b"created");

    assert!(
        publish_bytes_if_absent(&path, b"other", PublishOptions::default())
            .unwrap()
            .is_none()
    );
    assert_eq!(fs::read(&path).unwrap(), b"created");
}

#[test]
fn recover_after_killed_child_writer_releases_advisory_lease() {
    const DEST_ENV: &str = "SPIRULA_P1F05_KILL_DEST";
    const READY_ENV: &str = "SPIRULA_P1F05_KILL_READY";

    if let Some(dest) = std::env::var_os(DEST_ENV) {
        let dest = PathBuf::from(dest);
        let ready = PathBuf::from(std::env::var_os(READY_ENV).expect("ready path"));
        let _ = publish_writer(
            &dest,
            PublishOptions::default().with_sync_directory(false),
            |file| {
                file.write_all(b"partial-orphan").unwrap();
                fs::write(&ready, b"ready").unwrap();
                loop {
                    thread::sleep(Duration::from_secs(60));
                }
            },
        );
        return;
    }

    let harness = TempHarness::new().unwrap();
    let path = harness.prepare_file("dest.bin").unwrap();
    fs::write(&path, b"stable").unwrap();
    let ready = harness.child("ready.flag").unwrap();
    let lock = {
        let mut name = path.file_name().unwrap().to_owned();
        name.push(".spirula-lock");
        path.parent().unwrap().join(name)
    };

    let exe = std::env::current_exe().unwrap();
    let mut child = Command::new(&exe)
        .args([
            "--exact",
            "recover_after_killed_child_writer_releases_advisory_lease",
        ])
        .env(DEST_ENV, &path)
        .env(READY_ENV, &ready)
        .env("RUST_TEST_THREADS", "1")
        .spawn()
        .unwrap();

    let deadline = Instant::now() + Duration::from_secs(10);
    while !ready.exists() {
        assert!(
            Instant::now() < deadline,
            "child writer never signaled ready"
        );
        if let Some(status) = child.try_wait().unwrap() {
            panic!("child exited before ready: {status}");
        }
        thread::sleep(Duration::from_millis(20));
    }

    assert!(lock.exists(), "advisory lock inode should persist");
    assert!(
        path.parent()
            .unwrap()
            .read_dir()
            .unwrap()
            .filter_map(Result::ok)
            .any(|entry| is_publish_temp(&path, &entry.path())),
        "killed writer must leave a strict publish temp"
    );

    child.kill().unwrap();
    let _ = child.wait().unwrap();

    let removed = recover_publish_orphans(&path).unwrap();
    assert!(
        !removed.is_empty(),
        "recovery must succeed after killed writer despite lock inode"
    );
    assert!(lock.exists(), "lock inode remains; lease is advisory");
    assert_eq!(fs::read(&path).unwrap(), b"stable");
    assert!(
        path.parent()
            .unwrap()
            .read_dir()
            .unwrap()
            .filter_map(Result::ok)
            .all(|entry| !is_publish_temp(&path, &entry.path()))
    );
}
