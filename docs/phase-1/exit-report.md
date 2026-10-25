# P1-F06 verification record

Recorded: 2026-10-25 13:51 CST (2026-10-25T05:51:49Z), on the development
machine, in worktree `.worktrees/p1-f06` on branch `codex/p1-f06`. Parent tip
before this feature was `4de9e94` (`fix(P1-F05): bind publish leases to
destinations and backup temps`). Later merged roadmap `e548295`.

## 1. Scope and result

P1-F06 is complete: the workspace gate and policy check pass on this host.
Phase 1 continues through P1-F09. The next feature is P1-F07 (headless
application/session). Phase 2 stays unstarted.

## 2. Environment

| item | value |
|---|---|
| OS | macOS 27.0.1, arm64 (`Darwin 27.0.0`) |
| Host triple | `aarch64-apple-darwin` |
| Rust | rustc 1.99.0, cargo 1.99.0 |
| workspace minimum | `rust-version = "1.99"`, edition 2024, resolver 3 |
| Windows / cross targets | **not run** on this machine |
| C++ reference | checked when present via xtask baseline note |

## 3. Commands and results

```bash
cargo run -p spirula-xtask -- verify
cargo run -p spirula-xtask -- policy
cargo fmt --all -- --check
cargo check --workspace
cargo test --workspace
cargo clippy --workspace --all-targets --all-features -- -D warnings
git diff --check
```

Observed on this host (fresh verify after the P1-F06 changes):

| check | result |
|---|---|
| fixtures | pass (checksummed fixture set) |
| docs | pass (feature rows, links, completed-feature artifacts, error catalog) |
| policy | pass (8 members, lint inheritance, target architecture edges, CLI dep shape, feature combinations `[default]`, rustc host pinned via `--target`) |
| fmt / check / test / clippy / diff | pass |
| `spirula-xtask verify` | ok (8 checks: fixtures, docs, policy, fmt, check, test, clippy, diff) |
| optional features | none declared; `default` is the only meaningful combination |
| `--target` | compile steps pin live rustc host (not a cross matrix); Windows host triples are allowed when that is the host |

## 4. Feature evidence

| feature | record / owner | evidence |
|---|---|---|
| P1-F01 domain errors | `spirula-core`, runtime contracts §1.4 | stable codes, context, CLI text/exit |
| P1-F02 config documents | [config.md](config.md) | 251-field table, schema/legacy, preserved save |
| P1-F03 cancel/progress | [cancellation.md](cancellation.md) | token, progress, terminal payloads |
| P1-F04 math/camera/scene | [geometry.md](geometry.md) | finite math, pinhole, similarity, splat checks |
| P1-F05 atomic publish | [atomic.md](atomic.md) | leases, replace, harness, config FS save/migrate |
| P1-F06 workspace checks | [workspace-checks.md](workspace-checks.md) | fixed gate, lint inheritance, policy edges, CLI adapter |

## 5. Explicit non-claims

- No cross-compilation matrix was run; compile steps only pin the development
  machine’s rustc host triple.
- Phase 1 is not finished: P1-F08 and P1-F09 are still unstarted.
- Phase 2 GPU work (`P2-F01` onward) is unstarted.

---

# P1-F07 verification record

Recorded: 2026-10-25, worktree `.worktrees/p1-f07` on branch `codex/p1-f07`,
parent tip `0c73ec1` (P1-F06 on main). HOLD-MERGE review fixes landed after
`f6f5e0c` (unique SessionId, poison-free recovery, atomic admit, cancel
isolation, Stop save flag, LoadDone/Stop race, final progress ordering,
payload/result retention contracts, dropped-control watermark, subscriber
Drop capacity). Follow-up worker-boundary: `check_progress` calls
`ProgressSnapshot::validate()`, kind-vs-payload LoadDone rejection,
pre-mailbox sanitize/bound, bounded errors keep code/category/retryable/
user_action. Later acceptance: Reset clears `last_outcome`; successful
`Stop{save}` emits `Saving`+checkpoint; teardown deadline normalized to
cancel budget and always joins; O(1) accepted-id watermark (no unbounded
seen-set); oversized control entries dropped under byte budget while
results stay bounded; Accepted/Rejected approx_bytes include labels/errors;
honest ownership docs (serialized mutex binding, not one exclusive
writer thread).

## Scope and result

P1-F07 is complete: `spirula-application` is the unique headless session owner
with bounded admission/history/notify, independent Stop/Shutdown control,
fake probe worker, run-identity stale rejection, and lifecycle tests. No
train/GPU deps. Disk diagnostics remain unavailable (in-memory log tail only);
P1-F09 owns CLI disk integration. Next feature is P1-F08. Branch remains
unmerged until explicitly approved.

## Evidence

| item | location / command |
|---|---|
| Binding decisions | [application.md](application.md) |
| Crate | `crates/spirula-application` (deps: `spirula-core` only) |
| Lifecycle regressions | `crates/spirula-application/tests/session_lifecycle.rs` (**43** tests) |
| History unit regressions | `crates/spirula-application/src/history.rs` (`mod tests`, **3** tests) |
| Reproducible gate | `cargo test -p spirula-application -- --test-threads=1` then `cargo run -p spirula-xtask -- verify` (**8/8** on tip `cdb0655`) |

Supplemental ephemeral external probe binaries used during HOLD-MERGE review
are not checked in; treat them as non-reproducible reviewer aids only. The
authoritative, machine-independent evidence is the crate tests and xtask
verify above.

## Non-claims

- No real dataset load, training, GPU, Slint, or CLI SIGINT.
- No rotated disk log sink (explicitly unavailable, not a finished stub).
