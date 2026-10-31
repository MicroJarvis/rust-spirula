# P4-F06: readback and CLI preview output

Status: **进行中** (worktree implementation; plan row stays `当前` until
main-thread closure and independent review). Owner split:

| Layer | Owns |
|---|---|
| `spirula-render` | GPU-independent float→byte packing; headless GPU preview facade (context open / compose / retained cleanup) |
| `spirula-io` | Versioned preview request load; PNG encode; atomic publish |
| `spirula-application` | One `Preview` command/worker/session identity; bounded history summary |
| `spirula-cli` | argv / SIGINT / text presentation only |

Not claimed: UI display, appearance/PPISP/bilagrid, train, VJP, depth file
formats, generic job-config frameworks, or Phase 2 surface-present closure.

## Recorded C++ reference (pin `9a24f09927944a795e7ba1bd8243a625bf419479`)

Read-only. Untracked reference `CMakePresets.json` is a user change — preserve;
do not treat as Rust contract.

| Source | Behavior | Rust admission |
|---|---|---|
| `EngineQuery.cpp` `engine_copy_render_to_host` (~33+) | Device→host copy of f16 (expand table) or f32 RGB / depth / T / optional raw; **no** 8-bit encode | Informs that GPU working buffers stay float. Not the product file format. |
| `TrainerCore.cpp` `to_png_bytes` (~1977) | `clamp[0,1]` then `lround(v*255)` → u8; input may already be **appearance / display** space | Reuse **quantize arithmetic only**. Do **not** auto-add gamma/sRGB because of this helper. |
| `webviewer/RenderWorker.cpp` (~243–260) | Premultiplied RGBA: `a=1-T`, RGB `min(rgb,a)`, `to8 = clamp*255+0.5` | **Different export.** Not admitted for product CLI PNG. |
| `gui/Screenshot.cpp` (~378) | Encodes a prior host `rgb8` buffer (3-channel) via stbi | Confirms opaque RGB8 PNG is a valid display path; does not define color transfer. |
| P4-F05 compositing | Working-space linear RGB; `Constant(bg)` → `raw + T*bg` (unclamped); `alpha = 1-T` is splat coverage only | Source pixels for packaging. |

**Declared Rust preview format (this feature):** opaque **RGB8 PNG** of the
**working-space linear composite RGB after Constant background blend**,
quantized with TrainerCore’s clamp+`lround` rule. No second gamma. No
premultiplied RGBA. No packing of `(bg-blended RGB, splat-only alpha)` as
straight RGBA.

## Binding decisions

### Output

- **In-memory frame (render):** row-major RGB8, origin top-left, tightly packed
  `stride = width * 3` bytes, channel order R,G,B. Width/height match the
  composite request (asymmetric / non-square preserved; no transpose).
- **Quantize:** per channel `v' = clamp(v, 0, 1)`; `u8 = round_ties_away_from_zero(v' * 255)`
  (same as C++ `lround` for non-negative). Values already >1 (unclamped SH0 /
  Constant blend) clip to 255.
- **File:** PNG, 8-bit RGB, no alpha channel, no depth sidecar.
- **Alpha policy:** product PNG is **opaque**. Coverage / remaining `T` stay on
  `CompositeImage` for tests and metrics only. Constant background makes the
  displayed RGB include the backdrop; exporting that RGB with splat-only alpha
  as straight RGBA is forbidden.
- **Clear background:** not admitted on the product PNG path (`SP-INP-004` /
  typed reject). Callers that need holes must stay on float `CompositeImage`.
- **Depth:** preserved on `CompositeImage` / internal packaging metadata only;
  no depth image format in this slice.
- **Color space tag:** bytes are **linear working-space quantized**. Viewers
  that assume sRGB ICC will look wrong; that is explicit, not silent gamma.

### Input (versioned request descriptor)

Schema id `spirula-preview-request`, **version 1**. One JSON file; relative
paths resolve against the **request file’s parent directory**.

```json
{
  "schema": "spirula-preview-request",
  "version": 1,
  "splat_ply": "scene.ply",
  "camera": {
    "model": "PINHOLE",
    "width": 3,
    "height": 2,
    "fx": 2.0, "fy": 2.0, "cx": 1.5, "cy": 1.0,
    "c2w": {
      "rotation_row_major": [1,0,0, 0,1,0, 0,0,1],
      "translation": [0, 0, 0]
    }
  },
  "background": [0.1, 0.2, 0.3],
  "thresholds": {
    "alpha_cutoff": 0.01,
    "early_out_transmittance": 0.0001
  },
  "budgets": {
    "max_pixels": 480000,
    "max_host_bytes": 268435456,
    "max_gpu_bytes": 268435456,
    "max_file_bytes": 33554432,
    "max_image_bytes": 12582912
  }
}
```

- Supported domain (same as P4-F05): `3dgs` / SH0 / undistorted pinhole /
  linear RGB via `GpuCompositor`. Distortion, SH>0, MIP/3DGUT, sRGB appearance,
  noise/SH skybox → typed `UnimplementedOption` / `SP-INP-004`. No silent CPU
  production fallback.
- Non-finite / zero dims / overflow / over-budget → typed reject before GPU
  work when detectable.
- Changing splat / camera / background **must** change pixels (not a canned
  probe pattern).

### API shape (stable for independent review tests)

```text
// spirula-io
pub const PREVIEW_REQUEST_SCHEMA: &str = "spirula-preview-request";
pub const PREVIEW_REQUEST_VERSION: u32 = 1;

pub struct PreviewRequestV1 { /* paths resolved, camera, bg, thresholds, budgets */ }
pub fn load_preview_request(path: &Path, cancel: Option<&CancelToken>)
    -> Result<PreviewRequestV1, DomainError>;

pub fn encode_preview_png_rgb8(width: u32, height: u32, rgb8: &[u8], max_image_bytes: u64)
    -> Result<Vec<u8>, DomainError>;
pub fn publish_preview_png(destination: &Path, png_bytes: &[u8], cancel: Option<&CancelToken>)
    -> Result<PublishOutcome, DomainError>;
// postpublish_directory_sync → durability Err with destination already replaced

// spirula-render
pub const PREVIEW_PACK_CONTRACT_VERSION: u32 = 1;

pub struct PreviewRgb8 {
    pub contract_version: u32,
    pub width: u32,
    pub height: u32,
    pub stride_bytes: u32,          // == width * 3
    pub rgb8: Vec<u8>,              // len == stride * height
    pub source_background: BackgroundPolicy, // Constant only for pack success
}

pub fn pack_composite_to_preview_rgb8(image: &CompositeImage) -> Result<PreviewRgb8, DomainError>;
// rejects Clear; does not re-render

pub struct HeadlessPreviewMetrics {
    pub splat_count: u64,
    pub width: u32,
    pub height: u32,
    pub wall_render_ns: u128,       // public facade: open+compose+pack wall
    pub composite: CompositeMetrics,
    pub rgb8_bytes: u64,            // packed RGB8 payload size
    pub host_rgb8_payload_bytes: u64, // alias of rgb8_bytes (not bus-moved)
    pub host_float_plus_rgb8_reserve_bytes: u64, // checked 56*M+3*M before F05
    pub retained_wait_ok: bool,
    // Labels: wall-clock + accounted buffer peaks; not RSS/VRAM/bus counters.
}

// Application PreviewSummary / Event::PreviewPublished add:
//   wall_load_ns, wall_encode_ns, wall_publish_ns, png_bytes
// (host wall-clock stages + PNG payload size).

pub struct HeadlessPreviewResult {
    pub frame: PreviewRgb8,
    pub metrics: HeadlessPreviewMetrics,
}

/// Narrow render-owned facade: opens one GpuContext, runs GpuCompositor,
/// packs RGB8, drops context (retained/timeout cleanup stays GPU-ledger honest).
pub fn run_headless_gpu_preview(
    request: &RenderRequest,
    options: CompositeOptions,
    cancel: &CancelToken,
) -> Result<HeadlessPreviewResult, DomainError>;

// spirula-application
CommandPayload::Preview {
    request_path: String,   // bounded by label_max_bytes
    output_path: String,
}
Event::PreviewPublished {
    path: String,
    width: u32,
    height: u32,
    splat_count: u64,
    rgb8_bytes: u64,            // payload size, not bus-moved
    png_bytes: u64,
    wall_load_ns: u128,
    wall_render_ns: u128,
    wall_encode_ns: u128,
    wall_publish_ns: u128,
    // no image payload in history
}
```

### CLI shape

```text
spirula preview --request <path.json> --out <path.png> [--log-dir <dir>]
```

| Outcome | Exit |
|---|---|
| Success (file published) | 0 |
| In-session / domain failure (incl. post-publish durability) | 1 |
| Usage / unknown flags / missing paths before session | 2 |
| Cooperative cancel **before** publish commit | 130 |

- `preview` advertised **available** (admitted scope above); `train` still
  unavailable.
- Exactly one stderr `error:` line on failure; stable `SP-*` with
  `SPIRULA_ERROR_CODES=1`.
- No test-only flags in the product argv surface.
- Existing help / capabilities / probe / load-dataset semantics unchanged
  except capabilities list gains `preview`.

### Architecture / dependency

- Existing `application → spirula-gpu` (AD-P6-F07-01) remains **presentation-
  only**. Preview must **not** open/business-orchestrate `GpuContext` through
  that edge.
- **New** `application → spirula-render` edge (architecture + xtask policy).
  Application worker calls `run_headless_gpu_preview` only.
- Render facade owns context creation, compose, map/pack, and drop/retained
  cleanup. One private context per preview call is an accepted initial cost;
  cleanup evidence required.
- `wgpu*` types stay inside `spirula-gpu`. CLI stays `core` + `application`.
- Control history / `command_result` retain only bounded path + summary metrics
  — never `CompositeImage` / PNG bytes / full `RenderRequest` clones.

### Cancellation and publish commit

Declared cooperative check boundaries: request IO → PLY load → preflight →
GPU compose cancel hooks → pack → encode → **before** atomic rename. Kernel
map and third-party PNG encode are **not** claimed preemptible mid-call.

| Stage | Cancel / fail | Destination |
|---|---|---|
| Before rename commit | Cancel or Err | Prior file preserved; owned temps cleaned |
| Rename succeeded | Observable new file | Terminal for **Preview** is **Completed** even if sticky Stop arrives after commit; Stop still gets its **own** terminal exactly once |
| Rename ok, parent-dir fsync fails | `SP-IO-004` / `actual=postpublish_directory_sync` | **Published with durability risk** — must not claim “all Err leave old file” |

CPU f64 oracle (`CpuCompositor`) is test-only; production CLI/GPU path never
fills output from it.

### Resource / copy accounting (declared before allocate)

Simultaneous host peak includes (checked, overflow-safe): encoded request text,
PLY decode buffers, `RenderRequest` / scene, upstream F05 composite float
frame, RGB8 pack, PNG encoder buffer, atomic temp. Renderer GPU bytes stay on
the existing GPU ledger (separate from host/RSS/VRAM — RSS/VRAM not claimed).
Filesystem read capped (`max_file_bytes`, independent of the 1 MiB descriptor
hardcap; PLY reads charge Vec **capacity** before each chunk append with
cancel polls). PNG encode uses png 0.18.1 **streaming** `StreamWriter` +
`Compression::Fast` → `fdeflate::Compressor` into a fixed-capacity sink (no
full internal `zlib_encoded: Vec` before the outer cap).

#### Audited deterministic copy / payload labels (not bus counters)

| Label | Source formula | Honesty |
|---|---|---|
| `host_rgb8_payload_bytes` / `rgb8_bytes` | `width×height×3` | Packed RGB8 **payload** size only |
| `host_float_plus_rgb8_reserve_bytes` | `56×M + 3×M` checked before F05 | Simultaneous host **reserve**, not RSS |
| `CompositeMetrics.*_accounted_peak_bytes` | F05 GPU ledger | Accounted buffer peaks; **not** D2H moved bytes |
| `composite_encode_ns` / `composite_sync_ns` | F05 wall stages | Wall-clock stage ns; not GPU timestamps |
| `wall_render_ns` (public facade) | open + compose + pack | Host wall; includes `GpuContext::open` |
| `wall_load_ns` / `wall_encode_ns` / `wall_publish_ns` | application `PreviewSummary` | Host wall stages on CLI publish line |
| `png_bytes` | encoded container length | Payload size of published PNG |
| encode host need | RGB8 + 2×`max_image` + 3×row + 4KiB chunk + fdeflate compressor claim | Product encode path |
| decode scratch (`preview_png_decode_scratch_bytes`) | `max(2×(raw_frame+8KiB),128KiB) + raw_row + 18432 + 16KiB` from png 0.18.1 `UnfilteringBuffer` | **Canonical product PNG only** (IHDR+IDAT+IEND). Lookback is `data_stream`, not fdeflate `output.resize(+32KiB)` |

For the nonempty pressure sample (`N=65`, `P=487500`, `T=7500`,
`M=480000`), the actual buffer-copy requests in the current F03→F04→F05
implementation have the following deterministic sizes. These count requested
buffer ranges and the explicit mapped-byte host copies, not measured memory
bus traffic (Metal may share physical memory):

| Stage | Requested device→staging bytes | Explicit staging→host Vec bytes |
|---|---:|---:|
| F03 projection/scan records + emitted pairs (`64N + 8P`) | 3,904,160 | 3,904,160 |
| F04 keys/ranges + coverage (`16P + 8T + 16M`) | 15,540,000 | 15,540,000 |
| F05 float image (`32M`, 8 f32/status words) | 15,360,000 | 15,360,000 |
| Total | **34,804,160** | **34,804,160** |

Sources are the `readback_buffer` calls in `gpu_tiles.rs`,
`gpu_sort_raster.rs`, `gpu_compositing.rs`, and the `map_readback` host
`view.to_vec()` in `spirula-gpu/src/resource.rs`. F05 then converts the
32-byte GPU records to a 56-byte/pixel f64 `CompositeImage` (26,880,000 B),
and the RGB8 pack writes 1,440,000 B. The encoder reads that RGB8 slice,
writes compressed chunks to its capped sink, then copies the encoded prefix
once to the returned Vec (2,166,591 B in the measured CLI sample). Publish
validation decompresses one 1,440,000-byte RGB8 frame, consumes it without a
second RGB clone, and the atomic writer writes 2,166,591 B to its temporary
file. These counts omit driver/internal compressor movement and filesystem
cache traffic. They explain why output payload alone is not total copy cost.

`decode_preview_png_rgb8` caps compressed **container** bytes and IHDR
extent only. image 0.25.10 uses `set_ignore_text_chunk(false)`, so
decompressed text/ancillary is **not** ignored and is **outside** the product
scratch formula — no total arbitrary-metadata host budget is promised.
Render receives `max_host_bytes − scene Vec capacity` because F05 excludes
the caller scene. Whole-call budgets from request `budgets` with defaults.

**Cancel / late-SIGINT:** session commit arbitration keeps published Preview
`Completed` against late Stop. CLI `finish_preview_committed_success` drains/
shuts down but keeps exit `0` after authoritative Preview Completed; probe/
dataset idle success still uses cancel-override `0→130`. Physical atomic
helper coverage: `spirula-io` publish validate / lease paths; session
arbitration: `application/tests/p4_f06_review.rs` (injected commit). Parent
CLI physical rename→SIGINT regression:
`crates/spirula-cli/tests/p4_f06_review.rs`
(`cli_committed_preview_keeps_success_when_sigint_arrives_before_terminal_observation`).

**800×600 / 65-source CLI pressure (parent evidence, reused):**
Apple M5 Max / Metal, macOS 27.2, dev build, private context per invocation,
driver/pipeline cache temperature unknown. A real CLI subprocess sample took
1.084638958 s including startup/shutdown: load 1.425250 ms, compose/pack
575.956666 ms, encode 75.284791 ms, publish 28.918458 ms. This earlier sample
excluded device-open from the render field; the final public facade includes
open. A final facade pressure run measured open+compose+pack 574.543833 ms,
F05 encode 80.899667 ms and sync/map 3.393291 ms. These separate wall samples
are not performance guarantees or GPU timestamps. PNG size was 2,166,591 B;
independent stdlib zlib+NoFilter decoding checked all 480000 RGB pixels were
127 (65 broad SH0=.5 gray layers with residual T and background below .5).
Reproduction tests: `cargo test -p spirula-render --test p4_f06_metrics --
--nocapture` and `cargo test -p spirula-application --test p4_f06_metrics --
--nocapture`, both with `SPIRULA_REQUIRE_RENDER_GPU=1`. Tiny CLI/full golden
and physical commit tests live in the independent `p4_f06_review.rs` suites.

### Minimal CLI package

Shipped tiny request + PLY under the package allowlist; isolated
`spirula preview --request … --out …` with **no** source checkout / Cargo /
C++ / Python. Manifest / hash / tamper / symlink rules follow P2-F07. Dirty-dev
package alone is not clean-checkout proof; `--from-ref` after audit when a
committed tip is required.

## Acceptance (evidence required)

1. Real headless GPU CLI preview of a shipped tiny fixture; decode PNG;
   independent analytic / multi-pixel checks (incl. asymmetric 3×2); camera /
   scene perturbation changes pixels; bright / clamped / empty-bg /
   unsupported / nonfinite / overflow / resource errors.
2. Shared application: admit → progress → one Preview terminal; Stop/shutdown
   cancel before publish; post-commit Stop cannot rewrite Preview to Canceled;
   failure frees resources and does not corrupt destination; CLI subprocess
   exit mapping.
3. Resource ledger release; end-to-end + stage wall times and copy bytes
   (useful ≥800×600 sample); wall vs GPU-stage labels; no false perf guarantees.
4. Isolated minimal package preview golden/hash/manifest/tamper.
5. `SPIRULA_REQUIRE_RENDER_GPU=1 cargo run -p spirula-xtask -- verify`.

## Non-goals

Appearance chain, UI viewport, training, Phase 2 surface present, depth PNG/
EXR, Clear-background product PNG, webviewer premultiplied RGBA, claiming
EngineQuery float dump as the file format, generic plugin backends.
