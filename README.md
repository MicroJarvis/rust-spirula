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

The next feature is P1-F01 (domain errors and stable IDs) in
[docs/project-plan.md](docs/project-plan.md).

