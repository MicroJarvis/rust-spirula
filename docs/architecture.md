# Target architecture

## Goals

The rewrite must preserve the useful behavior of the C++ reference repository
while making ownership, testing, and portability explicit. The first release
target is a usable desktop application with a CLI, dataset loading, GPU
preview, training, checkpoint resume, and export. Large auxiliary pipelines
are added only after the core path is stable.

## Dependency direction

```text
spirula-ui ───────┐
spirula-cli ──────┼──> spirula-application
                  │          │
                  │          ├──> spirula-train ──> spirula-render ──> spirula-gpu  (later)
                  │          ├──> spirula-io ──────> spirula-core                 (later)
                  │          └──> spirula-core   (P1-F07: only this edge is live)
                  └──> spirula-core
```

`spirula-application` exists as of P1-F07: unique session owner for admission,
cancellation, recoverable control history, snapshots/cursors, and a fake probe
worker. In this slice it depends only on `spirula-core` (no train/GPU/io).
P1-F09 supplies the early CLI; P6-F01 integrates full business services.
`spirula-train` owns step scheduling, not a second application state machine.

## Responsibilities

### `spirula-core`

Pure domain types and algorithms: camera models, transforms, scene and splat
data, configuration, validation, errors, IDs, progress values, and stable
serialization models. It must not depend on Slint, `wgpu`, an operating system,
or a specific dataset parser.

### `spirula-io`

Dataset readers and writers, image and point-cloud access, checkpoint formats,
atomic persistence, cache policy, and export adapters. Format compatibility is
tested with fixtures and golden files. It owns no GPU resources.

### `spirula-gpu`

Adapter/device selection, queue and command submission, buffer and texture
lifetimes, shader loading, pipeline cache, synchronization, readback, and GPU
diagnostics. Public `wgpu` types stop at this boundary. Render/train consume
narrow typed wrappers with context/generation validation. P2-F03/P2-F04 prove
upload→compute→readback and P4-F02 proves resource sharing before expanding
the interface. A change to this boundary requires updating the runtime
contract and this document together; there is no implicit render exception.

### `spirula-render`

GPU-independent scene/render contracts plus GPU implementations for Gaussian
splat projection, sorting, rasterization, compositing, depth, preview
readback, saved training-forward state and hand-written raster/projection
VJPs. P4-F07 through P4-F10 own differentiable rendering and gradient checks.
The render API consumes domain data and submits work through
`spirula-gpu`.

### `spirula-train`

Training session state, step scheduling, loss computation, optimizer state,
densification, evaluation, stop/save/resume, and progress reporting. P5-F08
resolves compatible config into a capability-validated executable `TrainPlan`;
legacy field acceptance does not imply implemented training behavior. It calls
render and GPU services through explicit interfaces and never knows about
Slint widgets.

### `spirula-application`

Unique logical session owner: command admission, Stop/Shutdown control, cancel
token, recoverable control history, snapshots/cursors, bounded notify fanout,
a fake probe worker (P1-F07), and optional rotated disk diagnostics (P1-F09).
State mutations are serialized through one owner mutex (callers and the
mailbox loop); UI/CLI must not duplicate the business state machine. Public
deps today: `spirula-core` only. Bindings:
[phase-1/application.md](phase-1/application.md),
[phase-1/cli.md](phase-1/cli.md).

### `spirula-ui`

Slint components, screen state, commands, validation feedback, and event
subscriptions. UI code presents application state; it does not parse datasets,
create GPU pipelines, or implement training rules.

### `spirula-cli`

Argument parsing, presentation, and SIGINT. CLI commands call the same
application/session services as the desktop UI (P1-F09). Allowed workspace
deps are `spirula-core` and `spirula-application` only (no direct
IO/train/render/GPU). Bindings: [phase-1/cli.md](phase-1/cli.md),
[phase-1/workspace-checks.md](phase-1/workspace-checks.md).

## GPU strategy

The portability baseline is `wgpu` with WGSL shaders. P2-F01 pins `wgpu`
30.0.1 in `spirula-gpu`, enumerates adapters, and negotiates required/optional
features and limits (`ResolvedLimits` keeps required floors). P2-F02 adds a
session-owned `GpuContext` (instance/adapter/device/queue, `DeviceHandle` with
context id/generation, explicit close/`reopen`, lost latch + owner poll; no
global singleton, no auto-rebuild). Application does not yet own the context;
device open does not imply compute/train/UI readiness. Bindings:
[phase-2/gpu-adapter.md](phase-2/gpu-adapter.md),
[phase-2/device-context.md](phase-2/device-context.md),
[phase-2/gpu-resources.md](phase-2/gpu-resources.md). P2-F03 adds narrow
buffer/RGBA8 texture staging upload/readback with explicit submit/wait/map and
scoped accounted-byte budgets — not a general wgpu mirror. P2-F04 (已完成)
adds session-owned WGSL compute pipeline key/cache and a minimal affine-u32
proof on top of P2-F03 staging (layout epochs v1/v2 same ABI for key
invalidation only). P2-F05 (已完成) maps wgpu error scopes / submit / lost to
stable `SP-GPU-*` with a GPU-local bounded diagnostic ring and latched
post-submit wait failures (not application session history; P6-F01). Surface
smoke is P2-F06 (已完成): headless affine compute smoke with Fail reclaim, plus
an honest surface **unsupported** gate that will not forge a window
(`spirula-gpu-smoke`; present not verified). Packaging is P2-F07 (当前).
Application does not own GPU resources yet; diagnose/smoke are not
train/preview/UI readiness. Real host OOM remains unsupported; the 60 s stall
watchdog stays deferred. Bindings:
[phase-2/gpu-pipelines.md](phase-2/gpu-pipelines.md),
[phase-2/gpu-diagnostics.md](phase-2/gpu-diagnostics.md),
[phase-2/gpu-smoke.md](phase-2/gpu-smoke.md).
Each GPU feature has a CPU reference or deterministic fixture where practical,
followed by a GPU tolerance test.

The rewrite should avoid translating the entire CUDA/Vulkan implementation in a
single step. Kernels are ported by family: data upload, projection, tile/bin
construction, sorting, rasterization, backward pass, optimization, and
densification. Shader layouts and numeric precision are recorded as contracts.

## Runtime model

The application owns one session state machine. Commands enter through a
bounded channel, workers publish typed progress/events, and cancellation is
cooperative. The UI observes events and submits commands. The CLI renders the
same events as text. Control history, snapshot cursors and bounded
notifications isolate slow consumers; source sequence gaps and resync are
explicit. A cancellation mailbox remains responsive under full queues. GPU
work is isolated behind a worker/context so device loss or a pipeline error
becomes an observable session failure. Resource accounting and per-stage
measurements are defined in P1-F08
([docs/phase-1/performance.md](phase-1/performance.md)) and continue in each
slice, before P9 aggregation.

## Frozen runtime contracts

Phase 0 freezes the contracts every later crate implements: typed errors and
stable codes, command/event envelopes, the session state machine and
cancellation, the GPU resource/shader boundary, and format versioning. They are
recorded in [docs/phase-0/runtime-contracts.md](phase-0/runtime-contracts.md).
P1-F01 implements that error catalog in `spirula-core` (`ErrorCode`,
`DomainError`, context keys, CLI text and exit codes). P1-F02 implements the
training-config document in the same crate. P1-F05 implements atomic publish,
orphan recovery, the temp harness, and config filesystem save/migration in
`spirula-io` ([docs/phase-1/atomic.md](phase-1/atomic.md)); checkpoint bundle
markers stay in P5-F05. P1-F03 implements the cooperative cancel token and
progress/terminal payloads in `spirula-core`; the application owner is
`spirula-application` (P1-F07, [docs/phase-1/application.md](phase-1/application.md)),
training scheduling stays in P5-F01, and full integration in P6-F01. The
primitive contract is [docs/phase-1/cancellation.md](phase-1/cancellation.md).
P1-F04 adds the CPU math, pinhole projection, similarity, and splat value
checks in [docs/phase-1/geometry.md](phase-1/geometry.md). Fisheye and other
lenses stay in P3-F01; scene files stay in P3-F05; splat files stay in P3-F06.
P1-F06 fixes the workspace verification entry, lint inheritance, architecture
path-edge policy, and CLI entry-adapter rule in
[docs/phase-1/workspace-checks.md](phase-1/workspace-checks.md). P1-F08 adds
versioned performance reports (accounted bytes, phase budgets, provenance
hashes, explicit unavailable GPU/OS/timestamp fields) in `spirula-core` /
`spirula-io` ([docs/phase-1/performance.md](phase-1/performance.md)). A crate does
not define its own error string, event shape, or schema version; it implements
the catalog and envelope in that document.

## Compatibility model

The C++ reference checkout (`SPIRULA_REFERENCE_REPO`, default
`../spirula-studio`) is the behavior reference. Existing dataset formats, camera conventions, checkpoint metadata,
and exported assets are preserved through adapters and golden fixtures. Any
intentional format change requires a versioned migration and a rollback path.
The pinned commit, toolchain and build/test entry points are recorded in
[docs/phase-0/reference-baseline.md](phase-0/reference-baseline.md).

## Testing model

Every feature must pass the narrowest useful tests first, then the complete
workspace checks. Important categories are pure unit tests, parser and format
fixtures, CPU/GPU parity, render image tolerances, checkpoint round trips,
headless application integration tests, and Slint UI behavior tests. The first
checksummed fixture set is described in
[docs/phase-0/fixtures.md](phase-0/fixtures.md).

## Execution and evidence gates

[Development gates](development-gates.md) define per-feature acceptance for
forward/backward gradients, executable capabilities, resource budgets,
Slint presentation copies, full-state resume, early CLI packages and external
tool provenance. [OOOSplat / Brush review](research/ooosplat-brush-review.md)
records the pinned sources and accepted lessons. The stack remains Rust /
Slint / wgpu / WGSL; Brush is a development reference, not a runtime engine.
The C++ checkout remains the compatibility authority.

P6-F07 is an early presentation spike after Phase 2 packaging, measuring
renderer/backend, texture/thread ownership, readback fallback and bytes copied
per frame. P2-F07 (未批准) packages the minimal CLI + GPU diagnose/smoke
binaries via `spirula-xtask` without adding a CLI→GPU dependency edge; see
[phase-2/cli-package.md](phase-2/cli-package.md). P4-F06/P5-F07 add real
workflow samples; P9-F04 covers signed multi-platform installers. P5-F05 saves
full optimizer/scheduler/RNG/sampler/refinement state through versioned
immutable checkpoint generations; P5-F07 verifies new-process resume. Phase 2
exit stays open while surface present remains unverified.
