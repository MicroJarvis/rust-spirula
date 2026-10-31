# P4-F03: Gaussian projection and tile intersections

Status: **当前**, implementation and complete verify gate passed; awaiting
independent review before the completion/merge transition.
Owner: `spirula-render`; GPU resource/bind/dispatch implementation stays in
`spirula-gpu`. No sorting, rasterization, compositing, image preview, SH>0,
mip/3dgut, antialias compensation, or distorted camera execution is claimed.

## Recorded reference and deliberate differences

Pinned C++ source: `9a24f09927944a795e7ba1bd8243a625bf419479`.
`bash tools/reference-baseline.sh check` passed on 2026-10-31.

- `src/shaders/primitive_3dgs.slang:418-578`: normalize wxyz quat, exp log
  scales, rotate covariance into camera space; project with a perspective
  Jacobian whose z derivatives are clamped to the image plus 15% margins;
  add .3 to covariance diagonal in the non-antialiased path. Centers on or
  behind the camera plane are culled; there is no invented near/far plane.
  Screen depth is ray length; sorting depth is positive axial camera depth.
- The opacity-aware extent is `min(3.33, sqrt(2*log(opacity/cutoff)))`.
  Reference cutoff is 1/255. Rust keeps the P4-F01 explicit request threshold
  (default 1e-4); reference parity requests explicitly use 1/255.
- `src/kernels/projection/ProjectionFwd_kernel.cuh:87-102` clamps/quantizes
  AABBs; `src/core/AabbQuant.cuh` stores conservative floor/ceil 16-bit bounds.
  Rust uses full-precision unquantized bounds. Tests compare their clamped
  floating bounds against actual reference quantized output within one
  quantization step plus the recorded f32 tolerance.
- `src/kernels/tile/IntersectTile.cu:198-201` maps bounds with the pixel-center
  `+0.5` shift, floor minimum and ceil exclusive maximum. Rust uses that tile
  convention and conservative AABB coverage, without the reference ellipse
  strip rejection (`:54-117`). This can emit extra pairs, charged to the same
  explicit pair budget; it cannot silently coarsen or omit AABB intersections.
  An independent exact convex-quadratic/rectangle test checks no ellipse tile
  is missing at the center, all four clipped edges and a clipped corner.
  Measured center: AABB 36 pairs vs ellipse 16, +20 pairs / 160 GPU bytes;
  clipped edges: 18 vs 8, +10 / 80 bytes; corner: 9 vs 1, +8 / 64 bytes.
- `IntersectTile.cu:455-534` widens total reduction before the int32 scan and
  may coarsen. Rust rejects totals above the explicit budget or 2^31-1, with
  no algorithm change. The portable GPU scan is Hillis-Steele ping-pong
  (O(N log N), no subgroups/f16/float atomics); this is a correctness baseline,
  not a performance parity claim against CUB or the C++ backend.

Independent fixture: `crates/spirula-render/tests/data/p4-f03-reference.tsv`,
32 real C++ rows with inputs, quantized AABB, ray/sorting depths, means,
conics and opacity. Existing reference `build/projection_parity` ran `dump`
successfully (8,917,971 floats). Binary SHA256:
`87b5f7bd67616704612c7df8134282dd673be3244dad9c1c34a1e573e7d65fa4`;
dump SHA256:
`1ff12dce567b89ef887b63fb793cf00a11a1d9d1972dad25da28d06e86b02269`.
The binary was **not rebuilt** here; clean pinned source and an existing
executable's output are distinct provenance claims. Fixture generation details
and the exact existing-binary command are in the TSV comments. No C++/CUDA/
Vulkan runtime dependency is introduced into Rust.

## Execution and ABI

- `RenderRequest::validate` runs at CPU/GPU entry even after public fields
  were mutated. Only linear-RGB, degree-0 3dgs and undistorted pinhole are
  currently executable; nonzero distortion is `SP-INP-004`, never ignored.
  `PinholeCamera::distortion` is the minimal core accessor for that check.
  Projection requires positive finite cutoff; finite f64 inputs that cannot
  be represented in f32 are typed-rejected on the GPU path.
- `CpuTileBuilder::build` is f64 and culls invisible sources individually.
  Empty input has a separate `build_empty` entry, with no fake dummy Gaussian.
- `GpuTileBuilder::build` / `build_cancellable` run real WGSL projection,
  per-source counts, inclusive scan, total readback/host preflight, GPU pair
  emission, then final readback. Host code does not compute the GPU counts.
  Empty GPU entry makes zero dispatches. A synthetic context cannot produce
  GPU evidence. Devices beyond storage/dispatch limits return `SP-GPU-001`;
  no implicit CPU fallback changes the execution path.
- All shader operations guard non-multiple workgroups (64 threads). A projected
  record is 16 little-endian words: pixel xy, axial depth, ray depth, radii xy,
  covariance xx/xy/yy, opacity, tile bounds xmin/ymin/xmax/ymax, inclusive count,
  status (0 culled, 1 visible, 2 invalid). Float values use f32 bit patterns.
  Nonfinite covariance/determinant/depth is rejected, not published.
  GPU ray length is clamped up to axial depth against downward f32 roundoff.
- Camera header is 32 f32 words (view rotation 9, world camera translation 3,
  intrinsics 4, width/height/tile/cutoff, padding); splats follow in P4-F02's
  64-byte layout. This initial slice packs a combined input buffer; it does
  not claim zero-copy binding of a preexisting `GpuSplatBuffer`.
- Every source has an optional projected record and a contiguous pair range.
  Inclusive offsets preserve the original source indices, including culls.
  Pairs are source-major, then row-major tiles, with no depth sort. A pair is
  two u32 GPU words (tile id, source index); host uses usize fields.
  `TileBins::validate` checks dimensions, SPD/finite geometry, bounds, source
  ids, exact source counts, scan offsets and every emitted tile/source pair.
- `dispatch_projection_tiles` accepts only the v1 shader family and named
  project/scan/emit entries. It checks context/generation, aliasing, usage,
  byte ranges and enabled limits through opaque handles; wgpu types stay in
  GPU. It reuses the v1 uniform16 + storage-read + storage-readwrite binding
  ABI/cache proven by P2-F04; algorithm WGSL belongs to render.

## Budgets, failure and cancellation

`TileBuildOptions` limits tile dimensions (image sides <=65535), pair count,
owned host payload estimate and accounted GPU buffers/staging. Checked u64
CPU arithmetic rejects pair totals before allocating pairs. GPU counts/scan
saturate at 2^31 as an explicit rejected overflow marker; total is read back
and validated before allocating/emitting pair storage. The signed overflow
regression uses 65535² pixels / tile1 with one and three full-screen splats:
true GPU count/scan reject >2^31 intersections while accounted peak stays
below 4096 bytes, instead of attempting a multi-gigabyte allocation.

Host preflight is `256*N + 32*P + 4096`, a conservative bound on owned Vec
payloads (including packing/readback/metadata); it is **not process RSS**,
allocator metadata or a bound on caller-owned scene memory. GPU peak bounds:

- projection/scan/count readback: `320*N + 256 + 32*(scan_steps+1)`;
- emission/readback: `64*N + 16*P + 32`.

Both are checked against the per-call cap, separately from existing context
current bytes and its global budget. Buffers, upload staging, readback staging,
and all live scan uniforms are covered. Pipeline cache / driver memory and
whole-device VRAM peak are unavailable; accounted bytes are never called VRAM.

The synchronous call requires an empty unsubmitted context queue; it rejects
another owner's pending work unchanged. Exclusive `&mut GpuContext` then makes
failure cleanup local to this call. Accepted work is never abandoned unsafely.
Wait has a 30-second deadline; a timeout retains accepted resources until the
context owner explicitly invokes `wait_retained_submissions(total_timeout)`
or closes the context. That recovery waits all accepted work on the context,
never submits or clears its pending queue, and preserves original typed lost/
post-submit scope errors. Tests cover timeout then retry/reap to zero bytes,
multiple accepted submissions with latched OOM, separate pending ownership,
and device loss. Cancellation is checked before allocation, between scan
encodes and after each sync; no partial `TileBins` is returned. There is no
GPU preemption during an accepted dispatch/wait.

## Evidence and reproduction

Numerical reference gate:

```text
SPIRULA_REQUIRE_RENDER_GPU=1 cargo test -p spirula-render --tests -- --nocapture
cargo test -p spirula-gpu --test diagnostics_lifecycle_synthetic retained_owner -- --nocapture
cargo run -p spirula-xtask -- verify
```

GPU env is mandatory for the first command: absence fails instead of skipping.
Ordinary workspace tests print an exact open failure and skip host-only proof
when no adapter exists. CPU f64 and GPU f32 comparisons use actual reference
output with absolute 5e-3 plus relative 5e-4 tolerance; per-source cull decisions,
scan offsets and pair lists are exact on these fixtures. Tests additionally
cover public-field corruption, axis depth roundoff, non-block lengths, all
four screen-side culls, on/behind plane, empty output, exact budget rejection,
nonfinite geometry, pending ownership and explicit overflow.

Recorded environment: macOS arm64, Apple M5 Max / Metal, wgpu 30.0.1. Sample
single-run wall timings include queue/driver/readback/map costs; encode time is
host encoding/upload after pipeline obtain, not GPU timestamps. No warm/cold
performance target or C++ speedup is claimed:

| sample | sources | intersections | scan scratch | accounted peak bound | observed accounted peak | encode | count sync | emission + sync |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| real C++ fixture | 32 | 844 | 4096 B | 15584 B | <= bound | 2.52 ms | 6.26 ms | .96 ms |
| non-block | 65 | 1040 | 8320 B | 21312 B | <= bound | 2.87 ms | 3.46 ms | .80 ms |
| full-screen 64x48 | 257 | 12336 | 32896 B | 213856 B | <= bound | 1.25 ms | 1.60 ms | 1.50 ms |
| full-screen 800x600 | 65 | 487500 | 8320 B | 7804192 B | 7804192 B | 1.15 ms | 3.99 ms | 26.29 ms |

Large pressure rejects a 487499-pair cap after count/scan, with <100000 bytes
of cumulative accounted allocation and no pair-buffer allocation. Success
returns current accounted bytes and retained submission count to zero.
GPU total device/driver peak and GPU timestamp timings remain unmeasured.
Full gate on 2026-10-31: `SPIRULA_REQUIRE_RENDER_GPU=1 cargo run -p
spirula-xtask -- verify` passed **8/8 in 22.5 s** (workspace tests 20.9 s,
clippy .9 s). This includes fixture/doc/policy/fmt/check/test/clippy/diff.
Focused host render tests: 18/18 (7 unit, 3 real C++/GPU/axis, 8 tile/resource
cases); retained-owner synthetic recovery: 2/2. An earlier first run failed
only the Rust 1.99 `chunks_exact_to_as_chunks` lint; the parser now uses
fixed-array chunks and the final full gate passes. A subsequent document
synchronization gate passed 8/8 in 20.6 s. Independent review is complete;
P4-F04 owns the next sorting/rasterization slice.
