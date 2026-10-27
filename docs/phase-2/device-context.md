# P2-F02: device, queue, and context lifecycle

Status: **已完成** (code tip `75d1bd48efcc8ba6451af368ce60387e6cead6b9`).
Owner: `spirula-gpu`. Builds on [gpu-adapter.md](gpu-adapter.md) (P2-F01).
Buffer/texture/readback are P2-F03; shaders/pipelines/compute are P2-F04;
GPU error scopes / diagnostic events are P2-F05 (已完成); headless smoke
P2-F06 (已完成); packaging P2-F07 (当前). Phase 2 exit remains open.

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
live vector (crate-internal helpers validate the report at that slot,
including features/limits).

- Same instance + same enumerate call: index is the pairing key.
- Name/backend/vendor/device alone are **not** unique (two identical cards can
  collide); never re-resolve by scanning those fields for “first match”.
- Indices from a previous instance, a previous enumerate, or any external
  caller are out of contract for these helpers — no global UUID registry.
- Slot validation rejects many mismatched selections; it does **not** claim to
  detect every forged payload that is value-identical to some other snapshot.

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
| `reopen` | Owner-initiated recreate; same `ContextId`, new generation; any failure sets `CreateFailed`, keeps prior report, matches `report.status`, `queue_present=false`; old handles rejected |
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

wgpu `set_device_lost_callback` **publishes** a reason into a single-mutex
latch (`Option<String>` — trigger and payload together). The owner (or test)
must **poll** (`poll_host_lost` / `status` / handle APIs) to move the context
to `Lost`, drop device/queue, and record a `LostNotice` with
`LostSource::HostCallback`. The callback does not mutate
`spirula-application` `SessionState`. Automatic device recreation is
**forbidden** inside GPU.

wgpu may deliver an **empty** reason string; Spirula preserves it as-is and
consumes it once. This does **not** promise a non-empty reason. A successful
`reopen` installs a fresh latch — prior-generation callbacks do not affect the
new generation. The future session worker (not wired in this slice) must call
`reopen` explicitly after observing lost/closed.

### Owner call boundary (this slice)

| Wired now | Not wired |
|---|---|
| `spirula-gpu` library API + `spirula-gpu-diagnose --device` | `spirula-application` session ownership |
| Synthetic test doubles for fault injection | CLI/UI GPU commands |
| Host headless open → observe → close / owner reopen | Surface smoke (P2-F06) |

Honest scope: the library API is the owner boundary for this feature.
Application does **not** yet own or reopen the GPU context. No compute,
readback, training, or UI path is claimed ready because a device opened.

### Dependencies

Still: `spirula-gpu` → `spirula-core` + `wgpu` + `pollster`. No CLI/UI/core
wgpu edges. No new workspace path edges.

## Tests and host evidence

- **Unit** (adapter + enumerate pairing + LostWatch latch): including twin-name
  snapshot index pairing and empty-reason-preserving latch consumption.
- **Synthetic** (`tests/context_lifecycle_synthetic.rs`): create failure,
  inject lost, close idempotence, cross-context / stale generation, owner
  reopen success, DeviceCreate and **Select** reopen failures with report
  synced to `CreateFailed` / `queue_present=false`.
- **Host** (`tests/host_device_context.rs` + independent owner/callback probes):
  open→features/limits readback→close; cross-context; owner reopen; failed
  reopen recovery observability; real `device.destroy` + poll across
  generations (old latch isolated; current gen Lost once).
- Keep P2-F01 adapter unit + host enumerate regressions.
- Diagnose: `cargo run -p spirula-gpu --bin spirula-gpu-diagnose -- --device`.

### Host record (approval tip)

- Code tip: `75d1bd48efcc8ba6451af368ce60387e6cead6b9`.
- Independent full gate: `/tmp/spirula-p2-f02-75d1bd4-independent-verify.log`
  (8/8).
- Owner/cross-context/reopen/failure + non-default features/limits readback:
  `/tmp/spirula-p2-f02-host-owner-probe/evidence-75d1bd4.txt`.
- Actual host Destroyed callback isolation:
  `/tmp/spirula-p2-f02-review/final_actual_callback_evidence-75d1bd4.txt`.
- Sample diagnose host (this workstation): wgpu `30.0.1`, Metal,
  `Apple M5 Max`, integrated-gpu; negotiated features read back enabled;
  open/close only — **not** compute/train/UI.

### Tested vs not tested

| Tested | Not tested / not claimed |
|---|---|
| Headless create/close/reopen; handle validation; lost latch + poll | Buffer/texture/readback (P2-F03) |
| Negotiation applied to device request; enabled features/limits readback | Pipelines / WGSL compute (P2-F04) |
| Synthetic + real Destroyed callback isolation across generations | Surface smoke (P2-F06) |
| `spirula-gpu-diagnose --device` | Application/CLI/UI session GPU ownership |
| | Training, preview, or “device open ⇒ product ready” |

## Non-claims

- No buffers, textures, pipelines, submit, readback, or WGSL kernels (later P2).
- No application/CLI/UI integration; no TrainPlan execution.
- No implicit GPU auto-rebuild; no second session state machine in GPU.
- No claim that device creation implies train/preview/UI readiness.
