# P2-F07: minimal CLI package / isolated launch

Status: **已完成** (code tip `17c1202a6a2b70183192a1367d96ffbb0e992e67`).
Owner: `spirula-xtask` (workspace tooling). Builds on [cli.md](../phase-1/cli.md)
(P1-F09), [gpu-smoke.md](gpu-smoke.md) (P2-F06). Does **not** wire
application/train/UI GPU ownership. Phase 2 exit remains open (surface present
still unverified from F06). Next planned early spike: P6-F07 (**当前**).

## Reference behavior (C++)

Pinned: `9a24f09927944a795e7ba1bd8243a625bf419479`.

| C++ surface | Behavior recorded | Rust binding in this slice |
|---|---|---|
| `docs/build.md` Packaging | macOS `.app`/`.dmg` via `tools/package_macos.sh`; Linux/Windows are single binaries | **Deviation**: no `.app`/DMG/signed installer (P9-F04). Portable directory + manifest only |
| `SsMacBundle.cmake` / `ss_mac_prefer_static()` | Prefer static archives so the Mac bundle is one file | Record dylib deps honestly (`otool -L` / `ldd`); fail package if non-system dylibs appear on macOS |
| `package_macos.sh` otool gate | Reject anything outside `/usr/lib` or `/System/Library` | Same macOS system-path check for packaged binaries; no claim of three-platform matrix |
| Windows CRT note | Dynamic MSVC runtime redistributable required | Record host target + runtime dependency list; **Windows package publish is unsupported** in this tip |
| Signing / notarization | Ad-hoc or Developer ID + notarize | **Out of scope** — no signing, notarization, or store publish |
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

`MANIFEST.json` supports two schema versions (compatible read):

| schema_version | Layout | Notes |
|---|---|---|
| **1** (legacy P2-F07) | `MANIFEST.json`, `NOTICE`, `bin/` + three bins | Must **not** declare `samples[]` or contain `sample/` |
| **2** (P4-F06) | schema 1 + `sample/preview/{request.json,scene.ply}` | Requires `samples[]` with path/sha256/size_bytes for both files |

New packages that include the preview sample emit **schema 2**. Clean
`--from-ref` builds copy sample bytes from the selected `build_root` (same
revision as binaries); dirty-dev host assembly may embed fixtures compiled into
xtask (not clean-ref proof). A build tree that predates `fixtures/preview`
emits honest schema 1 without samples.

Common required fields (missing/empty/wrong-type → typed `manifest:` failure):

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

Symlinks (including under `bin/`/`sample/` or parents), extra files, or dangling
links fail with `layout:`. This is execution/integrity consistency, **not**
cryptographic signing of rewritten manifests.

### Atomic publish

Complete staging → **one** no-replace directory rename (not child-by-child):

1. Fast-fail if the destination entry already exists (`symlink_metadata`).
2. Write a sibling staging directory `.spirula-pkg-tmp.*` and verify the full
   package there. Until rename succeeds, `out` does not exist; interrupting
   leaves only staging (deletable) and no partial destination.
3. Publish with `rustix::fs::renameat_with(..., RenameFlags::NOREPLACE)` on
   macOS (`RENAME_EXCL`) / Linux (`RENAME_NOREPLACE`). **Windows and other
   hosts return a typed unsupported error** without creating `out`.
4. If the destination appeared meanwhile (`EEXIST`), **refuse**, delete
   staging only, and **preserve** the existing destination.
5. Concurrent publishers to the same `out`: exactly one rename winner; losers
   fail without overwrite; no staging residue.
6. Paths with spaces are supported for `--out` and isolation dirs.
7. No signing / installer / multi-platform matrix in this slice.

### Isolated launch

`spirula-xtask package-run --package <dir>`:

- Copies only the package into a source-free isolate directory (or verifies the
  package root itself has no repo markers).
- Runs with an arbitrary cwd (including paths with spaces) and a scrubbed env
  (no `CARGO_*` / `RUSTUP_*` / workspace `SPIRULA_*` that imply a checkout).
- After copy, re-run full `check_package` on the isolate (bin + sample hashes).
- Required checks:
  1. `spirula help` → exit 0
  2. `spirula capabilities` → exit 0
  3. `spirula probe --label pkg --steps 1` → exit 0 (fake probe)
  4. Preview:
     - schema 2 → `spirula preview` → exit 0; full RGB8 decode of 3×2 with the
       fixed independent golden pixels (not a signature-only check)
     - schema 1 → preview reported **unavailable** (not pretended as success)
  5. `spirula-gpu-smoke --compute` → exit 0 with either:
     - `status=pass` **and** `match=cpu_exact` **and** `after_drop current=0`, or
     - `status=skip` with an explicit open/adapter reason (**not** counted as pass)

### Platform honesty

| Host | Publish (`NOREPLACE` rename) | Isolated package-run / Metal-or-GPU smoke |
|---|---|---|
| macOS | **Implemented and host-tested** (this tip) | **Tested**: real M5 Metal compute smoke pass / `cpu_exact` / `current=0` |
| Linux | **Implemented** (same rustix path) | **Not host-tested** in this tip — no Linux claim |
| Windows / other | **Unsupported** (typed failure; no `out`) | Not claimed |

### Non-goals (this slice)

- No P9-F04 full installer / three-platform matrix / code signing / notarization.
- No train / UI / application GPU session wiring. Schema-2 packages include a
  tiny `spirula preview` sample; schema-1 remains readable without preview.
- No claim that surface present smoke is verified (F06 surface remains
  unsupported).
- Phase 2 exit **not** closed by completing P2-F01–P2-F07 alone (surface
  present still unverified).

## Acceptance (met)

1. Reproducible package from clean HEAD or `--from-ref` with `--locked`.
2. Manifest hashes bind to unique canonical `bin/<name>`; missing/tampered /
   path-alias / symlink / outside-root fail typed checks.
3. Destination-exists / NOREPLACE race / interrupt-before-rename / spaces /
   isolated no-source covered; concurrent same-`out` → single winner.
4. Isolated help/capabilities/fake-probe + compute smoke pass or precise skip.
5. macOS non-system dylibs rejected; system frameworks recorded honestly.
6. Docs keep plan IDs unique and unrenumbered (P2-F08 owns surface present);
   Phase 2 exit open; train/preview/UI not claimed.

## Commands

```bash
cargo run -p spirula-xtask -- package --out /tmp/spirula-cli-pkg
cargo run -p spirula-xtask -- package-check --package /tmp/spirula-cli-pkg
cargo run -p spirula-xtask -- package-run --package /tmp/spirula-cli-pkg
```

Optional: `--from-ref <sha>`, `--profile release|dev`.

## Tested evidence (approved tip)

Code tip: `17c1202a6a2b70183192a1367d96ffbb0e992e67`. Independent READY:
`/tmp/spirula-p2-f07-review/REVIEW-final-17c1202.md`. Parent exact-SHA
evidence (sample logs):

| Check | Sample result |
|---|---|
| Independent READY | `/tmp/spirula-p2-f07-review/REVIEW-final-17c1202.md` (3 P2 closed) |
| Parent fresh full verify | **8/8** |
| Host package tests | **4/4** |
| Exact `git archive` release package | `/tmp/spirula-codex-cli-pkg-17c1202` |
| package-check + source-free package-run | M5 Metal smoke **pass** / `cpu_exact` / `current=0` |

## Non-claims

- Not a signed / notarized / store-ready installer.
- Not three-platform release evidence (macOS tested; Linux code path untested
  on host; Windows publish unsupported).
- Not surface present verification; not train/preview/UI readiness.
- Not bit-for-bit reproducible across machines (locked deps + recorded hashes
  only).
- Completing all Phase 2 feature IDs does **not** close Phase 2 exit while
  surface present remains unverified.
