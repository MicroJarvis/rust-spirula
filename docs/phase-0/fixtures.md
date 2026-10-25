# P0-F04: minimal compatibility fixtures

The fixture set under [`fixtures/`](../../fixtures/README.md) gives later
features a small, deterministic input they can load and compare against. It is
derived from the pinned reference formats (see
[reference-baseline.md](reference-baseline.md)) and is checked by
`spirula-io`'s `fixtures` module.

## 1. What is pinned

`fixtures/manifest.json` is the index:

```json
{
  "schema": "spirula-fixtures",
  "version": 1,
  "files": [{"path": "...", "bytes": 123, "sha256": "..."}]
}
```

Every file except `manifest.json` and `README.md` must appear exactly once.
The checker verifies presence, byte size and SHA-256 before parsing anything.
A changed byte fails the check until the manifest is updated in the same
change. File versioning is monotonic: `version` is 1 and a breaking fixture
change bumps it and the affected golden schema.

## 2. Layout and formats

| fixture | files | format contract | reference |
|---|---|---|---|
| config | `config/catalog.json`, `config/defaults.json` | 251-field catalog and the current default document, including bare `Infinity` | `src/config/TrainConfig.h`, [../phase-1/config.md](../phase-1/config.md) |
| camera | `camera/single_camera.json`, `golden/camera.json` | `PINHOLE`, 64x48, intrinsics `[fx, fy, cx, cy]`, OpenCV distortion row `[k1, k2, p1, p2, 0, 0, 0, 0]`; `c2w` is OpenGL (x right, y up, -Z forward); projection samples are world points and expected pixels | `src/core/CameraModel.h:21-50`, `src/data/CameraMath.h:1-42` |
| dataset | `datasets/tiny-colmap/{cameras,images,points3D}.txt`, `datasets/tiny-colmap/images/tiny.png`, `golden/dataset.json` | COLMAP text records; one camera, one image, four points; image is a 64x48 8-bit RGB PNG | `src/data/parsers/ColmapParser.cpp:616-795` |
| scene | `scene/scene_transform.json`, `golden/scene.json` | `format: spirula-scene-transform`, `version: 1`; centering, georeference, and both similarity blocks with every reference spelling | `src/data/SceneTransform.cpp:110-163,244-331` |
| splats | `splats/tiny_splat.ply`, `splats/render_splat.ply`, `splats/seed_points.ply`, `golden/splats.json` | Gaussian PLY property order and raw values; seed PLY `x y z red green blue`; binary little-endian splats, ASCII seed | `src/checkpoint/SplatPly.h:3-11`, `src/checkpoint/SplatPly.cpp:177-195`, `src/data/parsers/NerfstudioParser.cpp:235-276` |
| checkpoint | `checkpoints/tiny-run/config.json`, `step-000000001.ckpt/{state.tar,splat.ply}`, `golden/checkpoint.json` | ustar `state.tar` with `state.json` + one float32 NPY per saved slot; `step-%09d.ckpt` directory naming; run `config.json` flat keys | `src/engine/EngineCheckpoint.cpp:74-124,309,358`, `src/core/CheckpointIO.h:5-16` |
| render | `render/tiny_render.json`, `render/golden.ppm`, `render/golden.png`, `golden/render.json` | 32x32 pinhole render of one splat; analytic straight-alpha golden with a documented tolerance | `src/kernels/raster/RasterizationEval3DFwd_kernel.cuh:234-325` |

### Camera

`c2w` is the reference's OpenGL camera-to-world frame; the checker converts a
world point with `p_camera = R^T (p_world - t)` and projects with the
OpenCV-normalized coordinates `(x/-z, -y/-z)`. Distortion uses the OpenCV tier
order `k1 k2 p1 p2`, with the remaining row slots zero. Golden pixels are
computed independently and must match within 1e-9 px, which pins the
intrinsics, the frame convention and the distortion order together.

### Dataset

The COLMAP records follow the reference parser's field order. The camera in
`cameras.txt` and the single-camera fixture describe the same lens (checked
cross-fixture). `images.txt` carries one image line and the required empty
points line; `points3D.txt` carries four points with RGB, error and a track.
The image is a deterministic gradient; only its IHDR dimensions and color type
are asserted in Phase 0.

### Scene transform

The fixture uses `s = 0.5`, a +90 degree rotation about Z, translation
`[1.5, -2.0, 0.25]`, centering mode `point-median`, center
`[4.0, 3.0, -0.5]`, and a georeference block. The checker verifies:

- `matrix_3x3` equals both flat matrix spellings and the quaternion spellings;
- `matrix_4x4` and `matrix_3x4` encode `sR` and `t`;
- `translation_before_rotation = R^T t / s`, and `center_world` is its negative;
- `world_from_train` is the inverse: `R_w = R^T`, `s_w = 1/s`,
  `t_w = -(1/s) R^T t`;
- the golden sample points transform exactly under `train_from_world`.

### Splat PLY

The binary layout is exactly the reference order:

```text
x y z | nx ny nz | f_dc_0..2 | opacity | scale_0..2 | rot_0..3
```

All properties are float32 little-endian. Stored values are raw: log scale,
logit opacity, `(w,x,y,z)` quaternion, and DC coefficient
`(colour - 0.5) / C0` with `C0 = 0.28209479177387814`. The fixture has SH
degree 0, so no `f_rest` and no `sb_params`. The golden checks the property
order, count, first raw row, decoded opacity/color and the bounds.

### Checkpoint

The run directory is `checkpoints/tiny-run/`:

```text
config.json
step-000000001.ckpt/
  splat.ply     # byte-identical to splats/tiny_splat.ply
  state.tar     # ustar: state.json, world.means.npy, world.opacities.npy
```

`state.json` uses the reference keys (`format_version: 1`, `full_resume: 1`,
`step`, counts, SH layout, camera summary). The `.npy` files are NPY v1.0,
`<f4`, C order, shapes `(3, 3)` and `(3,)`. The checker verifies the tar
member list, the state fields, both NPY headers and payloads, and the run
config keys. Phase 0 deliberately does not validate `quant_codec`, matching
the reference's current load behavior; P3-F07 decides how to close that gap.

### Micro render

`render/tiny_render.json` asks for a 32x32 pinhole image (fx = fy = 80,
cx = cy = 16) of a single splat at `(0, 0, -2)`, color `(1, 0, 0)`, opacity
`0.99`, log scale `log(0.2)`. The documented golden formula is:

```text
alpha(x, y) = opacity * exp(-0.5 * r^2 / sigma^2)
sigma       = fx * scale / depth            # 8 px
rgb8        = round(255 * alpha * color)     # over black
depth       = 2.0 at the center pixel
```

`golden.ppm` is the human-readable golden; `golden.png` is the same image for
viewing. The checker validates PPM/PNG dimensions, the center/corner samples,
the projected center, and every pixel against the formula with tolerance 2
(8-bit). A renderer that differs in Gaussian normalization or pixel-center
convention must be reconciled with this formula when P4-F01 defines the render
contract; if it intentionally differs, bump the fixture version and document
the new golden.

## 3. Checker behavior

`spirula_io::fixtures::check_fixtures(root)`:

1. reads and validates `manifest.json` (`schema`, `version`);
2. rejects absolute paths, `..` and duplicate entries;
3. verifies every listed file's size and SHA-256;
4. requires the canonical fixture/golden list to be present;
5. parses each format and compares it with its golden;
6. returns a `FixtureReport` naming the checks that ran, or a `FixtureError`
   with the offending path and expectation.

Integration tests in `crates/spirula-io/tests/fixtures.rs` cover the happy
path plus two negative cases: a single-bit corruption is rejected by checksum,
and a missing golden file is rejected. The fixture root can be overridden with
`SPIRULA_FIXTURES`; tests use the workspace `fixtures/`.

Dependencies added for this feature (recorded in `Cargo.lock`):

- `sha2` 0.10.9 for SHA-256;
- `serde_json` 1.0.154 for manifest and golden parsing.

They are `spirula-io` runtime dependencies and are expected to be reused by
the real readers and by P1-F02's versioned serialization.

## 4. Limitations and follow-up

- The fixtures were **hand-derived from the reference source and format
  documentation**, not exported by a running C++ build (the reference build is
  not available in this environment; see
  [reference-baseline.md](reference-baseline.md) section 5). Before Phase 3 is
  marked complete, a machine with the reference toolchain must confirm that
  the COLMAP dataset, scene transform, splat PLY and checkpoint are accepted by
  the C++ tools, or the differences must be recorded as explicit compatible
  deviations.
- The render golden is analytic, not a C++ render. P4-F01 records the render
  contract and the actual tolerance; the fixture is the deterministic input for
  that comparison.
- The PNG image is only header-checked in Phase 0. P3-F02 adds decode,
  orientation and mask coverage.
- Single-file atomic publish is P1-F05. Checkpoint directory atomicity,
  completion markers, version validation and corruption recovery remain
  P3-F07 / P5-F05; this fixture only covers a complete, well-formed save.
- The fixture set is intentionally small. It does not cover fisheye/equirect
  cameras, distortion fitting, multiple cameras, eval splits, masks, or SH
  degrees above zero; those arrive with the Phase 3 fixtures.

## 5. Capability mapping

The fixtures are the verification input for:

| fixture | capability rows |
|---|---|
| camera | C-DATA-11, C-DATA-12, C-DATA-14, C-REN-02 |
| dataset | C-DATA-01..03, C-DATA-06, C-DATA-07 |
| scene | C-DATA-15, C-DATA-16 |
| splats | C-DATA-17, C-DATA-18, C-REN-01 |
| checkpoint | C-DATA-19..21 |
| render | C-REN-03..07, C-REN-10 |

See [capability-matrix.md](capability-matrix.md).
