# P3-F02: image orientation and mask

Status: **进行中** (branch `codex/p3-f02` from main `dfb17ab`; **P3-F01 not
merged** — do not claim camera tip complete on this line). Owner:
`spirula-io`. Prerequisite P1-F02 done. Not COLMAP parsers (P3-F03), not
training cache (P3-F08), not UI mask editor (P7-F01).

Pinned C++ reference: `9a24f09927944a795e7ba1bd8243a625bf419479`.

## Inventory (reference)

| Topic | Location | Observed behavior |
|---|---|---|
| EXIF Orientation tag | `src/sfm/core/Exif.h:45,235,408-441` | Values **1..8**; missing → **1**. `exifTransform` maps to `{turns_cw, mirror}` (CW then horizontal mirror). |
| Read orientations | `src/data/parsers/DatasetCommon.cpp:42-59` | Modes `none` / `orient` / `apply`. Only **`.jpg`/`.jpeg`** are opened; other extensions keep default 1. Empty vector when mode is none or every JPEG is identity-1. |
| Gauge vs pixels | `DatasetParser.h:199-202`; Colmap/Nerf parsers | `orient`: level scene via EXIF up, **pixels untouched**. `apply`: bake turns into pixels; levelling skipped. |
| Pixel bake | `src/core/ImageOrient.h`; `DataManager.cpp:377-382` | Full API: `orient_pixels(..., turns_cw, mirror, dst)` + `oriented_size` swaps W/H when `turns_cw` odd. **DataManager `apply` calls `orient_pixels` with `mirror=false`**, using only `exifTransform(o).turns_cw` — orientations 2/4/5/7 lose the mirror in the train path. |
| Size probe | `src/data/ImageProbe.cpp:9-19` | Header size via stb / ImageFile; no full decode. |
| Mask file | `DataManager.cpp:521-545` | Load as gray (`stbi_load` req_comp=1); **binarize `!=0` → 1**; apply quarter turns; **nearest** resize to batch HxW; optional invert; optional boundary offset (out of this tip). |
| Alpha as mask | `DataManager.cpp:554-585`; `ImageProbe.cpp:60` | Alpha channel; area/bilinear resize then gate **`>= 128`**; opaque RGB → all-ones if probed as alpha but file lost alpha. |
| Formats (decode) | DataManager RGB path | PNG/JPEG/BMP/TGA via stb; EXR/TIFF via separate modules. **This tip:** PNG + JPEG only for probe/decode/mask (explicit boundary). |

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
`MAX_IMAGE_PIXELS` (16_777_216) with `SP-INP-003`, matching “avoid uncontrolled
allocation”. Original files are never rewritten.

## Capability binding

| Matrix | In this tip |
|---|---|
| C-DATA-10 | JPEG EXIF Orientation 1..8; modes none/orient/apply; oriented size |
| C-DATA-08 | Mask binarize / nearest resize / optional flip; alpha≥128 path |
| C-DATA-07 | Partial: PNG/JPEG size probe + gray/RGBA decode for mask/orient only |

## Errors

| Condition | Code |
|---|---|
| Missing / unreadable path | `SP-IO-001` / `SP-IO-003` |
| Unsupported extension for this tip | `SP-FMT-001` |
| Truncated / bad EXIF or image | `SP-FMT-002` / `SP-IO-005` |
| Orientation not in 1..8 when required | `SP-INP-003` |
| Zero / oversize dimensions | `SP-INP-003` |
| Mask channel count not 1/2/4 when required | `SP-INP-001` |

## Non-claims

- P3-F01 camera tip **not** merged on this branch baseline.
- No GPU / train / COLMAP / EXR-TIFF / mask boundary EDT / UI.
- 74 plan IDs unchanged; current plan pointer on this branch remains P3-F01
  until that tip merges; this feature is **进行中**.
