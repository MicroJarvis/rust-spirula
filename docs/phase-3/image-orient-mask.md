# P3-F02: image orientation and mask

Status: **进行中** (branch `codex/p3-f02`; HOLD candidate). Owner:
`spirula-io` + admitted `spirula-application` session commands. Prerequisite
P1-F02 done. Not COLMAP parsers (P3-F03), not training cache (P3-F08), not UI
mask editor (P7-F01).

Pinned C++ reference: `9a24f09927944a795e7ba1bd8243a625bf419479`.

## Capability scope (matrix)

| Matrix | Delivered in this tip |
|---|---|
| **C-DATA-07** | PNG / JPEG / BMP / TGA / TIFF / EXR decode + header/dimension probe; `DecodedRgb::{U8,U16,F32}`; encoded + pixel budgets before decode |
| **C-DATA-08** | NonZero (stbi gray, alpha ignored); Alpha LA/RGBA + `cpu_resize` ≥128; flip; **boundary offset** (`fraction * sqrt(W*H)` EDT) |
| **C-DATA-09** | 16-bit depth PNG (+ bilinear); RGB normal + `turn_normal_vectors` + bilinear |
| **C-DATA-10** | JPEG EXIF Orientation 1..8; modes; Full/TurnsOnly bake; SHORT `u16` (257/262 rejected); LE/BE + bad IFD |

These rows stay **进行中** until independent READY — not claimed complete by
shrinking scope in docs.

## Inventory (reference)

| Topic | Location | Observed behavior |
|---|---|---|
| RGB decode | `DataManager.cpp:447-512` | stb for PNG/JPEG/BMP/TGA (8/16); custom EXR/TIFF readers |
| Mask NonZero | `DataManager.cpp:521-552` | `stbi_load` req_comp=1; `!=0`; turns; nearest; flip; then EDT offset |
| Alpha mask | `DataManager.cpp:554-614` | Alpha plane; `cpu_resize` then `>=128` |
| Depth | `DataManager.cpp:616-636` | 16-bit PNG; bilinear |
| Normal | `DataManager.cpp:638-656` | RGB; turn pixels; `turn_normal_vectors`; bilinear |
| EDT offset | `DistanceTransform.h`; `apply_boundary_offset` | `fraction * sqrt(W*H)` px dilate/erode |
| EXIF | `Exif.h` | Orientation SHORT 1..8; `exifTransform` |

## AD-P3-F02-01 / AD-P3-F02-02

Unchanged: TurnsOnly vs FullTransform; `MAX_IMAGE_PIXELS` + `MAX_ENCODED_BYTES`
before `load_from_memory` / path read.

## Application / session entry

Admitted commands (wiring not deferred):

- `PrepareOrientedImage`
- `PrepareNormalizedMask` (includes `boundary_offset_frac`)

Events: `ImagePrepared` / `MaskPrepared`. E2E:
`crates/spirula-application/tests/image_ops_e2e.rs`.

## C++ pixel goldens

`tools/image-orient-golden`:

1. `extract_ref.py` pulls **real** pin blobs (`DataManager.cpp` resize/
   `turn_normal`, `Exif.h` transform, `ImageOrient.h`, `DistanceTransform.h`)
   into **gitignored** `.build/`.
2. `main.cpp` only drives those extracted functions — no hand-copied
   `cpu_resize` / EXIF tables in-tree.
3. `bin/` is gitignored.

Fixture: `fixtures/images/cpp_orient_mask_golden.json` (schema version **2**),
consumed by orient/mask/format/offset tests.

## HOLD c0b4c50 counterexamples (must pass)

| Case | Expected |
|---|---|
| RGBA black opaque / red transparent NonZero | `[0,1]` (luma), not alpha `[1,0]` |
| Alpha `[0,255]` 2→1 | bin `1` (area/bilinear → 128) |
| `apply_orientation` identity `channels=0` | `Err` |
| EXIF SHORT `262` | `Err` (not truncate to 6) |

## Errors

| Condition | Code |
|---|---|
| Missing / unreadable path | `SP-IO-001` / `SP-IO-003` |
| Unsupported extension | `SP-FMT-001` |
| Truncated / bad EXIF / IFD / image | `SP-FMT-002` / `SP-IO-005` |
| Orientation / size / encoded / non-finite offset | `SP-INP-003` |
| Channel count / non-binary offset input | `SP-INP-001` |
| Session not Idle\|Ready | `SP-SESS-001` |

## Non-claims

- P3-F01 camera tip not merged on this baseline; main dirty docs untouched.
- Custom C++ EXR/TIFF colour-management edge cases beyond `image` crate decode
  are not claimed bit-identical to `ExrImage.cpp`/`TiffImage.cpp`; fixtures
  cover probe/decode/dtype routing.
- UI mask editor remains P7-F01.
