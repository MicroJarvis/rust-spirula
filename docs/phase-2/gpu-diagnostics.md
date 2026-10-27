# P2-F05: GPU errors and diagnostic events

Status: **已完成** (code tip `31b6087695fcf54322bca822b2de36f1f906b6fe`).
Owner: `spirula-gpu`. Builds on [device-context.md](device-context.md),
[gpu-resources.md](gpu-resources.md), [gpu-pipelines.md](gpu-pipelines.md).
Headless smoke is P2-F06 (已完成; see [gpu-smoke.md](gpu-smoke.md)); packaging
is P2-F07 (已完成). Application session wiring of these events stays P6-F01;
plan current is **P3-F01**; surface present owned by P2-F08 (未开始).
Phase 2 exit remains open.

## Reference behavior (C++)

Pinned: `9a24f09927944a795e7ba1bd8243a625bf419479`.

| C++ surface | Behavior recorded | Rust binding in this slice |
|---|---|---|
| `Context::set_error` / sticky `last_error` | Submit/wait failures sticky; training thread throws if unread (`TrainerCore.cpp:1633-1636`) | Typed `DomainError` with stable `SP-GPU-*`; no silent continue after mapped failure |
| `vkQueueSubmit` / `vkWaitSemaphores` fail | `set_error` + false/0 | `SP-GPU-003` on submit/wait failure; diagnostic event emitted |
| `GpuStallWatch` ~60 s no progress | Treat as device lost (`VulkanContext.cpp:694-742`) | Stall watchdog remains optional/later; host Destroyed callback + synthetic inject → `SP-GPU-002` + Lost status |
| `VK_ERROR_DEVICE_LOST` / driver reset text | Sticky lost | Context `Lost`; old generation handles rejected; one lost notice + diagnostic |
| BackendRuntime allocation fail | Named OOM | `SP-GPU-004` with requested/current/budget/label when known |
| Process-global sticky error | Cleared by `last_error()` read | **Deviation**: session-owned context diagnostics ring (no process singleton); close/reopen starts a new generation — events do not cross generations |

## Binding decisions

### Error scope → stable codes

| Source | Code | Notes |
|---|---|---|
| No eligible adapter / open select | `SP-GPU-001` | Unchanged (P2-F01) |
| Device lost / destroy callback / inject lost | `SP-GPU-002` | Status `Lost`; handles rejected; one diagnostic |
| Submit / wait / scoped validation during submit path | `SP-GPU-003` | Retryable per catalog |
| OOM / allocation fail (create/upload/readback/dispatch) | `SP-GPU-004` | Attach operation + spirula label + requested/current/budget; never remap OOM → input |
| Shader / pipeline compile | `SP-GPU-005` | Unchanged (P2-F04); emits diagnostic on scoped failure |
| Resource-path scoped validation (host probe) | `SP-INP-003` | Same F03 mapping; diagnostic emitted (not silent) |

Public GPU failures carry `operation` (`GpuOperation` → `ContextKey::Command`),
spirula `ResourceLabel` (wins over P1-F08 `accounted_work_buffer` placeholder),
and budget fields when applicable. Upload/readback/map allocation and scoped
failures emit diagnostics; on failure only the unsubmitted reserve is rolled
back (F03 retry/lifetime unchanged).

### Post-submit scope observation

`queue.submit` may accept work before error-scope pop. When scopes (or
`inject_next_post_submit_scope_error`) report after accept:

- submission record stays; **`Ok(SubmissionHandle)` is returned** so the caller
  can `wait`/reap;
- enriched typed failure (`SP-GPU-003` / `SP-GPU-004`) is **latched on the
  `SubmissionEntry`** (survives diagnostic ring eviction);
- a typed diagnostic is also emitted (not silent);
- `wait` first completes safe GPU sync and reclaims inflight/staging, then
  returns that canonical `Err` (failed readbacks are **not** marked
  `mapped_ready` / must not succeed on `map_readback`);
- wait timeout keeps the latch for a later wait; if poll delivers DeviceLost,
  wait returns `SP-GPU-002` (resources only via safe teardown/reap);
- inflight bytes are **not** freed until wait/teardown (no premature free).

Pre-mutation `inject_next_submit_error` still returns `Err` with no handle
(work never submitted).

### Diagnostic events (GPU-local only)

Events live on `GpuContext` — **not** the application control history (P6-F01).

```text
GpuDiagnosticEvent {
  seq, context_id, generation, kind, operation, label?, code?, message,
  requested_bytes?, current_bytes?, budget_bytes?, truncated
}
```

Ring policy: default capacity **64**; overflow **drop oldest** +
`dropped_oldest`; UTF-8 per-field + soft payload caps mark `truncated`;
`poll_diagnostics` / `filter_diagnostics` are pull copies; close/reopen
clears so events never cross generations. Ring eviction must not erase a
latched post-submit wait failure.

### Probes

- Synthetic: `inject_lost`, `inject_next_submit_error`, `inject_next_scope_error`,
  `inject_next_post_submit_scope_error`, wait-timeout retain, map-after-lost.
- Host: `host_probe_validation_scope`; destroy/lost; upload-budget metadata;
  post-submit Validation/OOM → waitable handle + typed wait `Err` + reclaim.
- Real host OOM: **unsupported** — not claimed (source-scheduled inject only).

## Acceptance (met)

1. Scoped validation/OOM/submit/wait/lost map to stable codes with operation +
   label (+ budget) — including upload/readback/map paths.
2. Lost marks context Lost; prior handles reject; one lost notice + diagnostic.
3. Diagnostic ring: ordered, bounded, drop-oldest observable, generation-isolated,
   payload caps + `truncated`.
4. Post-submit scope: `Ok(handle)` then wait returns latched typed failure after
   reclaim; ring cover does not hide it; failed readback not map-success.
5. Dispatch/create OOM keeps `SP-GPU-004` (not remapped to input) with bytes.
6. No application session subscription; no F06 surface.

## Tested evidence (approved tip)

Code tip: `31b6087695fcf54322bca822b2de36f1f906b6fe`. Independent review READY:
`/tmp/spirula-p2-f05-review/REVIEW-final-31b6087.md`. Commands below are the
reproducible checks; machine `/tmp` logs are sample runs, not the sole evidence.

| Check | Command | Sample result |
|---|---|---|
| Synthetic diagnostics | `cargo test -p spirula-gpu --test diagnostics_lifecycle_synthetic -- --nocapture` | **15/15** (sample: `/tmp/spirula-cursor-p2-f05-focused-diag.log`) |
| Host validation/lost/budget/post-submit | `cargo test -p spirula-gpu --test host_diagnostics -- --nocapture` | **6/6** on Apple M5 Max / Metal (same sample log) |
| Fresh full gate | `cargo fmt/check/test/clippy` workspace gate | ok (sample: `/tmp/spirula-cursor-p2-f05-verify.log`) |
| Independent probes | exact `git archive` + host Validation/OOM wait latch | READY; evidence under `/tmp/spirula-p2-f05-review/final31/` |

## Non-claims

- Not F06 surface smoke; not app/train/UI / P6-F01 fan-out.
- Not real hardware OOM; not 60 s stall watchdog.
- Not a general logging framework or P9 aggregation.
- Phase 2 exit is **not** closed; P2-F06 is current.
