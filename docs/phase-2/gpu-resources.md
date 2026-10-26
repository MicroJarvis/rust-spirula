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
`SP-INP-001` / `SP-INP-003` / `SP-GPU-004`, not panic. Buffer copy
`offset`/`size` and mapped-at-creation staging sizes must respect
`COPY_BUFFER_ALIGNMENT` (4). Staging buffer creates (upload/readback, including
padded texture staging) are checked against the enabled
`max_buffer_size` before reserve/wgpu — a valid 1×1 RGBA8 texture can still
require a 256-byte staging that exceeds a low buffer limit.

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
- **Preflight** before allocate. **Unsubmitted** failure (create/encode before
  `submit`, or abandoning pending work on context teardown) may roll back the
  reserve. Clone does not `allocate` again.
- **Encoded pending (before `submit`)**: upload/readback that has been encoded
  but not submitted still holds real GPU resources. Dropping the last buffer/
  texture handle must **not** free accounted bytes while `pending_hold > 0`.
  Ownership runs encode → submit → complete (same for textures).
- **After `submit`**, staging and resource bytes stay accounted while GPU may
  still reference them (`inflight`). A `wait` **timeout** is only the caller’s
  deadline: it must **not** free staging, drop tickets, or decrement budget.
  Ownership and accounting remain until a later successful wait/reap, or until
  safe device teardown (`close` / lost / reopen invalidate). Destroy of a
  resource with pending or in-flight work waits for completion (or teardown)
  before free.
- **`ReadbackTicket`**: Arc + RAII. `map_readback(&ticket)` borrows so not-ready
  / deadline `Err` never consumes the only reclaim path. Drop of the last clone
  abandons staging (free if pending or GPU-ready; if submitted incomplete, mark
  abandon-on-complete so wait reaps). Successful map marks consumed.

### Submit / wait / map

- Ops that encode GPU work leave commands pending; `submit` returns
  `SubmissionHandle` and does **not** imply a device-wide wait.
- `wait(handle, timeout)` on timeout returns typed `SP-GPU-003` **without**
  releasing in-flight staging/resources; the same handle may be waited again
  while still incomplete. After a **successful** wait the submission record is
  reaped (no unbounded ID-vector retention). A later wait on that token returns
  `SP-INP-001` (unknown submission) — not a retained history Ok.
- Device loss delivered by wait’s own `device.poll` (e.g. Destroyed callback) is
  observed **after** poll and **before** reporting success → `SP-GPU-002`.
  Ledger is not held across poll (avoids deadlock with lost invalidate).
- Readback: `map` only after wait has marked staging ready (GPU copy finished).
  Map deadline (inject or real poll timeout) **returns without waiting** on a
  pending callback: `unmap` cancels in-flight `map_async` (wgpu Waiting→Idle /
  MapAborted), staging stays accounted for retry. Inject issues `map_async`
  then takes that cancel path (not a pre-map Err). Do not busy-drain the
  callback after the caller deadline — that would hold `&mut GpuContext`
  indefinitely. Completed map, ticket Drop/abandon, teardown, or definitive
  post-GPU map failure frees the reserve.
- Device limits: `create_texture_2d` checks actual `max_texture_dimension_2d`
  before wgpu; staging paths check `max_buffer_size`. Create/staging paths use
  a minimal validation/OOM error scope so illegal sizes return typed errors with
  reserve rollback — not process abort. Full diagnostic event fan-out stays
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
2. Illegal range/usage/size/alignment/budget/overflow / oversize texture
   dimension / staging over `max_buffer_size` → typed errors; no wgpu panic /
   process abort; reserve/current unchanged on rejection.
3. Cross-context / close / reopen reject old handles and submissions.
4. Submit then drop resource; wait still succeeds; accounted current → 0;
   drop after encode-before-submit retains accounting until wait; repeated
   cycles / ticket abandon / early-map Err paths do not leak budget; completed
   submission metadata is reaped (retained count stays bounded); peak source
   recorded.
5. Post-submit wait timeout retains **this submission’s** staging/resources and
   allows a later wait to complete; unrelated handle/ticket Drop during the
   unlocked poll window may change total current and must not panic. Wait that
   observes host loss during its poll returns `SP-GPU-002`. Map deadline
   cancels via `unmap` and retries on the same ticket.
6. Diagnose prints device/backend, accounted current/peak/budget, phase
   timings for upload/wait/readback; unavailable device → precise reason.

## Non-claims

- No general-purpose wgpu resource API / all formats/usages.
- No compute shaders, pipelines, or TrainPlan execution.
- No claim of measured device-wide VRAM or OS process peak.
- No application/CLI/UI GPU command beyond diagnose.
- Full diagnostic events remain P2-F05.
