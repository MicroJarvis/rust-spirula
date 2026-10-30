# P3-F05 — Scene transform, center, and region

**Status:** HOLD (awaiting independent review). Not merged.

## Scope

Vertical slice for C-DATA-15 / C-DATA-16 plus Region/AABB JSON:

1. **SceneTransform JSON** (`spirula-scene-transform` v1): active
   `train_from_world`, emitted `world_from_train` inverse, matrix / quaternion /
   Euler / centering / georeference fields; illegal inputs rejected.
2. **Apply path** on P3-F03/F04 `ParsedDataset`: undo Nerfstudio
   `applied_transform`, resolve/apply scene center to poses + points, fill
   normalize metadata (`train_frame_scale`, `train_to_normalized`,
   `normalized_rotation`). Preserves `center_json = A @ center + b`.
3. **Six center modes** + `auto` / empty-cloud fallback / finite threshold.
4. **Region / AABB**: box, sphere, ellipsoid, cylinder, halfspace, CSG
   (union/intersection/difference); JSON round-trip; atomic writes leave the
   destination untouched on validation failure.
5. Point / splat bounds × region containment tests.

**Out of scope:** P3-F06 splat file formats, P3-F08 cache, mesh/prism/label
region kinds (explicit Non-claim; C++ also has those).

## Pins

| Topic | Reference |
|---|---|
| JSON emit/read | `SceneTransform.h/.cpp` |
| Modes / auto | `SceneCenter.h`, `DatasetCommon.cpp` |
| Apply + AT undo | `NerfstudioParser.cpp`, `ColmapParser.cpp` |
| Region | `Region.h/.cpp` |
| Tests | `scene_center_test.cpp`, `gauge_georeference_test.cpp`, `roi_document_test.cpp` |

## Contracts

- Similarity: `p_train = scale * R * p_world + t` (row-major `R`).
- Read path uses `train_from_world.scale` + `rotation.matrix_3x3_flat_row_major`
  + `translation` (same as C++).
- Writes go through `publish_bytes_validated` (`spirula-io` atomic lease).
- `ColmapLoadConfig::{scene_center, scene_center_threshold,
  scene_center_explicit, apply_scene}` — default `scene_center=none`,
  `apply_scene=true`.

## Fixtures / goldens

| Path | Role |
|---|---|
| `fixtures/scene/scene_transform.json` | Full spelling fixture |
| `fixtures/golden/scene.json` | Apply samples |
| `fixtures/golden/scene_cpp_compare.json` | C++ compare ledger |
| `fixtures/scene/region_sphere.json` | Minimal region |

## Tests

- Unit: `spirula-core` `scene_center` / `region`; `spirula-io`
  `scene_transform` / `region_io` / `scene_apply`.
- Integration: `crates/spirula-io/tests/scene_transform_region.rs`
  (fixture/golden, inverse, empty/degenerate, nonfinite, region bounds,
  fail-write, cancel, splat bounds).

## Non-claims

- Mesh / prism / label regions not implemented (reject unknown `type`).
- EXIF-driven normalize leveling not wired (identity EXIF path).
- Does not load splat PLY layouts (P3-F06) or training cache (P3-F08).
