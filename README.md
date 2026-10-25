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

P1-F01 (domain errors and stable IDs), P1-F02 (versioned training
config), P1-F03 (cancellation and progress primitives), and P1-F04 (math,
pinhole camera, similarity, and splat checks) are implemented in
`spirula-core`. The contracts are
[docs/phase-1/config.md](docs/phase-1/config.md),
[docs/phase-1/cancellation.md](docs/phase-1/cancellation.md), and
[docs/phase-1/geometry.md](docs/phase-1/geometry.md). Full lenses, scene
files, and splat files stay in P3. The next feature is P1-F05 (atomic files
and fixture harness) in [docs/project-plan.md](docs/project-plan.md).

