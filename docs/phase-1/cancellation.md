# P1-F03: cancellation and progress primitives

`spirula-core` owns a cooperative cancel token and typed progress/terminal
payloads. It does not run a session, open a channel, or touch the filesystem.
Session checkpoints, save-on-cancel, SIGINT, and event envelopes stay with
P5-F01 and P6-F01.

## Reference behavior

Pinned sources: `src/sfm/core/Cancel.h`, `src/app/TrainerCore.h`,
`src/app/gui/TrainRunner.cpp`, `src/app/cli/main.cpp`.

- SfM cancellation is a process-global `atomic<bool>*`. `cancel::check`
  throws `sfm::Cancelled` (`what()` is `sfm: cancelled`). A null flag means
  nothing can stop the run. There is no reason and no second request.
- Training uses `TrainerCore::stop_requested`. The loop breaks at a step
  boundary. `save_on_stop` defaults to true; clearing it before the flag
  skips the final checkpoint. GUI `request_stop` sets the same flag. CLI
  SIGINT points `g_train_stop` at it and exits 130 after the graceful stop.
- Parse, post-split, and image preload have no token. Progress text is
  `step i/N`, splat count, elapsed, ETA, and configured metrics.

## Binding decisions

These fill the section 3 and section 5 sketches. They do not add error codes.

- `CancelToken` is an `Arc` around one atomic word. `0` means not canceled.
  Any other value is the first `CancelReason` (`User`, `Shutdown`, `Timeout`,
  `Superseded`). `request` installs that word with a compare-exchange, so a
  clone cannot observe canceled without the winning reason. Later requests,
  including the same reason, do nothing.
- `check` returns `SP-CAN-001`. The reason is context `actual`
  (`user`, `shutdown`, `timeout`, `superseded`). Cancellation is not
  `Failed`.
- There is no process-global token and no exception type.
- `ProgressSnapshot` matches the frozen `Progress` fields. `total_steps:
  None` is an indeterminate total, not a zero-length run. `Some(0)` is a
  real empty total and allows only `step == 0`. A known total requires
  `step <= total` (`SP-INP-003`). `eta_ms: None` means the estimate is
  unknown. Metric names are ordered, unique, and non-blank; values must be
  finite (`SP-INP-001` or `SP-INP-003`).
- `TerminalOutcome` is `Completed { step }`, `Canceled { step, reason,
  saved_checkpoint }`, or `Failed { error }`. The canceled reason keeps
  `Superseded` so the token reason is not rewritten. The checkpoint is a
  path string, not a `PathBuf`, because this crate does not touch files.
  A blank path is `SP-INP-001`.
- `ProgressPublisher` is the publication boundary for one piece of work. It
  keeps the latest valid snapshot and exactly one terminal outcome. Progress
  after that outcome, and a second terminal outcome, return `SP-SESS-001`
  and leave the first outcome in place. It does not assign `seq`, coalesce
  under backpressure, or change session state.
