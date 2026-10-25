# P1-F07: minimal headless application/session

`spirula-application` is the unique owner of session state, command admission,
control history, cancellation, and subscriber fanout. It depends only on
`spirula-core` in this slice: no `spirula-train`, no `spirula-gpu`, and no
filesystem I/O. Workers are fake probes that exercise lifecycle contracts.
Real load/train/preview attach later (P3/P4/P5/P6); this crate must not claim
training capability.

## Reference behavior

Pinned contracts: [runtime-contracts.md](../phase-0/runtime-contracts.md)
§§2–5, [development-gates.md](../development-gates.md) §§1/4/5,
[cancellation.md](cancellation.md). C++ `TrainerCore` / CLI stop paths inform
cancel and SIGINT timing; they do not dictate channel layouts.

## Binding decisions

These fill the frozen sketches. They do not add error codes.

### Ownership and dependencies

- One `ApplicationService` per process role under test is the **unique logical
  application owner**. There is a single session-state record; UI/CLI/workers
  must not keep a parallel business state machine. All mutations of
  `SessionState`, `seq`, active `CommandId`/`RunId`, history, and drop
  counters take the same owner `Mutex`. Callers of `submit` (including
  immediate `Pause`/`Resume`/`Reset`/`Stop`/`Shutdown`) and `Drop` may hold
  that lock on their threads; the dedicated owner loop holds it when applying
  worker mailboxes and finishing shutdown. That is a **serialized shared
  owner-state binding**, not “exactly one OS writer thread”. Cancel/Stop/
  Shutdown stay responsive because control admission is lock-bounded and
  independent of subscriber/progress backpressure. Each instance allocates a
  unique `SessionId`. Command ids and run ids use separate counters so
  accepted-command expiry is an O(1) watermark, not an unbounded set.
- Public crate deps: `spirula-core` only. Allowed future edges to
  `spirula-io`/`spirula-train` stay unused here.
- One dedicated owner thread drains worker mailboxes and drives shutdown
  completion. Rejected or admitted commands never spawn a thread. The fake
  worker is one long-lived thread; it publishes through a bounded
  worker→owner mailbox and never awaits a subscriber or log sink. Service
  `Drop` always joins owner and worker within the effective
  `teardown_deadline` (normalized to cover cancel observation); it never
  detaches immortal threads or emits canonical events after
  `ShutdownComplete`.

### Commands (P1-F07 surface)

Full payload names from the runtime contract stay reserved. This slice admits:

| Payload | Meaning |
|---|---|
| `ProbeLoad { label }` | Fake load: `Idle`/`Failed`/`Ready` → `Loading` → `Ready` |
| `ProbeRun { steps }` | Fake multi-step work: `Ready` → `Preparing` → `Running` (maps to contract `Training`) |
| `Pause` / `Resume` | Cooperative pause at the next fake step boundary |
| `Stop { save }` | Cancel active probe; `save=true` takes `Stopping`→`Saving`→checkpoint→`Ready` |
| `Shutdown` | Cancel, drain, emit `ShutdownComplete`, close notify |
| `Reset` | `Failed` → `Idle` and clears `last_outcome` |

`StartTraining` / real `LoadDataset` are not implemented. Submitting an
unsupported payload is impossible in the type system; CLI wiring of those
names is P1-F09+.

`SessionState::Running` is the Rust name for the contract `Training` state so
tests do not imply a real trainer. Display maps it as `training`.

### Admission and control

- Normal commands enter one bounded `command_queue`
  (`SessionLimits::command_queue_capacity`). A full queue rejects with
  `SP-SESS-003` without blocking the caller. There is no per-command task
  queue growth beyond that capacity.
- `Stop` / `Shutdown` also signal an independent control path: at most one
  pending `Stop` identity and one pending `Shutdown` identity
  (`Option<CommandId>` each), plus the shared `CancelToken`. They are not
  stored in an unbounded deque. A repeated `Stop`/`Shutdown` while one is
  already pending or draining is an accepted idempotent no-op (new id still
  accepted; each id gets its own terminal; only `shutdown_owner` emits
  `ShutdownComplete`).
- **Run vs control lifecycles are separate.** `active_command` / `RunId` /
  `last_outcome` track the mutating probe only. Every accepted
  `Pause` / `Resume` / `Stop` / `Shutdown` / coalesced no-op also receives
  exactly one canonical terminal for **its** `command_id` and must not
  overwrite the probe's `last_outcome`.
- Those control paths stay responsive when the work queue, progress slot, or
  notify queues are full.
- At most one mutating probe is in flight. A second mutating probe is
  `SP-SESS-003` unless it is coalesced `Stop`/`Shutdown`.
- Invalid-in-state commands return `CommandRejected` with `SP-SESS-001` and
  do not change state. Accepted commands return `CommandAccepted { id }`
  immediately; work completes on the owner/worker threads already described.

### Payload and retention bounds

Every admitted or retained variable payload has an explicit cap. Oversized
admitted strings are rejected with `SP-INP-003` (or `SP-INP-001` when empty).
Rejected command payloads are **shrunk before retention** (a 1 MiB reject
label is never stored verbatim). Worker→owner enqueue and owner retention
apply the same caps to load labels, checkpoint labels, error/context text,
and progress. Stable `ErrorCode` values survive oversized-view substitution.

`ApplicationService::start` applies `SessionLimits::normalized()` so zero
capacities, tiny byte floors, and `cancel_poll_interval > min(heartbeat,
250ms)` cannot silently violate policy; `limits()` returns the effective
values.

| Cap | Default | Applies to |
|---|---|---|
| `label_max_bytes` | 256 | `ProbeLoad` label; checkpoint/probe labels in events |
| `reason_max_bytes` | 256 | `StateChanged.reason` |
| `log_message_max_bytes` | 2 KiB | `Log.message`; in-memory log tail entries |
| `progress_max_bytes` | 4 KiB | serialized progress snapshot estimate |
| `error_message_max_bytes` | 1 KiB | retained/emitted error `message` |
| `context_value_max_bytes` | 256 | each retained context value |
| `error_event_max_bytes` | 2 KiB | whole retained Warning/Failed view |

Core `DomainError` APIs are unchanged. The owner copies event-visible errors
with `without_source()` and **rejects retention** of an oversized copy:
terminal publication still happens, but the retained/emitted error is a new
`DomainError` built with existing constructors (`ErrorCode::InvalidArgument`
or the original code when already short) carrying a short fixed message and
`field=error_payload` — never by mutating core validation or treating
`SP-CAN-001` as `Failed`. Fake probes only emit short catalog-built errors.

Additional structure bounds:

| Cap | Default |
|---|---|
| `command_queue_capacity` | 8 |
| `worker_progress_queue_capacity` | 32 (try_send; drops counted) |
| `worker_result_queue_capacity` | 8 (terminals/load; owner drains first) |
| `control_history_capacity` | 64 |
| `control_history_max_bytes` | 256 KiB |
| `notification_queue_capacity` | 16 (per subscriber) |
| `max_subscribers` | 32 (global fanout cardinality) |
| `log_tail_capacity` | 32 |
| `result_retention` | 32 |
| `pending_stop_slots` | 1 (fixed) |
| `pending_shutdown_slots` | 1 (fixed) |
| `teardown_deadline` | 2s |

Control history is a bounded ring (count **and** bytes). There is no
unbounded control deque.

### Events, cursors, and results

- The owner assigns strictly increasing `seq` before fanout. Numbers are
  never reused.
- **Replaceable** `Progress` / `Log` are latest-wins, not retained in control
  history. Delivered streams may omit them (intentional gaps). That is **not**
  expired history and does **not** alone trigger resync.
- **Control** events enter bounded control history before notify. Overflow
  drops oldest control entries, records `max_dropped_control_seq`, and
  increments `drops.control_history`.
- Cursor type: `EventCursor { session_id, seq }`.
  - Foreign `session_id` → `CursorError::ForeignSession` (`SP-SESS-001`).
  - `seq > snapshot.seq` → `CursorError::Future` (`SP-INP-003`).
  - Resync when `max_dropped_control_seq > cursor.seq` (a dropped **control**
    lies after the cursor). Progress-only seq gaps do **not** bump that
    watermark and must not force resync. `ResyncRequired` carries the latest
    snapshot plus retained terminals from the **result cache** (and any
    surviving control-ring terminals), never a fabricated gap fill.
  - Otherwise replay returns retained control events with `seq > cursor.seq`
    (progress/log absences are intentional).
- `subscribe()` / `subscribe_and_replay()` register the subscriber and take
  the snapshot under the owner lock so the live stream boundary matches the
  snapshot `seq`. `subscribe_and_replay` also returns the control
  `ReplayOutcome` from that cursor in the same critical section, so retained
  control events are not lost between replay and live handoff. A cursor older
  than retained history still yields `ResyncRequired` (not progress gaps).
- New subscribers beyond `max_subscribers` are rejected
  (`SubscribeError::SubscriberLimit`); count is global, not only per-queue.
  Dropping a `Subscription` releases its slot immediately (idle cleanup, not
  only on publish disconnect).
- `command_result(id)` → `Pending` | `Ready` | `Expired` | `Unknown`. Pending
  covers the active probe **and** outstanding `stop_owner` /
  `shutdown_owner` identities.   Accepted command ids use a contiguous id space and an O(1)
  `max_accepted` watermark so an evicted result stays `Expired`, never
  `Unknown`, without retaining every historical id. Control history
  `approx_bytes` counts full command labels and rejection errors; a single
  entry larger than `control_history_max_bytes` is dropped (watermark /
  resync) while its terminal may remain in the bounded result cache.
  `SessionLimits::normalized` raises the history byte floor to one legal
  max control event.
- Every accepted command produces exactly one canonical terminal for its
  `command_id`: `Completed { outcome }` or `ShutdownComplete` (shutdown owner
  only). `Failed` is only a `RunOutcome` / `TerminalOutcome` payload.
  Injected worker terminals that violate core outcome rules
  (`Failed` with `SP-CAN-001`, blank `saved_checkpoint`) or wrong-kind
  `LoadDone` (cannot complete `ProbeRun` even with matching ids) are
  rejected without closing the active run. `check_progress` requires
  `ProgressSnapshot::validate()` (step≤total, finite unique non-blank
  metrics) **and** the byte cap; malformed progress is never
  canonicalized. Worker→owner enqueue and `inject_worker_event` sanitize
  first: illegal events are dropped before the mailbox, oversized
  load/checkpoint labels and errors are bounded before retention, and
  bounded error views keep the original code/category plus any
  `retryable_override` / `user_action`.

### Save-failure vs cancel (§4 vs §5)

Successful `Stop{save:true}` follows the frozen machine:
`Running|Paused` → `Stopping` → `Saving` → `CheckpointSaved` → `Ready`, with
the run terminal `Canceled` carrying the checkpoint label.

Runtime-contracts §4 lists `Stopping | save failed | Failed`. §5 states that
save-on-cancel failure keeps the run `Canceled` with a `Warning` and leaves
the previous checkpoint intact. **P1-F07 binding for save failure:** §5 wins
for cancel-driven save failure — `Canceled` + `Warning` (`SP-IO-004`) and
`Stopping`→`Ready` without entering `Failed`. The §4 `Failed` transition is
reserved for a future **explicit Save** command failure after entering
`Saving`, not for cancel save-on-stop. Tests cover both paths.

### Progress, log, diagnostics

- Heartbeat: while a probe is active the worker publishes progress at least
  once per `heartbeat_interval` (default 1s), including during a stalled /
  paused step. Cancel is observed at least every `cancel_poll_interval`
  (default 50ms, and always ≤ heartbeat). A long `worker_step_delay` is
  sliced into poll intervals so a stalled step still meets both deadlines.
  Final progress rides with the result mailbox and is published on the live
  stream **before** the terminal (separate mailboxes; owner drains results
  first). Fresh runs allocate a new `CancelToken` and do not inherit a prior
  cancel reason unless Stop/Shutdown is still pending for that admission.
  `Stop { save }` updates a live `AtomicBool` on the running worker so
  `save=false` suppresses checkpoints immediately.
- Notification fanout: bounded **subscriber count** (`max_subscribers`) and
  bounded per-subscriber queues; `try_send` only; full queues drop and count.
  Workers never wait on them.
- After `ShutdownComplete`, notification channels close. Snapshot/history
  remain queryable until the service is dropped.

### Teardown and worker failure

- `ApplicationService` drop: clear stall/pause, request `CancelReason::Shutdown`,
  submit `Shutdown`, join the owner within `teardown_deadline`, then force
  terminals for any still-pending probe/stop/shutdown so nothing stays
  `Pending`. The worker receives `Shutdown` and is joined via `WorkerHandle`.
- Worker panic (caught at the fake-worker boundary) or result-channel
  disconnect publishes `Failed { Internal }` for the active probe and clears
  pause; no detached paused worker remains.
- Last-client / closed notify: further `subscribe` returns
  `SubscribeError::Closed`.

### Diagnostics policy (honest)

- **Implemented in F07:** bounded in-memory log tail (`log_tail_capacity` ×
  `log_message_max_bytes`) and drop counters. No disk writes.
- **Unavailable in F07:** rotated disk diagnostics. Not implemented; not
  marked done. **P1-F09** owns CLI-facing disk log integration when that
  slice lands. Failure policy for a future disk sink is therefore not
  claimable here.

### Run identity

- Each accepted mutating probe allocates a new `RunId`. Worker events with a
  mismatched `run_id` or `command_id` are rejected (`drops.stale_worker`) and
  must not change state.
- Session-level events omit `run_id`.

### Timing defaults

| Limit | Default |
|---|---|
| `heartbeat_interval` | 1s |
| `cancel_poll_interval` | 50ms |
| `worker_step_delay` | 10ms (tests may shorten; still sliced by poll) |

### Errors

Stable codes only: `SP-SESS-001`/`002`/`003`, `SP-CAN-001`, and existing
input codes from `spirula-core`. Fake probes use `DomainError::try_new`,
`with_context`, `without_source`, `CancelToken`, `ProgressSnapshot::try_new`,
and `TerminalOutcome` as defined. Cancellation remains
`Completed { outcome: Canceled { .. } }`, never `Failed` with `SP-CAN-001`.

## Out of scope

- Real dataset load, GPU device ownership, training steps, Slint, CLI SIGINT
  and disk log wiring (P1-F09), measurement infrastructure (P1-F08).
