# Phase 0 exit report

Recorded: 2026-10-23 17:52 CST (2026-10-23T09:52:20Z), on the development
machine. The repository state is the Phase 0 tree; the parent commit is
`2b4c6e9` (`feat(P0-F05): freeze runtime contracts`) and this report is added
by `feat(P0-F06): add Phase 0 verification entry`.

## 1. Scope and result

Phase 0 is complete. The baseline, inventory, capability matrix, fixtures,
runtime contracts and verification entry all exist, and the workspace gate
passes from this tree.

```text
fixtures: ok (21 files verified; 6 format checks)
docs: ok (61 features, 17 files, 63 links, 26 error codes)
reference baseline: ok
fmt: ok
check: ok
test: ok
clippy: ok
diff: ok
verify: ok (7 checks passed)
```

## 2. Environment

| item | value |
|---|---|
| OS | macOS 27.0.1, arm64 |
| Rust | rustc 1.99.0, cargo 1.99.0 |
| workspace minimum | `rust-version = "1.99"`, edition 2024, resolver 3 |
| C++ reference | `9a24f09927944a795e7ba1bd8243a625bf419479`, application version 2026.10.16 |
| CMake | 4.3.0 |
| Apple Clang | 21.0.0 |
| Python | 3.11.2 |
| Ninja | not installed |
| CUDA / nvcc | not installed |
| Vulkan SDK / Slang | not installed |

The C++ reference checkout was verified by
`bash tools/reference-baseline.sh check` (via
`cargo run -p spirula-xtask -- baseline`): correct remote, pinned commit, no
tracked modifications.

## 3. Commands and results

Run from a checkout of this commit:

```bash
cargo run -p spirula-xtask -- verify     # complete Phase 0 gate
cargo run -p spirula-xtask -- fixtures   # fixture checks only
cargo run -p spirula-xtask -- docs       # document/status checks only
cargo run -p spirula-xtask -- baseline   # C++ reference check (may skip)
```

Observed results:

| check | result |
|---|---|
| fixture manifest and formats | 21 files, SHA-256 and sizes verified; camera, dataset, scene, splat, checkpoint and render checks passed |
| fixture negative tests | single-bit corruption rejected; missing golden rejected |
| document/status | 61 feature rows, exactly one `当前` (P1-F01), 17 Markdown files, 63 relative links resolve, 26 stable error codes |
| reference baseline | pinned commit and clean tracked tree; toolchain recorded |
| `cargo fmt --all -- --check` | pass |
| `cargo check --workspace` | pass |
| `cargo test --workspace` | pass (6 tests: 3 fixture integration, 2 xtask unit, 1 docs integration) |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | pass |
| `git diff --check` | pass |

The explicit gate commands from
[feature-definition-of-done.md](../agent/feature-definition-of-done.md) were
also run individually and pass.

## 4. Feature evidence

| feature | record | evidence |
|---|---|---|
| P0-F01 reference baseline | [reference-baseline.md](reference-baseline.md) | upstream URL, full commit, toolchain/dependency versions, build/test/failure-log entry points, capture script; `tools/reference-baseline.sh check` passes |
| P0-F02 subsystem inventory | [subsystem-inventory.md](subsystem-inventory.md) | entry points and call chains, formats/readers/writers, render/GPU/train/UI behavior, error/cancel/partial-failure matrix, observable outputs |
| P0-F03 capability matrix | [capability-matrix.md](capability-matrix.md) | capability rows with reference source, formats, compatibility assumption, verification and owner; every Rust row `未开始` |
| P0-F04 fixtures | [fixtures.md](fixtures.md), [`fixtures/`](../../fixtures/README.md) | checksummed camera, COLMAP dataset + PNG, scene transform, splat/seed PLY, checkpoint, micro render golden; `spirula-io` checker and tests |
| P0-F05 runtime contracts | [runtime-contracts.md](runtime-contracts.md) | 26-code error catalog, command/event envelopes, state machine, cancellation, GPU/shader boundary, versioning, reference divergences |
| P0-F06 verification entry | this report, `tools/xtask` | one command runs fixtures + docs + baseline + workspace gate; docs checker enforces statuses, artifacts, links, feature ids and error codes |

Phase 0 exit criteria from [project-plan.md](../project-plan.md):

- P0-F01 through P0-F06 each have a record in `docs/phase-0/`: **met**.
- The workspace builds and passes fmt/check/test/clippy: **met**.
- The architecture, capability matrix and fixtures can be referenced by later
  features: **met** (links are checked; `spirula-io` exposes the fixture
  checker).

## 5. Not run and known limitations

- **C++ build and parity suite were not run.** The machine has no Ninja, CUDA
  toolkit, Vulkan SDK or `slangc`. The exact commands are recorded in
  [reference-baseline.md](reference-baseline.md) sections 3 and 4. A machine
  with that toolchain must run them before Phase 3 marks format compatibility
  complete.
- **Fixtures were hand-derived from reference formats**, not exported by a
  running C++ build. The compatibility check against the reference tools is
  still owed; [fixtures.md](fixtures.md) section 4 records the follow-up.
- **The micro render golden is analytic**, not a C++ render. P4-F01 owns the
  actual renderer tolerance and may revise the fixture version with a recorded
  reason.
- **No GPU tests exist yet.** Phase 2 introduces adapter/device smoke tests;
  Phase 0 asserts no GPU behavior.
- **No Windows/Linux run was recorded.** P9-F01 owns the cross-platform
  adapter matrix; the Rust workspace checks here are host-only.
- **First build needs crates.io access** for `sha2` and `serde_json`;
  `Cargo.lock` pins the exact versions. No vendored registry is committed.
- The local reference checkout had an untracked `CMakePresets.json`; it is not
  part of the baseline, and `check` only rejects tracked modifications.

## 6. Reproducing from a clean checkout

```bash
git clone <this repository>
cd rust-spirula
cargo run -p spirula-xtask -- verify
```

Expected: all fixture, document, fmt, check, test and clippy checks pass. The
reference baseline check is skipped when `SPIRULA_REFERENCE_REPO` (or a sibling
`spirula-studio` checkout) is absent; that skip is reported, not treated as a
failure. On a machine with the reference checkout, run
`cargo run -p spirula-xtask -- baseline` and record the result.

## 7. Next step

P1-F01, domain errors and stable IDs, is marked `当前` in
[project-plan.md](../project-plan.md). It implements the error catalog and
fields frozen in [runtime-contracts.md](runtime-contracts.md) section 1 in
`spirula-core`, with the code/category/retryable snapshot and invalid-input
tests required by the plan.
