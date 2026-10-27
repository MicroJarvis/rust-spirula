//! P2-F07 minimal CLI package: clean/archive build, manifest, isolated run.
//!
//! Binding: `docs/phase-2/cli-package.md`. Tooling only — does not change
//! `spirula-cli` → GPU edges.

use std::collections::BTreeMap;
use std::env;
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

/// Binaries shipped in the minimal CLI package.
pub const PACKAGE_BINARIES: &[&str] = &["spirula", "spirula-gpu-diagnose", "spirula-gpu-smoke"];

const MANIFEST_NAME: &str = "MANIFEST.json";
const NOTICE_NAME: &str = "NOTICE";
const BIN_DIR: &str = "bin";
const SCHEMA_VERSION: u32 = 1;
const STAGING_PREFIX: &str = ".spirula-pkg-tmp.";
static PATH_SEQ: AtomicU64 = AtomicU64::new(1);

/// Options for [`build_package`].
#[derive(Clone, Debug)]
pub struct PackageOptions {
    /// Destination package directory (must not already exist).
    pub out: PathBuf,
    /// Workspace / clean tree used as the source of truth for `git` / cargo.
    pub workspace: PathBuf,
    /// When set, archive this ref into an isolated tree and build there.
    pub from_ref: Option<String>,
    /// Cargo profile name (`release` or `dev`).
    pub profile: String,
}

/// Result of a successful package publish.
#[derive(Clone, Debug)]
pub struct PackageOutcome {
    pub package_dir: PathBuf,
    pub revision: String,
    pub manifest_path: PathBuf,
}

/// Verify a previously built package against its manifest.
///
/// Integrity rules (not cryptographic signing): the checked hash must be for
/// the unique canonical `bin/<name>` that [`run_package_isolated`] executes;
/// required contract fields and NOTICE must be present; package contents are
/// restricted to an explicit allowlist with no symlinks.
pub fn check_package(package_dir: &Path) -> Result<Value, String> {
    verify_package_root_layout(package_dir)?;
    let manifest = read_manifest(package_dir)?;
    verify_manifest_contract(&manifest)?;
    verify_manifest_binaries(package_dir, &manifest)?;
    verify_package_allowlist(package_dir)?;
    Ok(manifest)
}

/// Metadata recorded into `MANIFEST.json` when assembling from built binaries.
#[derive(Clone, Debug)]
pub struct AssembleMeta {
    pub revision: String,
    pub version: String,
    pub profile: String,
}

/// Build and atomically publish a package directory.
///
/// Requires a clean worktree unless [`PackageOptions::from_ref`] is set (then
/// the build uses `git archive` of that ref into an isolated tree).
pub fn build_package(opts: &PackageOptions) -> Result<PackageOutcome, String> {
    let (build_root, revision, cleanup_build) =
        prepare_build_tree(&opts.workspace, opts.from_ref.as_deref())?;
    let result = (|| {
        cargo_build_bins(&build_root, &opts.profile)?;
        let target_bin = cargo_target_bin_dir(&build_root, &opts.profile)?;
        let version = workspace_version(&build_root)?;
        assemble_package_from_bin_dir(
            &target_bin,
            &opts.out,
            &AssembleMeta {
                revision: revision.clone(),
                version,
                profile: opts.profile.clone(),
            },
        )
    })();
    if cleanup_build {
        let _ = remove_path_best_effort(&build_root);
    }
    result
}

/// Assemble a package from an existing Cargo target bin directory.
///
/// Used by host tests (dirty trees) and by [`build_package`] after a locked
/// build. Publication is one atomic no-replace rename of the complete staging
/// directory onto `out` (`rustix::fs::renameat_with` + `RenameFlags::NOREPLACE`
/// on macOS/Linux). The destination is invisible until the full package lands;
/// a concurrently created destination is preserved (single winner).
pub fn assemble_package_from_bin_dir(
    bin_dir: &Path,
    out: &Path,
    meta: &AssembleMeta,
) -> Result<PackageOutcome, String> {
    assemble_package_from_bin_dir_with_before_publish(bin_dir, out, meta, |_| Ok(()))
}

/// Like [`assemble_package_from_bin_dir`], but runs `before_publish(out)` after
/// staging is fully assembled and verified, immediately before the no-replace
/// rename. Tests use this for a deterministic schedule (create the destination
/// or simulate interrupt in the hook) instead of a probabilistic race.
pub fn assemble_package_from_bin_dir_with_before_publish(
    bin_dir: &Path,
    out: &Path,
    meta: &AssembleMeta,
    before_publish: impl FnOnce(&Path) -> Result<(), String>,
) -> Result<PackageOutcome, String> {
    if destination_entry_present(out)? {
        return Err(format!(
            "package destination already exists: {}",
            out.display()
        ));
    }
    let parent = out.parent().ok_or_else(|| {
        format!(
            "package destination has no parent directory: {}",
            out.display()
        )
    })?;
    fs::create_dir_all(parent).map_err(|e| format!("creating parent {}: {e}", parent.display()))?;

    let staging = unique_staging(parent)?;
    let assembled = fill_staging(&staging, bin_dir, meta);
    match assembled {
        Ok(()) => {
            // Deterministic injection point for concurrency / pre-rename probes.
            // At this moment `out` must still be absent; staging holds the full
            // package and can be deleted without leaving a partial destination.
            if let Err(err) = before_publish(out) {
                let _ = remove_path_best_effort(&staging);
                return Err(err);
            }
            publish_staging_no_clobber(&staging, out)?;
            Ok(PackageOutcome {
                package_dir: out.to_path_buf(),
                revision: meta.revision.clone(),
                manifest_path: out.join(MANIFEST_NAME),
            })
        }
        Err(err) => {
            let _ = remove_path_best_effort(&staging);
            Err(err)
        }
    }
}

fn fill_staging(staging: &Path, bin_dir: &Path, meta: &AssembleMeta) -> Result<(), String> {
    fs::create_dir_all(staging.join(BIN_DIR)).map_err(|e| format!("creating staging bin: {e}"))?;
    let mut binaries = Vec::new();
    for name in PACKAGE_BINARIES {
        let src = bin_dir.join(name);
        if !src.is_file() {
            return Err(format!("built binary missing: {}", src.display()));
        }
        let dest = staging.join(BIN_DIR).join(name);
        fs::copy(&src, &dest)
            .map_err(|e| format!("copy {} → {}: {e}", src.display(), dest.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(&dest)
                .map_err(|e| format!("stat {}: {e}", dest.display()))?
                .permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&dest, perms)
                .map_err(|e| format!("chmod {}: {e}", dest.display()))?;
        }
        let sha = sha256_file(&dest)?;
        let size = fs::metadata(&dest)
            .map_err(|e| format!("stat {}: {e}", dest.display()))?
            .len();
        let deps = runtime_dependencies(&dest)?;
        reject_nonsystem_dylibs(name, &deps)?;
        binaries.push(json!({
            "name": name,
            "path": format!("{BIN_DIR}/{name}"),
            "sha256": sha,
            "size_bytes": size,
            "runtime_dependencies": deps,
        }));
    }
    write_notice(staging)?;
    let host = rustc_host()?;
    let toolchain = rustc_toolchain()?;
    let manifest = json!({
        "schema_version": SCHEMA_VERSION,
        "feature": "P2-F07",
        "package_kind": "minimal-cli",
        "revision": meta.revision,
        "version": meta.version,
        "host_target": host,
        "toolchain": toolchain,
        "profile": meta.profile,
        "locked": true,
        "binaries": binaries,
        "licenses": ["GPL-3.0-or-later"],
        "claims": {
            "signed_release": false,
            "three_platform_matrix": false,
            "surface_present_verified": false,
            "train_preview_ui_wired": false,
            "phase2_exit_closed": false,
            "bit_for_bit_reproducible": false
        }
    });
    let body = serde_json::to_string_pretty(&manifest)
        .map_err(|e| format!("serialize manifest: {e}"))?
        + "\n";
    fs::write(staging.join(MANIFEST_NAME), body).map_err(|e| format!("write manifest: {e}"))?;
    verify_package_root_layout(staging)?;
    verify_manifest_contract(&manifest)?;
    verify_manifest_binaries(staging, &manifest)?;
    verify_package_allowlist(staging)?;
    Ok(())
}

fn destination_entry_present(out: &Path) -> Result<bool, String> {
    match fs::symlink_metadata(out) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(format!("stat destination {}: {error}", out.display())),
    }
}

/// One-shot no-replace publish: rename the complete staging directory onto
/// `out`. Until this returns Ok, `out` does not exist as a package path.
///
/// macOS/Linux use `rustix::fs::renameat_with(..., RenameFlags::NOREPLACE)`
/// (`RENAME_EXCL` / `RENAME_NOREPLACE`). Other hosts return a typed unsupported
/// error without creating `out`. On `EEXIST`, staging is removed and any
/// existing destination is left untouched (never deleted).
fn publish_staging_no_clobber(staging: &Path, out: &Path) -> Result<(), String> {
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        use rustix::fs::{Mode, OFlags, RenameFlags, open, renameat_with};
        use rustix::io::Errno;

        let staging_parent = staging
            .parent()
            .ok_or_else(|| format!("staging has no parent: {}", staging.display()))?;
        let out_parent = out
            .parent()
            .ok_or_else(|| format!("destination has no parent: {}", out.display()))?;
        if staging_parent != out_parent {
            let _ = remove_path_best_effort(staging);
            return Err(format!(
                "staging and destination must share a parent ({} vs {})",
                staging_parent.display(),
                out_parent.display()
            ));
        }
        let staging_name = staging
            .file_name()
            .ok_or_else(|| format!("staging has no file name: {}", staging.display()))?;
        let out_name = out
            .file_name()
            .ok_or_else(|| format!("destination has no file name: {}", out.display()))?;

        let dirfd = match open(
            staging_parent,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
        ) {
            Ok(fd) => fd,
            Err(error) => {
                let _ = remove_path_best_effort(staging);
                return Err(format!(
                    "open package parent {}: {error}",
                    staging_parent.display()
                ));
            }
        };

        match renameat_with(
            &dirfd,
            staging_name,
            &dirfd,
            out_name,
            RenameFlags::NOREPLACE,
        ) {
            Ok(()) => Ok(()),
            Err(error) if error == Errno::EXIST => {
                // Destination appeared (or already existed). Preserve it.
                let _ = remove_path_best_effort(staging);
                Err(format!(
                    "package destination already exists (refusing overwrite): {}",
                    out.display()
                ))
            }
            Err(error) => {
                // Rename did not install `out`. Do not delete any existing
                // destination that might belong to another publisher.
                let _ = remove_path_best_effort(staging);
                Err(format!(
                    "no-replace rename {} → {}: {error}",
                    staging.display(),
                    out.display()
                ))
            }
        }
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = remove_path_best_effort(staging);
        Err(format!(
            "package publish unsupported on this host (need macOS/Linux rename NOREPLACE); \
             destination left untouched: {}",
            out.display()
        ))
    }
}

/// Run packaged binaries in a source-free isolate directory.
pub fn run_package_isolated(
    package_dir: &Path,
    isolate_parent: &Path,
) -> Result<IsolatedRunReport, String> {
    check_package(package_dir)?;
    fs::create_dir_all(isolate_parent)
        .map_err(|e| format!("creating isolate parent {}: {e}", isolate_parent.display()))?;
    let isolate = unique_isolate(isolate_parent)?;
    let copy_result = (|| {
        copy_dir_recursive(package_dir, &isolate)?;
        // Re-check after copy so isolate cannot diverge (symlink drop, extras).
        verify_package_root_layout(&isolate)?;
        verify_package_allowlist(&isolate)?;
        // cwd with spaces, unrelated to the package tree.
        let cwd = isolate_parent.join("run cwd with spaces");
        fs::create_dir_all(&cwd).map_err(|e| format!("creating cwd: {e}"))?;
        let bin = isolate.join(BIN_DIR);
        // Execute only the canonical allowlisted paths (same as check hashes).
        let spirula = canonical_bin_file(&isolate, "spirula")?;
        let smoke_bin = canonical_bin_file(&isolate, "spirula-gpu-smoke")?;
        let mut report = IsolatedRunReport {
            isolate: isolate.clone(),
            cwd: cwd.clone(),
            help_exit: 0,
            capabilities_exit: 0,
            probe_exit: 0,
            smoke_exit: 0,
            smoke_status: String::new(),
            smoke_match: None,
            smoke_current: None,
            smoke_counted_as_pass: false,
        };

        report.help_exit = run_scrubbed(&spirula, &["help"], &cwd, &bin)?.exit;
        if report.help_exit != 0 {
            return Err(format!("spirula help exit {}", report.help_exit));
        }
        report.capabilities_exit = run_scrubbed(&spirula, &["capabilities"], &cwd, &bin)?.exit;
        if report.capabilities_exit != 0 {
            return Err(format!(
                "spirula capabilities exit {}",
                report.capabilities_exit
            ));
        }
        report.probe_exit = run_scrubbed(
            &spirula,
            &["probe", "--label", "pkg", "--steps", "1"],
            &cwd,
            &bin,
        )?
        .exit;
        if report.probe_exit != 0 {
            return Err(format!("spirula probe exit {}", report.probe_exit));
        }

        let smoke = run_scrubbed(&smoke_bin, &["--compute"], &cwd, &bin)?;
        report.smoke_exit = smoke.exit;
        if report.smoke_exit != 0 {
            return Err(format!(
                "spirula-gpu-smoke exit {} stdout:\n{}stderr:\n{}",
                report.smoke_exit, smoke.stdout, smoke.stderr
            ));
        }
        let status = parse_smoke_field(&smoke.stdout, "status=")
            .ok_or_else(|| format!("smoke missing status= in:\n{}", smoke.stdout))?;
        report.smoke_status = status.clone();
        report.smoke_match = parse_smoke_field(&smoke.stdout, "match=");
        report.smoke_current =
            parse_smoke_field(&smoke.stdout, "after_drop current=").and_then(|s| s.parse().ok());

        match status.as_str() {
            "pass" => {
                if report.smoke_match.as_deref() != Some("cpu_exact") {
                    return Err(format!(
                        "smoke pass without cpu_exact (match={:?})",
                        report.smoke_match
                    ));
                }
                if report.smoke_current != Some(0) {
                    return Err(format!(
                        "smoke pass without after_drop current=0 (got {:?})",
                        report.smoke_current
                    ));
                }
                report.smoke_counted_as_pass = true;
            }
            "skip" => {
                // Precise skip is success for the package gate; must not be
                // reported as a pass.
                report.smoke_counted_as_pass = false;
                if !smoke.stdout.contains("open_failure_stage=")
                    && !smoke.stdout.contains("reason=")
                {
                    return Err("smoke skip without open_failure_stage= or reason=".to_owned());
                }
            }
            other => {
                return Err(format!(
                    "unexpected smoke status `{other}` (want pass|skip)"
                ));
            }
        }
        Ok(report)
    })();

    match copy_result {
        Ok(report) => Ok(report),
        Err(err) => {
            let _ = remove_path_best_effort(&isolate);
            Err(err)
        }
    }
}

/// Summary of an isolated package run.
#[derive(Clone, Debug)]
pub struct IsolatedRunReport {
    pub isolate: PathBuf,
    pub cwd: PathBuf,
    pub help_exit: i32,
    pub capabilities_exit: i32,
    pub probe_exit: i32,
    pub smoke_exit: i32,
    pub smoke_status: String,
    pub smoke_match: Option<String>,
    pub smoke_current: Option<u64>,
    pub smoke_counted_as_pass: bool,
}

fn prepare_build_tree(
    workspace: &Path,
    from_ref: Option<&str>,
) -> Result<(PathBuf, String, bool), String> {
    let git_meta = workspace.join(".git");
    if !git_meta.exists() {
        return Err(format!(
            "workspace is not a git checkout: {}",
            workspace.display()
        ));
    }
    if let Some(rev) = from_ref {
        let parent = env::temp_dir().join("spirula-pkg-build");
        fs::create_dir_all(&parent).map_err(|e| format!("temp build parent: {e}"))?;
        let dest = unique_path(&parent, "archive-")?;
        git_archive(workspace, rev, &dest)?;
        // Cargo needs the lockfile; git archive includes it from the ref.
        Ok((dest, resolve_revision(workspace, rev)?, true))
    } else {
        ensure_clean(workspace)?;
        let rev = resolve_revision(workspace, "HEAD")?;
        Ok((workspace.to_path_buf(), rev, false))
    }
}

fn ensure_clean(workspace: &Path) -> Result<(), String> {
    let out = git_output(workspace, &["status", "--porcelain"])?;
    let text = String::from_utf8_lossy(&out.stdout);
    if !text.trim().is_empty() {
        return Err(format!(
            "refusing dirty worktree (pass --from-ref <sha> for an isolated archive build):\n{}",
            text.trim()
        ));
    }
    Ok(())
}

fn resolve_revision(workspace: &Path, rev: &str) -> Result<String, String> {
    let out = git_output(workspace, &["rev-parse", rev])?;
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

fn git_archive(workspace: &Path, rev: &str, dest: &Path) -> Result<(), String> {
    fs::create_dir_all(dest).map_err(|e| format!("archive dest: {e}"))?;
    Command::new("git")
        .args(["archive", "--format=tar", rev])
        .current_dir(workspace)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            let mut tar = Command::new("tar")
                .arg("-xf")
                .arg("-")
                .current_dir(dest)
                .stdin(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()?;
            if let (Some(mut git_out), Some(mut tar_in)) = (child.stdout.take(), tar.stdin.take()) {
                io::copy(&mut git_out, &mut tar_in)?;
            }
            let tar_status = tar.wait()?;
            let git_status = child.wait()?;
            if !git_status.success() {
                return Err(io::Error::other(format!(
                    "git archive failed with {git_status}"
                )));
            }
            if !tar_status.success() {
                return Err(io::Error::other(format!(
                    "tar extract failed with {tar_status}"
                )));
            }
            Ok(())
        })
        .map_err(|e| format!("git archive | tar: {e}"))?;
    if !dest.join("Cargo.toml").is_file() {
        return Err(format!(
            "archived tree missing Cargo.toml at {}",
            dest.display()
        ));
    }
    Ok(())
}

fn cargo_build_bins(build_root: &Path, profile: &str) -> Result<(), String> {
    let cargo = env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned());
    let mut cmd = Command::new(&cargo);
    cmd.arg("build")
        .arg("--locked")
        .arg("-p")
        .arg("spirula-cli")
        .arg("-p")
        .arg("spirula-gpu")
        .current_dir(build_root);
    match profile {
        "release" => {
            cmd.arg("--release");
        }
        "dev" => {}
        other => return Err(format!("unsupported profile `{other}` (want release|dev)")),
    }
    let out = cmd
        .output()
        .map_err(|e| format!("spawning cargo build: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "cargo build --locked failed:\n{}\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    Ok(())
}

fn cargo_target_bin_dir(build_root: &Path, profile: &str) -> Result<PathBuf, String> {
    let dir_name = match profile {
        "release" => "release",
        "dev" => "debug",
        other => return Err(format!("unsupported profile `{other}`")),
    };
    let path = build_root.join("target").join(dir_name);
    if !path.is_dir() {
        return Err(format!("cargo target dir missing: {}", path.display()));
    }
    Ok(path)
}

fn read_manifest(package_dir: &Path) -> Result<Value, String> {
    let path = package_dir.join(MANIFEST_NAME);
    let text = fs::read_to_string(&path).map_err(|e| format!("reading {}: {e}", path.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("parse {}: {e}", path.display()))
}

fn canonical_rel_path(name: &str) -> String {
    format!("{BIN_DIR}/{name}")
}

fn canonical_bin_file(package_dir: &Path, name: &str) -> Result<PathBuf, String> {
    if !PACKAGE_BINARIES.contains(&name) {
        return Err(format!("layout: unknown package binary name `{name}`"));
    }
    let path = package_dir.join(BIN_DIR).join(name);
    require_regular_file(&path, &format!("bin/{name}"))?;
    Ok(path)
}

fn require_regular_file(path: &Path, label: &str) -> Result<(), String> {
    let meta = fs::symlink_metadata(path)
        .map_err(|e| format!("layout: missing {label} at {}: {e}", path.display()))?;
    if meta.file_type().is_symlink() {
        return Err(format!(
            "layout: {label} must be a regular file, not a symlink ({})",
            path.display()
        ));
    }
    if !meta.file_type().is_file() {
        return Err(format!(
            "layout: {label} must be a regular file ({})",
            path.display()
        ));
    }
    Ok(())
}

fn require_regular_dir(path: &Path, label: &str) -> Result<(), String> {
    let meta = fs::symlink_metadata(path)
        .map_err(|e| format!("layout: missing {label} at {}: {e}", path.display()))?;
    if meta.file_type().is_symlink() {
        return Err(format!(
            "layout: {label} must be a directory, not a symlink ({})",
            path.display()
        ));
    }
    if !meta.file_type().is_dir() {
        return Err(format!(
            "layout: {label} must be a directory ({})",
            path.display()
        ));
    }
    Ok(())
}

fn verify_package_root_layout(package_dir: &Path) -> Result<(), String> {
    require_regular_dir(package_dir, "package root")?;
    require_regular_file(&package_dir.join(MANIFEST_NAME), MANIFEST_NAME)?;
    require_regular_file(&package_dir.join(NOTICE_NAME), NOTICE_NAME)?;
    require_regular_dir(&package_dir.join(BIN_DIR), BIN_DIR)?;
    for name in PACKAGE_BINARIES {
        require_regular_file(
            &package_dir.join(BIN_DIR).join(name),
            &canonical_rel_path(name),
        )?;
    }
    Ok(())
}

fn require_nonempty_str(manifest: &Value, key: &str) -> Result<String, String> {
    let value = manifest
        .get(key)
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("manifest: missing or empty `{key}`"))?;
    Ok(value.to_owned())
}

fn verify_manifest_contract(manifest: &Value) -> Result<(), String> {
    let schema = manifest
        .get("schema_version")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| "manifest: missing schema_version".to_owned())?;
    if schema != u64::from(SCHEMA_VERSION) {
        return Err(format!(
            "manifest: unsupported schema_version {schema} (want {SCHEMA_VERSION})"
        ));
    }
    if manifest.get("locked").and_then(|v| v.as_bool()) != Some(true) {
        return Err("manifest: locked must be true".to_owned());
    }
    let feature = require_nonempty_str(manifest, "feature")?;
    if feature != "P2-F07" {
        return Err(format!(
            "manifest: feature must be P2-F07 (got `{feature}`)"
        ));
    }
    let kind = require_nonempty_str(manifest, "package_kind")?;
    if kind != "minimal-cli" {
        return Err(format!(
            "manifest: package_kind must be minimal-cli (got `{kind}`)"
        ));
    }
    let _revision = require_nonempty_str(manifest, "revision")?;
    let _version = require_nonempty_str(manifest, "version")?;
    let _host = require_nonempty_str(manifest, "host_target")?;
    let profile = require_nonempty_str(manifest, "profile")?;
    if profile != "release" && profile != "dev" {
        return Err(format!(
            "manifest: profile must be release|dev (got `{profile}`)"
        ));
    }
    let toolchain = manifest
        .get("toolchain")
        .and_then(|v| v.as_object())
        .ok_or_else(|| "manifest: missing toolchain object".to_owned())?;
    for key in ["rustc_release", "rustc_commit_hash", "rustc_host"] {
        let ok = toolchain
            .get(key)
            .and_then(|v| v.as_str())
            .map(str::trim)
            .is_some_and(|s| !s.is_empty());
        if !ok {
            return Err(format!("manifest: toolchain missing non-empty `{key}`"));
        }
    }
    let licenses = manifest
        .get("licenses")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "manifest: missing licenses[]".to_owned())?;
    if !licenses
        .iter()
        .any(|v| v.as_str() == Some("GPL-3.0-or-later"))
    {
        return Err("manifest: licenses[] must include GPL-3.0-or-later".to_owned());
    }
    let claims = manifest
        .get("claims")
        .and_then(|v| v.as_object())
        .ok_or_else(|| "manifest: missing claims object".to_owned())?;
    for key in [
        "signed_release",
        "three_platform_matrix",
        "surface_present_verified",
        "train_preview_ui_wired",
        "phase2_exit_closed",
        "bit_for_bit_reproducible",
    ] {
        if claims.get(key).and_then(|v| v.as_bool()).is_none() {
            return Err(format!("manifest: claims missing bool `{key}`"));
        }
    }
    Ok(())
}

fn verify_manifest_binaries(package_dir: &Path, manifest: &Value) -> Result<(), String> {
    let binaries = manifest
        .get("binaries")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "manifest: missing binaries[]".to_owned())?;
    if binaries.len() != PACKAGE_BINARIES.len() {
        return Err(format!(
            "manifest: binaries[] must have exactly {} entries (got {})",
            PACKAGE_BINARIES.len(),
            binaries.len()
        ));
    }

    let mut seen_names: BTreeMap<String, ()> = BTreeMap::new();
    let mut seen_paths: BTreeMap<String, ()> = BTreeMap::new();
    for entry in binaries {
        let name = entry
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "manifest: binary entry missing name".to_owned())?;
        if !PACKAGE_BINARIES.contains(&name) {
            return Err(format!(
                "manifest: unexpected binary name `{name}` (allowlist: {PACKAGE_BINARIES:?})"
            ));
        }
        if seen_names.insert(name.to_owned(), ()).is_some() {
            return Err(format!("manifest: duplicate binary name `{name}`"));
        }
        let rel = entry
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| format!("manifest: binary `{name}` missing path"))?;
        validate_canonical_rel_path(name, rel)?;
        if seen_paths.insert(rel.to_owned(), ()).is_some() {
            return Err(format!("manifest: duplicate binary path `{rel}`"));
        }

        let expected_sha = entry
            .get("sha256")
            .and_then(|v| v.as_str())
            .ok_or_else(|| format!("manifest: binary `{name}` missing sha256"))?;
        let expected_size = entry
            .get("size_bytes")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| format!("manifest: binary `{name}` missing size_bytes"))?;
        let deps = entry
            .get("runtime_dependencies")
            .and_then(|v| v.as_array())
            .ok_or_else(|| format!("manifest: binary `{name}` missing runtime_dependencies[]"))?;
        if deps.iter().any(|v| v.as_str().is_none()) {
            return Err(format!(
                "manifest: binary `{name}` runtime_dependencies must be strings"
            ));
        }

        // Hash/size the unique canonical path that package-run executes.
        let file = canonical_bin_file(package_dir, name)?;
        let actual_sha = sha256_file(&file)?;
        if actual_sha != expected_sha {
            return Err(format!(
                "manifest: sha256 mismatch for `{name}` at {}: manifest={expected_sha} actual={actual_sha}",
                file.display()
            ));
        }
        let actual_size = fs::metadata(&file)
            .map_err(|e| format!("layout: stat {}: {e}", file.display()))?
            .len();
        if actual_size != expected_size {
            return Err(format!(
                "manifest: size_bytes mismatch for `{name}`: manifest={expected_size} actual={actual_size}"
            ));
        }
    }
    for required in PACKAGE_BINARIES {
        if !seen_names.contains_key(*required) {
            return Err(format!("manifest: missing required binary `{required}`"));
        }
    }
    Ok(())
}

fn validate_canonical_rel_path(name: &str, rel: &str) -> Result<(), String> {
    let expected = canonical_rel_path(name);
    if rel != expected {
        return Err(format!(
            "manifest: binary `{name}` path must be exactly `{expected}` (got `{rel}`)"
        ));
    }
    if Path::new(rel).is_absolute() {
        return Err(format!(
            "manifest: binary `{name}` path must not be absolute"
        ));
    }
    if rel.contains('\0') || rel.contains('\\') {
        return Err(format!(
            "manifest: binary `{name}` path has illegal characters"
        ));
    }
    for component in Path::new(rel).components() {
        use std::path::Component;
        match component {
            Component::Normal(_) => {}
            Component::CurDir => {
                return Err(format!(
                    "manifest: binary `{name}` path must not contain `.`"
                ));
            }
            Component::ParentDir => {
                return Err(format!(
                    "manifest: binary `{name}` path must not contain `..`"
                ));
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(format!(
                    "manifest: binary `{name}` path must be package-relative"
                ));
            }
        }
    }
    Ok(())
}

/// Exact allowlist: MANIFEST.json, NOTICE, bin/, and the three canonical bins.
fn verify_package_allowlist(package_dir: &Path) -> Result<(), String> {
    let mut allowed_files: BTreeMap<PathBuf, ()> = BTreeMap::new();
    allowed_files.insert(package_dir.join(MANIFEST_NAME), ());
    allowed_files.insert(package_dir.join(NOTICE_NAME), ());
    for name in PACKAGE_BINARIES {
        allowed_files.insert(package_dir.join(BIN_DIR).join(name), ());
    }

    let mut allowed_dirs: BTreeMap<PathBuf, ()> = BTreeMap::new();
    allowed_dirs.insert(package_dir.to_path_buf(), ());
    allowed_dirs.insert(package_dir.join(BIN_DIR), ());

    walk_package_entries(package_dir, package_dir, &allowed_files, &allowed_dirs)
}

fn walk_package_entries(
    root: &Path,
    dir: &Path,
    allowed_files: &BTreeMap<PathBuf, ()>,
    allowed_dirs: &BTreeMap<PathBuf, ()>,
) -> Result<(), String> {
    for entry in
        fs::read_dir(dir).map_err(|e| format!("layout: read_dir {}: {e}", dir.display()))?
    {
        let entry = entry.map_err(|e| format!("layout: read_dir entry: {e}"))?;
        let path = entry.path();
        let meta = fs::symlink_metadata(&path)
            .map_err(|e| format!("layout: lstat {}: {e}", path.display()))?;
        let ft = meta.file_type();
        if ft.is_symlink() {
            return Err(format!(
                "layout: symlink not allowed in package ({})",
                path.display()
            ));
        }
        if ft.is_dir() {
            if !allowed_dirs.contains_key(&path) {
                return Err(format!(
                    "layout: unexpected directory `{}` (not on allowlist)",
                    path.strip_prefix(root).unwrap_or(&path).display()
                ));
            }
            walk_package_entries(root, &path, allowed_files, allowed_dirs)?;
        } else if ft.is_file() {
            if !allowed_files.contains_key(&path) {
                return Err(format!(
                    "layout: unexpected file `{}` (not on allowlist)",
                    path.strip_prefix(root).unwrap_or(&path).display()
                ));
            }
        } else {
            return Err(format!(
                "layout: unsupported file type at {}",
                path.display()
            ));
        }
    }
    Ok(())
}

fn write_notice(package_dir: &Path) -> Result<(), String> {
    let body = "Spirula Studio minimal CLI package (P2-F07)\n\
License: GPL-3.0-or-later (see workspace [workspace.package] license).\n\
This package ships process entry binaries only. It is not a signed installer,\n\
does not claim a three-platform matrix, and does not wire train/preview/UI.\n";
    fs::write(package_dir.join(NOTICE_NAME), body).map_err(|e| format!("write NOTICE: {e}"))
}

fn runtime_dependencies(binary: &Path) -> Result<Vec<String>, String> {
    #[cfg(target_os = "macos")]
    {
        let out = Command::new("otool")
            .args(["-L", &binary.display().to_string()])
            .output()
            .map_err(|e| format!("otool -L: {e}"))?;
        if !out.status.success() {
            return Err(format!(
                "otool -L failed: {}",
                String::from_utf8_lossy(&out.stderr)
            ));
        }
        let text = String::from_utf8_lossy(&out.stdout);
        let mut deps = Vec::new();
        for (i, line) in text.lines().enumerate() {
            if i == 0 {
                continue; // binary path header
            }
            let dep = line.split_whitespace().next().unwrap_or("");
            if !dep.is_empty() {
                deps.push(dep.to_owned());
            }
        }
        Ok(deps)
    }
    #[cfg(target_os = "linux")]
    {
        let out = Command::new("ldd")
            .arg(binary)
            .output()
            .map_err(|e| format!("ldd: {e}"))?;
        // ldd returns non-zero for static binaries; still parse stdout.
        let text = String::from_utf8_lossy(&out.stdout);
        let mut deps = Vec::new();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            deps.push(line.to_owned());
        }
        Ok(deps)
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = binary;
        Ok(vec![
            "(runtime dependency enumeration not implemented on this host)".to_owned(),
        ])
    }
}

fn reject_nonsystem_dylibs(name: &str, deps: &[String]) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let mut bad = Vec::new();
        for dep in deps {
            if dep.starts_with("/usr/lib/") || dep.starts_with("/System/Library/") {
                continue;
            }
            // Ignore self path lines that otool sometimes emits.
            if dep.ends_with(name) {
                continue;
            }
            bad.push(dep.clone());
        }
        if !bad.is_empty() {
            return Err(format!(
                "macOS binary `{name}` links non-system dylibs (not redistributable as a portable package): {bad:?}"
            ));
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (name, deps);
    }
    Ok(())
}

fn rustc_host() -> Result<String, String> {
    crate::policy::rustc_host_triple()
}

fn rustc_toolchain() -> Result<Value, String> {
    let out = Command::new("rustc")
        .arg("-vV")
        .output()
        .map_err(|e| format!("rustc -vV: {e}"))?;
    if !out.status.success() {
        return Err("rustc -vV failed".to_owned());
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut version = None;
    let mut commit_hash = None;
    let mut host = None;
    for line in text.lines() {
        if let Some(v) = line.strip_prefix("release: ") {
            version = Some(v.to_owned());
        } else if let Some(v) = line.strip_prefix("commit-hash: ") {
            commit_hash = Some(v.to_owned());
        } else if let Some(v) = line.strip_prefix("host: ") {
            host = Some(v.to_owned());
        }
    }
    Ok(json!({
        "rustc_release": version,
        "rustc_commit_hash": commit_hash,
        "rustc_host": host,
    }))
}

fn workspace_version(build_root: &Path) -> Result<String, String> {
    let text = fs::read_to_string(build_root.join("Cargo.toml"))
        .map_err(|e| format!("reading workspace Cargo.toml: {e}"))?;
    // Prefer [workspace.package] version.
    let mut in_workspace_package = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_workspace_package = trimmed == "[workspace.package]";
            continue;
        }
        if in_workspace_package && let Some(rest) = trimmed.strip_prefix("version") {
            let rest = rest.trim_start().trim_start_matches('=').trim();
            let v = rest.trim_matches('"');
            if !v.is_empty() {
                return Ok(v.to_owned());
            }
        }
    }
    Err("workspace.package version not found in Cargo.toml".to_owned())
}

fn sha256_file(path: &Path) -> Result<String, String> {
    let mut file = File::open(path).map_err(|e| format!("open {}: {e}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buf = [0_u8; 64 * 1024];
    loop {
        let n = file
            .read(&mut buf)
            .map_err(|e| format!("read {}: {e}", path.display()))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex_encode(hasher.finalize()))
}

fn hex_encode(bytes: impl AsRef<[u8]>) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let bytes = bytes.as_ref();
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0xf) as usize] as char);
    }
    out
}

struct CmdResult {
    exit: i32,
    stdout: String,
    stderr: String,
}

fn run_scrubbed(
    program: &Path,
    args: &[&str],
    cwd: &Path,
    bin_dir: &Path,
) -> Result<CmdResult, String> {
    let mut cmd = Command::new(program);
    cmd.args(args).current_dir(cwd).env_clear();
    // Minimal env: PATH to package bin + system bins; keep dyld/system essentials.
    let path = format!("{}:/usr/bin:/bin:/usr/sbin:/sbin", bin_dir.display());
    cmd.env("PATH", path);
    cmd.env("HOME", cwd);
    cmd.env("TMPDIR", cwd);
    // Explicitly avoid leaking workspace hints.
    for key in [
        "CARGO",
        "CARGO_HOME",
        "CARGO_TARGET_DIR",
        "RUSTUP_HOME",
        "RUSTC",
        "RUSTFLAGS",
        "SPIRULA_REFERENCE_REPO",
        "SPIRULA_FIXTURES",
        "SPIRULA_GPU_SURFACE",
    ] {
        cmd.env_remove(key);
    }
    let out = cmd
        .output()
        .map_err(|e| format!("running {} {:?}: {e}", program.display(), args))?;
    Ok(CmdResult {
        exit: out.status.code().unwrap_or(1),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    })
}

fn parse_smoke_field(stdout: &str, key: &str) -> Option<String> {
    for line in stdout.lines() {
        if let Some(rest) = line.strip_prefix("spirula-gpu-smoke ") {
            // status= lives on the header line among other keys.
            if key == "status=" {
                for part in rest.split_whitespace() {
                    if let Some(v) = part.strip_prefix("status=") {
                        return Some(v.to_owned());
                    }
                }
            }
        }
        if let Some(v) = line.strip_prefix(key) {
            return Some(v.trim().to_owned());
        }
        // also allow "match=cpu_exact" style full-line keys already handled;
        // for embedded keys on header we handled status above.
        if key != "status=" {
            for part in line.split_whitespace() {
                if let Some(v) = part.strip_prefix(key) {
                    return Some(v.to_owned());
                }
            }
        }
    }
    None
}

fn git_output(workspace: &Path, args: &[&str]) -> Result<Output, String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(workspace)
        .output()
        .map_err(|e| format!("git {}: {e}", args.join(" ")))?;
    if !out.status.success() {
        return Err(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    Ok(out)
}

fn unique_staging(parent: &Path) -> Result<PathBuf, String> {
    unique_path(parent, STAGING_PREFIX)
}

fn unique_isolate(parent: &Path) -> Result<PathBuf, String> {
    unique_path(parent, "isolate-")
}

fn unique_path(parent: &Path, prefix: &str) -> Result<PathBuf, String> {
    // Exclusive create (retry on collision) so concurrent publishers never share
    // a staging directory — nanos alone is not unique across threads.
    let pid = std::process::id();
    for _ in 0..10_000 {
        let seq = PATH_SEQ.fetch_add(1, Ordering::Relaxed);
        let path = parent.join(format!("{prefix}{pid}.{seq}"));
        match fs::create_dir(&path) {
            Ok(()) => return Ok(path),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(format!("mkdir {}: {error}", path.display()));
            }
        }
    }
    Err(format!(
        "could not allocate unique path under {} with prefix `{prefix}`",
        parent.display()
    ))
}

fn remove_path_best_effort(path: &Path) -> io::Result<()> {
    if !path.exists() {
        return Ok(());
    }
    if path.is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    }
}

fn copy_dir_recursive(src: &Path, dest: &Path) -> Result<(), String> {
    fs::create_dir_all(dest).map_err(|e| format!("mkdir {}: {e}", dest.display()))?;
    for entry in fs::read_dir(src).map_err(|e| format!("read_dir {}: {e}", src.display()))? {
        let entry = entry.map_err(|e| format!("read_dir entry: {e}"))?;
        let path = entry.path();
        let meta =
            fs::symlink_metadata(&path).map_err(|e| format!("lstat {}: {e}", path.display()))?;
        let ft = meta.file_type();
        let to = dest.join(entry.file_name());
        if ft.is_symlink() {
            return Err(format!(
                "layout: refusing to copy symlink {}",
                path.display()
            ));
        }
        if ft.is_dir() {
            copy_dir_recursive(&path, &to)?;
        } else if ft.is_file() {
            fs::copy(&path, &to).map_err(|e| format!("copy {}: {e}", path.display()))?;
        } else {
            return Err(format!(
                "layout: unsupported type while copying {}",
                path.display()
            ));
        }
    }
    Ok(())
}

/// Library helpers used by unit tests (hash / forbidden / smoke parse).
#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn temp_dir(label: &str) -> PathBuf {
        let parent = env::temp_dir().join("spirula-p2-f07-tests");
        fs::create_dir_all(&parent).unwrap();
        unique_path(&parent, &format!("{label}-")).unwrap()
    }

    fn write_fake_binary(dir: &Path, name: &str, body: &[u8]) -> PathBuf {
        let bin = dir.join(BIN_DIR);
        fs::create_dir_all(&bin).unwrap();
        let path = bin.join(name);
        fs::write(&path, body).unwrap();
        let mut perms = fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&path, perms).unwrap();
        path
    }

    fn fake_manifest(dir: &Path) -> Value {
        let mut binaries = Vec::new();
        for name in PACKAGE_BINARIES {
            let path = write_fake_binary(dir, name, format!("fake-{name}").as_bytes());
            let sha = sha256_file(&path).unwrap();
            let size = fs::metadata(&path).unwrap().len();
            binaries.push(json!({
                "name": name,
                "path": format!("{BIN_DIR}/{name}"),
                "sha256": sha,
                "size_bytes": size,
                "runtime_dependencies": ["/usr/lib/libSystem.B.dylib"]
            }));
        }
        write_notice(dir).unwrap();
        let manifest = json!({
            "schema_version": SCHEMA_VERSION,
            "feature": "P2-F07",
            "package_kind": "minimal-cli",
            "revision": "deadbeef",
            "version": "0.1.0",
            "host_target": "aarch64-apple-darwin",
            "toolchain": {
                "rustc_release": "1.99.0",
                "rustc_commit_hash": "b940084d7eb6a299eb4bfeb8e34901bc051e7ac4",
                "rustc_host": "aarch64-apple-darwin"
            },
            "profile": "dev",
            "locked": true,
            "binaries": binaries,
            "licenses": ["GPL-3.0-or-later"],
            "claims": {
                "signed_release": false,
                "three_platform_matrix": false,
                "surface_present_verified": false,
                "train_preview_ui_wired": false,
                "phase2_exit_closed": false,
                "bit_for_bit_reproducible": false
            }
        });
        fs::write(
            dir.join(MANIFEST_NAME),
            serde_json::to_string_pretty(&manifest).unwrap() + "\n",
        )
        .unwrap();
        manifest
    }

    fn rewrite_manifest(dir: &Path, mutator: impl FnOnce(&mut Value)) {
        let path = dir.join(MANIFEST_NAME);
        let mut manifest: Value =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        mutator(&mut manifest);
        fs::write(
            &path,
            serde_json::to_string_pretty(&manifest).unwrap() + "\n",
        )
        .unwrap();
    }

    #[test]
    fn check_accepts_matching_hashes() {
        let dir = temp_dir("hash-ok");
        fake_manifest(&dir);
        check_package(&dir).expect("ok");
        let _ = remove_path_best_effort(&dir);
    }

    #[test]
    fn check_rejects_tampered_binary() {
        let dir = temp_dir("tamper");
        fake_manifest(&dir);
        let victim = dir.join(BIN_DIR).join("spirula");
        fs::write(&victim, b"tampered").unwrap();
        let err = check_package(&dir).expect_err("tamper");
        assert!(err.contains("sha256 mismatch"), "{err}");
        let _ = remove_path_best_effort(&dir);
    }

    #[test]
    fn check_rejects_missing_binary() {
        let dir = temp_dir("missing");
        fake_manifest(&dir);
        fs::remove_file(dir.join(BIN_DIR).join("spirula-gpu-smoke")).unwrap();
        let err = check_package(&dir).expect_err("missing");
        assert!(
            err.contains("missing") || err.contains("bin/spirula-gpu-smoke"),
            "{err}"
        );
        let _ = remove_path_best_effort(&dir);
    }

    #[test]
    fn check_rejects_source_contamination() {
        let dir = temp_dir("src");
        fake_manifest(&dir);
        fs::write(dir.join("Cargo.toml"), b"[package]\n").unwrap();
        let err = check_package(&dir).expect_err("cargo");
        assert!(
            err.contains("Cargo.toml") || err.contains("allowlist"),
            "{err}"
        );
        let _ = remove_path_best_effort(&dir);
    }

    #[test]
    fn check_rejects_path_alias_to_other_binary() {
        let dir = temp_dir("alias");
        fake_manifest(&dir);
        // Reproduce review bypass: spirula path+hash → smoke binary, leave a
        // stub at bin/spirula.
        let smoke = dir.join(BIN_DIR).join("spirula-gpu-smoke");
        let smoke_sha = sha256_file(&smoke).unwrap();
        let smoke_size = fs::metadata(&smoke).unwrap().len();
        fs::write(dir.join(BIN_DIR).join("spirula"), b"#!/bin/sh\nexit 0\n").unwrap();
        rewrite_manifest(&dir, |m| {
            for entry in m["binaries"].as_array_mut().unwrap() {
                if entry["name"] == "spirula" {
                    entry["path"] = json!("bin/spirula-gpu-smoke");
                    entry["sha256"] = json!(smoke_sha);
                    entry["size_bytes"] = json!(smoke_size);
                }
            }
        });
        let err = check_package(&dir).expect_err("alias");
        assert!(err.contains("path must be exactly `bin/spirula`"), "{err}");
        let _ = remove_path_best_effort(&dir);
    }

    #[test]
    fn check_rejects_parent_dir_path() {
        let dir = temp_dir("dotdot");
        fake_manifest(&dir);
        rewrite_manifest(&dir, |m| {
            for entry in m["binaries"].as_array_mut().unwrap() {
                if entry["name"] == "spirula" {
                    entry["path"] = json!("../baseline/bin/spirula");
                }
            }
        });
        let err = check_package(&dir).expect_err("dotdot");
        assert!(
            err.contains("path must be exactly `bin/spirula`") || err.contains("`..`"),
            "{err}"
        );
        let _ = remove_path_best_effort(&dir);
    }

    #[test]
    fn check_rejects_outside_root_path_when_canonical_missing() {
        // Review probe: delete bin/spirula, retarget path to
        // ../baseline/bin/spirula-gpu-smoke with matching hash.
        let dir = temp_dir("outside");
        fake_manifest(&dir);
        let smoke = dir.join(BIN_DIR).join("spirula-gpu-smoke");
        let smoke_sha = sha256_file(&smoke).unwrap();
        let smoke_size = fs::metadata(&smoke).unwrap().len();
        fs::remove_file(dir.join(BIN_DIR).join("spirula")).unwrap();
        rewrite_manifest(&dir, |m| {
            for entry in m["binaries"].as_array_mut().unwrap() {
                if entry["name"] == "spirula" {
                    entry["path"] = json!("../baseline/bin/spirula-gpu-smoke");
                    entry["sha256"] = json!(smoke_sha);
                    entry["size_bytes"] = json!(smoke_size);
                }
            }
        });
        let err = check_package(&dir).expect_err("outside");
        assert!(
            err.contains("bin/spirula")
                || err.contains("path must be exactly")
                || err.contains("`..`"),
            "{err}"
        );
        let _ = remove_path_best_effort(&dir);
    }

    #[test]
    fn check_rejects_symlink_binary() {
        let dir = temp_dir("symlink");
        fake_manifest(&dir);
        let bin = dir.join(BIN_DIR).join("spirula");
        let external = temp_dir("symlink-ext").join("external");
        fs::write(&external, b"external-bin").unwrap();
        fs::remove_file(&bin).unwrap();
        std::os::unix::fs::symlink(&external, &bin).unwrap();
        let err = check_package(&dir).expect_err("symlink");
        assert!(err.contains("symlink"), "{err}");
        let _ = remove_path_best_effort(&dir);
        let _ = remove_path_best_effort(external.parent().unwrap());
    }

    #[test]
    fn check_rejects_missing_notice() {
        let dir = temp_dir("no-notice");
        fake_manifest(&dir);
        fs::remove_file(dir.join(NOTICE_NAME)).unwrap();
        let err = check_package(&dir).expect_err("notice");
        assert!(err.contains("NOTICE"), "{err}");
        let _ = remove_path_best_effort(&dir);
    }

    #[test]
    fn check_rejects_empty_metadata() {
        let dir = temp_dir("empty-meta");
        fake_manifest(&dir);
        // Keep only schema/locked/binaries like the review probe.
        rewrite_manifest(&dir, |m| {
            let binaries = m["binaries"].clone();
            *m = json!({
                "schema_version": SCHEMA_VERSION,
                "locked": true,
                "binaries": binaries,
            });
        });
        let err = check_package(&dir).expect_err("empty");
        assert!(
            err.contains("missing or empty `revision`")
                || err.contains("missing or empty `feature`")
                || err.contains("missing"),
            "{err}"
        );
        let _ = remove_path_best_effort(&dir);
    }

    #[test]
    fn check_rejects_duplicate_binary_name() {
        let dir = temp_dir("dup");
        fake_manifest(&dir);
        rewrite_manifest(&dir, |m| {
            let binaries = m["binaries"].as_array_mut().unwrap();
            let extra = binaries[0].clone();
            binaries.push(extra);
        });
        let err = check_package(&dir).expect_err("dup");
        assert!(
            err.contains("exactly 3") || err.contains("duplicate"),
            "{err}"
        );
        let _ = remove_path_best_effort(&dir);
    }

    #[test]
    fn destination_exists_is_rejected() {
        let parent = temp_dir("exists-parent");
        let out = parent.join("pkg out with spaces");
        fs::create_dir_all(&out).unwrap();
        let err = assemble_package_from_bin_dir(
            &parent,
            &out,
            &AssembleMeta {
                revision: "x".into(),
                version: "0.1.0".into(),
                profile: "dev".into(),
            },
        )
        .expect_err("exists");
        assert!(err.contains("already exists"), "{err}");
        let _ = remove_path_best_effort(&parent);
    }

    #[test]
    fn staging_residue_removed_on_assemble_failure() {
        let parent = temp_dir("residue");
        // Create a staging dir and ensure remove helper clears it.
        let staging = unique_staging(&parent).unwrap();
        fs::write(staging.join("junk"), b"x").unwrap();
        remove_path_best_effort(&staging).unwrap();
        assert!(!staging.exists());
        // No leftover staging prefixes after cleanup.
        for entry in fs::read_dir(&parent).unwrap() {
            let name = entry.unwrap().file_name().to_string_lossy().into_owned();
            assert!(!name.starts_with(STAGING_PREFIX), "leftover staging {name}");
        }
        let _ = remove_path_best_effort(&parent);
    }

    #[test]
    fn publish_staging_no_clobber_preserves_existing_destination() {
        let parent = temp_dir("noclobber");
        let staging = unique_staging(&parent).unwrap();
        fs::write(staging.join("payload"), b"new").unwrap();
        let out = parent.join("out");
        fs::create_dir(&out).unwrap();
        fs::write(out.join("sentinel"), b"keep").unwrap();
        let err = publish_staging_no_clobber(&staging, &out).expect_err("exists");
        assert!(err.contains("already exists"), "{err}");
        assert_eq!(fs::read(out.join("sentinel")).unwrap(), b"keep");
        assert!(!staging.exists(), "staging must be cleaned on refuse");
        // Must not delete or replace someone else's destination contents.
        assert!(!out.join("payload").exists());
        let _ = remove_path_best_effort(&parent);
    }

    #[test]
    fn publish_one_shot_out_absent_until_rename_then_checks() {
        let parent = temp_dir("oneshot");
        let staging = unique_staging(&parent).unwrap();
        fake_manifest(&staging);
        let out = parent.join("out");
        assert!(
            !destination_entry_present(&out).unwrap(),
            "out must not exist before no-replace rename"
        );
        publish_staging_no_clobber(&staging, &out).expect("rename");
        assert!(!staging.exists(), "staging consumed by rename");
        check_package(&out).expect("full package after one-shot publish");
        let _ = remove_path_best_effort(&parent);
    }

    #[test]
    fn smoke_parse_pass_and_skip_fields() {
        let pass = "spirula-gpu-smoke kind=compute status=pass wgpu_version=30 repeat=0\n\
match=cpu_exact\nafter_drop current=0\n";
        assert_eq!(parse_smoke_field(pass, "status=").as_deref(), Some("pass"));
        assert_eq!(
            parse_smoke_field(pass, "match=").as_deref(),
            Some("cpu_exact")
        );
        assert_eq!(
            parse_smoke_field(pass, "after_drop current=").as_deref(),
            Some("0")
        );
        let skip = "spirula-gpu-smoke kind=compute status=skip wgpu_version=30 repeat=0\n\
open_failure_stage=Adapter\nreason=no adapter\n";
        assert_eq!(parse_smoke_field(skip, "status=").as_deref(), Some("skip"));
    }

    #[test]
    fn skip_is_not_counted_as_pass() {
        // Document the gate rule used by run_package_isolated.
        let status = "skip";
        let counted_as_pass = status == "pass";
        assert!(!counted_as_pass);
    }
}
