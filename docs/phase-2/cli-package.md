# P2-F07: minimal CLI package / isolated launch

Status: **当前 / 未批准** (integrity fix in progress; not merge-ready).
Owner: workspace / `spirula-xtask`. Builds on [cli.md](../phase-1/cli.md)
(P1-F09), [gpu-smoke.md](gpu-smoke.md) (P2-F06). Does **not** wire
application/train/UI GPU ownership. Phase 2 exit remains open (surface present
still unverified from F06). Next planned early spike after merge: P6-F07.

## Reference behavior (C++)

Pinned: `9a24f09927944a795e7ba1bd8243a625bf419479`.

| C++ surface | Behavior recorded | Rust binding in this slice |
|---|---|---|
| `docs/build.md` Packaging | macOS `.app`/`.dmg` via `tools/package_macos.sh`; Linux/Windows are single binaries | **Deviation**: no `.app`/DMG/signed installer (P9-F04). Portable directory + manifest only |
| `SsMacBundle.cmake` / `ss_mac_prefer_static()` | Prefer static archives so the Mac bundle is one file | Record dylib deps honestly (`otool -L` / `ldd`); fail package if non-system dylibs appear on macOS |
| `package_macos.sh` otool gate | Reject anything outside `/usr/lib` or `/System/Library` | Same macOS system-path check for packaged binaries; no claim of three-platform matrix |
| Windows CRT note | Dynamic MSVC runtime redistributable required | Record host target + runtime dependency list; Windows not claimed tested here |
| Signing / notarization | Ad-hoc or Developer ID + notarize | **Out of scope** — no signing, notarization, or publish |
| Build scripts `build_develop.bash` | Configure+build in-tree with backend switches | Clean `git archive` (or clean HEAD) + `cargo build --locked`; no C++/CUDA/Vulkan runtime path |

The C++ reference has **no** portable CLI package manifest that pins revision,
binary hashes, toolchain, and licenses together. Rust adds that contract here.

## Binding decisions

### Package contents

Shipped binaries only (release or explicit profile):

| Binary | Crate | Role |
|---|---|---|
| `spirula` | `spirula-cli` | help / capabilities / fake probe (P1-F09) |
| `spirula-gpu-diagnose` | `spirula-gpu` | adapter/device/resource/compute diagnose |
| `spirula-gpu-smoke` | `spirula-gpu` | headless compute smoke (P2-F06) |

Layout:

```text
<package>/
  MANIFEST.json
  NOTICE          # license id + pointer (GPL-3.0-or-later)
  bin/spirula
  bin/spirula-gpu-diagnose
  bin/spirula-gpu-smoke
```

Forbidden in the package: source trees, `.git`, `Cargo.toml`/`Cargo.lock`,
`target/`, credentials, datasets, model weights, absolute machine-only paths
in the manifest values beyond recorded host triple/toolchain strings.

### Dependency direction

- CLI stays `spirula-core` + `spirula-application` only — **no** new CLI→GPU edge.
- GPU tools remain separate binaries under `spirula-gpu`.
- Packaging lives in `spirula-xtask` (tooling), not a runtime crate.

### Clean / dirty policy

- Packaging `HEAD` requires a clean worktree (`git status --porcelain` empty).
- Dirty trees must pass `--from-ref <commit>` so the build uses `git archive` of
  that ref into an isolated checkout (no local dirty sources).
- Build always uses `cargo build --locked` against the archived (or clean) tree.

### Manifest fields

`MANIFEST.json` schema version 1 records (all required; missing/empty/wrong-type
→ typed `manifest:` failure):

- `feature` = `P2-F07`, `package_kind` = `minimal-cli`
- `revision` (git SHA), `version` (workspace package version)
- `host_target`, `toolchain.{rustc_release,rustc_commit_hash,rustc_host}`
- `profile` (`release`|`dev`), `locked: true`
- exactly three `binaries[]` entries; each `name` is unique and from the
  allowlist; **`path` must equal `bin/<name>`** (no absolute/`..`/aliases);
  `sha256` / `size_bytes` / `runtime_dependencies[]` must match that
  canonical regular file — the same path `package-run` executes
- `licenses[]` includes `GPL-3.0-or-later`
- `claims` object with the six honesty bools
- `NOTICE` must exist as a regular (non-symlink) file

Package layout allowlist only: `MANIFEST.json`, `NOTICE`, `bin/`, and the three
canonical binaries. Symlinks (including under `bin/` or parents), extra files,
or dangling links fail with `layout:`. This is execution/integrity consistency,
**not** cryptographic signing of rewritten manifests.

### Atomic publish

1. Refuse if the destination path already exists.
2. Write a sibling staging directory `.spirula-pkg-tmp.*`.
3. On success, rename staging → destination (same parent).
4. On failure, delete staging residue so a retry is clean.
5. Paths with spaces are supported for `--out` and isolation dirs.

### Isolated launch

`spirula-xtask package-run --package <dir>`:

- Copies only the package into a source-free isolate directory (or verifies the
  package root itself has no repo markers).
- Runs with an arbitrary cwd (including paths with spaces) and a scrubbed env
  (no `CARGO_*` / `RUSTUP_*` / workspace `SPIRULA_*` that imply a checkout).
- Required checks:
  1. `spirula help` → exit 0
  2. `spirula capabilities` → exit 0
  3. `spirula probe --label pkg --steps 1` → exit 0 (fake probe)
  4. `spirula-gpu-smoke --compute` → exit 0 with either:
     - `status=pass` **and** `match=cpu_exact` **and** `after_drop current=0`, or
     - `status=skip` with an explicit open/adapter reason (**not** counted as pass)

### Non-goals (this slice)

- No P9-F04 full installer / three-platform matrix / code signing / notarization.
- No train / preview / UI / application GPU session wiring.
- No download of release artifacts; no git push/pull.
- No claim that surface present smoke is verified (F06 surface remains
  unsupported).
- Phase 2 exit **not** closed by this feature alone.

## Acceptance (for clean candidate; not approved)

1. Reproducible package command from clean HEAD or `--from-ref`.
2. Manifest hashes verify; missing/tampered binaries fail check.
3. Destination-exists / failure-residue / spaces / isolated no-source covered.
4. Isolated help/capabilities/fake-probe + compute smoke pass or precise skip.
5. macOS non-system dylibs rejected; system frameworks recorded honestly.
6. Docs keep 74 plan IDs; Phase 2 exit open; C-X-06 stays in progress until
   independent READY + docs closure.

## Commands

```bash
cargo run -p spirula-xtask -- package --out /tmp/spirula-cli-pkg
cargo run -p spirula-xtask -- package-check --package /tmp/spirula-cli-pkg
cargo run -p spirula-xtask -- package-run --package /tmp/spirula-cli-pkg
```

Optional: `--from-ref <sha>`, `--profile release|dev`.

## Tested evidence (candidate tip)

| Check | Command | Sample result |
|---|---|---|
| Unit (canonical path/symlink/NOTICE/metadata/…) | `cargo test -p spirula-xtask --lib package::` | **14/14** |
| Host assemble + isolated run | `cargo test -p spirula-xtask --test package_host` | **1/1** |
| Docs consistency (74 IDs) | `cargo test -p spirula-xtask --test docs` | **ok** |
| Fresh full gate | `cargo run -p spirula-xtask -- verify` | **8/8** `/tmp/spirula-cursor-p2-f07-integrity-full-verify.log` |
| Focused aggregate | unit+host+docs | `/tmp/spirula-cursor-p2-f07-integrity-focused.log` |
| Review probes (reject) | manifest-path / empty-metadata / missing-notice / symlink / missing-canonical | package-check fails as typed `manifest:`/`layout:` |

## Non-claims

- Not a signed / notarized / store-ready installer.
- Not three-platform release evidence (host under test only).
- Not surface present verification; not train/preview/UI readiness.
- Not bit-for-bit reproducible across machines (locked deps + recorded hashes
  only).
- Phase 2 exit remains open until surface and remaining exit evidence land.
