# P2-F01: adapter selection and capability negotiation

Status: **当前 / 待审** (not merged). Owner: `spirula-gpu`. Device/queue
lifecycle remains P2-F02; buffer/texture/readback remains P2-F03. The shared
CLI session does **not** gain a business GPU command in this slice.

## Reference behavior

- C++ `backend/vulkan/VulkanContext.h` ~44–46: prefer discrete, then
  integrated, then other; VRAM as a tie-break when known.
- C++ CLI `cli/main.cpp` ~287–349: device table / index / name / uuid / auto
  selection. wgpu has no portable VRAM or stable UUID query — those fields are
  recorded as **unavailable** with an explicit reason; selection never invents
  numbers. Auto selection filters **required** features/limits first, then
  applies a deterministic ranking (class → optional VRAM when available →
  backend preference → name → enumerate index).

## Binding decisions

### wgpu pin

- Public dependency: `wgpu = "30.0.1"` (MSRV 1.87; workspace `rust-version`
  1.99). Locked in `Cargo.lock` after a successful host resolve.
- Blocking enumerate uses `pollster` inside `spirula-gpu` only.
- No ash / CUDA / direct Vulkan runtime path. C++ remains a behavior reference.

### Public surface (narrow)

`spirula-gpu` exposes value types for later TrainPlan / P2-F02 consumers:

- `BackendKind`, `AdapterClass`, `AdapterIdentity`, `AdapterCapabilities`
- `CapabilityId` (`Subgroup`, `ShaderF16`, `TimestampQuery`, `Float32AtomicAdd`)
- `CapabilityRequest`, `LimitFloor` (same three fields for required min and
  optional desire)
- `NegotiatedPlan` (feature plans + `ResolvedLimits` / `ResolvedLimit`)
- `select_auto`, `negotiate`, pure ranking helpers (synthetic-testable)
- `enumerate_host_adapters` (real wgpu; returns zero or more reports)

wgpu types stay inside `spirula-gpu` implementation modules. This is **not** a
general-purpose wgpu mirror and does not create devices, queues, or resources.

### Required vs optional

- **Required** feature / `required_min_limits` miss → adapter is ineligible. If
  none remain → `SP-GPU-001` (`NoAdapter`) with context naming the missing
  item(s). No silent downgrade of a required demand.
- **Optional** feature miss → keep the adapter; negotiate an explicit feature
  fallback. Optional absence must not reject an otherwise eligible adapter.
- **Optional** `optional_desired_limits` over device capacity → keep the
  adapter; `ResolvedLimit` sets `desired_met=false` and `effective` to the
  adapter capacity (still ≥ any required floor). Never claim the requested
  desire was satisfied.
- Required floors outrank optional desires in **both** eligibility and the
  returned plan: failing a required floor rejects; on success,
  `effective >= required` and `effective <= adapter`. An optional desire
  below the required floor must not lower `effective` (it stays at least the
  required floor); `desired_met` still reports whether the adapter meets the
  optional desire alone. When desire ≥ required and the adapter meets desire,
  `effective` equals the desire.
- Detecting a wgpu feature bit means **capability declared**, not that a
  Spirula kernel already implements it. Real compute stays later P2/P4 slices.

### Baseline / fallback plans

| capability | when present | when absent (optional) |
|---|---|---|
| Subgroup | `AvailableDeclared` | `FallbackNonSubgroup` (host-serial / non-subgroup path) |
| Shader f16 | `AvailableDeclared` | `FallbackF32` |
| Timestamp query | `AvailableDeclared` | `FallbackHostTimer` |
| Float32 atomic add (grad accum) | `AvailableDeclared` | default `FallbackCas`; request may choose `FallbackStagedReduce` |

### Diagnostic entry

Binary `spirula-gpu-diagnose` (crate `spirula-gpu`):

- Enumerates adapters; prints backend/name/class/driver and limit/feature
  summaries; marks VRAM/uuid unavailable honestly.
- Runs auto-select with a documented default request (no required extras;
  all four capabilities optional).
- Exit `0` on selection; exit `1` on `SP-GPU-001`; usage → exit `2`.
- Not wired into `spirula-cli` / application session.

### Tests and host evidence

- Pure/synthetic matrix: empty set, required miss, optional feature fallback,
  required limit floors, optional limit clamp (`desired_met=false`), required
  outranks optional desire, deterministic ranking, no silent required
  downgrade.
- Host headless enumerate always runs: records actual adapters on this machine
  **or** an empty list with a precise no-adapter reason. An empty machine is
  not recorded as a successful “measured GPU” result.

## Non-claims

- No device/queue/context (P2-F02).
- No buffers, textures, shaders, or submit (P2-F03+).
- No CLI/session GPU commands; no Slint; no train/preview.
- No fabricated VRAM/uuid; no cross-platform CI matrix (P9-F01).
