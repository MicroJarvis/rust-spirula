# P2-F02: device, queue, and context lifecycle

Status: **当前 / 待审**. Owner: `spirula-gpu`. Builds on
[gpu-adapter.md](gpu-adapter.md) (P2-F01). Buffer/texture/readback stay
P2-F03; shaders/pipelines/compute stay P2-F04; full error scopes / diagnostic
event fan-out stay P2-F05. Phase 2 exit remains open.

## Reference behavior

- C++ `VulkanContext.cpp` ~389 init, ~603 device create, ~619/671 destroy;
  `EngineState.cpp` ~17–25 reset frees scene GPU pool while keeping the
  device. Rust **deviates**: no process-global device singleton — a
  session-owned [`GpuContext`](../../crates/spirula-gpu) replaces it (runtime
  contracts §6.1).
- **Scene reset** (`reset_scene`): drop scene GPU resources, keep device/queue
  (no resources in this slice → validated no-op while open).
- **Device close** (`close` / `Drop`): tear down device/queue/instance binding;
  all prior `DeviceHandle` values become unusable.

## Binding decisions

### Live select → device (same enumerate snapshot)

`GpuContext::open` creates a **new** wgpu `Instance`, calls
`enumerate_on_instance` once, and keeps **paired** report + live adapter
vectors from that snapshot. P2-F01 `select_auto` runs on the reports; the
chosen adapter is retrieved by the **snapshot-local index** into the paired
live vector (validated against the report at that slot, including
features/limits).

- Same instance + same enumerate call: index is the pairing key.
- Name/backend/vendor/device alone are **not** unique (two identical cards can
  collide); never re-resolve by scanning those fields for “first match”.
- Indices from a previous instance, a previous enumerate, or any external
  caller are rejected — no global UUID registry.

### Negotiation applied to `DeviceDescriptor`

P2-F01 `NegotiatedPlan` is not diagnostic-only:

- `required_features`: wgpu feature bits for capabilities whose plan is
  `AvailableDeclared` (only bits the selected adapter actually has among the
  requested set).
- `required_limits`: start from the live adapter’s limits, then set the three
  negotiated fields to `ResolvedLimits.*.effective` (already ≥ required floors
  and ≤ adapter). Other limit fields remain the adapter’s values so the request
  stays within device capability.

Enabled features/limits after create are read back from the device and stored
on the context for diagnose/tests.

### Narrow public surface

| Type / API | Role |
|---|---|
| `ContextId`, `ContextGeneration` | Identity; generation bumps on owner `reopen` |
| `DeviceHandle` | Opaque `{context_id, generation}` — no wgpu types |
| `GpuContext` | Owns instance/device/queue while open |
| `open` / `close` / `Drop` | Create and explicit teardown |
| `reset_scene` | Scene-resource clear; keep device (no-op body until P2-F03) |
| `reopen` | **Owner-initiated** recreate; same `ContextId`, new generation.
  Any failure path (`Select` / `DeviceCreate` / host) sets
  `CreateFailed`, keeps the prior report, sets `report.status` to match,
  and forces `queue_present=false`. Old handles stay rejected. |
| `device_handle` / `validate_handle` | Issue / check handles |
| `status` / `take_lost_notices` | Observable open/closed/lost; lost notice for owner |
| `inject_lost` | Synthetic lost only (`LostSource::SyntheticInject`) |

`validate_handle` failures:

- wrong `context_id` → `SP-INP-001` (cross-context)
- stale `generation` or closed → `SP-GPU-002`
- lost → `SP-GPU-002`

Device **create** failure → `SP-GPU-004` (`AllocationFailed`) at stage
`DeviceCreate`. Adapter select/empty → `SP-GPU-001`.

### Lost callback vs application owner

wgpu `set_device_lost_callback` marks the context `Lost`, drops device/queue
handles inside `spirula-gpu`, and records a `LostNotice` with
`LostSource::HostCallback`. The lost latch stores reason in a single mutex
(`Option<String>`) so take cannot observe “triggered” with an empty/missing
reason; each notice is consumed once. A successful `reopen` installs a fresh
latch — prior-generation callbacks do not affect the new generation. It
**must not** mutate `spirula-application` `SessionState`. Automatic device
recreation is **forbidden** inside GPU. The future session worker (not wired
in this slice) must call `reopen` explicitly after observing lost/closed.

### Owner call boundary (this slice)

| Wired now | Not wired |
|---|---|
| `spirula-gpu` library API + `spirula-gpu-diagnose --device` | `spirula-application` session ownership |
| Synthetic test doubles for fault injection | CLI/UI GPU commands |
| Host headless open → observe → close | Error scopes / submit diagnostics (P2-F05) |

### Dependencies

Still: `spirula-gpu` → `spirula-core` + `wgpu` + `pollster`. No CLI/UI/core
wgpu edges. No new workspace path edges.

## Tests and host evidence

- **Synthetic** (`tests/context_lifecycle_synthetic.rs`): create failure,
  inject lost, close idempotence, cross-context and stale-generation rejection,
  owner `reopen_synthetic` bumps generation and old handles stay dead.
- **Snapshot pairing** (`enumerate` unit tests): two adapters with identical
  name/backend/vendor/device but different required features — `select_auto`
  picks the second; resolve returns snapshot index 1 (not first name match).
  External/stale indices rejected.
- **Host** (`tests/host_device_context.rs`): always run open→status/features/
  limits→close (or precise `SP-GPU-001` / `SP-GPU-004`). Distinguish real lost
  (`HostCallback`) from synthetic inject in test names/API. Enumeration/device
  create ≠ compute.
- Keep P2-F01 adapter unit + host enumerate regressions.
- Diagnose: `spirula-gpu-diagnose --device`.

## Non-claims

- No buffers, textures, pipelines, submit, readback, or WGSL kernels.
- No application/CLI/UI integration; no TrainPlan execution.
- No implicit GPU auto-rebuild; no second session state machine in GPU.
- No claim that device creation implies train/preview/UI readiness.
- Full diagnostic event / error-scope fan-out remains P2-F05.
