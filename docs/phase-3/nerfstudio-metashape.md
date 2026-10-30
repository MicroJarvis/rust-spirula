# P3-F04: Nerfstudio / Metashape adapters

Status: **进行中 (HOLD)**. Tip: `a677f4017011f0f0efeeff1ebc09a379cbd9a663`.
Owner: `spirula-io` (+ admitted `spirula-application` / `spirula-cli`
load path). Prerequisites: P3-F03. Lens path follows
`NerfstudioParser.cpp:650-745` (declared model + raw/skew fit; empty
`camera_distortion` is **not** a known hint).
Does **not** deliver scene center/normalize (P3-F05), splat init (P3-F06),
or training cache (P3-F08).

Pinned C++ reference: `9a24f09927944a795e7ba1bd8243a625bf419479`
(`NerfstudioParser.cpp`, `MetashapeParser.cpp`, `DatasetParser.h`,
`ColmapParser.cpp` auto-detect 965–1077). Format notes: C++ `app/README.md`
136–137 / 141.

## Capability binding

| Matrix | Scope |
|---|---|
| **C-DATA-01** (extend) | Auto-detect order Nerfstudio > COLMAP > Metashape; explicit format dispatch; COLMAP behavior unchanged |
| **C-DATA-04** | `transforms.json` → `ParsedDataset`; camera fields/models; `applied_transform` invertibility; OpenGL c2w; safe frame paths |
| **C-DATA-05** | Metashape camera-export XML + PLY; sensor calibration rules; component transform; OpenCV→OpenGL Y/Z flip; image match |
| PLY | Self-contained ASCII + `binary_little_endian` vertex reader (x/y/z; optional RGB) |

## Inventory (reference)

| Topic | Location | Observed behavior |
|---|---|---|
| Auto-detect | `ColmapParser.cpp:1019-1077` | `transforms.json` first; COLMAP recon; Metashape `.xml`/`.psx` sniff |
| Metashape sniff | `ColmapParser.cpp:977-1005` | `<document` + (`path=` for `.psx` / `<chunk` for `.xml`) |
| PLY | `NerfstudioParser.cpp:37-276` | ASCII / LE binary; reject list / big-endian / truncated / unknown type |
| NS frames | `NerfstudioParser.cpp:436-470` | `file_path`; skip `.webp`; missing images when required |
| NS c2w | `NerfstudioParser.cpp:472-483` | `transform_matrix` 3×4 already OpenGL |
| applied_transform | `NerfstudioParser.cpp:402-414,527+` | 3×3 invert; singular → error |
| Centering | NS / Metashape post-pose | **Deferred to P3-F05** — this feature leaves poses as written after format conversion |
| Metashape calib | `MetashapeParser.cpp:487-538` | `class != initial`; p1/p2 swap; b1/b2÷f; k4 radial (`THIN_PRISM`); p3/p4 ≠ 0 reject |
| Component XF | `MetashapeParser.cpp:543-739` | scale×rotation + translation; descaling; Y/Z column negate |
| .psx | `MetashapeParser.cpp` + miniz | Nested zip `doc.xml` — **typed unsupported** without introducing a zip runtime |

## Architecture

- `spirula-io::ply`: shared vertex reader → `ColmapPoints3D`.
- `spirula-io::nerfstudio`: `transforms.json` load → `ParsedDataset`.
- `spirula-io::metashape`: XML export + PLY; `.psx` → stable
  `UnimplementedOption` / probe unavailable with reason.
- `load_dataset` / probe: format hint or auto-detect; reuse
  `ParsedDataset` / `LoadedFrame` / `ColmapPoints3D`.
- No P3-F05 `train_to_normalized` / scene center; no splat; no cache.

## Budgets / cancel

- File size + cumulative `ParseMem` **before** allocation/grow (same
  ceilings as COLMAP load where shared). Charges cover:
  - JSON: `json_dom_budget_cap` charged **before** `serde_json::from_reader`
    covers `2×size_of::<Value>()×len` (Vec capacity cliff), plus map /
    nesting Values, string/key payloads, and scratch — not a false
    `24×len` claim (independent 131073-zero array counterexample).
  - XML: `xml_chars` charged before `Vec<char>` reserve; name / attr /
    text `String` charged before collect/decode/push_str; attrs /
    children use capacity-aware `prepare_vec_push` (amortized doubling).
  - PLY: header element/property growth via `prepare_vec_push`; offsets
    and ASCII row scratch charged before reserve; ASCII floats are
    **token-bounded** (not `from_utf8` of the remaining body).
  - Metashape→JSON conversion charges sensor/component/camera/frame map
    entries and Value clones **before** each `json!` / insert / clone
    (no post-return `estimate_json_bytes` after conversion returns).
  - Image-dir walk charges `visited` BTree paths and filename Vec growth
    **before** insert/push.
  - Builds/missing paths and PLY points share the **same** live budget.
- Cooperative cancel: JSON parse uses `serde_json::from_reader` with
  per-byte ticks (IoRead); XML ticks inside long attribute/text/comment
  scans; PLY header/skip/body loops tick. After-start cancel proven for
  JSON parse, XML long attr, PLY ASCII, and Metashape dir walk.
- XML nesting capped (`MAX_XML_DEPTH`); legal `<!-- … -->` comments
  (including trailing siblings under a parent), numeric entities
  (`&#NNN;` / `&#xHH;`), leading UTF-8 BOM (C++ `skip_prolog`), and
  `<![CDATA[…]]>` raw text (C++ `parse_element`) are accepted. Image-dir
  walk skips symlink cycles via canonicalize/`visited` and caps depth.
- Stable `DomainError` (no panic on malformed input; UTF-8 labels use
  char-safe `match_substring`).

## .psx policy

Workspace has no approved zip crate and must not vendor miniz as a runtime
dependency for this slice. Detection of a Metashape `.psx` remains for
auto-detect / probe messaging. Load of `.psx`-only datasets returns a typed
unsupported error naming the missing zip capability and pointing operators
at the XML + PLY export path (primary supported Metashape path).

## Fixtures

Under `fixtures/datasets/`:

| Fixture | Purpose |
|---|---|
| `nerfstudio-min/` | `transforms.json` + ASCII PLY + one PNG |
| `ply-binary-le.ply` | binary_little_endian vertex golden |
| `metashape-min/` | camera-export XML + PLY + image |
| `metashape-xml-compat/` | UTF-8 BOM + CDATA-wrapped transform variants of min cameras.xml |
| Counter-examples | truncated PLY, big-endian, list prop, non-finite / singular / path escape |

## Non-claims

- Scene centering / normalize / `train_frame_scale` (P3-F05).
- `.psx` nested zip project load (no zip runtime).
- Dense `fit_model` mid-LM preemptibility (inherited from P3-F03).
- Outlier rejection / EXIF quarter-turn / aux mask-depth-normal discovery
  beyond explicit frame keys (follow-ups if needed).

## Verification (target)

```text
cargo test -p spirula-io --test nerfstudio_metashape
cargo test -p spirula-application --test dataset_load_e2e
cargo fmt --all -- --check
cargo check --workspace
cargo test --workspace
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo run -p spirula-xtask -- verify
git diff --check
```
