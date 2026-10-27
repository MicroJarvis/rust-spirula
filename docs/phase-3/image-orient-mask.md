# P3-F02: image orientation and mask

Status: **进行中** (branch `codex/p3-f02`; HOLD candidate). Owner:
`spirula-io` + admitted `spirula-application` session commands. Prerequisite
P1-F02 done. Not COLMAP parsers (P3-F03), not training cache (P3-F08), not UI
mask editor (P7-F01).

Pinned C++ reference: `9a24f09927944a795e7ba1bd8243a625bf419479`.

## Capability scope (matrix)

| Matrix | Delivered in this tip |
|---|---|
| **C-DATA-07** | PNG / JPEG / BMP (CORE+INFO) / TGA / TIFF / EXR decode + header/dimension probe; `DecodedRgb::{U8,U16,F32}` preserves 16-bit/`f32` DynamicImage variants; encoded + pixel budgets before decode/alloc |
| **C-DATA-08** | NonZero (stbi gray, alpha ignored); Alpha LA/RGBA + `cpu_resize` ≥128; flip; **boundary offset** (`fraction * sqrt(W*H)` EDT); nonfinite offset rejected before mutation |
| **C-DATA-09** | 16-bit depth PNG (+ bilinear); RGB normal: pixel turn → bilinear resize → `turn_normal_vectors` (pin `decode_normal_into` order) |
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
| Normal | `DataManager.cpp:638-656` | RGB; turn pixels; bilinear resize; then `turn_normal_vectors` on dst |
| EDT offset | `DistanceTransform.h`; `apply_boundary_offset` | `fraction * sqrt(W*H)` px dilate/erode |
| EXIF | `Exif.h` | Orientation SHORT 1..8; `exifTransform` |

## AD-P3-F02-01 / AD-P3-F02-02

Unchanged: TurnsOnly vs FullTransform; `MAX_IMAGE_PIXELS` + `MAX_ENCODED_BYTES`
before `load_from_memory` / path read.

## Application / session entry

Admitted commands (wiring not deferred):

- `PrepareOrientedImage`
- `PrepareNormalizedMask` (includes `boundary_offset_frac`)

Heavy decode/normalize runs on the existing fake-worker thread (not the
owner dispatch loop). Stop/Shutdown set the cancel token; canceled work
emits exactly one `Canceled` terminal and **never** `ImagePrepared` /
`MaskPrepared` or a silent `Completed`. Destination dims and options are
validated at command admission and again before allocation.

### Cancel responsiveness (actual limits)

Runtime §5 requires cancel observation at least every **250 ms** inside long
owned CPU/GPU loops. Stage-boundary-only checks do **not** replace that.
`CancelCheckpoint::poll` only observes when *called* after the interval;
callers must `tick` (or poll) at **bounded work chunks**
(`CANCEL_WORK_CHUNK`) so a silent wide row or area accumulation cannot
exceed the gap. Runtime §5 also lists **per-image decode** as a declared
checkpoint boundary — that is distinct from the owned-loop 250 ms rule.

| Region | Cancel behavior |
|---|---|
| `image` crate decode (`load_from_memory` / TGA hinted decode) | **Non-preemptible third-party call.** Runtime §5 per-image decode checkpoint surrounds the call; **no** mid-decode 250 ms guarantee. |
| Capped file read | One bounded read; checkpoint before/after (not mid-`read`). |
| Owned CPU on **image prepare** path: plane extract / `orient_pixels` / mask nearest + area/bilinear resize (incl. area source accumulation) / threshold / flip / EDT row·column·1d inners / binary validation / worker kept count | `CancelCheckpoint::tick` every `CANCEL_WORK_CHUNK` work units (+ wall-clock ≥250 ms). Returns `SP-CAN-001`. |
| C-DATA-09 sync library only: `normalize_normal_rgb8`, `cpu_bilinear_resize_rgb8`, `turn_normal_vectors`, `normalize_depth16`, `cpu_bilinear_resize_u16` | **Not** on admitted/sync prepare. No `CancelCheckpoint`; **not** claimed for the 250 ms loop contract. |
| Worker enqueue + owner apply | Post-work cancel before `ImagePrepareDone`; owner arbitrates queued Ok/Err vs pending Stop/Shutdown under the state lock. |
| Service Drop / teardown | Owner drain waits up to `teardown_deadline`, then still calls unbounded `JoinHandle::join` / `WorkerHandle::shutdown_join`. A non-preemptible decoder (or other in-flight work) can outlast the drain deadline; **Drop does not prove a hard join within `teardown_deadline`**. Forced terminals may publish after the deadline; actual worker exit is separate from the drain deadline. |

Also: pre-decode StallGate (tests); wrong `ImagePrepareOk` kind vs active
payload is stale and cannot close the command. Path extension hints are
preserved for TGA (no reliable magic): cancelable prepare uses
`decode_rgb8_bytes_hinted` / `decode_mask_image_bytes_hinted` (same as
path-based `decode_rgb8` / hinted mask decode). Sync + admitted TGA coverage:
`sync_and_admitted_prepare_tga_preserves_ext_hint` and
`sync_and_admitted_prepare_tga_mask_preserves_ext_hint` in
`image_ops_e2e.rs`. Wide-row / downsample after-start cancel regressions live
in `image_mask` unit tests.

**HOLD:** mid-decode cancel inside the third-party decoder remains unmet
relative to a strict 250 ms-inside-decode reading of §5; docs do not claim
it. Drop/join-after-deadline vs true worker exit remains honestly documented
above — global §5 requirements are not weakened to close the feature.

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
| APP1 `Exif\0\0` without TIFF header | `MalformedRecord` (not identity) |

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

- This branch already contains landed P3-F01 (camera) via merge from main;
  main tip is clean (`33cf0c1` at P3-F02 integration). Do not treat P3-F01 as
  missing on this baseline.
- Custom C++ EXR/TIFF colour-management edge cases beyond `image` crate decode
  are not claimed bit-identical to `ExrImage.cpp`/`TiffImage.cpp`; fixtures
  cover probe/decode/dtype routing.
- Third-party `image` decode is not cancelable mid-call (see cancel table).
- UI mask editor remains P7-F01.
- C-DATA-07..10 remain **进行中 / HOLD** until independent READY.
