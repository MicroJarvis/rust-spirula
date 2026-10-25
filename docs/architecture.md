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
spirula-cli ──────┼──> spirula-application (planned)
                  │          │
                  │          ├──> spirula-train ──> spirula-render ──> spirula-gpu
                  │          ├──> spirula-io ──────> spirula-core
                  │          └──> spirula-core
                  └──> spirula-core
```

The current skeleton keeps the future crate boundaries visible. An
`spirula-application` orchestration crate may be introduced when the first
end-to-end slice needs it; it must own sessions, commands, cancellation, and
progress events so the UI and CLI do not duplicate business logic.

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
diagnostics. `wgpu` types must stop at this boundary or at a deliberately
reviewed render interface.

### `spirula-render`

GPU-independent scene/render contracts plus GPU implementations for Gaussian
splat projection, sorting, rasterization, compositing, depth, and preview
readback. The render API consumes domain data and submits work through
`spirula-gpu`.

### `spirula-train`

Training session state, step scheduling, loss computation, optimizer state,
densification, evaluation, stop/save/resume, and progress reporting. It calls
render and GPU services through explicit interfaces and never knows about
Slint widgets.

### `spirula-ui`

Slint components, screen state, commands, validation feedback, and event
subscriptions. UI code presents application state; it does not parse datasets,
create GPU pipelines, or implement training rules.

### `spirula-cli`

Argument parsing and process entry point. CLI commands call the same
application/session services as the desktop UI.

## GPU strategy

The portability baseline is `wgpu` with WGSL shaders. The first GPU slice is a
device smoke test and a small render path. Each GPU feature has a CPU reference
or deterministic fixture where practical, followed by a GPU tolerance test.

The rewrite should avoid translating the entire CUDA/Vulkan implementation in a
single step. Kernels are ported by family: data upload, projection, tile/bin
construction, sorting, rasterization, backward pass, optimization, and
densification. Shader layouts and numeric precision are recorded as contracts.

## Runtime model

The application owns one session state machine. Commands enter through a
bounded channel, workers publish typed progress/events, and cancellation is
cooperative. The UI observes events and submits commands. The CLI renders the
same events as text. GPU work is isolated behind a worker/context so a device
loss or pipeline error becomes an observable session failure.

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
progress/terminal payloads in `spirula-core`; session orchestration stays in
P5-F01 and P6-F01. The primitive contract is
[docs/phase-1/cancellation.md](phase-1/cancellation.md). P1-F04 adds the CPU
math, pinhole projection, similarity, and splat value checks in
[docs/phase-1/geometry.md](phase-1/geometry.md). Fisheye and other lenses stay
in P3-F01; scene files stay in P3-F05; splat files stay in P3-F06. A crate does not define
its own error string, event shape, or schema version; it implements the
catalog and envelope in that document.

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

