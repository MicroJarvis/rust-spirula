# P2-F05: GPU errors and diagnostic events

Status: **已完成** (HOLD-MERGE pending independent review). Owner: `spirula-gpu`.
Builds on [device-context.md](device-context.md), [gpu-resources.md](gpu-resources.md),
[gpu-pipelines.md](gpu-pipelines.md). Surface smoke stays P2-F06; application
session wiring of these events stays P6-F01. Phase 2 exit remains open.

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
| OOM / allocation fail | `SP-GPU-004` | Attach requested/current/budget when known |
| Shader / pipeline compile | `SP-GPU-005` | Unchanged (P2-F04); emits diagnostic on scoped failure |
| Resource-path scoped validation (host probe) | `SP-INP-003` | Same F03 mapping; diagnostic emitted (not silent) |

Public GPU failures carry `operation` (`GpuOperation` → `ContextKey::Command`),
optional `ResourceLabel`, and budget fields when applicable. Pre-wgpu typed
rejects (alignment, alias, etc.) stay as today; budget preflight keeps the
P1-F08 `Resource=accounted_work_buffer` key while the spirula label is on the
diagnostic event.

### Diagnostic events (GPU-local only)

Events live on `GpuContext` — **not** the application control history (P6-F01).

```text
GpuDiagnosticEvent {
  seq,                  // strictly increasing within a live generation
  context_id,
  generation,
  kind,                 // Error | DeviceLost | Warning | Info
  operation,            // create|upload|dispatch|readback|submit|wait|destroy|open|…
  label,                // optional spirula:… resource label
  code,                 // optional SP-GPU-00x / SP-INP-003
  message,
  requested_bytes?, current_bytes?, budget_bytes?
}
```

Ring policy (explicit; phase-0 control-history style, GPU-scoped):

- Default capacity **64** events (`DEFAULT_DIAGNOSTIC_CAPACITY`; tests may shrink).
- On overflow: **drop oldest**, increment `dropped_oldest` (never silent-drop
  without a counter).
- `poll_diagnostics(since_seq)` returns retained events with `seq > since_seq`
  in order; does not clear the ring (read copy).
- `filter_diagnostics(since_seq, kind?, label?)` for observable subscription.
- `close` / `reopen` / generation bump **clears** the ring and resets `seq` so
  events never cross generations.
- Subscribers are pull-based cursors; slow readers observe gaps via
  `dropped_oldest` / missing seq — they do not block GPU work.

### Probes

- Synthetic: `inject_lost`, `inject_next_submit_error`, `inject_next_scope_error`.
- Host: `host_probe_validation_scope` (invalid WGSL under error scopes);
  `destroy_host_device_without_poll` + `wait`/`poll_host_lost` for Destroyed →
  lost mapping + event; `inject_next_submit_error` on a live host device.
- Real host OOM: **unsupported** — not claimed. Typed `SP-GPU-004` covered by
  budget preflight + synthetic `ScopeErrorInject::OutOfMemory`.

## Acceptance

1. Scoped validation/OOM/submit/wait/lost map to stable codes with operation +
   label (+ budget fields when applicable). ✅
2. Lost marks context Lost; prior handles reject; one lost notice + one
   device-lost diagnostic for a single inject/callback delivery. ✅
3. Diagnostic ring: ordered seq, bounded capacity, observable oldest-drop,
   generation-isolated; filterable by label / kind. ✅
4. Host evidence for validation-scope typed failure and destroy/lost → event;
   OOM hardware path honest **unsupported**. ✅
5. No application session subscription; no F06 surface; F03/F04 behavior
   preserved. ✅

## Evidence

| Check | Command / artifact |
|---|---|
| Synthetic diagnostics | `cargo test -p spirula-gpu --test diagnostics_lifecycle_synthetic` |
| Host validation + lost | `cargo test -p spirula-gpu --test host_diagnostics` |
| Focused GPU suite | `/tmp/spirula-codex-p2-f05-focused-gpu-tests.log` |
| Fresh full gate | `cargo run -p spirula-xtask -- verify` → `/tmp/spirula-codex-p2-f05-independent-verify.log` |

## Non-claims

- Not a general logging framework or P9 metrics aggregation.
- Not application/train/UI event fan-out (P6-F01).
- Not surface present smoke (P2-F06).
- Not a guaranteed 60 s stall watchdog implementation in this slice (reference
  noted; host Destroyed + inject cover loss mapping).
- Not real hardware OOM reproduction.
- Phase 2 exit is **not** closed; 74-ID plan keeps application/train/UI off this
  GPU-local diagnostic boundary.
