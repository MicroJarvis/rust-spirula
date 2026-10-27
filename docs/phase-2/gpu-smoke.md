# P2-F06: headless GPU smoke

Status: **已完成** (code tip `e66af3d6e5ecdd4b6ecaac90759fad0079489a95`).
Owner: `spirula-gpu`. Builds on [gpu-pipelines.md](gpu-pipelines.md) (affine
compute), [gpu-diagnostics.md](gpu-diagnostics.md). Packaging is P2-F07
(已完成; [cli-package.md](cli-package.md)); application session / train / UI
stay later. Phase 2 exit remains open (surface present owned by P2-F08);
plan current on this branch is **P3-F02**.

**Scope narrowed 2026-10-27:** real surface present verification moved to
**P2-F08**. This feature's surface deliverable is only the honest unsupported
gate; it was never a present-pass and must not be cited as one.

## Reference behavior (C++)

Pinned: `9a24f09927944a795e7ba1bd8243a625bf419479`.

| C++ surface | Behavior recorded | Rust binding in this slice |
|---|---|---|
| `vk_runtime_smoke.cpp` | Device select + malloc/memcpy/memset/stream/event sticky-error path over `BackendRuntime` | **Deviation**: no CUDA-shaped runtime mirror; smoke uses session `GpuContext` + accounted buffers (P2-F03) |
| `vk_pipeline_smoke.cpp` | Embedded SPIR-V saxpy/iota dispatch + CPU exact check; unknown entry sticky error | Reuse P2-F04 **affine-u32** WGSL upload→dispatch→readback vs `cpu_affine_u32` (not saxpy/iota; not SPIR-V) |
| `vk_sortscan_test.cpp` | Sort/scan parity vs CPU | **Out of scope** (no sort/scan in P2); not claimed |
| Process exit 0/1 | Failures counted; ALL PASSED line | Structured `SmokeReport` + exit 0 for pass\|skip\|unsupported, 1 for fail, 2 usage |
| (no surface binary in these three) | — | Honest surface gate: **no forged window**; remains `unsupported` (present not verified in this slice) |

## Binding decisions

### Compute smoke

- Entry: library `run_compute_smoke` / binary `spirula-gpu-smoke [--compute]`.
- Path: open → affine buffers → upload/submit/wait → obtain pipeline → dispatch →
  readback/map → CPU exact → drop → `after_drop current=0`,
  `retained_submissions=0`, context `Live`.
- Records device name, backend, class, enabled features, device limits,
  accounted source, diagnostic error count, repeat index.
- Adapter/device open failure → `status=skip` with `open_failure_stage` + code
  (not a false pass). Exit 0.
- Fail probes (tests): validation → `SP-INP-003`; wait timeout → `SP-GPU-003`
  then reclaim; lost → `SP-GPU-002`. Reuses P2-F05 inject/diagnostic paths.
- Every compute Fail exit abandons **unsubmitted** pending (staging/holds) and
  waits any accepted submission before dropping handles — report
  `after_drop current=0` / `retained=0` must match live context accounting so
  the context stays reusable (submit-inject → retry pass). Never frees in-flight
  GPU work without wait.

### Surface smoke

- Entry: `run_surface_smoke` / `--surface`.
- Default: `status=unsupported` (will not forge a window).
- `SPIRULA_GPU_SURFACE=1`: still `unsupported` if no safe present target is
  wired in this crate (honest; present path stays later / P6-F07). Open failure
  → `skip`.
- **Not verified as present success** on any host in this feature — do not claim
  surface smoke passed; only the honest unsupported/skip gate is complete.

### Non-goals (this slice)

- No changes to P2-F03/F04/F05 contracts beyond consuming them.
- No P2-F07 packaging / isolated checkout.
- No P9 perf baselines; no application/train/UI session ownership.
- No sort/scan; no Vulkan/CUDA runtime path.

## Acceptance (met)

1. Host compute smoke: pass with cpu_exact + current 0, or precise skip.
2. Repeat runs stable; device missing → skip (not fail-as-pass).
3. Validation / lost / timeout fail probes return typed codes + reclaim where
   applicable; Fail report bytes match live context.
4. Surface: unsupported without forged window; opt-in still honest (**not** a
   present-pass claim).
5. Docs list device/backend/features/limits, skip/pass, exit, cleanup.
6. No F07 / app / train / UI claims; Phase 2 exit still open (surface present
   owned by P2-F08); plan IDs remain unique and are not renumbered.

## Tested evidence (approved tip)

Code tip: `e66af3d6e5ecdd4b6ecaac90759fad0079489a95`. Independent review READY:
`/tmp/spirula-p2-f06-review/REVIEW-final-e66af3d.md`. Commands below are the
reproducible checks; machine `/tmp` logs are sample runs, not the sole evidence.

| Check | Command | Sample result |
|---|---|---|
| Fresh full gate | parent independent verify | **8/8** (sample: `/tmp/spirula-codex-p2-f06-e66af3d-independent-verify.log`) |
| Focused host+synthetic | `cargo test -p spirula-gpu --test host_smoke --test smoke_lifecycle_synthetic` | host **5/5**, synthetic **6/6** (sample: `/tmp/spirula-codex-p2-f06-e66af3d-focused.log`) |
| Smoke binary | `cargo run -p spirula-gpu --bin spirula-gpu-smoke -- --all --repeat 2` | compute pass×2 / surface unsupported |

## Non-claims

- Not a verified surface present pass (surface remains unsupported/honest gate).
- Not P2-F07 packaging / isolated checkout.
- Not application/train/UI readiness; not P9 aggregation.
- Not real hardware OOM; not 60 s stall watchdog.
- Not C++ saxpy/iota/sortscan parity; affine proof is the compute smoke kernel.
- Phase 2 exit is **not** closed (surface present still unverified; owned by
  P2-F08); plan current on this branch is **P3-F02**.
