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
| `help` / `--help` / bare argv | Help text; exit 0 |
| `capabilities` | Available vs unavailable; exit 0 |
| `probe-load --label <s> [--log-dir]` | Session + ProbeLoad; shutdown |
| `probe-run --steps <n> [--load-label] [--log-dir] [--step-delay-ms] [--save\|--no-save]` | Optional load then ProbeRun |
| `probe --label [--steps] …` | Convenience load[+run] |
| `train` / `preview` | `UnimplementedOption`, exit 2 |

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

1. First SIGINT → submit `Stop { save: true }` (or `--no-save` when requested),
   wait for the command terminal (live subscribe **or** `command_result` /
   replay if the subscriber drops), exit 130.
2. Second SIGINT → restore default disposition (`SIG_DFL`). Further SIGINT
   uses OS default (may signal-terminate).

### Disk diagnostics (application-owned)

Explicit caps on `DiskLogLimits`: `line_max_bytes`, `queue_capacity`,
`tail_capacity`, `rotation_bytes`, `max_files`, `global_disk_budget_bytes`,
plus `flush_join_deadline`. Async sink thread; `try_enqueue` only from the
owner. Full / oversized → drop counters. Write failure → counters + at most
one in-memory `Warning` with `field=disk_log` (never re-enters the sink).
`flush` / `join` honor deadlines; Drop of `ApplicationService` joins under
`teardown_deadline`. Workers and UI/CLI notify paths never block on disk.

### Recovery

If the live subscriber disconnects or coalesces, the CLI polls
`command_result(id)` (and treats snapshot `Closed`) so a terminal is still
observed. That reuses F07 result retention — no parallel business machine.

### Test hooks (non-capability)

| Env | Purpose |
|---|---|
| `SPIRULA_TEST_WORKER_BEHAVIOR=fail\|panic\|cancel_save_fail` | Inject fake worker outcome |
| `SPIRULA_TEST_DISK_LIMITS=1` | Use `DiskLogLimits::for_test` |

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
