# Spirula Studio Rust

Rust rewrite of `/Users/tfjiang/Projects/spirula-studio`.

The target architecture uses:

- Rust for application, data, rendering, and training orchestration.
- Slint for the desktop UI.
- `wgpu` for portable GPU resource and pipeline management.
- WGSL for portable GPU kernels and render shaders.

The migration is intentionally staged. The architecture and compatibility
contracts are defined before feature work begins. See
[docs/project-plan.md](docs/project-plan.md) for the Chinese project plan,
[Agents.md](Agents.md) for the working rules and execution contract, and
[docs/architecture.md](docs/architecture.md) for the target architecture.

The C++ repository remains the behavioral reference during migration. Its
pinned commit, toolchain and build/test entry points are recorded in
[docs/phase-0/reference-baseline.md](docs/phase-0/reference-baseline.md); set
`SPIRULA_REFERENCE_REPO` to locate the checkout (default `../spirula-studio`).

Phase 0 is complete. Phase 1 runs through P1-F09; P1-F01–P1-F07 are done and
P1-F08 is next. Phase 0 evidence is in
[docs/phase-0/exit-report.md](docs/phase-0/exit-report.md). The P1-F06 record is
[docs/phase-1/exit-report.md](docs/phase-1/exit-report.md); P1-F07 bindings are
[docs/phase-1/application.md](docs/phase-1/application.md). The single local
verification entry is:

```bash
cargo run -p spirula-xtask -- verify
```

That command runs fixture and document checks, workspace policy (lint
inheritance, target architecture path edges, declared feature combinations,
CLI dependency shape), and the fixed `fmt` / `check` / `test` / `clippy` gate
with compile steps pinned to the rustc host via `--target`. Details:
[docs/phase-1/workspace-checks.md](docs/phase-1/workspace-checks.md).

P1-F01 through P1-F04 live in `spirula-core`. P1-F05 (atomic publish, temp
harness, config filesystem save/migration) lives in `spirula-io`; see
[docs/phase-1/atomic.md](docs/phase-1/atomic.md). Full lenses, scene files, and
splat files stay in P3. Checkpoint bundle markers stay in P5-F05.
`spirula-application` (P1-F07) owns the headless session, bounded commands,
control history, and fake probe worker; it depends only on `spirula-core`.
`spirula-cli` stays an entry adapter until P1-F09. The next feature is P1-F08
in [docs/project-plan.md](docs/project-plan.md). Phase 2 has not started.

The [OOOSplat / Brush review](docs/research/ooosplat-brush-review.md) informs
planned early session/CLI delivery, differentiable rendering, resource budgets,
validated execution plans, full-state resume and packaging. Detailed acceptance
is indexed in [development gates](docs/development-gates.md). These are pending
features; the implementation status above remains authoritative. Brush is an
offline reference and does not change the Rust/Slint/wgpu/WGSL runtime stack.
