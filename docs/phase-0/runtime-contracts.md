# P0-F05: runtime contracts

This document freezes the contracts that later crates implement. It is the
authority for error codes, commands, events, session state, cancellation, the
GPU/shader boundary and format versioning. The reference behavior it preserves
is recorded in [subsystem-inventory.md](subsystem-inventory.md); intentional
divergences are listed in section 9.

The contracts are language-neutral here. P1-F01 (errors), P1-F03
(cancellation/progress), P2-F05 (GPU errors), P5-F01 (session) and P6-F01
(application service) implement them in Rust. A contract change requires
updating this document, the capability matrix and the affected implementation
in the same feature.

## 1. Error model

### 1.1 Stable fields

Every error crossing a public API or an event boundary has:

| field | type | stability |
|---|---|---|
| `code` | `SP-<AREA>-<NNN>` | stable, never reused |
| `category` | enum | stable |
| `message` | string | human-readable, **not** machine-parsed |
| `context` | string map | stable keys, optional values |
| `retryable` | bool | stable per code unless an operation refines it |
| `user_action` | enum, optional | stable |
| `source` | error chain, local only | not serialized across the event boundary |

Stable context keys: `path`, `format`, `field`, `expected`, `actual`, `line`,
`step`, `device`, `resource`, `checkpoint`, `command`, `codec`. Context values
are strings; structured consumers must use `code` and the catalog, not parse
`message`.

### 1.2 Categories and codes

The catalog is append-only. A code keeps its meaning forever; a new condition
gets a new number. `SP-INT-001` is the catch-all for a violated internal
invariant or a panic caught at a worker boundary.

| code | category | retryable | meaning |
|---|---|---|---|
| `SP-INP-001` | input | no | invalid argument, option or value |
| `SP-INP-002` | input | no | unknown CLI flag or command |
| `SP-INP-003` | input | no | value outside the accepted range / choices |
| `SP-INP-004` | input | no | recognized but not implemented option |
| `SP-IO-001` | not_found | no | file or directory does not exist |
| `SP-IO-002` | permission | no | permission denied |
| `SP-IO-003` | io | yes | read failed (transient I/O) |
| `SP-IO-004` | io | yes | write failed (transient I/O) |
| `SP-IO-005` | data_decode | yes | image/map could not be decoded |
| `SP-FMT-001` | unsupported | no | format is not supported |
| `SP-FMT-002` | invalid_data | no | malformed record/field |
| `SP-FMT-003` | unsupported | no | unsupported schema/format version |
| `SP-FMT-004` | invalid_data | no | checksum/integrity mismatch |
| `SP-SESS-001` | conflict | no | command is invalid in the current state |
| `SP-SESS-002` | conflict | no | session is not ready for the command |
| `SP-SESS-003` | conflict | yes | a mutating command is already in flight |
| `SP-CAN-001` | canceled | no | cooperative cancellation completed |
| `SP-GPU-001` | gpu_unavailable | no | no adapter satisfies the request |
| `SP-GPU-002` | gpu_device_lost | no | device lost or stalled; recreate required |
| `SP-GPU-003` | gpu_submit | yes | submission or wait failed |
| `SP-GPU-004` | resource | no | device allocation failed |
| `SP-GPU-005` | unsupported | no | shader compile or pipeline layout failed |
| `SP-CKPT-001` | unsupported | no | checkpoint is not resumable (inference-only) |
| `SP-CKPT-002` | invalid_data | no | checkpoint is corrupt |
| `SP-CKPT-003` | invalid_data | no | checkpoint is incomplete (partial write) |
| `SP-INT-001` | internal | no | invariant violation / caught panic |

### 1.3 Exit codes and text

Preserve the reference exit codes:

| outcome | exit |
|---|---|
| success | 0 |
| any non-canceled failure | 1 |
| usage/invalid input before a session exists | 2 |
| cooperative cancellation / SIGINT | 130 |

The CLI writes exactly one physical line to stderr for a failure:

```text
error: <message>
```

and, when `SPIRULA_ERROR_CODES=1`, appends the stable code:

```text
error: <message> (code: SP-GPU-002)
```

The `error:` prefix and exit code are the compatibility contract; the message
may improve. `cli_stderr_line` escapes LF, CR, VT (U+000B), FF (U+000C),
NEL (U+0085), U+2028 and U+2029 inside that line as `\n`, `\r`, `\v`, `\f`,
`\u0085`, `\u2028` and `\u2029`. The stored `message` is unchanged. The UI
displays `message` and offers the code
for copying. In
structured/event form the same error serializes as:

```json
{"code": "SP-IO-005", "category": "data_decode", "message": "...",
 "retryable": true, "context": {"path": "...", "format": "png"}}
```

`source` chains are logged locally and never serialized.

### 1.4 P1-F01 binding decisions

`spirula-core` implements this section. These choices fill details the catalog
left open; they do not add or renumber codes.

- `user_action` had no members. The closed set is `correct_input`,
  `check_permissions`, `retry`, `recreate_device`, `use_complete_checkpoint`
  and `report_defect`. Variants are append-only. `ErrorCode::suggested_user_action`
  returns one only when the catalog meaning implies it (`SP-INP-001`
  through `SP-INP-004`, `SP-IO-002`, the five retryable codes, `SP-GPU-002`,
  `SP-CKPT-002`, `SP-CKPT-003`, `SP-INT-001`). Other codes suggest nothing. The field is
  omitted from event JSON unless a caller sets it.
- `retryable` on an error starts at the catalog value. `with_retryable`
  refines that one error and does not change `ErrorCode::retryable`.
- `message` must contain non-whitespace text. Empty messages are rejected.
  `Display` uses `message` unchanged and does not embed the code. The CLI
  line also omits the code unless the code form below is requested.
- `cli_stderr_line` returns exactly one physical line in both `show_code`
  modes. LF, CR, VT (U+000B), FF (U+000C), NEL (U+0085), U+2028 and U+2029
  are escaped as `\n`, `\r`, `\v`, `\f`, `\u0085`, `\u2028` and `\u2029`.
  This is a display encoding: a message that already contains those backslash
  sequences is unchanged and therefore indistinguishable on the CLI line.
  `message()`, `Display` and event JSON keep the original characters. JSON
  string escaping is unchanged: VT and FF stay `\u000b` and `\u000c`, and
  U+2028 and U+2029 stay raw.
- Context accepts only the twelve stable keys. A second insert of the same
  key fails. Values are opaque strings, including empty. Event JSON emits
  present keys in the key order listed in section 1.1, not insertion order.
- `source` is a local `std::error::Error` chain. Event JSON never includes it.
- Event JSON fields, in order, are `code`, `category`, `message`, `retryable`,
  `context`, then optional `user_action`.
- Core does not read `SPIRULA_ERROR_CODES`. `cli_stderr_line(show_code)`
  formats the stderr contract; the CLI adapter owns the environment variable.
- Exit `2` applies to category `input` only when no session exists yet.
  Category `canceled` (`SP-CAN-001`) uses exit `130` at either stage. Every
  other failure uses exit `1`. Exit `0` is success and is not an error value.
  SIGINT shares exit `130` but is not itself a catalog code.

## 2. Commands

One session owns all business state. UI and CLI submit commands and render
events; neither mutates session state directly.

```text
CommandId(u64)
Command {
  id: CommandId,
  issued_at_ms: u64,
  payload: CommandPayload,
}

CommandPayload =
    LoadDataset { path, format_hint, options }
  | StartTraining { config, resume_from: Option<PathBuf> }
  | Pause
  | Resume
  | Stop { save: bool }
  | Save { label: Option<String> }
  | Evaluate
  | Export { target, options }
  | Shutdown
```

Rules:

- commands enter through one bounded channel; the session is the only writer
  of its state;
- an accepted command returns `CommandAccepted { id }`, a rejected one returns
  `CommandRejected { id, error }`; the caller never blocks on the work itself;
- at most one mutating command is in flight; a second one is rejected as
  `SP-SESS-003` (or coalesced for `Stop`/`Shutdown`, see section 5);
- UI and CLI share the same payloads and semantics; CLI-only presentation
  (tables, colors) is outside the command.

## 3. Events

```text
EventEnvelope {
  event_version: 1,
  session_id: SessionId,
  seq: u64,                 // strictly increasing per session, no gaps
  command_id: Option<CommandId>,
  monotonic_ms: u64,        // display only
  event: Event,
}

Event =
    StateChanged { from, to, reason }
  | DatasetLoaded { summary }
  | TrainingStarted { config_digest }
  | Progress { step, total_steps, splats, elapsed_ms, eta_ms, metrics }
  | Log { level, message }
  | Warning { error }
  | CheckpointSaved { step, path, resumable }
  | EvaluationFinished { metrics_json, metrics }
  | ExportFinished { path }
  | CommandAccepted { command }
  | CommandRejected { command, error }
  | Completed { outcome }
  | ShutdownComplete
```

Ordering and delivery guarantees:

- `seq` is strictly increasing per session with no gaps; state changes,
  warnings, `Completed`, `Failed` and `ShutdownComplete` are never dropped;
- every accepted command receives exactly one terminal event
  (`Completed`, `ShutdownComplete`, or a command-specific terminal event) with
  the same `command_id`;
- `Progress` and `Log` are latest-wins under backpressure: the application may
  coalesce/drop them, but must emit at least one heartbeat per second while
  active and a final progress before completion;
- when a bounded subscriber queue is full, state/terminal events block the
  producer; progress/log drop with one `Warning` carrying the drop count;
- after `ShutdownComplete` there are no more events and the channel closes;
- an event consumer that falls behind does not stall GPU work; the UI reads the
  latest snapshot plus the event stream.

`RunOutcome`:

```text
Completed { step }
Canceled  { step, reason: User | Shutdown | Timeout | Superseded, saved_checkpoint: Option<String> }
Failed    { error }
```

## 4. Session state machine

The transition table below is the authoritative model. The normal path is
`Idle -> Loading -> Ready -> Preparing -> Training`, with `Paused` as a
training sub-loop, `Stopping -> Saving -> Ready` for a saved stop, and
`Evaluating` reachable from `Ready` or from a clean `Training`/`Paused` run.
`Failed` is reachable from every working state.

State set: `Closed`, `Idle`, `Loading`, `Ready`, `Preparing`, `Training`,
`Paused`, `Stopping`, `Saving`, `Evaluating`, `Failed`.

Allowed transitions and commands:

| from | command | to | notes |
|---|---|---|---|
| Closed | init | Idle | one session per application instance |
| Idle | LoadDataset | Loading | dataset path required |
| Loading | cancel | Idle | no partial dataset is published |
| Loading | completion | Ready | emits `DatasetLoaded` |
| Loading | error | Failed | typed error, retryable per code |
| Ready | LoadDataset | Loading | replaces the current dataset after validation |
| Ready | StartTraining | Preparing | config validated first |
| Ready | Evaluate | Evaluating | requires a loaded run/model |
| Preparing | cancel | Ready | session remains usable |
| Preparing | error | Failed | |
| Preparing | completion | Training | emits `TrainingStarted` |
| Training | Pause | Paused | takes effect at the next step boundary |
| Paused | Resume | Training | |
| Training/Paused | Stop{save} | Stopping | save=true -> Saving, else Ready |
| Stopping | save done | Saving -> Ready | checkpoint event before state change |
| Stopping | save failed | Failed | previous checkpoint stays intact |
| Training | Evaluate | Evaluating | only after a clean run or paused |
| Evaluating | completion | Ready | metrics event first |
| Failed | LoadDataset | Loading | recovery from a load failure |
| Failed | reset | Idle | clears the error |
| any | Shutdown | Closed | cooperative; emits `ShutdownComplete` |

Rules:

- a command invalid in the current state is rejected with `SP-SESS-001` and
  does not change state;
- `Pause` while `Paused`, `Resume` while `Training`, and repeated `Stop` are
  accepted no-ops (no duplicate state events);
- `Shutdown` is idempotent; it requests cancellation, performs the configured
  final save, then closes;
- a state transition emits exactly one `StateChanged`; command rejection is
  `CommandRejected`, not a state change;
- failures are terminal for the active command but not for the session: the
  last complete checkpoint and dataset remain usable in `Ready`/`Failed`.

## 5. Cancellation

```text
CancelToken::request(reason)   // idempotent
CancelToken::is_cancelled() -> bool
CancelToken::check() -> Result<(), Error>   // SP-CAN-001
CancelReason = User | Shutdown | Timeout | Superseded
```

- cancellation is cooperative and observed at declared checkpoints:
  dataset file/chunk boundary, per-image decode, per-checkpoint entry,
  per training step and at least every 250 ms inside long GPU/CPU loops,
  per exported frame, and per model/download chunk;
- in-flight GPU submissions are not torn down mid-flight: finish or wait, then
  release resources in the documented order;
- `request` is idempotent; a second request does not change the reason or
  emit a second `CancelRequested`;
- cancellation is not an error: the terminal outcome is
  `Completed { outcome: Canceled { .. } }`, not `Failed`;
- save-on-cancel defaults to true for training (the reference saves a final
  checkpoint on SIGINT). If the save succeeds, the outcome names the
  checkpoint; if it fails, the run is still `Canceled` with a `Warning` and
  the previous checkpoint remains intact;
- CLI SIGINT requests `CancelReason::User`, waits for the terminal event, and
  exits 130. A second SIGINT restores the default disposition (reference
  behavior);
- a timeout is a `Canceled { reason: Timeout }`, not a retryable I/O error;
- shutdown from any state requests cancellation and waits for
  `ShutdownComplete` or a bounded drain deadline, whichever comes first.

### 5.1 P1-F03 binding decisions

`spirula-core` implements the token and the progress/terminal payloads.
Session loops, channels, SIGINT, and checkpoint files stay later. Details
are in [../phase-1/cancellation.md](../phase-1/cancellation.md).

- The token is cloneable and shared. One atomic word stores the first
  `CancelReason`, so a canceled observation includes that reason. Repeated
  `request` calls do nothing. `check` returns `SP-CAN-001` with context
  `actual` set to `user`, `shutdown`, `timeout`, or `superseded`.
- `total_steps` may be absent. That means the total is indeterminate.
  `Some(0)` is an empty total. A known total requires `step <= total`
  (`SP-INP-003`). Metric names are unique and non-blank (`SP-INP-001`);
  metric values are finite (`SP-INP-003`).
- A terminal outcome is `Completed`, `Canceled`, or `Failed`. `Canceled`
  keeps `Superseded`. The checkpoint path is a string. `Failed` keeps the
  event-visible error, including context, retryable override, and user
  action; `source` stays local. `Failed` with `SP-CAN-001` is `SP-INP-001`
  and does not close publication. Closure is checked before payload
  validation, so a late or duplicate publication returns `SP-SESS-001`
  even when the payload is malformed. That includes progress whose `step`
  exceeds `total_steps` and a second `Canceled` with a blank checkpoint.
  The first outcome stays.

## 6. GPU boundary

### 6.1 Ownership

- `spirula-gpu` is the only crate that names `wgpu` types in public APIs.
  `spirula-render` and `spirula-train` use opaque handles
  (`DeviceHandle`, `BufferHandle`, `TextureHandle`, `PipelineHandle`,
  `SubmissionHandle`) and value types (layouts, usage flags, labels);
- a session owns one GPU context; no global device singleton (reference
  deviation). Context creation happens in the session worker, never on the UI
  thread;
- GPU work is submitted through the context; readback and sync are explicit;
- the UI never creates, uploads, or reads GPU resources.

### 6.2 Resource labels and lifetime

Label format: `spirula:<domain>:<kind>:<name>`, lowercase kebab, for example
`spirula:render:buffer:splat-means`, `spirula:train:texture:gt-rgb`,
`spirula:gpu:pipeline:rasterize-fwd`.

Every buffer/texture/pipeline creation carries a label. Diagnostics events and
errors include the label and operation (`create`, `upload`, `dispatch`,
`readback`, `destroy`). Lifetime rules:

- resources are created and destroyed in a defined order; a context records
  live resources in debug builds;
- dropping a handle releases the resource after outstanding submissions
  complete; no use-after-free is possible by construction;
- `engine reset` semantics become "drop the scene GPU resources and keep the
  device", with an explicit `close` for the context;
- a device-lost error does not allow further use of existing handles;
  recreation starts from scratch.

### 6.3 Submission, synchronization and readback

- `submit` returns a `SubmissionHandle`; `wait` is explicit. No implicit
  device-wide synchronization on every operation;
- upload uses staging buffers and a recorded copy; readback uses a staging
  buffer with an explicit wait and map;
- buffer usage flags, alignment and size are part of the public request, not
  inferred silently;
- errors are wrapped in an error scope for the duration of a submission and
  mapped to `SP-GPU-003`/`SP-GPU-002`; out-of-memory is `SP-GPU-004` with the
  requested size, current usage and label;
- device loss marks the context lost, fails in-flight submissions with
  `SP-GPU-002`, and emits a diagnostic event. Automatic recreation and
  checkpoint reload are a session policy (P2-F02), not hidden inside the
  backend;
- a stall watchdog (reference: 60 s) reports `SP-GPU-002` with the last
  submitted label.

### 6.4 Shader and pipeline contract

- shaders are WGSL under `crates/spirula-gpu/shaders/<name>.wgsl`;
- each file starts with `// spirula-shader: <name> version: <n>`;
- entry points and bind group layout are declared in a companion contract (the
  render interface), not discovered by string convention at runtime;
- uniform/storage structs use explicit alignment; layout version is a
  constant in the pipeline key;
- pipeline key =
  `(shader name, shader version, entry point, spec constants, bind layout version)`;
  changing any element creates a new pipeline and invalidates the cache entry;
- shader compilation failure is `SP-GPU-005` with file, line and compiler
  message; the pipeline is never created in a partially valid state;
- optional features (e.g. timestamp queries, f16) are negotiated through
  adapter limits/features; an unavailable feature yields `SP-GPU-001` or a
  documented fallback, never a silent assumption.

## 7. Versioning policy

| artifact | current | version field | compatibility rule |
|---|---|---|---|
| fixture manifest | 1 | `schema`, `version` | unknown version rejected with `SP-FMT-003` |
| fixture golden files | 1 | `schema`, `version` | same |
| `scene_transform.json` | 1 | `format`, `version` | additive fields allowed; unknown version rejected |
| run `config.json` | 1 | `schema_version` | missing stays legacy; unknown rejected with `SP-FMT-003`; see [../phase-1/config.md](../phase-1/config.md) |
| checkpoint `state.json` | 1 | `format_version` | must be validated on load from P3-F07 onward |
| splat PLY | none | property set is the identity | reader keeps the documented property order; writer may add a comment |
| event envelope | 1 | `event_version` | additive events/fields keep 1; removed/renamed fields bump |
| error catalog | 1 | this document | append-only codes; category changes are breaking |
| shader layout | 1 | file header + layout constant | bump invalidates pipeline cache |

Rules:

- a new persisted format has a required version field from its first write;
- additive fields keep the version; readers ignore unknown fields but preserve
  them when rewriting a file they did not originate (compatibility readers and
  editors);
- breaking changes bump the version, ship a one-way migration with a backup
  path, and keep the old reader until the replacement is confirmed;
- unknown enum values are rejected with `SP-FMT-003` unless the format
  explicitly defines an "unknown" fallback;
- migrations are explicit commands/features that can be rolled back from the
  backup; no silent in-place rewrite;
- `config.json` readers accept unversioned legacy files indefinitely until a
  later feature records a deprecation date.

## 8. Cross-entry consistency

- CLI and UI are adapters over the same `spirula-application` session commands
  and events (P6-F01). Neither implements training, IO or GPU logic.
- The CLI maps events to text; the UI maps them to widgets. Both print the
  same `code`, `message` and terminal outcome for the same fixture run.
- The session owns the dataset, engine/GPU context, checkpoint writes and
  cancellation token. There is no process-global engine.
- The same fixture under CLI and UI must produce the same state sequence,
  error code and saved checkpoint (P6-F06 acceptance).

## 9. Reference mapping and intentional divergences

| area | reference | this contract |
|---|---|---|
| errors | `std::runtime_error` with English text | typed code + category + context; text retained in `message` |
| CLI failure | `error: <message>`, exit 1/2/130 | same, plus opt-in `(code: ...)` |
| cancellation | atomics for training/SfM; no token for parse/load/export | one cooperative token with declared checkpoints |
| session | process-global engine, one session | session-owned context, one application session per instance |
| checkpoint write | direct writes, no completion marker | file-level temp + atomic replace (P1-F05); bundle completion marker (P5-F05) |
| checkpoint version | `format_version` written but not validated | validate on load, reject unknown with `SP-FMT-003` |
| config | no schema version | versioned reader with legacy defaults |
| GPU errors | sticky string, step-level check | typed errors + diagnostics events, per-resource labels |
| shaders | Slang to SPIR-V | WGSL with explicit version/layout contract |
| server/UI | in-process engine access | UI only sees application events |

The reference's user-visible log lines, exit codes and file formats remain as
recorded in [subsystem-inventory.md](subsystem-inventory.md) section 14 unless
a row above explicitly changes them.

## 10. Acceptance tests for later features

| contract | owner | test |
|---|---|---|
| error catalog/display/exit codes | P1-F01 | code snapshot, category/retryable, CLI text and exit table |
| cancellation idempotency and terminal outcome | P1-F03 | repeated request, cancel before start, cancel during load/train/save |
| command/event ordering and bounded channel | P6-F01 | exactly one terminal event, monotonic seq, backpressure drop rules |
| state machine | P5-F01 | transition table driven test, invalid-command rejection |
| GPU error mapping and labels | P2-F05 | injected submit failure, OOM, device-lost/stall |
| shader and pipeline key | P2-F04 | compile error carries file/line; layout/version changes create a new key |
| version rejection/migration | P1-F02/P3-F07 | legacy config accepted, unknown version rejected, backup preserved |
| CLI/UI parity | P6-F06 | same fixture, same state/error/terminal outcome |

Contract text is checked for internal consistency (every referenced code is in
the catalog, every command has a terminal event, every state has a transition
out) by the Phase 0 document/status checker.
