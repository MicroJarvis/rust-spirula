# P2-F03: buffer, texture, and readback

Status: **当前 / 待审**. Owner: `spirula-gpu`. Builds on
[device-context.md](device-context.md) (P2-F02) and P1-F08
[performance.md](../phase-1/performance.md) accounted-byte tracking.
Shaders/pipelines/compute stay P2-F04. Full GPU diagnostic event fan-out stays
P2-F05. Phase 2 exit remains open.

## Reference behavior (C++)

Pinned header: `src/backend/api/BackendRuntime.h` ~1–140 (+ memcpy/sync below).

| C++ surface | Behavior recorded | Rust binding in this slice |
|---|---|---|
| `device_malloc` / `device_free` | Free waits in-flight refs (cudaFree sync contract) | Drop of last handle defers free until outstanding submissions finish |
| `host_malloc_pinned` / staging ring | Host-visible staging for pageable copies | Per-op staging buffers; bytes counted in scoped tracker |
| `memcpy_sync` / `memcpy_async` | H2D/D2H/D2D with explicit kind | Staging + `copy_*` encoded; **submit** then **wait** (no silent global sync on every call) |
| `device_synchronize` / stream sync | Explicit fence | `wait(SubmissionHandle, timeout)` only |
| `MemoryUsage` process/used/total | Best-effort VRAM telemetry | **Not claimed** — counter source is accounted scoped bytes only (P1-F08) |
| `throw_out_of_memory` | Named size + device state | `SP-GPU-004` with requested/current/budget/label via preflight |

Rust continues the P2-F02 deviation: session-owned `GpuContext`, not a process
singleton. Scene `reset_scene` still keeps the device; resources created here
are owned by the context generation.

## Binding decisions

### Narrow types (not a wgpu mirror)

| Type | Role |
|---|---|
| `ResourceLabel` | `spirula:<domain>:<kind>:<name>` (lowercase kebab); reject otherwise |
| `BufferHandle` / `TextureHandle` | Opaque `{context_id, generation, id}`; `Clone` shares one alloc (no double accounting) |
| `SubmissionHandle` | Opaque submit token; wait is explicit |
| `BufferUsage` / `TextureUsage` | Small enums for this slice (copy src/dst + map read/write as needed) |
| `TextureFormat::Rgba8Unorm` | Sole 2D format for measurable image round-trips |

Public APIs never return `wgpu::*`. Validation of size/usage/offset/range/
alignment happens in Rust **before** wgpu calls — bad params → typed
`SP-INP-001` / `SP-INP-003` / `SP-GPU-004`, not panic.

### Staging, pitch, and accounting

- Buffer upload/readback: staging buffer sized to the **logical** copy range;
  copy via encoder; staging counted until wait completes then freed.
- Texture2D RGBA8: logical bytes = `width * height * 4`. Row pitch =
  `align(width * 4, 256)` (`COPY_BYTES_PER_ROW_ALIGNMENT`). Padded staging =
  `bytes_per_row * height`. Readback strips padding before returning host
  bytes.
- Scoped resident tracker (reuse `AccountedBytesTracker` idea): device buffer/
  texture bytes + staging + in-flight staging. Source string:
  `accounted_scoped:gpu_context_resources` — **not** device-wide VRAM / OS peak.
- **Preflight** before allocate; on create/map/wait/submit failure or cancel,
  reserved bytes roll back. Clone does not `allocate` again. Destroy of a
  resource with in-flight work does not free accounting until wait completes.

### Submit / wait / map

- Ops that encode GPU work leave commands pending; `submit` returns
  `SubmissionHandle` and does **not** imply a device-wide wait.
- `wait(handle, timeout)` fails with typed error on timeout / lost / wrong
  generation (`SP-GPU-002` / `SP-GPU-003` as appropriate).
- Readback: after wait, `map` staging (or finish ticket) returns host bytes;
  map/wait failure rolls back staging reserve.
- Minimal error-scope around submit/map only to avoid panic; event fan-out is
  P2-F05.

### Lifetime vs context generation

Handles and submissions bind `context_id` + `generation`. Cross-context,
closed, lost, or post-`reopen` generations reject use. Dropping a buffer/
texture after submit but before wait must still allow wait+readback success;
accounting reaches zero after completion.

### Owner / diagnose boundary

| Wired | Not wired |
|---|---|
| `spirula-gpu` APIs + `spirula-gpu-diagnose --resources` | Application session ownership |
| Synthetic budget/handle fault injection | Compute/WGSL/pipelines (P2-F04) |
| Host headless buffer + non-256-aligned-width texture round-trip | Train/preview/UI |

## Acceptance

1. Real host: buffer pattern bytes upload→copy→readback equal; RGBA8 texture
   with width whose `width*4` is **not** multiple of 256 still round-trips
   after pitch strip.
2. Illegal range/usage/size/budget/overflow → typed errors; no wgpu panic.
3. Cross-context / close / reopen reject old handles and submissions.
4. Submit then drop resource; wait still succeeds; accounted current → 0;
   repeated cycles do not leak budget; peak source recorded.
5. Diagnose prints device/backend, accounted current/peak/budget, phase
   timings for upload/wait/readback; unavailable device → precise reason.

## Non-claims

- No general-purpose wgpu resource API / all formats/usages.
- No compute shaders, pipelines, or TrainPlan execution.
- No claim of measured device-wide VRAM or OS process peak.
- No application/CLI/UI GPU command beyond diagnose.
- Full diagnostic events remain P2-F05.
