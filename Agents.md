# Agents.md — Spirula Studio Rust rewrite

This file is the operating contract for humans and coding agents working in
this repository. Read it before changing code.

## Project objective

Rewrite `/Users/tfjiang/Projects/spirula-studio` in Rust. The desktop UI will
use Slint and the GPU implementation will use `wgpu` with WGSL shaders. The
rewrite is incremental: establish the architecture first, then port one
observable capability at a time.

The C++ repository is the behavioral reference. It is not a dependency of this
repository and must not be modified as part of this migration unless the user
explicitly asks for that separate change.

## Current state

The initial commit contains only a buildable Rust workspace skeleton:

```text
crates/
  spirula-core     domain contracts and pure algorithms
  spirula-io       datasets, images, checkpoints, and exports
  spirula-gpu      wgpu device and resource boundary
  spirula-render   Gaussian rendering and preview
  spirula-train    training orchestration and resume
  spirula-ui       Slint UI and view models
  spirula-cli      `spirula` process entry point
docs/architecture.md
```

No business feature is considered ported yet.

## Non-negotiable rules

1. Architecture comes before feature breadth. Do not add a feature that creates
   an unreviewed dependency edge or bypasses the ownership rules in
   `docs/architecture.md`.
2. Preserve behavior before improving behavior. Record the C++ behavior with a
   fixture, golden output, or explicit acceptance case before changing it.
3. Port in vertical slices. A slice includes its domain contract, implementation,
   user-visible entry point where applicable, tests, and documentation.
4. Every feature must be fully tested before its Git commit. A commit must leave
   the workspace buildable and the feature reproducible from a clean checkout.
5. Keep changes reversible. Do not delete compatibility readers, old fixtures,
   or migration paths until the replacement has passed the required parity
   checks and the removal is a separately identified task.
6. Keep UI and GPU details out of the domain layer. UI code sends commands;
   training and rendering code publish state and events.
7. Avoid duplicate implementations. One parser, one checkpoint policy, one
   session state machine, and one render interface should serve both the CLI and
   the Slint UI.
8. Do not add Python, C++, CUDA, or a direct Vulkan dependency to the new runtime
   path. Development scripts may use external tools only when they are not
   required to build or run the Rust application.
9. Keep unsafe code out of the initial port. Any later unsafe block needs a
   narrow module boundary, a safety comment, and a focused test.
10. Do not optimize before a benchmark or profile identifies the bottleneck.

## Definition of done for each feature

A feature is ready to commit only when all applicable conditions hold:

- its owner crate and public API are clear;
- the C++ reference behavior and compatibility assumptions are documented;
- happy path, invalid input, cancellation, and persistence cases are covered;
- CPU reference and GPU implementation agree within a documented tolerance when
  the feature uses the GPU;
- headless tests pass without requiring a visible window;
- Slint behavior is tested separately from application services;
- `cargo fmt --check`, `cargo check --workspace`, `cargo test --workspace`, and
  `cargo clippy --workspace --all-targets --all-features -- -D warnings` pass;
- the relevant documentation and migration notes are updated;
- the commit contains one coherent feature and a message that names its result.

If a check cannot run because a GPU, model, or fixture is unavailable, record
the exact command, environment, and reason in the feature task. Do not call an
untested feature complete.

## Migration method

For each subsystem, map the current readers, writers, data shape, ownership,
compatibility window, and rollback path before editing. Follow this order:

1. **Inventory** — identify C++ entry points, formats, tests, and observable
   outputs.
2. **Contract** — write Rust types, invariants, error cases, and compatibility
   fixtures.
3. **Expand** — implement the new Rust path alongside the reference behavior.
4. **Verify** — compare outputs, timings, errors, and persisted data.
5. **Adopt** — connect the CLI/UI to the Rust path after parity passes.
6. **Contract** — remove obsolete code only in a later, explicit cleanup task.

Partial failures must be visible and retries must be safe. Checkpoints and
exports must use atomic writes. A stopped training job must either resume from a
complete checkpoint or report why it cannot do so.

## Complete project plan

### Phase 0 — baseline and architecture

- Freeze a reference commit of `spirula-studio` and record its build/test entry
  points.
- Inventory the existing features from `src/core`, `src/data`, `src/engine`,
  `src/backend`, `src/app`, `src/app/gui`, `src/checkpoint`, `src/mesh`,
  `src/sfm`, `src/nn`, `src/sam`, and `viewer`.
- Build a capability matrix: supported dataset formats, camera models, image
  formats, checkpoint versions, GPU adapters, CLI commands, and UI flows.
- Capture golden fixtures for camera projection, dataset parsing, scene transforms,
  splat serialization, checkpoint metadata, and a tiny rendered scene.
- Finalize dependency direction, session state machine, error model, event model,
  GPU boundary, shader conventions, and test strategy.
- Acceptance: this workspace builds, the architecture documents are reviewed,
  and the capability matrix has a named source or fixture for each item.

### Phase 1 — Rust foundation

- Add shared error, ID, logging, configuration, and cancellation conventions.
- Add `spirula-core` math and scene primitives with no UI/GPU dependencies.
- Add CI-equivalent local commands and deterministic test fixture layout.
- Establish serialization versioning and atomic file helpers.
- Acceptance: clean checkout passes all workspace checks and has no feature code
  hidden in the CLI placeholder.

### Phase 2 — GPU foundation

- Implement `wgpu` instance, adapter selection, device/queue creation, limits,
  feature negotiation, labels, error scopes, and device-loss reporting.
- Add buffer/texture abstractions, staging/readback, uniform layouts, and shader
  loading with deterministic pipeline keys.
- Add a headless compute smoke test and a minimal window surface smoke test.
- Document supported adapters and fallback behavior.
- Acceptance: headless GPU test runs on the development machine and reports a
  clear error on unsupported adapters.

### Phase 3 — data and scene compatibility

- Port camera models and projection math first, including perspective, fisheye,
  equirectangular, distortion, coordinate conventions, and EXIF orientation.
- Port COLMAP, Nerfstudio, and Metashape dataset readers with fixture tests.
- Port image probing/decoding, masks, point clouds, scene transforms, regions,
  and deterministic random point seeding.
- Port splat and checkpoint read/write compatibility before training.
- Acceptance: reference fixtures load and round-trip, invalid datasets produce
  stable actionable errors, and existing checkpoint metadata is understood.

### Phase 4 — render MVP

- Define the render scene and camera interfaces.
- Port upload, projection, tile/bin construction, sorting, rasterization,
  compositing, depth, and readback as separate GPU features.
- Keep a tiny CPU reference renderer for correctness and debugging.
- Add image golden tests with documented tolerances and a benchmark scene.
- Acceptance: a loaded fixture renders a stable preview through a headless render
  API and through the application session.

### Phase 5 — training core

- Implement session lifecycle, step scheduling, optimizer state, loss terms,
  densification/pruning, evaluation, and progress events.
- Add stop, cancel, save, crash recovery, and resume semantics.
- Reuse the render interfaces; do not create a UI-specific training path.
- Add deterministic tiny-scene training tests and checkpoint continuation tests.
- Acceptance: a tiny training run reaches a known step, resumes from a checkpoint,
  and produces bounded metric/output differences from the reference fixture.

### Phase 6 — CLI and Slint application shell

- Replace the placeholder CLI with command parsing and the shared application
  session service.
- Add Slint window, navigation, commands, validation, logs, progress, and
  cancellation views.
- Implement dataset open/preview, training start/stop/resume, and render/export
  flows as complete UI slices.
- Add headless view-model tests and targeted Slint component tests.
- Acceptance: the same session service works from CLI and desktop UI, and a
  user can load a fixture, preview it, train a tiny run, stop/save, and resume.

### Phase 7 — editing, mesh, and export

- Port selection, masks, regions, scene transforms, sparse edits, mesh creation,
  mesh import/export, image/video render export, and web viewer assets.
- Keep document formats versioned and use atomic saves.
- Add round-trip and visual regression fixtures.
- Acceptance: edited scene documents reopen without losing state and exports are
  consumable by the existing viewer/reference tools.

### Phase 8 — auxiliary pipelines

- Port SfM, dense reconstruction, depth/normal estimation, segmentation,
  feature matching, video decode, and model/license management in separate
  feature slices.
- Treat model weights, licenses, platform codecs, and patent-gated features as
  explicit capability checks.
- Acceptance: each pipeline has an isolated integration test, failure reporting,
  and a documented availability matrix.

### Phase 9 — parity, performance, and release

- Run cross-platform adapter tests on macOS, Linux, and Windows where hardware
  is available.
- Compare render quality, training convergence, memory use, startup time, and
  checkpoint size against the reference.
- Add profiling, shader/pipeline caching, packaging, crash reporting, and release
  documentation only after correctness gates pass.
- Acceptance: release checklist is reproducible from a clean checkout, all
  supported feature gates are explicit, and known differences are documented.

## Planned feature order

The first implementation sequence is:

1. core errors, IDs, math, camera contracts, and fixture harness;
2. `wgpu` device/context smoke test;
3. dataset parsing and camera compatibility;
4. splat scene upload and preview rendering;
5. checkpoint read/write;
6. tiny training loop and resume;
7. CLI commands;
8. Slint shell and shared session events;
9. editing/export/mesh;
10. SfM, dense, AI masking, depth/normal, and video capabilities.

Each item is a separate feature task and commit after its tests pass.

## Git workflow

Use small commits with one outcome. Suggested message format:

```text
feat(core): add camera projection contracts
feat(gpu): add headless wgpu device smoke test
feat(data): parse colmap sparse datasets
feat(render): render fixture gaussian scene
feat(train): save and resume tiny training run
feat(ui): add dataset preview screen
```

Before committing, inspect `git diff`, run the complete applicable checks, and
record any environment-dependent checks. Never commit generated build output,
local datasets, model weights, credentials, or machine-specific paths.

## Review checklist

- Does the change fit the architecture and dependency direction?
- Is the behavior captured by a test or fixture?
- Are file formats and checkpoint compatibility preserved?
- Are cancellation, invalid input, and partial failure handled?
- Does the CLI/UI share the same application service?
- Does GPU code have labels, error reporting, and a testable fallback?
- Is the commit small enough to revert safely?

