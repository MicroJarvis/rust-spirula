# Phase 0: baseline and architecture

Phase 0 fixes the behavioral reference, inventories the C++ subsystems,
records a capability matrix, creates the first compatibility fixtures, freezes
the runtime contracts, and provides one verification entry point. The feature
numbers and status live in [../project-plan.md](../project-plan.md).

| document | content | feature | state |
|---|---|---|---|
| [reference-baseline.md](reference-baseline.md) | pinned C++ commit, toolchain and dependency versions, build/test/failure-log entry points | P0-F01 | available |
| [subsystem-inventory.md](subsystem-inventory.md) | C++ entry points, call chains, readers/writers, formats, error and cancel paths | P0-F02 | available |
| [capability-matrix.md](capability-matrix.md) | capability, reference source, compatibility assumption, owner crate, verification and status | P0-F03 | available |
| fixtures | fixture set, golden metadata and checksum conventions | P0-F04 | planned |
| runtime contracts | error codes, events, session state machine, cancellation, GPU and version boundaries | P0-F05 | planned |
| exit report | Phase 0 acceptance evidence, commands and environment | P0-F06 | planned |

Rows gain a link when the corresponding feature is committed. Fixture files
will live under `fixtures/`.

## Phase 0 exit criteria

The exit criteria from the project plan are:

- P0-F01 through P0-F06 each have a record in this directory;
- the Rust workspace builds and passes `cargo fmt --all -- --check`,
  `cargo check --workspace`, `cargo test --workspace` and
  `cargo clippy --workspace --all-targets --all-features -- -D warnings`;
- the architecture, capability matrix and the first fixtures can be referenced
  directly by later features.

The verification entry point will be:

```bash
cargo run -p spirula-xtask -- verify
```

It runs the fixture checks, the document/status consistency checks, and the
workspace gate, and prints a short summary.
