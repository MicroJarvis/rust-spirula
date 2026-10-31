# P4-F05: compositing and depth

Status: **已完成** (reviewed and merged; current feature is P4-F06). Owner: `spirula-render`; opaque compute binding/dispatch is
`spirula-gpu`. Full image file / CLI preview stays P4-F06. Training forward
saved state stays P4-F07. VJP stays P4-F08+.

## Recorded C++ behavior before implementation

Pinned source `9a24f09927944a795e7ba1bd8243a625bf419479`:

- `Primitive3DGS.cuh:252-261`: pixel-center conic
  `alpha = min(0.999, opacity*exp(-sigma))`; `sigma < 0 → 0`.
- `Primitive3DGS.cuh:290-294`: `evaluate_color` returns SH-evaluated
  `screen.rgb` and **ray** depth (`screen.depth`), not sorting axial depth.
- SH0 DC → linear RGB is **not** `GaussianSplat::base_color` (that helper has
  no clamp). C++ truth is `harmonics.slang:1056-1067` /
  `generated/harmonics.cuh:133-135` `sh0_to_color`:
  `max(SH_C0 * coeff_dc + 0.5, 0)` — **lower clamp to 0 only**; channels may
  exceed 1. Rust f64 uses `spirula_core::SH_C0 = 0.28209479177387814`; the
  generated CUDA f32 literal is `0.282094806432724` (same value rounded).
- `RasterizationEval3DFwd_kernel.cuh:145-154`: `T = 1`, zero accumulators;
  median virtual previous `(z = 0, T = 1)`; median stays `0` if transmittance
  never crosses `1/2`.
- `:247-248`: skip when `alpha <= cutoff` (equality excluded).
- `:258-274`: `next_T = T * (1 - alpha)`. Median crossing is evaluated
  **before** early-out: `prev_T >= 0.5 && next_T < 0.5`, then
  `f = (ln(0.5) - ln(prev_T)) / (ln(next_T) - ln(prev_T))` with
  `z = prev_z + (ray_z - prev_z) * f`; then update median previous to
  `(ray_z, next_T)` even when the splat will not accumulate.
- `:275-296`: accumulate **only if** `next_T > early_out` (C++ fixed
  `1e-4`). Then `vis = alpha * T`, `RGB += color * vis`,
  `depth_acc += ray_depth * vis`, `T = next_T`. Else set `done` and **do not**
  accumulate the terminating splat; keep the old `T`. This is not
  “accumulate then early-out”.
- `:315-322` (non-distortion): expected arithmetic ray depth is **unconditional**
  `depth_acc / max(1 - T, 1e-10)`. There is **no** `weight == 0 → clear 0`
  shortcut: when `T` rounds to 1 but `vis` left a tiny `depth_acc` (e.g.
  `2e-30`), the result is `≈2e-20`, not zero. Truly empty pixels still yield
  `0` because `depth_acc` is zero. Distortion log-depth is unsupported here.
- `pixel_wise.slang:54-56` / `EngineForward.cpp:478+`: after raw render,
  constant background is `rgb + T * background` (unclamped). Appearance /
  PPISP / bilagrid / color-space conversion are **not** in this feature.
  Noise and SH skybox backgrounds are unsupported and typed-rejected.

Rust generalizes the fixed C++ `1e-4` early-out via
`RenderThresholds::early_out_transmittance` (finite, in `(0,1]`). The
comparison remains strict `next_T > early_out`. Non-default values must be
honored; they are not silently replaced by `1e-4`.

## Scope

Executable path (same as P4-F01–F04): degree-0 `3dgs`, undistorted pinhole,
linear RGB. Typed reject (`UnimplementedOption` / `SP-INP-004` class): SH>0,
mip/3dgut, distorted cameras, sRGB / appearance chain, depth-distortion mode,
noise/SH skybox backgrounds.

**Not claimed:** CLI/image file packaging (P4-F06), training forward state
(P4-F07), VJP (P4-F08+), UI, or inventing zero-copy of a preexisting
`GpuSplatBuffer` into the composite kernel.

## Owner, API and association

- `CpuCompositor`: f64 oracle. Walks sorted front-to-back tile pairs, evaluates
  conic alpha, accumulates RGB / T / expected ray depth / median depth, then
  applies the declared background policy.
- `GpuCompositor`: real WGSL composite kernel. Host packs inputs and
  readbacks; host code must not fill GPU RGB/T/depth results from the CPU
  oracle. Timings are wall-clock encode/sync around submit/wait/map — not GPU
  timestamps or process VRAM peaks.
- **Public API is request-only**: re-validate mutable `RenderRequest`, then
  chain projection → sort/raster → composite internally (CPU or GPU). A
  prepared `(bins, sorted, colors)` path stays **crate-private** so dims /
  threshold checks alone cannot bind a foreign scene's colors to unrelated
  bins.
- Empty / clear:
  - `CpuCompositor::compose_empty(width, height, alpha_cutoff, early_out, options)`
    is an explicit **host** clear (typed-rejects illegal `tile_size`; no
    `max(1)` swallow). It is **not** a GPU clear substitute.
  - `GpuCompositor::compose_empty(...)` runs a real `composite` dispatch that
    **writes every pixel** (`rgb=bg`, `T=1`, depths 0). Uncovered candidates
    on non-empty scenes are likewise shader-written; success never relies on
    zero-initialized output buffers.
- Alpha cutoff follows P4-F04: requested f64 metadata preserved; GPU compares
  with `effective = requested as f32`. Raster/composite cutoff must be
  `>= projection_alpha_cutoff`.
- f64→f32 pack rejects nonzero underflow to 0; exact 0 (including SH0 lower
  clamp to 0) is admitted. Scene DC / background / cutoff / early-out are
  f32-preflighted **before** F03/F04 allocation on the GPU public entry.
- Output pixels are finite-checked (CPU accum and GPU decode); non-finite or
  out-of-range `T`/depth fails typed — never published as success.
- `max_pairs` limits pairs only; it is **not** an undeclared source-count cap.

### Background and alpha semantics

| Policy | RGB | Transmittance `T` | Alpha (`1-T`) | Depths |
|---|---|---|---|---|
| `Clear` (default) | raw front-to-back accum (`Σ color·vis`) | remaining `T` | coverage | expected + median as below |
| `Constant(rgb)` | `raw + T * rgb` (unclamped) | unchanged remaining `T` | coverage of splats only | unchanged; bg does not write depth |

Constant black equals raw RGB with holes where `T > 0`. Premultiplied export
relative to an opaque backdrop is `raw + T * bg` with effective opacity 1 for
the backdrop; this feature still reports remaining `T` so callers can see
coverage. Noise / SH skybox are out of scope.

### Public types and method signatures

```text
pub const COMPOSITE_CONTRACT_VERSION: u32 = 1;

pub fn sh0_to_color(dc: [f64; 3]) -> Result<[f64; 3], DomainError>;
// max(SH_C0 * dc + 0.5, 0) per channel

pub enum BackgroundPolicy { Clear, Constant([f64; 3]) }

pub struct CompositeOptions {
    pub max_pairs: u64,
    pub max_tiles: u64,
    pub max_pixels: u64,
    pub max_host_bytes: u64,
    pub max_gpu_bytes: u64,
    pub tile: TileBuildOptions,
    pub sort: SortRasterOptions,
    pub background: BackgroundPolicy,   // default Clear
}

pub struct CompositePixel {
    pub rgb: [f64; 3],
    pub transmittance: f64,   // remaining T
    pub alpha: f64,           // 1 - T (coverage; bg does not force 1)
    pub expected_depth: f64,  // arithmetic ray depth; 0 if uncovered
    pub median_depth: f64,    // log-T half cross; virtual prev (z=0,T=1);
                              // strict next_T < 0.5; 0 if never crossed
}

pub struct CompositeImage {
    pub contract_version: u32,
    pub width: u32,
    pub height: u32,
    pub alpha_cutoff: f64,
    pub effective_alpha_cutoff: f32,
    pub early_out_transmittance: f64,
    pub effective_early_out: f32,
    pub background: BackgroundPolicy,
    pub pixels: Vec<CompositePixel>,  // row-major width*height
}

impl CpuCompositor {
    pub fn compose(request: &RenderRequest, options: CompositeOptions)
        -> Result<CompositeImage, DomainError>;
    pub fn compose_empty(
        width: u32, height: u32,
        alpha_cutoff: f64, early_out_transmittance: f64,
        options: CompositeOptions,
    ) -> Result<CompositeImage, DomainError>;  // host clear only
}

impl GpuCompositor {
    pub fn compose(ctx: &mut GpuContext, request: &RenderRequest, options: CompositeOptions)
        -> Result<(CompositeImage, CompositeMetrics), DomainError>;
    pub fn compose_cancellable(
        ctx: &mut GpuContext, request: &RenderRequest,
        options: CompositeOptions, cancel: &CancelToken,
    ) -> Result<(CompositeImage, CompositeMetrics), DomainError>;
    pub fn compose_empty(
        ctx: &mut GpuContext, width: u32, height: u32,
        alpha_cutoff: f64, early_out_transmittance: f64,
        options: CompositeOptions,
    ) -> Result<(CompositeImage, CompositeMetrics), DomainError>;
    // real WGSL write of every pixel — not host CPU fill
}
```

Clear / uncovered with `BackgroundPolicy::Clear`: `rgb=0`, `T=1`, `alpha=0`,
depths `0`. With `Constant(bg)` and no coverage: `rgb=bg`, `T=1`, `alpha=0`,
depths `0`.

## GPU family and ABI

- New shader family `compositing` version 1, bind layout v1, reusing the
  private three-buffer ABI (`uniform Params` + storage read + storage
  read_write). Entry: `composite`.
- `dispatch_compositing` admits only that family/entry/version/layout.
  Guards mirror sort-raster: aliasing, zero/oversize ranges, `invocations=0`,
  cross-context, closed, reopen, pending-queue owner work, cancel,
  partial failure / inject OOM, deterministic `inject_lost` → exact
  `DeviceLost`, timeout-accepted resources retained until
  `wait_retained_submissions`.
- `wgpu*` types stay inside `spirula-gpu`. No new CUDA / direct Vulkan /
  Python runtime path. Minimal new dependencies.

### Buffer formulas (accounted; not VRAM)

| Symbol | Meaning |
|---|---|
| `N` | source count |
| `P` | sorted pair count |
| `T` | tile count |
| `M` | pixel count `width * height` |

Host **stage peak** (colors deferred until after F04; entry only allocation-free
SH0/f32 preflight — no early `24N` Vec). Color table is built with
`Vec::with_capacity(N)` + push so `capacity == len == N` (not
`Result<Vec>::collect` growth).

- `during_f03 = 256N + 32P + 4096` (F03 `host_preflight`)
- `during_sort = 2×bins_len + (80P+128N+24T+48M+8192)` (no color Vec yet;
  `2×` covers Vec capacity growth vs len for bins)
- `during_comp = 2×bins_len + sorted_owned + 24N + 2×pack + 32M + 56M + 8192`
  (`pack = 64+64N+16P+8T`; colors `24N`; `CompositePixel` is 56 B;
  mid-call uses actual `bins` Vec **capacity**)

`host_peak = max(during_f03, during_sort, during_comp)`. Excludes caller
scene/`RenderRequest`, allocator metadata, driver heap, RSS. Nested sort
`max_host` deducts actual `bins` Vec capacity before the sort stage. Source
count is **not** a pair budget: `N > max_pairs` is allowed when realized `P`
fits.

GPU composite pack bytes: `64 + 64N + 16P + 8T`. Output staging: `32 * M`.
Accounted GPU composite peak: `2*input + 2*output_staging + 4096`. Whole-call
GPU peak is the **max** of stage peaks (stages release before the next), not
the sum. Excludes driver heap, pipeline cache, process RSS, GPU timestamps.
Focused admission tests exercise `dispatch_compositing` family/layout/version/
entry/range/alias/cross-context/reopen; `compose_empty` injects OOM/timeout so
the first submit/error scope is the F05 composite stage (empty tile build does
not submit).

## Numerical tolerance

- Independent golden values (hand-derived or fixed fixture) for clear,
  single splat, two-layer color/depth, equal-depth stable tie, alpha cutoff
  equality skip, 0.999 clamp, early-out boundary (terminating splat not
  accumulated), background blend, expected ray vs axial depth, median
  half-crossing with virtual `z=0`, and unsupported/non-finite inputs.
- At least one small multi-pixel image golden — not only CPU↔GPU mutual
  agreement.
- CPU f64 ↔ GPU f32 sample absolute tolerances are measured on the exercised
  Metal/Apple fixtures and recorded beside those tests. They are **not** a
  global error bound. Threshold-neighborhood flips near cutoff follow the
  P4-F04 geometry/cutoff divergence note.
- Representative pressure reuses 800×600 / 65 fullscreen / 487500 pairs when
  feasible; if early-out limits contributing splats, say so. Chain real GPU
  F03→F04→F05 against the CPU oracle.

## Budgets, failure and cancellation

`CompositeOptions` limits pixels, pairs, tiles, host payload estimate and
accounted GPU buffers/staging. Entry rejects when the known F05 GPU lower
bound `2*(64+64N+8T)+64M+4096` (`P=0`) exceeds `max_gpu_bytes` / context
budget (`AllocationFailed`) — before F03/F04 allocation. After realized `P`,
the exact F05 peak is rechecked before F04. In the composite stage, known
byte sizes, budget, and device binding/workgroup limits run **before**
`pack_input` (still before GPU alloc). Pack failures (f64→f32 overflow /
underflow / non-finite) therefore never build a knowingly rejected pack.

Exclusive `&mut GpuContext`: unrelated pending owner work is rejected and
left intact. Cancel checks at documented boundaries. Accepted submissions on
timeout remain context-owned until `wait_retained_submissions`. Partial
failure abandons unsubmitted pending. Error codes stay precise — do not widen
to multiple unrelated codes to keep tests green.

## Verification

Focused render compositing tests + full render suite +
`SPIRULA_REQUIRE_RENDER_GPU=1 cargo run -p spirula-xtask -- verify` +
`git diff --check`. Document measured adapter, tolerances, accounted peaks
and any honest gaps. P4-F05 independently reviewed and merged (`74c537e` / `9ea81be`); GPU-required
verify passed 8/8 (29.1 s). Detailed 54-test counts, measured stage timings,
resource bounds and exclusions are recorded in the [phase README](README.md).
Appearance, CLI preview and training remain separate pending capabilities.
