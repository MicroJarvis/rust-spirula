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

Phase 0 and Phase 1 are complete. P2-F01–P2-F06 (adapter negotiate, device/
queue/context, buffer/texture/readback, shader/pipeline/affine compute, GPU
errors / diagnostic events, headless compute smoke) are complete; the current
feature is P2-F07 (minimal CLI package / isolated launch). Phase 2 exit is not
closed. Phase 0 evidence is in
[docs/phase-0/exit-report.md](docs/phase-0/exit-report.md).
Phase 1 records are in
[docs/phase-1/exit-report.md](docs/phase-1/exit-report.md);
P1-F07 bindings are [docs/phase-1/application.md](docs/phase-1/application.md);
P1-F08 bindings are
[docs/phase-1/performance.md](docs/phase-1/performance.md);
P1-F09 bindings are [docs/phase-1/cli.md](docs/phase-1/cli.md);
P2-F01 bindings are [docs/phase-2/gpu-adapter.md](docs/phase-2/gpu-adapter.md);
P2-F02 bindings are
[docs/phase-2/device-context.md](docs/phase-2/device-context.md);
P2-F03 bindings are
[docs/phase-2/gpu-resources.md](docs/phase-2/gpu-resources.md);
P2-F04 bindings are
[docs/phase-2/gpu-pipelines.md](docs/phase-2/gpu-pipelines.md);
P2-F05 bindings are
[docs/phase-2/gpu-diagnostics.md](docs/phase-2/gpu-diagnostics.md);
P2-F06 bindings are
[docs/phase-2/gpu-smoke.md](docs/phase-2/gpu-smoke.md);
P2-F07 bindings are
[docs/phase-2/cli-package.md](docs/phase-2/cli-package.md) (HOLD-MERGE).
The single local verification entry is:

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
P1-F08 adds versioned performance reports and the `spirula-perf-sample`
binary in `spirula-core`/`spirula-io`. `spirula-cli` is the P1-F09 process
adapter (help/capabilities, fake probe, SIGINT, optional disk diagnostics);
real train/preview stay later features. Phase 2 current work is P2-F07
(HOLD-MERGE) in [docs/project-plan.md](docs/project-plan.md). GPU diagnose
(enumerate, `--device`, `--resources`, or `--compute`) and smoke
(`spirula-gpu-smoke`; not application/train/UI readiness; surface remains honest
unsupported / not a present-pass claim). Minimal portable package (no signed
installer; CLI does not gain a GPU edge):
`cargo run -p spirula-xtask -- package --out /tmp/spirula-cli-pkg` then
`package-check` / `package-run`. Smoke alone:
`cargo run -p spirula-gpu --bin spirula-gpu-smoke -- --all`.

The [OOOSplat / Brush review](docs/research/ooosplat-brush-review.md) informs
planned early session/CLI delivery, differentiable rendering, resource budgets,
validated execution plans, full-state resume and packaging. Detailed acceptance
is indexed in [development gates](docs/development-gates.md). These are pending
features; the implementation status above remains authoritative. Brush is an
offline reference and does not change the Rust/Slint/wgpu/WGSL runtime stack.
