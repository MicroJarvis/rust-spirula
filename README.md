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

Phase 0 (reference baseline, subsystem inventory, capability matrix,
checksummed compatibility fixtures, runtime contracts and the verification
entry) is complete. See [docs/phase-0/README.md](docs/phase-0/README.md) and
the recorded [exit report](docs/phase-0/exit-report.md). Verify the workspace
and fixtures with:

```bash
cargo run -p spirula-xtask -- verify
```

P1-F01 through P1-F04 are implemented in `spirula-core`. P1-F05 (atomic
publish, temp harness, and config filesystem save/migration) is implemented
in `spirula-io`; see [docs/phase-1/atomic.md](docs/phase-1/atomic.md). Full
lenses, scene files, and splat files stay in P3. Checkpoint bundle markers
stay in P5-F05. The next feature is P1-F06 in
[docs/project-plan.md](docs/project-plan.md).


The [OOOSplat / Brush review](docs/research/ooosplat-brush-review.md) informs
planned early session/CLI delivery, differentiable rendering, resource budgets,
validated execution plans, full-state resume and packaging. Detailed acceptance
is indexed in [development gates](docs/development-gates.md). These are pending
features; the implementation status above remains authoritative. Brush is an
offline reference and does not change the Rust/Slint/wgpu/WGSL runtime stack.
