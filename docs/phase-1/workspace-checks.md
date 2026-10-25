# P1-F06: workspace foundation checks

`spirula-xtask verify` is the single local verification entry. It reuses the
Phase 0 fixture/docs/baseline checks and adds a workspace policy gate plus the
fixed cargo fmt/check/test/clippy sequence. There is no second parallel tool
tree.

## Fixed gate

| step | command | notes |
|---|---|---|
| fixtures | in-process fixture checker | checksums and format probes |
| docs | in-process plan/link/artifact check | exactly one `当前` feature |
| policy | in-process workspace policy | see below |
| fmt | `cargo fmt --all -- --check` | host only |
| check | `cargo check --workspace` | default features |
| test | `cargo test --workspace` | default features |
| clippy | `cargo clippy --workspace --all-targets --all-features -- -D warnings` | deny warnings |
| diff | `git diff --check` | when `.git` is present |

The gate never passes `--target`. Cross-compilation and Windows CI remain
P9-F01 / C-X-05; this feature only records the host triple that actually ran.

## Lint inheritance

Root `[workspace.lints]` is the policy source:

- `rust.unsafe_code = "deny"`
- `rust.unused_must_use = "deny"`
- `clippy.dbg_macro = "deny"`
- `clippy.todo` / `clippy.unimplemented` = `"warn"`

Every workspace member sets `[lints] workspace = true`. New `unsafe` must be
narrowly `#[allow(unsafe_code)]` at the call site with a reason. Today that is
limited to `spirula-io` atomic publish: `OsStr::from_encoded_bytes_unchecked`
after a verified whole-`OsStr` prefix strip, and Windows `MoveFileExW` FFI.

## Feature combinations

Cargo metadata is the source of truth. When no optional features are declared
(current tree), the only meaningful combination is `default`, and
`clippy --all-features` is equivalent to default. When optional features appear,
policy requires the gate to cover `--all-features` for clippy and check; the
gate must be extended before those features can land.

## Architecture edges

Policy allowlists path dependencies among `spirula-*` crates:

| crate | allowed path deps |
|---|---|
| `spirula-core` | — |
| `spirula-io` | `spirula-core` |
| `spirula-gpu` / `render` / `train` / `ui` | — (skeleton) |
| `spirula-cli` | — (entry adapter only) |
| `spirula-xtask` | `spirula-io` (tooling) |

`spirula-application` must not be a workspace member and must not appear as a
dependency until Phase 6. UI/CLI business orchestration stays absent.

## CLI entry adapter

Until P6-F01, `spirula-cli` must:

- declare no `spirula-*` path dependencies;
- contain only `src/main.rs` (no nested modules);
- avoid referencing domain crates by Rust path.

Hidden training, IO, GPU, or session logic in the CLI package fails policy.

## Commands

```bash
cargo run -p spirula-xtask -- verify
cargo run -p spirula-xtask -- policy
cargo run -p spirula-xtask -- docs
```

Individual DoD commands from
[feature-definition-of-done.md](../agent/feature-definition-of-done.md) remain
valid and are what the verify gate invokes.
