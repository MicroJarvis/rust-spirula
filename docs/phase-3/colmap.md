# P3-F03: COLMAP dataset reading

Status: **READY**. Code tip `813729f8238f6730ad0235416371197fc4bd985a`.
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
Host pin holdout: `fixtures/datasets/colmap-goldens/bake-pin/` — **C++ pin
oracle** via `tools/bake-pin-golden` (extract/compile under gitignored
`.build/`; no committed C++). Rust compares baked `FrameCamera` rays against
C++ `camhost::ray_pixel` with absolute pixel tolerances.

### Bounded IO / cancel

- Binary `points` / observations: remaining-byte check before fallible reserve;
  `MAX_POINTS` must not allow multi-GB reserve from an 8-byte file.
- Per-table cumulative `ParseMem` (`MAX_PARSE_MEMORY_BYTES` = 512 MiB) charges
  file bytes + Vec/BTree/string growth (not reset per line). Text `read_capped`
  and binary `BinReader::open` charge metadata file bytes **before**
  `try_reserve`/read (TOCTOU growth re-charges deltas). Text obs/track
  charge+`try_reserve` **before** parse; camera params take exact model arity
  via iterator (no intermediate `Vec<&str>`). `points3D` duplicate-id
  `BTreeSet` charges `BTREE_ENTRY_OVERHEAD` (64 B) per entry in addition to
  key bytes.
- Load-wide cumulative budget (`MAX_LOAD_MEMORY_BYTES` = 512 MiB) is **independent**
  of the three per-table caps: after each table materializes, load re-charges
  live cameras/images/points (incl. observations/tracks + BTree overhead),
  then pre-charges baked map, `image_ids` set, builds/paths/sort keys, split
  name strings, split index vectors, and frames with fallible `try_reserve`.
  Cross-table or emit-phase overflow → typed `SP-INP-003` (`ValueOutOfRange`).
  Inject via `load_colmap_dataset_budgeted`.
- Frame build **moves** `ColmapImage` (observations) out of the images map and
  into emitted `LoadedFrame` (no full observation clone chain).
- Probe: `images.bin` 8 bytes; Metashape **8192** requires `<document` and
  (`path=` for `.psx` / project, or `<chunk` for xml) — pin has **no** literal
  `psx:` check; `images.txt` full streaming count (header atoll early-return;
  byte-chunk cancel via private `Read` helper; comment prefix capped — no
  `read_line` into an unbounded `String`).
- Text + binary tables: `metadata` + **chunked** cancel-aware read (not
  whole-file `fs::read`) re-checks size (TOCTOU growth) and ticks
  `ParseCancel` per chunk.
- Cancel checkpoints in text observation/track tokenization, binary cstring,
  track association (outer points + inner track elems), probe dir scan/sort /
  image-count stream, and around each camera bake.
- Load calls `bake_and_fit_cancelable` (token forwarded into
  `fit_colmap_source_cancelable` → `fit_camera_auto_cancelable`). Azimuth /
  candidate polls are cooperative; **dense mid-`fit_model` least-squares is
  still non-preemptible**. `bake_and_fit` remains the no-token wrapper.

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
- `fixtures/datasets/colmap-goldens/bake-pin/` C++ pin bake/fit + ray + pose
  (no committed C++; regen via `tools/bake-pin-golden`).
- `fixtures/golden/dataset.json` retained; `fixtures.rs` remains a checker only.

## Remaining gaps

- Dense mid-`fit_model` inside `fit_camera_auto_cancelable` remains
  non-preemptible (azimuth/candidate polls only). Load can cancel between
  cameras and between domain-scan azimuth steps / candidate models, but not
  inside a single dense LM solve.
- Later features (out of scope here): **P3-F04**, **P3-F05**, **P3-F08**,
  and **P2-F08**.

## Verification

- `cargo test -p spirula-io --test colmap_dataset`
- `cargo test -p spirula-application --test dataset_load_e2e`
- CLI: `spirula load-dataset --path fixtures/datasets/tiny-colmap`
- `cargo run -p spirula-xtask -- verify` → 8/8
- `git diff --check 5bd2b16..HEAD`
