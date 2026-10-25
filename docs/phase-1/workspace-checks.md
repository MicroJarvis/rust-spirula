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
| fmt | `cargo fmt --all -- --check` | no `--target` (fmt is host-tooling) |
| check | `cargo check --workspace --target <rustc-host>` | default features |
| test | `cargo test --workspace --target <rustc-host>` | default features |
| clippy | `cargo clippy --workspace --all-targets --all-features --target <rustc-host> -- -D warnings` | deny warnings |
| diff | `git diff --check` | when `.git` is present |

`<rustc-host>` is the live `rustc -vV` `host` triple (Windows hosts are valid).
Compile steps pass `--target` explicitly so `CARGO_BUILD_TARGET` and
`.cargo` / `~/.cargo` `build.target` cannot silently cross-compile under a
“host” gate. Policy checks that pin; it does **not** reimplement Cargo’s
config merge. Cross-compilation matrices remain P9-F01 / C-X-05.

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

Cargo metadata is the source of truth for declared optional features. Policy
records only meaningful combinations:

- no optional features → `{ default }` (current tree); `clippy --all-features`
  is equivalent to default and stays in the gate
- optional features present → `{ default, all-features }`, and the gate must
  cover `--all-features` for **check**, **test**, and **clippy**

Policy does **not** invent `--no-default-features` or placeholder features.
Extend the gate when real optional features land; do not add features only to
exercise the matrix.

## Architecture edges

Policy allowlists **target** workspace path edges (unused edges are fine):

| crate | allowed workspace path deps |
|---|---|
| `spirula-core` | — |
| `spirula-io` | `spirula-core` |
| `spirula-gpu` | `spirula-core` |
| `spirula-render` | `spirula-core`, `spirula-gpu` |
| `spirula-train` | `spirula-core`, `spirula-io`, `spirula-render`, `spirula-gpu` |
| `spirula-application` | `spirula-core`, `spirula-io`, `spirula-train` (when added) |
| `spirula-ui` | `spirula-core`, `spirula-application` |
| `spirula-cli` | `spirula-core`, `spirula-application` |
| `spirula-xtask` | `spirula-io`, `spirula-core` (tooling) |

`spirula-application` is planned, not banned: when it is a workspace member,
edges are validated like any other crate. Dependencies are inspected by package
identity (rename aliases do not hide a forbidden package). Every path
dependency is considered, including non-`spirula-*` local paths (rejected).

Additional external / layer rules:

- `spirula-core` must not take GPU/UI/OS runtime crates (`wgpu`, `slint`,
  `tokio`, `nix`, `windows`, …) as normal, dev, or build deps.
- `spirula-ui` must not take `spirula-io` / `train` / `render` / `gpu` path deps
  (normal or dev); route through application.

## CLI entry adapter

Policy enforces **dependency** shape for `spirula-cli`: only
`spirula-core` and `spirula-application` are allowed workspace edges. It does
**not** AST-scan `main.rs` for business logic (`std::fs`, inline training, and
similar would not be detected). The current `main.rs` is a documented skeleton
placeholder until P1-F09 wires application commands; keep business orchestration
out of this crate by convention and review, not by substring bans.

## Commands

```bash
cargo run -p spirula-xtask -- verify
cargo run -p spirula-xtask -- policy
cargo run -p spirula-xtask -- docs
```

Individual DoD commands from
[feature-definition-of-done.md](../agent/feature-definition-of-done.md) remain
valid and are what the verify gate invokes.
