# P3-F02: image orientation and mask

Status: **进行中** (branch `codex/p3-f02`; HOLD candidate after six HOLD
revisions). Owner: `spirula-io` + minimal `spirula-application` entry.
Prerequisite P1-F02 done. Not COLMAP parsers (P3-F03), not training cache
(P3-F08), not UI mask editor (P7-F01).

Pinned C++ reference: `9a24f09927944a795e7ba1bd8243a625bf419479`.

## Inventory (reference)

| Topic | Location | Observed behavior |
|---|---|---|
| EXIF Orientation tag | `src/sfm/core/Exif.h:45,235,408-441` | Values **1..8**; missing → **1**. `exifTransform` maps to `{turns_cw, mirror}` (CW then horizontal mirror). TIFF SHORT — full `u16` must be validated (257/262 are illegal). |
| Read orientations | `src/data/parsers/DatasetCommon.cpp:42-59` | Modes `none` / `orient` / `apply`. Only **`.jpg`/`.jpeg`** are opened; other extensions keep default 1. Empty vector when mode is none or every JPEG is identity-1. |
| Gauge vs pixels | `DatasetParser.h:199-202`; Colmap/Nerf parsers | `orient`: level scene via EXIF up, **pixels untouched**. `apply`: bake turns into pixels; levelling skipped. |
| Pixel bake | `src/core/ImageOrient.h`; `DataManager.cpp:377-382` | Full API: `orient_pixels(..., turns_cw, mirror, dst)` + `oriented_size` swaps W/H when `turns_cw` odd. **DataManager `apply` calls `orient_pixels` with `mirror=false`**, using only `exifTransform(o).turns_cw` — orientations 2/4/5/7 lose the mirror in the train path. Identity bake still requires channels ∈ 1..=4. |
| Size probe | `src/data/ImageProbe.cpp:9-19` | Header size via stb / ImageFile; **no full decode**. Rust: PNG IHDR / JPEG SOF only; `MAX_IMAGE_PIXELS` + `MAX_ENCODED_BYTES` before `load_from_memory`; path reads cap encoded bytes. |
| Mask file | `DataManager.cpp:521-545` | Load as gray (`stbi_load` req_comp=1 → RGB luma, **alpha ignored**); **binarize `!=0` → 1**; apply quarter turns; **nearest** resize to batch HxW; optional invert. |
| Alpha as mask | `DataManager.cpp:554-585`, `605-616`; `ImageProbe.cpp:60` | Alpha channel only (LA/RGBA); L/RGB without alpha → all opaque. **`cpu_resize` area (down) / bilinear (up)** then gate **`>= 128`**. Not nearest. |
| Formats (decode) | DataManager RGB path | PNG/JPEG/BMP/TGA via stb; EXR/TIFF via separate modules. **This tip:** PNG + JPEG only for probe/decode/mask (explicit boundary). Mask boundary EDT remains out of tip (P7 / later). |

## AD-P3-F02-01 — apply mirror policy

Reference **ImageOrient.h** supports full EXIF mirrors. Reference **train
`apply` path** discards `mirror` and only applies `turns_cw`. Rust exposes:

- `exif_transform(orientation)` — full `{turns_cw, mirror}` (Exif.h parity).
- `orient_pixels(..., transform)` — full bake (ImageOrient.h parity).
- `ApplyPolicy::TurnsOnly` — DataManager/`apply` parity (mirror ignored).
- `ApplyPolicy::FullTransform` — bake turns **and** mirror.

Callers that match training load use `TurnsOnly`. Documented; not silent.

## AD-P3-F02-02 — allocation bound

Decodes reject images/masks whose `width*height` exceeds
`MAX_IMAGE_PIXELS` (16_777_216) **before** pixel decode. Encoded buffers /
path reads reject sizes above `MAX_ENCODED_BYTES` (64 MiB). Original files are
never rewritten.

## Application / session entry

`spirula-application` depends on `spirula-io` (allowed architecture edge) and
exposes session-gated helpers on `ApplicationService`:

- `probe_image_file_size`
- `prepare_oriented_rgb_file`
- `prepare_normalized_mask_file`

Idle|Ready only. E2E: `crates/spirula-application/tests/image_ops_e2e.rs`.

## C++ pixel goldens

Generator: `tools/image-orient-golden` (host, pin-archives `ImageOrient.h` +
inlined `cpu_resize` / nearest). Fixture:
`fixtures/images/cpp_orient_mask_golden.json` (schema
`spirula-image-orient-mask-cpp-golden`), registered in `fixtures/manifest.json`.
Consumed by `crates/spirula-io/tests/image_orient_mask.rs` for all 8
orientations Full/TurnsOnly (gray+RGB) plus mask nearest and alpha area/bilinear
holdouts on an asymmetric 3×2 unique-pixel source.

## Capability binding

| Matrix | In this tip |
|---|---|
| C-DATA-10 | JPEG EXIF Orientation 1..8; modes none/orient/apply; oriented size; LE/BE + bad TIFF/IFD rejection |
| C-DATA-08 | Mask NonZero (stbi gray) / nearest; Alpha LA/RGBA + `cpu_resize` ≥128; optional flip |
| C-DATA-07 | Partial: PNG/JPEG header probe + channel-preserving decode for mask/orient only |

## Errors

| Condition | Code |
|---|---|
| Missing / unreadable path | `SP-IO-001` / `SP-IO-003` |
| Unsupported extension for this tip | `SP-FMT-001` |
| Truncated / bad EXIF or image / IFD | `SP-FMT-002` / `SP-IO-005` |
| Orientation not in 1..8 (incl. SHORT 257/262) | `SP-INP-003` |
| Zero / oversize dimensions or encoded bytes | `SP-INP-003` |
| Mask / pixel channel count not 1..=4 | `SP-INP-001` |
| Session not Idle\|Ready for image ops | `SP-SESS-001` |

## Non-claims

- P3-F01 camera tip **not** merged on this branch baseline (main may still hold
  unrelated dirty docs — do not touch).
- No GPU / train / COLMAP / EXR-TIFF / mask boundary EDT / UI editor.
- Format scope remains PNG+JPEG; EDT stays out per plan (not shrunk for READY).
- 74 plan IDs unchanged; feature remains **进行中** until independent READY.
