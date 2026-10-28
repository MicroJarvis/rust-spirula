# P3-F03: COLMAP dataset reading

Status: **进行中 / HOLD** (awaiting independent review). Owner:
`spirula-io` (production parsers) + admitted `spirula-application` /
`spirula-cli` session entry. Prerequisites P0-F02 / P0-F04 / P3-F01.
Not Nerfstudio/Metashape parsers (P3-F04 → typed **unavailable**), not
scene center/normalize (P3-F05), not training cache (P3-F08), not UI.

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
| Recon probe | `ColmapParser.cpp:633-708` | sparse enumeration + fallbacks |
| Auto-detect | `ColmapParser.cpp:1019-1077` | NS > COLMAP > Metashape |
| Pose | `ColmapParser.cpp:509-530` | qvec wxyz w2c → OpenGL c2w (y/z flip) |
| Splits | `DatasetCommon.cpp:183-258` | linspace / interval / filename |
| Intrinsics bake | `ColmapParser.cpp:309-407` | exact tiers + fit ladder |

## Architecture

- `spirula-io::colmap`: production readers (not `fixtures.rs` checker).
- `spirula-core`: `CameraModel` / `DistortionTier` / `SourceCamera` /
  `fit_camera_auto` reused for unrepresentable lenses.
- `spirula-application`: `LoadDataset` → worker → `DatasetLoaded` summary;
  Stop/Shutdown → exactly one `Canceled` terminal; **never** `DatasetLoaded`
  after cancel.
- `spirula-cli`: `load-dataset --path …` shares the same owner path.

## Non-goals (this feature)

- Scene center / normalized transform / outlier filter (P3-F05).
- Nerfstudio / Metashape parse (P3-F04).
- Image decode cache / batch loader (P3-F08).
- UI dataset browser (P6-F04).

## Errors (typed, no panic)

| Condition | Code |
|---|---|
| Missing dataset / recon / images | `SP-IO-001` |
| Truncated / malformed tables | `SP-FMT-002` |
| Unknown model / unsupported dataset | `SP-FMT-001` |
| Nonfinite pose/params, illegal `..` path, budgets | `SP-INP-003` / `SP-INP-001` |
| Nerfstudio / Metashape detected | `SP-INP-004` |
| Empty train split | `SP-INP-001` |
| Cancel | `SP-CAN-001` |

## Fixtures

- `fixtures/datasets/tiny-colmap/` text + LE binary (bin preferred).
- `fixtures/datasets/colmap-goldens/nontrivial/` non-identity pose, OPENCV
  lens, no points, filename train/eval split.
- `fixtures/golden/dataset.json` retained; `fixtures.rs` remains a checker only.

## Verification

- `cargo test -p spirula-io --test colmap_dataset`
- `cargo test -p spirula-application --test dataset_load_e2e`
- CLI: `spirula load-dataset --path fixtures/datasets/tiny-colmap`
- `cargo run -p spirula-xtask -- verify` → 8/8
- `git diff --check`
