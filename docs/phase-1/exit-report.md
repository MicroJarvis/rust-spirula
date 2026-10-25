# Phase 1 exit report

Recorded: 2026-10-25 13:51 CST (2026-10-25T05:51:49Z), on the development
machine, in worktree `.worktrees/p1-f06` on branch `codex/p1-f06`. Parent tip
before this feature was `4de9e94` (`fix(P1-F05): bind publish leases to
destinations and backup temps`).

## 1. Scope and result

Phase 1 is complete. P1-F01 through P1-F06 have code and/or recorded contracts,
and the workspace gate including the new policy check passes on this host.
Phase 2 is not started; `P2-F01` becomes the sole `当前` feature after merge.

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
| policy | pass (8 members, lint inheritance, architecture edges, CLI entry-only, feature combinations `[default]`, host `aarch64-apple-darwin`) |
| fmt / check / test / clippy / diff | pass |
| `spirula-xtask verify` | ok (8 checks: fixtures, docs, policy, fmt, check, test, clippy, diff) |
| optional features | none declared; `default` is the only meaningful combination |
| `--target` / Windows | not part of the gate; not executed |

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

- No Windows host or cross `--target` build was run for this exit.
- `spirula-application` was not added; CLI remains a skeleton entry adapter.
- Phase 2 GPU work (`P2-F01` onward) is unstarted.
