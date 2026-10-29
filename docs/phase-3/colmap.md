# P3-F03: COLMAP dataset reading

Status: **进行中 / HOLD** (awaiting independent re-review after round-3 fixes).
Owner: `spirula-io` (production parsers) + admitted `spirula-application` /
`spirula-cli` session entry. Prerequisites P0-F02 / P0-F04 / P3-F01.
Not Nerfstudio/Metashape parsers (P3-F04 → typed **unavailable**), not
scene center/normalize (P3-F05), not training cache (P3-F08), not UI.
**P2-F08** remains 未开始.

Pinned C++ reference: `9a24f09927944a795e7ba1bd8243a625bf419479`
(`ColmapParser.cpp`, `DatasetCommon.cpp`, `DatasetParser.h`).

## Capability binding

| Matrix | Scope delivered |
|---|---|
| **C-DATA-01** | Directory probe: `transforms.json` → Nerfstudio unavailable; COLMAP recon probe; Metashape markers → unavailable; explicit `recon_dir` |
| **C-DATA-02** | COLMAP text `cameras` / `images` / `points3D` → `ParsedDataset`; 18 models; points optional; bake + P3-F01 fit |
| **C-DATA-03** | COLMAP little-endian `.bin` (preferred per file when both exist); text/bin equivalence tests |
| **C-DATA-06** | Train/eval/validation: `all` / `fraction` / `interval` / `filename`; empty eval OK; empty train = error |

## Inventory (reference)

| Topic | Location | Observed behavior |
|---|---|---|
| Model table | `ColmapParser.cpp:72-100` | 18 ids / names / param counts |
| Bin/text prefer | `ColmapParser.cpp:615-628` | Per-file `.bin` else `.txt` |
| Recon probe | `ColmapParser.cpp:633-708` | sparse enumeration + fallbacks; count desc, path asc |
| Image count header | `ColmapParser.cpp:600` | `atoll` leading prefix (`10, mean…` → 10) |
| Auto-detect | `ColmapParser.cpp:1019-1077` | NS > COLMAP > Metashape |
| Pose | `ColmapParser.cpp:509-530` | qvec wxyz w2c → OpenGL c2w (y/z flip) |
| Splits | `DatasetCommon.cpp:183-258` | linspace / interval / filename; val once on train |
| Intrinsics bake | `ColmapParser.cpp:309-407` | exact tiers + fit ladder |

## Architecture

- `spirula-io::colmap`: production readers (not `fixtures.rs` checker).
- `spirula-core`: `CameraModel` / `DistortionTier` / `SourceCamera` /
  `fit_camera_auto` reused for unrepresentable lenses.
- `spirula-application`: `LoadDataset` → worker → `DatasetLoaded` summary;
  Stop/Shutdown → exactly one `Canceled` terminal; **never** `DatasetLoaded`
  after cancel. Panic routes through `DatasetLoadDone{Err(Internal)}` (not
  `RunTerminal::Failed`, which the owner drops for image/dataset payloads).
- `spirula-cli`: `load-dataset --path …` shares the same owner path.

### Split index domain (pin)

`SplitPlan::{train,eval,validation}` are indices into the **emitted** frame
list (`SplitPlan::emit` → `ParsedDataset::frames`). Validation is carved
**once** from the true train keep-set; `filename` / `interval` / `fraction`
never mix eval-complement frames into train. `eval_mode=all` keeps reference
coincidence (`eval` covers every sorted frame on `split=all`).
`split=eval` (C++ DataManager eval-only) forces `validation_fraction=0` and
sets local `train = 0..emit.len` (selected frames are all “train” to the
loader); local `eval` stays empty because membership was applied by emit.

### Numeric bake boundary

`bake_colmap_intrinsics` (and `bake_and_fit`) reject exact-arity mismatches,
non-positive size, non-finite params, and f32 overflow (no silent sanitize).
EQUIRECTANGULAR bake requires finite positive derived focals from params.
Host pin holdout: `fixtures/datasets/colmap-goldens/bake-pin/` (regen via
`tools/fixtures` / `spirula-io` example `regen_bake_pin`).

### Bounded IO / cancel

- Binary `points` / observations: remaining-byte check before fallible reserve;
  `MAX_POINTS` must not allow multi-GB reserve from an 8-byte file.
- Cumulative `ParseMem` charges file bytes + Vec/BTree/string growth (not
  reset per line).
- Probe: `images.bin` 8 bytes; Metashape **8192** requires `<document` and
  (`path=` for `.psx` / `<chunk` for xml); `images.txt` full streaming
  count (header atoll early-return; no 64 KiB approximate fallback).
- `metadata` + `fs::read` re-checks size (TOCTOU growth).
- Cancel checkpoints in text observation/track tokenization, binary cstring,
  track association, probe dir scan/sort / image-count stream, and around
  each camera bake.
- `fit_camera_auto_cancelable` polls between domain-scan azimuth steps and
  candidate models; dense mid-`fit_model` least-squares remains
  non-preemptible. Load still polls immediately before/after each camera.

### Admission (`LoadDataset`)

All variable fields are capped: `path` / nonempty `recon_dir` ≤ 4096 bytes;
`eval_mode` / `split` ≤ 32 bytes. Invalid enum errors never concatenate the
input. Rejected payloads are UTF-8-safe length-limited via
`bound_command_payload`.

## Non-goals (this feature)

- Scene center / normalized transform / outlier filter (P3-F05).
- Nerfstudio / Metashape parse (P3-F04).
- Image decode cache / batch loader (P3-F08).
- UI dataset browser (P6-F04).
- Chunked cancel inside dense `fit_model` LM (azimuth/candidate polls only).

## Errors (typed, no panic)

| Condition | Code |
|---|---|
| Missing dataset / recon / images | `SP-IO-001` |
| Truncated / malformed tables; duplicate ids; text camera param arity | `SP-FMT-002` |
| Unknown model / unsupported dataset | `SP-FMT-001` |
| Nonfinite / illegal focal / illegal image path; budgets; admission | `SP-INP-003` / `SP-INP-001` |
| Nerfstudio / Metashape detected | `SP-INP-004` |
| Empty train split | `SP-INP-001` |
| Cancel | `SP-CAN-001` |
| Worker panic on load | `SP-INT-001` via `DatasetLoadDone` |

## Fixtures

- `fixtures/datasets/tiny-colmap/` text + LE binary (bin preferred).
- `fixtures/datasets/colmap-goldens/nontrivial/` non-identity pose, OPENCV
  lens, no points, filename train/eval split.
- `fixtures/datasets/colmap-goldens/bake-pin/` host-only bake/fit pin rows
  (no C++ sources).
- `fixtures/golden/dataset.json` retained; `fixtures.rs` remains a checker only.

## Remaining gaps (HOLD)

- Independent re-review of round-3 HOLD fixes (streaming count, Metashape
  sniff, bake arity, eval-local train, ParseMem, holdout, fit cancel docs).
- Fitted-model numeric tolerance vs host pin rows may need tightening after
  review (loose tol on FOV/EUCM/thin-prism ladder); regen may rewrite
  `expected.txt` pose to the documented host q/t.
- Dense mid-`fit_model` inside `fit_camera_auto_cancelable` remains
  non-preemptible (azimuth/candidate polls only).
- P3-F04/F05/F08 and P2-F08 deliberately untouched.

## Verification

- `cargo test -p spirula-io --test colmap_dataset`
- `cargo test -p spirula-application --test dataset_load_e2e`
- CLI: `spirula load-dataset --path fixtures/datasets/tiny-colmap`
- `cargo run -p spirula-xtask -- verify` → 8/8
- `git diff --check 5bd2b16..HEAD`
