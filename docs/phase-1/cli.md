# P1-F09: minimal CLI session entry

`spirula-cli` is a process adapter only: argv parse, text presentation, and
SIGINT. All business orchestration, fake probe lifecycle, recoverable
`command_result` / replay, and rotated disk diagnostics are owned by
`spirula-application`. There is no second session state machine in the CLI.

## Reference behavior

Pinned contracts: [runtime-contracts.md](../phase-0/runtime-contracts.md)
§§1–5 (exit codes, commands/events, cancellation / SIGINT),
[application.md](application.md) (F07 unique owner),
[development-gates.md](../development-gates.md). C++ CLI stop paths inform
SIGINT timing (first cooperative save, second restores default disposition);
they do not dictate Rust channel layouts.

## Binding decisions

### Dependencies and ownership

- Workspace edges: `spirula-core` + `spirula-application` only (policy-
  enforced). No direct `spirula-io` / train / render / GPU.
- Disk persistence for diagnostics is opened and owned inside
  `ApplicationService::start_with(..., Some(DiskLogConfig))`. The CLI only
  passes `--log-dir`.
- Commands admitted: `ProbeLoad`, `ProbeRun`, `Stop`, `Shutdown` (and the
  session’s existing Pause/Resume/Reset surface remains available to other
  callers). Real `train` / `preview` names are **explicitly unavailable**
  (`SP-INP-004`, exit 2 before session).

### CLI surface

| Invocation | Behavior |
|---|---|
| `help` / `--help` / bare argv | Help text; exit 0 (no trailing flags) |
| `capabilities` | Available vs unavailable; exit 0 (no trailing flags) |
| `probe-load --label <s> [--log-dir]` | Session + ProbeLoad; shutdown |
| `probe-run --steps <n> [--load-label] [--log-dir] [--step-delay-ms] [--save\|--no-save]` | Optional load then ProbeRun |
| `probe --label [--steps] …` | Convenience load[+run] |
| `train` / `preview` | `UnimplementedOption`, exit 2 |

Pre-session usage (`exit 2`, one stderr line): empty `--label` / `--load-label`,
`--steps 0`, unknown flags on any command including `help`/`capabilities`.
In-session admission reject (e.g. `probe-run` without a prior load) stays
`exit 1` with exactly one stderr `error:` line and a stable code.

Ready markers on stdout: `spirula:ready <state>` for `loading` / `ready` /
`training` (contract name for Running). Subprocess tests wait on these lines,
never on fixed sleeps, before sending SIGINT.

### Exit codes and errors

| Outcome | Exit |
|---|---|
| Success | 0 |
| In-session failure | 1 |
| Usage / unimplemented / unknown before session | 2 |
| Cooperative cancel / SIGINT | 130 |

Stderr failures use `DomainError::cli_stderr_line`. `SPIRULA_ERROR_CODES=1`
appends `(code: SP-…)`.

### SIGINT

1. First SIGINT sets sticky cancel **intent**. The CLI admits `Stop { save:
   true }` when valid; a Rejected Stop (e.g. Ready between load and run) does
   **not** clear intent, suppresses further Stop attempts until a probe is
   active, and does **not** start the next stage — Shutdown runs and the
   process exits 130. Once a probe is active, Stop is armed and retried once.
   Live subscribe **or** `command_result` recovers terminals.
2. Second SIGINT → restore `SIG_DFL` and `raise(SIGINT)` so **this** delivery
   terminates under the default disposition (C++ `main.cpp` 63–64). Exactly
   two signals; tests require `WTERMSIG == SIGINT`, not exit 130.

Stdout presentation is a bounded async sink (`OUTPUT_QUEUE_CAPACITY`); the
control loop never blocks on a full pipe. Overflow drops lines and increments
an observable drop counter. Sink flush/join use deadlines on session end.

### Disk diagnostics (application-owned)

Explicit caps on `DiskLogLimits`: `line_max_bytes`, `queue_capacity`,
`tail_capacity`, `rotation_bytes`, `max_files`, `global_disk_budget_bytes`,
plus `flush_join_deadline`. `normalized()` ensures one admitted line plus the
trailing newline fits rotation and the global budget. Newlines in the logical
line are rejected (oversized), not silently rewritten. `file_stem` must be a
single basename (`SP-INP-001`); `../` escapes are rejected before start.

Async sink thread; `try_enqueue` only from the owner. Shutdown is out-of-band
(`AtomicBool`) so a full or stalled data queue cannot block join. `flush` /
`join` use one absolute deadline from method entry; a timed-out `join` keeps
the handle and returns `false` (retry must not report success while
`sink_alive()`). Drop shares the **remaining** teardown budget across
flush+join.

Full / oversized → drop counters. Write failure → counters + at most one
in-memory `Warning` per sink lifetime (`field=disk_log`; never re-enters the
sink). The Warning is sized with F07 `bound_error` under `SessionLimits`.
Join’s absolute deadline includes mutex acquisition across clones; Drop
always signals OOB shutdown even with a zero remaining teardown budget.
Rotation rebuilds inventory from the real directory (numeric order);
failed `remove_file` does not subtract bytes or forget the path. Non-file
siblings (e.g. directories named like `stem.N`) are not treated as successful
trims.

**Honest limit:** the sink thread itself may block inside kernel open/write
on a FIFO or hanging filesystem; caller-side `join` still returns by the
deadline, but the detached-or-still-alive thread may remain until the FS
unblocks.

### Recovery and stderr

If the live subscriber disconnects or coalesces, the CLI polls
`command_result(id)` every loop (not only on recv timeout) so continuous
replaceable Progress cannot starve the terminal. Snapshot `Closed` ends a
drain. That reuses F07 result retention — no parallel business machine.

Exactly one stderr `error:` line for a failing process: `main_from_args`
owns the write for `CliExit::Error` (admission reject / domain failure /
invalid disk config). Live and recovered failure paths return
`CliExit::Error` with the stable `DomainError` (use `SPIRULA_ERROR_CODES=1`).

### Test hooks (non-capability)

| Env | Purpose |
|---|---|
| `SPIRULA_TEST_WORKER_BEHAVIOR=fail\|panic\|cancel_save_fail` | Inject fake worker outcome |
| `SPIRULA_TEST_DISK_LIMITS=1` | Use `DiskLogLimits::for_test` |
| `SPIRULA_TEST_GAP_AFTER_LOAD_MS=<n>` | Hold at the load→run phase boundary while still polling cancel/SIGINT (reproduces Ready-gap Stop reject) |
| `SPIRULA_TEST_HOLD_AFTER_CANCEL=1` | After cancel is observed, emit `spirula:test cancel-held` and block before the canceled terminal (second-SIGINT death gate) |
| `SPIRULA_TEST_TINY_NOTIFY=1` | Cap notify/progress queues at 1 and starve live drain so terminals drop; CLI must restore via `command_result` (`spirula:recovered`, `notify-drops`) |

These are test-only; they are not advertised as product capabilities.

## Evidence

| item | location |
|---|---|
| CLI adapter | `crates/spirula-cli` |
| Disk sink | `crates/spirula-application/src/disk_log.rs` |
| Arg/unit tests | `spirula-cli` lib tests |
| Subprocess / SIGINT / log | `crates/spirula-cli/tests/cli_subprocess.rs` |
| Session disk mirror | `crates/spirula-application/tests/disk_diagnostics.rs` |
| Disk unit policy | `disk_log` module tests |

## Explicit non-claims

- Real dataset load, training, preview, GPU, and Slint remain later features
  (P3/P4/P5/P6).
- Flat C++ flag parity / presets are P6-F02, not this slice.
- Disk diagnostics are best-effort bounded logs, not a crash reporter or
  structured telemetry bus.
