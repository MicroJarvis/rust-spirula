# P4-F04: stable depth sorting and raster coverage

Status: **当前**, implementation under independent review; no completion or
merge claim. Owner: `spirula-render`; opaque compute binding/dispatch is
`spirula-gpu`. Color/transmittance/depth compositing stays P4-F05; CLI preview
stays P4-F06.

## Recorded C++ behavior before implementation

Pinned source `9a24f09927944a795e7ba1bd8243a625bf419479`:

- `src/kernels/tile/IntersectTile.cu:228-235`: positive absolute f32 depth is
  bitcast and XOR `0x80000000`; tile id is the high u32 of a 64-bit key. WGSL
  keeps low/high words separate (no u64 requirement).
- `IntersectTile.cu:568-577`: stable CUB `SortPairs` and selected double-buffer
  result. Equal tile/depth keys preserve P4-F03 source-major emission order
  (ascending source / input index). CPU oracles must sort the same canonical
  f32 keys, including distinct f64 depths that round to one f32 key.
- `Primitive3DGS.cuh:252-261`: `delta = mean - pixel`,
  `sigma = 0.5*(conic.x*dx² + conic.z*dy²) + conic.y*dx*dy` with
  `conic = (yy/det, -xy/det, xx/det)`; `sigma < 0 → 0`;
  `alpha = min(0.999, opacity*exp(-sigma))`.
- `RasterizationEval3DFwd_kernel.cuh:247`: skip when `alpha <= cutoff`
  (equality excluded). Pixel sample is `(col+0.5, row+0.5)` with image bounds.
- Transmittance, early-out, color accumulation and depth output are **not**
  claimed here.

## Execution and ABI

- `TileBins` contract is **v2**: adds `projection_alpha_cutoff`. Empty bins may
  use `0.0` as a sentinel; non-empty bins reject cutoff `0`. **v1** was a
  non-persistent internal buffer without this field and is typed-rejected
  (`UnsupportedVersion`) with an explicit compatibility note. See also
  [projection-tiles.md](projection-tiles.md).
- Raster cutoff must be `>= projection_alpha_cutoff`. A lower raster cutoff is
  rejected so P4-F03's opacity-aware 3.33 extent truncation cannot silently
  omit support that a looser raster threshold would expect. Coverage is only
  evaluated for existing candidate pairs; tails are not restored.
- **Cutoff metadata**: `SortedRaster.alpha_cutoff` is always the **requested**
  f64 (CPU, GPU, empty, and non-empty). `effective_alpha_cutoff` is
  `requested as f32`, the value GPU coverage compares use after pack. CPU
  coverage compares use the f64 request. Returning `f32 as f64` in place of the
  request is forbidden.
- CPU path (`CpuSortRaster`) is an f64 oracle that sorts canonical f32 keys
  with a stable `(tile_id, depth_key)` order and evaluates conic coverage.
  `coverage_alpha` is `pub(crate)` only (rejects non-finite / out-of-range
  opacity so NaN cannot become 0.999 via `f64::min`).
- GPU path (`GpuSortRaster`) packs inputs and runs real WGSL: `keys`, then
  sixteen 4-bit LSD rounds (`histogram` → exclusive `prefix` → stable
  `scatter`), then `ranges`, then `raster`. Host code does not sort pairs for
  the GPU result. Empty pair input makes zero dispatches.
- `dispatch_sort_raster` admits only the `sort-raster` v1 family and entries
  `keys|histogram|prefix|scatter|ranges|raster` with bind layout v1, reusing
  the private three-buffer ABI. Tests cover family/entry/version/layout guards,
  aliasing, zero and oversize storage ranges, `invocations=0`, cross-context,
  closed, and reopen (new live generation rejects prior-generation handles).
  Build-path lost uses the deterministic `inject_lost` hook and asserts
  `DeviceLost` exactly. `wgpu*` types stay inside `spirula-gpu`.
- f64→f32 packing typed-rejects cutoff/opacity/covariance/pixel/radius
  overflow, underflow to zero, non-finite values, and non-SPD after f32 round
  (e.g. cov `[1, 1-1e-9, 1]`). Pack failures happen before GPU buffer
  allocation (accounted `allocated_bytes` unchanged).

### Symbols and buffer formulas

| Symbol | Meaning |
|---|---|
| `N` | source count (`bins.projected.len()`) |
| `P` | pair count (`bins.pairs.len()`) |
| `T` | tile count (`tiles_x * tiles_y`) |
| `M` | pixel count (`width * height`) |
| `groups` | `ceil(P / 64)` |

Host payload preflight (owned Vec estimate, **not** RSS):

`80*P + 128*N + 24*T + 48*M + 8192`

GPU input bytes: `32 + 64*N + 8*P`  
GPU state bytes: `32 + 16*P + 64*groups + 8*T + 64*N`  
Sort readback: `16*P + 8*T`  
Raster output: `16*M`

Accounted GPU peaks (buffers + upload/readback staging; **not** VRAM,
driver heap, pipeline cache, or process RSS):

- sort phase: `2*input + 2*state + sort_readback + 4096`
- raster phase: `state + 32*M + 256`

Wall timings: `sort_encode_ns` (host encode/upload after pipeline obtain),
`sort_sync_ns` / `raster_sync_ns` (submit/wait/map). **Not** GPU timestamps.

### Numerical tolerance and threshold neighborhood

- Pair lists and tile ranges: exact CPU↔GPU match on the exercised fixtures.
- `max_alpha`: absolute ~2e-5 is a **sample** tolerance used by the focused
  CPU f64 ↔ GPU f32 fixtures on this Metal/Apple M5 Max path. It is **not** a
  global error bound for arbitrary inputs, resolutions, or platforms.
- Threshold / count / `first_source` disagreement can still occur even when
  `requested` is an exact f32 value. Causes include, at least:
  - cutoff path: CPU compares `alpha > requested` (f64) while GPU compares
    `alpha > effective_f32` when the request is not f32-exact;
  - geometry path: f32 packing of means/covariance/opacity and WGSL
    conic/`exp` rounding move per-splat alphas relative to the f64 oracle, so
    alphas near the cutoff can flip inclusion independently of cutoff
    rounding.
  Divergence must not be attributed only to cutoff quantization.
- Representative pressure uses exact f32 cutoff `1/256` above the projection
  cutoff and chains **GpuTileBuilder → GpuSortRaster(gpu_tiles)** against the
  CPU tile+sort reference. Default projection cutoff `1e-4` may still be used
  as the raster request (requested f64 metadata preserved; f32 pack must not
  fail validate).

## Budgets, failure and cancellation

`SortRasterOptions` limits pairs, tiles, pixels, raster tests, host payload
estimate and accounted GPU buffers/staging. Workload and device storage /
workgroup limits are checked before allocation.

The synchronous call requires an empty unsubmitted context queue and rejects
another owner's pending work unchanged. Exclusive `&mut GpuContext` keeps
failure cleanup local. Wait uses a 30 s deadline; timeout retains accepted
resources until `wait_retained_submissions` or context close. Cancellation is
checked before allocation and between radix rounds / after sync; no partial
`SortedRaster` is returned. Explicit tests cover cancel, budget OOM,
deterministic `inject_next_scope_error(OutOfMemory)` (not a real allocator
fault), pending ownership, timeout reclaim, closed context, and deterministic
`inject_lost` → exact `DeviceLost` (not a claimed host/driver device fault).

## Evidence and reproduction

```text
SPIRULA_REQUIRE_RENDER_GPU=1 cargo test -p spirula-render --test p4_f04_sort_raster -- --nocapture
SPIRULA_REQUIRE_RENDER_GPU=1 cargo test -p spirula-render --test p4_f04_review -- --nocapture
SPIRULA_REQUIRE_RENDER_GPU=1 cargo test -p spirula-render --tests -- --nocapture
SPIRULA_REQUIRE_RENDER_GPU=1 cargo run -p spirula-xtask -- verify
```

Covered cases: empty input; non-multiple-of-64 pair counts; duplicate keys;
f64-distinct / f32-identical stable ties; partial-tile **last** pixel coverage;
**800×600 / 65 sources / 487500 pairs / 31.2M raster tests** pressure chaining
`GpuTileBuilder → GpuSortRaster(gpu_tiles)` against `CpuTileBuilder →
CpuSortRaster`, with scratch, accounted peak vs bound, phased wall times,
current/retained=0;
cutoff equality excluded; v1 / cutoff-mismatch / f32 overflow / f32 SPD
degeneration / underflow rejects before allocation; dispatch family / entry /
version / layout / alias / range / cross-context / closed / reopen-stale;
build cancel / budget OOM / injected scope OOM / pending / timeout reclaim /
closed / `inject_lost`.

Independent review fixture: `tests/p4_f04_review.rs` (includes 257-source
full normal-f32 exponent-range key monotonicity without a CPU-sort oracle).

Recorded environment: macOS arm64, Apple M5 Max / Metal, wgpu 30. Sample
pressure from `/tmp/p4-f04-pressure.log` (single debug-test run of the real
GPU F03→F04 chain; wall clocks include queue/driver/readback/map; **not** GPU
timestamps; accounted = buffers+staging only). This was a fresh `GpuContext`'s
first sort-family call; driver/pipeline cache state is unknown — **not** a
formal performance benchmark:

```text
pairs=487500 tests=31200000 scratch=16575104 ranges=60000 raster_out=7680000
bound=32375968 observed_accounted_peak=32373472
cpu_build=1.182284041s encode=10.863625ms sort_sync=171.139209ms
raster_sync=1.9055ms
current_bytes=0 retained_submissions=0 after return
```

Final local gates before this feature commit (code review passed; completion /
merge still owned by the main thread): GPU
`SPIRULA_REQUIRE_RENDER_GPU=1 cargo run -p spirula-xtask -- verify` **8/8 in
23.2 s**; `spirula-render` tests **30/30**; independent
`p4_f04_review` **4/4**. Status remains **当前**.
