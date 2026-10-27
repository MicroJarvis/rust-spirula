# P2-F04: shader and pipeline contract

Status: **已完成** (code tip `2bd98deae866da09a3c14f694d9d2028f8b17f35`).
Owner: `spirula-gpu`. Builds on [gpu-resources.md](gpu-resources.md) (P2-F03).
GPU diagnostic events: P2-F05 (已完成); headless smoke P2-F06 (已完成);
packaging P2-F07 (当前); pipeline cache *optimization* profiling stays P9-F03.
Phase 2 exit remains open.

## Reference behavior (C++)

Pinned: `9a24f09927944a795e7ba1bd8243a625bf419479`
`src/backend/vulkan/VulkanPipelines.{h,cpp}`.

| C++ surface | Behavior recorded | Rust binding in this slice |
|---|---|---|
| Process maps `g_modules` / `g_layouts` / `g_pipelines` | Cache keyed by blob name + push size + specialization tuple (+ optional `\|base`) | Session-owned cache on `GpuContext` generation; key = `(name, version, entry, spec, bind_layout_version)` |
| `SpecList` ≤16 × u32 IDs 0..n-1 | 4-byte specialization constants | WGSL `override` constants via `PipelineCompilationOptions.constants` |
| Push-constant `params` ABI | Byte image of uniform struct as push constants | **Deviation**: explicit 16-byte **uniform buffer** binding (WebGPU has no push constants); same host-visible params layout |
| `dispatch` / sticky error | Bind pipeline, push, dispatch, barrier | Encode bind+dispatch into pending commands; `submit`/`wait` from P2-F03 |
| `pipelines_shutdown` | Destroy all cached objects | `close` / lost / `reopen` invalidate generation cache |
| Embedded SPIR-V registry | Build-time Slang→SPIR-V blobs | WGSL under `crates/spirula-gpu/shaders/<name>.wgsl` with header contract |

Rust continues session-owned `GpuContext` (no process-global pipeline maps).

## Binding decisions

### Shader source contract

- Path: `crates/spirula-gpu/shaders/<name>.wgsl` where `<name>` equals the
  header name (affine proof: `shaders/affine-u32.wgsl`).
- First line: `// spirula-shader: <name> version: <n>` (kebab name, integer version).
- Entry + bind layout are declared by the Rust companion (`BindLayoutVersion` +
  typed bind slots), not discovered by scanning WGSL at runtime.
- Compile failure → `SP-GPU-005` with `path` = `shaders/<name>.wgsl` (must exist
  on disk), `line` (when parseable), and compiler message. No half-valid cache
  entry; no leftover pending reserve.

### Pipeline key

```text
key = (shader_name, shader_version, entry_point, spec_constants, bind_layout_version)
```

Changing **any** field yields a distinct cache slot. Same key must not reuse a
pipeline built from different source bytes (header name/version are part of the
key; source is loaded from the named file / embedded module for that name).

### Cache lifetime

Cache is owned by the live `GpuContext` generation. `close`, device lost, and
`reopen` clear modules/pipelines/layouts. Cross-context or stale-generation
handles are rejected (`SP-INP-001` / `SP-GPU-002`).

### Minimal real compute proof (affine u32)

- Bind layout epochs **v1 and v2** (narrow affine ABI only): identical
  `@group(0)` uniform `Params` (16 bytes: `scale`, `bias`, `count`, `_pad`) +
  storage read `input` + storage read_write `output`. v2 exists so a
  `bind_layout_version` key change is a real cache miss / new pipeline id with
  CPU-exact dispatch on both epochs; other versions → typed reject. Not a
  general layout/reflection system. **Boundary**: v2 is the same binding ABI as
  v1 — epoch invalidation proof only, not a second layout design.
- Kernel: `output[i] = input[i] * scale + bias + MODE` for `i < Params.count`.
- **Count contract**: dispatch `count` is the launch extent (workgroups cover
  `0..count`; buffers must be sized for it). Uniform `Params.count` is the
  shader write guard. CPU / synthetic / host write only `i < Params.count`;
  other launched indices stay unchanged.
- `MODE` is a WGSL `override` (specialization) in the pipeline key; changing
  MODE must change host-visible results with the same buffers/params.
- Workgroup size 64; inputs whose length is **not** a multiple of 64 must still
  round-trip exactly (CPU reference = GPU) for the guarded range.
- Buffer usages extended only as needed: `STORAGE` / `UNIFORM` (+ existing copy).
- Dispatch encodes like P2-F03 copies: pending → submit → wait; drop/budget
  rules unchanged. Alias (input==output, params∩storage), bind sizes vs enabled
  `max_*_buffer_binding_size`, alignments, and dispatch limits are validated in
  Rust before wgpu (typed error, not panic). Params bind as a narrow 16-byte
  range; storage as `count*4` — not entire-buffer binds. Bind/encode/finish run
  under error scope; failures leave no pending command/reserve.
- Pending commands carry optional staging ownership. Compute commands have no
  staging; upload/readback commands own theirs. Abandoning a pending readback
  ticket removes **that** owned command (not by equal-len/index pairing with
  `pending_staging`). `GpuContext::pending_resource_counts` exposes a read-only
  immutable snapshot for tests/diagnose.
- Pipeline resolve is by **handle id**. Same-key source rebuild replaces the
  cache entry only after successful create; old handles are typed-rejected and
  must not execute the new body. Failed same-key replace keeps the prior entry.

### Diagnose

`spirula-gpu-diagnose --compute` runs the affine round-trip and prints cold/warm
pipeline obtain and compute phase timings plus accounted peak/source. Does
**not** claim application/train/UI readiness.

## Acceptance

1. Host: affine upload→dispatch→readback matches CPU reference for non-WG
   multiple lengths; MODE override changes output; `Params.count` guard matches
   CPU/synthetic/host.
2. Key: each of name/version/entry/spec/bind_layout_version change → cache miss
   / new pipeline; repeat same key → hit (observable). Affine supports layout
   epochs v1 and v2 (same ABI); obtain(v1) vs obtain(v2) yields distinct ids and
   both dispatch CPU-exact (host + synthetic). Unsupported layout versions
   typed-reject. Same-key source rebuild yields a new id; old handle is
   typed-rejected (does not run new body); failed replace keeps the prior entry.
3. Illegal alias/bind range/size/alignment/usage/dispatch/binding-limit → typed
   errors (no wgpu panic); accounting unchanged; no half pending on encode fail.
4. Bad WGSL → `SP-GPU-005` with file/line/message; retry with good source works;
   no poisoned half-cache.
5. Close/lost/reopen / cross-context reject old pipeline handles; dispatch after
   drop of buffers still respects pending/inflight reclaim → current 0.
6. Diagnose `--compute` records device/backend and timings when GPU available.

## Tested evidence (approved tip)

Code tip: `2bd98deae866da09a3c14f694d9d2028f8b17f35`. Independent review READY:
`/tmp/spirula-p2-f04-review/REVIEW-final-2bd98de.md`. Commands below are the
reproducible checks; machine `/tmp` logs are sample runs, not the sole evidence.

| Check | Command | Sample result |
|---|---|---|
| Fresh full gate | `cargo run -p spirula-xtask -- verify` | **8/8** ok (sample: `/tmp/spirula-codex-p2-f04-2bd98de-independent-verify.log`) |
| Host affine suite | `cargo test -p spirula-gpu --test host_compute_affine -- --nocapture` | **3/3** on Apple M5 Max / Metal (sample: `/tmp/spirula-codex-p2-f04-2bd98de-host-tests.log`) |
| Diagnose compute | `cargo run -p spirula-gpu --bin spirula-gpu-diagnose -- --compute` | `match=cpu_exact`, `after_drop current=0` (sample: `/tmp/spirula-codex-p2-f04-2bd98de-host-diagnose.log`) |

Representative host diagnose fields (one M5 Max / Metal run; timings vary):

- wgpu 30.0.1; `max_buffer_size=4294967295`, storage buffers/stage=29,
  compute workgroup storage=32768, workgroups/dim=65535, texture 2d=32768
- Proof: `count=100` (non-WG multiple), MODE override, layout v1/v2 distinct ids
- Accounted: `source=accounted_scoped:gpu_context_resources`, peak observed on
  diagnose path 1232, `after_drop current=0`

## Non-claims

- No general shader reflection / all bind types / render pipelines.
- Layout epoch v2 shares the v1 binding ABI (key invalidation only).
- No application/CLI/UI session GPU ownership; real train/preview not wired.
- No surface present smoke (P2-F06).
- No P9-F03 cache performance optimization claims beyond this slice’s cold/warm
  obtain timings.
- Diagnose `--compute` does **not** claim application/train/UI readiness.
