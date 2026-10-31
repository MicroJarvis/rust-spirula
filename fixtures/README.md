# Phase 0 compatibility fixtures

Minimal, deterministic samples used by later features as acceptance inputs.
The format contracts and compatibility notes are in
[docs/phase-0/fixtures.md](../docs/phase-0/fixtures.md).

## Layout

```text
fixtures/
  manifest.json                       # size + SHA-256 for every file below
  camera/single_camera.json           # pinhole + OpenCV distortion
  camera/cpp-golden/camera_math.json  # C++ CameraMath holdout (P3-F01; not in Phase-0 required set)
  config/catalog.json                 # all 251 training-config fields
  config/defaults.json                # current default document
  datasets/tiny-colmap/               # COLMAP text dataset + 64x48 PNG
  scene/scene_transform.json          # every similarity spelling + georeference
  splats/tiny_splat.ply               # 3 Gaussian splats, binary little-endian
  splats/seed_points.ply              # 4 seed points, ASCII PLY
  splats/render_splat.ply             # one splat for the micro render
  checkpoints/tiny-run/               # config.json + step-000000001.ckpt/
  render/                             # render request, golden PPM and PNG
  preview/                            # P4-F06 preview request + scene.ply
  golden/                             # parsed expectations for each fixture
  images/                             # P3-F02 formats/orient/mask/depth/normal + C++ golden
```

## Rules

- Every file except `manifest.json` and this README is pinned by size and
  SHA-256 in `manifest.json`.
- Do not edit a fixture in place. Change the file, update its golden values and
  update the manifest in the same commit. A silent regeneration is a broken
  baseline.
- The fixtures are derived from the pinned C++ reference formats; they were not
  produced by running the C++ engine (it was not built in this phase). See the
  limitations section in [docs/phase-0/fixtures.md](../docs/phase-0/fixtures.md).

## Check

```bash
cargo test -p spirula-io --test fixtures
```

or, once the workspace verification entry exists:

```bash
cargo run -p spirula-xtask -- fixtures
```
