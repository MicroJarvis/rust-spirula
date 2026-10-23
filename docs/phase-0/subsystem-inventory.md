# P0-F02: C++ subsystem behavior inventory

This inventory records the observable behavior of the pinned reference tree
(`9a24f099`, see [reference-baseline.md](reference-baseline.md)). All citations
are `path:line` in the reference checkout. It is the input to the
[capability matrix](capability-matrix.md) and the [runtime contracts](runtime-contracts.md).

The reference is one executable (`spirula`) with a process-global engine and
one live training session per process. The Rust rewrite keeps the same
user-visible behavior but not the global-singleton architecture.

## 1. Process entry and call chain

| concern | reference |
|---|---|
| `main` and pre-dispatch (macOS env, line buffering, `--lang` strip, license gate, crash handler) | `src/app/Main.cpp:161-224` |
| subcommand dispatch, `argv[0]` aliases, help/version, GUI fallback | `src/app/Main.cpp:230-273` |
| tool table (`train`, `mesh`, `sfm`, `sam`, `geometry`, `dense`, `gui`, `encode`, `e57`, `lidar`, `partition`) | `src/app/Main.cpp:71-106` |
| `spirula train` flow | `src/app/cli/main.cpp:468-642` |
| CLI flag parsing from the config table | `src/app/cli/main.cpp:82-135`, `:499-527` |
| GUI startup and frame loop | `src/app/gui/GuiMain.cpp:146-326` |
| GUI training worker | `src/app/gui/TrainRunner.cpp:178-282` |

`spirula train` call chain:

```text
argv + preset
  -> TrainConfig (src/config/TrainConfig.h:122,405)
  -> --resume config layering (src/app/cli/main.cpp:539-542)
  -> TrainerSession (src/app/TrainerCore.h:229)
  -> check_config -> load_dataset -> setup_engine
  -> engine_reset + set_data_3dgs + engine_setup_data_manager
     (src/app/TrainerCore.cpp:1267,1271,1412)
  -> engine_train_step_managed per step (src/app/TrainerCore.cpp:1621)
  -> eval / metrics.json
```

The GUI reaches the same `TrainerSession` through
`GuiApp::launch_training` -> `TrainRunner::start_training`
(`src/app/gui/GuiApp.cpp:1391-1410`, `src/app/gui/TrainRunner.cpp:194-260`).
There is no separate GUI training implementation.

## 2. Session lifecycle and engine state

Phases in the GUI worker (`src/app/gui/TrainRunner.h:32-43`):
`Idle`, `Loading`, `Ready`, `LoadError`, `Preparing`, `Training`,
`Evaluating`, `Done`, `TrainError`. The CLI sequences the same calls inline.

Engine and session invariants:

- the engine is a process-global singleton: `engine()`
  (`src/engine/EngineState.cpp:12-15`);
- one live training session per process (`src/app/TrainerCore.h:17-19`);
- `engine_reset()` overwrites the state, forgets viewer scenes and frees the
  global device pool (`src/engine/EngineState.cpp:17-25`);
- `setup_engine` order: output-dir naming -> `config.json` +
  `scene_transform.json` -> `engine_reset()` -> seed splats -> region masks ->
  background/color space -> data manager -> appearance models -> **resume
  last** (`src/app/TrainerCore.cpp:1227-1530`).

Control flags are public atomics: `stop_requested`, `paused`, `save_on_stop`,
`render_pending`, `cur_step` (`src/app/TrainerCore.h:270-280`). The step loop
breaks between steps, saves a final checkpoint when asked, then skips eval
(`src/app/TrainerCore.cpp:1764-1772`, `:1883-1893`).

## 3. Dataset and camera data path

### 3.1 Formats and parsers

| format | reference | read files | notes |
|---|---|---|---|
| COLMAP | `src/data/parsers/ColmapParser.cpp:713-1019` | `cameras`, `images`, `points3D` each `.bin` or `.txt`; `.bin` wins | probe order over `sparse/*`, `colmap/sparse/*`, root; `points3D` optional |
| Nerfstudio | `src/data/parsers/NerfstudioParser.cpp:419-817` | `transforms.json`, images, optional masks/depths/normals, seed PLY | `applied_transform` inverted; `.webp` skipped |
| Metashape | `src/data/parsers/MetashapeParser.cpp:385-785` | camera `.xml` + point `.ply`, or `.psx` zip tree | converted to a Nerfstudio-shaped meta, then shared parser |
| auto-detect | `src/data/parsers/ColmapParser.cpp:1019-1062` | as above | `transforms.json` -> COLMAP -> Metashape; COLMAP wins if both exist, with Metashape fallback on throw |
| E57/LAS laser clouds | `src/data/PointCloudFile.cpp`, `src/data/E57Reader.cpp` | scans | separate from `parse_dataset`; not a trainer input format |

The public entry points are `parse_dataset(dataset_dir, cfg, format)` and
`bake_post_split(...)` (`src/data/DatasetParser.h:325-327`, `:385-389`).
`ParsedDataset` is the stage-1 contract (`src/data/DatasetParser.h:213-291`):
per-input cameras sorted by filename, `c2w` camera-to-world OpenGL,
intrinsics `[fx, fy, cx, cy]`, an 8-wide distortion row, train/eval indices,
seed `points`, centering/normalization matrices, EXIF quarter turns and gauge
flags. Stage 2 produces the engine camera arrays (`viewmats`, split faces,
`c2w_flip`); see `src/data/PostSplit.cpp:58-250`.

### 3.2 Camera models and conventions

- `CameraModelType`: `PINHOLE`, `FISHEYE`, `EQUISOLID`, `EQUIRECTANGULAR`
  (`src/core/CameraModel.h:21-26`).
- Distortion tiers are orthogonal and per-camera:
  `None`, `OpenCV` (`k1 k2 p1 p2`), `ThinPrism`
  (`k1 k2 k3 k4 p1 p2 sx1 sy1`) (`src/core/CameraModel.h:28-50`).
- Slot meaning differs per tier; the 8-wide row is zero-extended.
- Host math works in normalized image coordinates and OpenCV camera rays
  (+Z forward, +Y down), mirrored by Slang device code
  (`src/data/CameraMath.h:1-42`, `src/core/CameraDistortion.cuh`).
- COLMAP world-to-camera is converted to `c2w` by transpose and column
  negation (`src/data/parsers/ColmapParser.cpp:517-533`).
- Unrepresentable lenses are fitted onto the nearest tier by a near-minimax
  fit and re-distorted through the true source projection; the fit never
  rejects a dataset (`src/data/DistortionFit.h`, `src/data/SourceCamera.h`).
- COLMAP's 18 model names are accepted; equirectangular is canonicalized to
  `fx = fy = w / 2*pi`, `cx = w/2`, `cy = h/2`
  (`src/data/parsers/ColmapParser.cpp:330-395`).

### 3.3 Split, images, masks

- Train/eval split: `eval_mode = all | fraction | interval | filename`, with
  `train_split_fraction` (0.9), `eval_interval` (8),
  `validation_fraction` (0) (`src/data/parsers/DatasetCommon.cpp:196-260`).
  An empty eval split is legal; an empty train split is an error.
- Aux files are discovered beside the image with `_mask`/`_depth`/`_normal`
  conventions (`src/data/parsers/DatasetCommon.cpp:264-284`).
- Masks are binary (`!= 0` means kept), can be flipped, optionally EDT-offset,
  and are nearest-resized; alpha masks are area-resampled and thresholded at
  128 (`src/data/DataManager.cpp:521-615`). White keeps subject; transparent
  alpha keeps subject; both are ANDed.
- RGB decodes via stb_image (8/16-bit PNG, JPEG, BMP, TGA), EXR and TIFF
  readers (`src/data/DataManager.h:14-22`, `src/data/DataManager.cpp:447-518`).
  Depth is 16-bit PNG or float arrays; masks/depths/normals may have a
  different size and are resized in the loss path.
- EXIF modes are `none | orient | apply`; `apply` turns pixels and aux maps
  (`src/core/ImageOrient.h`, `src/core/ImageProbe.cpp`).

### 3.4 Normal, error and cancellation behavior

- success prints `Cameras: N (M after splitting), seed points: K
  (train_frame_scale=...)` (`src/app/TrainerCore.cpp:1073-1077`);
- all parse/load failures are `std::runtime_error` with English format/file
  detail; no partial dataset escapes;
- parsing, post-split baking and image preload are synchronous and have **no
  cancellation token**;
- a mid-run unreadable image becomes `DataDecodeError`; the trainer either
  rethrows (CLI) or pauses and asks retry/stop (GUI)
  (`src/data/DataManager.h:28-47`, `src/app/TrainerCore.cpp:1801-1818`).

## 4. Scene transform and config

### 4.1 `scene_transform.json`

`SceneTransform` is `p_train = scale * R * p_world + t`, written after the
run's `config.json` (`src/data/SceneTransform.h:13-21`,
`src/data/SceneTransform.cpp:244-308`, `src/app/TrainerCore.cpp:742-757`).
Top-level fields:

- `format: "spirula-scene-transform"`, `version: 1`, `readme`;
- `centering: { mode, center_world }`;
- optional `georeference: { frame: "enu", origin_lat, origin_lon,
  origin_alt?, alt_source? }`;
- `train_from_world` and `world_from_train`, each a similarity block with
  `scale`, `translation`, `translation_before_rotation`, rotation in every
  common spelling (3x3 matrices row/column major, quaternions wxyz/xyzw,
  axis-angle, rotation vector, intrinsic/extrinsic Euler), 4x4 and 3x4
  matrix spellings.

`read_scene_transform_json` silently returns `false` on parse/shape failure
(`src/data/SceneTransform.cpp:310-331`). The writer is non-atomic
(`fopen`/`fputs`, error text `cannot write <path>`).

Scene-center modes: `none | point-median | camera-median | camera-focus |
point-mean | camera-mean`; `auto` resolves at load time and is never stored
(`src/data/SceneCenter.h:16-22`, `src/data/parsers/DatasetCommon.cpp:523-545`).
`gauge.txt` supplies `oriented`, `metric` and the geo origin
(`src/data/parsers/DatasetCommon.cpp:682-707`).

### 4.2 `config.json`

- One flat key per CLI flag, written in table order, no sections
  (`src/config/TrainConfig.h` X-macro table; `src/app/TrainerCore.cpp:759-768`).
- Unknown keys are ignored, absent keys take defaults, and a known key with a
  wrong shape throws (`src/config/TrainConfigJson.h`).
- There is **no version field and no migration logic**. The only special case
  is a pre-`scene_center` run resuming as `none` (`src/checkpoint/Resume.cpp:100-107`).
- Presets are compiled-in named bundles (`3dgs`, `360-camera`, `in-the-wild`,
  `centered-object`, `hdr`, `synthetic`, `meshing`) plus GUI preset files
  (`src/config/TrainConfig.h:440-600`, `src/app/gui/PresetFile.*`).
- Unsupported flags are refused up front with `--<flag> is not supported yet`
  (`src/app/TrainerCore.cpp:785-816`).

## 5. Splat and point-cloud on-disk formats

### 5.1 Gaussian splat PLY

Property order: `x y z / nx ny nz / f_dc_0..2 / f_rest_* / sb_params_* /
opacity / scale_0..2 / rot_0..3`, binary little-endian `float`
(`src/checkpoint/SplatPly.h:3-11`, `src/checkpoint/SplatPly.cpp:177-195`).
Stored values are raw: log scales, logit opacity, `(w,x,y,z)` quaternion,
DC = `(colour - 0.5) / C0`. The file stores `f_rest` channel-major and
`sb_params` field-major; the engine converts both to coefficient-major. The
reader accepts ASCII and binary little-endian; the writer always binary
little-endian float32 (`src/checkpoint/SplatPly.cpp:81-110`, `:330-377`).

### 5.2 Seed point PLY

`x y z` plus required `red green blue`; coordinates kept as double, RGB is
uchar 0-255 or float 0-1 (`src/data/parsers/NerfstudioParser.cpp:235-276`).
ASCII and binary little-endian are accepted. Dense/seed clouds use the same
reader; generic laser PLY output is binary little-endian with float32/float64
xyz + uchar RGB (`src/data/SparseEdit.cpp:666-701`).

## 6. Checkpoint

Layout: `<run>/step-%09d.ckpt/splat.ply` + `state.tar`
(`src/checkpoint/Resume.h:5-8`, `src/engine/EngineCheckpoint.cpp:309,358`).
Zero padding makes lexicographic order numeric (`src/checkpoint/Resume.cpp:51-57`).

`state.tar` is POSIX ustar with `state.json` plus one flat `.npy` per saved
pool slot (`<pool.name>.npy`), NPY v1 written / v1-v3 read
(`src/core/CheckpointIO.h:5-16`, `:34-123`, `:179-211`). Quantized slots add
`.q`/`.qb` siblings under one logical name (`src/checkpoint/Adapt.cpp:537-545`).

`state.json` records `format_version` (1), `full_resume`, quant codec/block/eps,
`step`, primitive, splat counts, SH/SB layout, packed/bit widths, camera
summary, and bilagrid/PPISP layout (`src/engine/EngineCheckpoint.cpp:74-124`).
`full_resume == 0` checkpoints are inference/meshing-only and are refused for
resume with an actionable message (`src/checkpoint/Resume.cpp:78-97`).

Resume requires the engine skeleton to already match (`max_num_splats`,
appearance channels); a layout mismatch is adapted into `<out>/.resume_adapt`
first (`src/app/TrainerCore.cpp:1551-1593`). Load rejects unknown slots and
byte-size mismatches (`src/engine/EngineCheckpoint.cpp:425-485`).

Known gaps to fix or explicitly preserve in the rewrite:

- `format_version` and `quant_codec` are written but **not validated** on load
  (`src/engine/EngineCheckpoint.cpp:411-422`);
- checkpoint writes are **not atomic** and have no completion marker; a crash
  can leave a `step-*.ckpt` that resume will still select;
- older step directories are deleted after a newer save
  (`src/app/TrainerCore.cpp:1600-1610`).

## 7. Render path

Scene storage: `means[N,3]`, `quats[N,4]` `(w,x,y,z)`, log `scales[N,3]`,
logit `opacities[N]`, `features_dc[N,3]`, `features_sh[N,K,3]`
(`src/primitives/Primitive.cuh`). Camera parameters are set through
`set_camera_params(width, height, model, distortion, viewmats, intrins,
dist_coeffs)` (`src/engine/Engine.h:75-84`).

Stages:

1. projection forward (`3dgs`/`mip`/`3dgut`, packed and unpaced):
   `src/engine/EngineForward.cpp:160-330`;
2. tile intersection and per-tile coverage with an int32-safe coarsening:
   `src/kernels/tile/IntersectTile.cu:420-580`;
3. 64-bit key sort `(tile_id << 32 | depth)` and per-tile offsets
   (`IntersectTile.cu:517-548`); masked tiles can be skipped exactly;
4. front-to-back alpha compositing with early-out `next_T <= 1e-4`, writing
   RGB, ray depth, transmittance, last IDs, optional median depth and
   distortion (`src/kernels/raster/RasterizationEval3DFwd_kernel.cuh:234-325`);
5. appearance chain: background -> optional PPISP -> color-space encode ->
   optional PPISP -> bilateral grid (`src/engine/EngineForward.cpp:478-489`);
6. loss + backward and densification map
   (`src/engine/EngineLoss.cpp:969`, `src/engine/Engine.h:152-194`);
7. readback: `engine_copy_render_to_host` expands f16 on the host
   (`src/engine/EngineQuery.cpp:50-118`); `engine_blit_view` fuses viewer
   annotations into uint8 (`src/engine/Engine.h:620-657`).

Host/device split: host does parsing, decoding, warping, checkpoint tar/PLY
bytes, eval metrics, color correction and PNG encode; device does projection,
intersection, sort, raster, loss, backward, optimizer, densification and
appearance models. The backend seam is `backend::` plus per-kernel launch
declarations (`src/backend/README.md:8-80`).

## 8. GPU backend

- compile-time backend choice `SS_BACKEND=cuda|vulkan`
  (`cmake/SsBackendCuda.cmake:1-20`, `cmake/SsBackendVulkan.cmake:1-70`);
- runtime surface: device count/info/select/bind, memory usage, malloc/free,
  pinned host memory, sync/async memcpy, memset, events, stream/device sync,
  `last_error` (`src/backend/api/BackendRuntime.h:1-140`);
- device selection by `--device` index/name/uuid/auto; Vulkan has a scored
  auto-select (discrete > integrated > other, VRAM tie-break)
  (`src/app/cli/main.cpp:287-349`, `src/backend/vulkan/VulkanContext.h:44-46`);
- resource pool: high-water per named `PoolSlot`, arena-carved by phase with
  aliasing; reset frees all (`src/core/Tensor.h:139-240,662`,
  `src/core/PoolSlots.h:404-631`);
- errors: OOM names size/usage/device; Vulkan keeps a sticky error, a 60 s
  stall watchdog reports `VK_ERROR_DEVICE_LOST`; the trainer converts
  `backend::last_error()` after each step into
  `GPU backend error at step N: ...`
  (`src/backend/api/BackendRuntime.h:130-150`,
  `src/backend/vulkan/VulkanContext.cpp:694-742`,
  `src/app/TrainerCore.cpp:1633-1636`);
- shaders: Slang is the single device-math language. CUDA generates committed
  CUDA C++ headers; Vulkan compiles Slang to SPIR-V at build time with
  capability variants and embeds the blobs
  (`src/shaders/build.bash`, `src/backend/vulkan/shaders/SpirvShaders.cmake`);
- pipeline cache: process maps keyed by blob name / push size / (entry,
  specialization tuple); kernel ABI is push-constant bytes + 4-byte
  specialization constants (`src/backend/vulkan/VulkanPipelines.cpp:15-40`,
  `src/backend/vulkan/VulkanPipelines.h:1-55`);
- readback/sync: sync memcpy flushes and waits; async memcpy records into the
  stream; pinned memory is persistently mapped
  (`src/backend/vulkan/VulkanRuntime.cpp:612-880`).

## 9. Training

- Loss dictionary: `rgb_loss`, `psnr`, optional `ssim`/`flip`/`delta_e`,
  `depth`, `normal`, `alpha`, regularizers, `bilagrid_tv`, `ppisp_reg_*`,
  splat counters (`src/engine/EngineLoss.cpp:929-963`).
- Optimizer: Adam `beta1=0.9`, `beta2=0.999`, `eps=1e-15`, per-attribute
  scheduled learning rates with optional warmup
  (`src/engine/EngineOptim.cpp:667,768`, `src/app/TrainerCore.cpp:182-192,583-601`).
  Optional fused projection-backward + Adam, split batch, quantized SH state,
  color trust region (`src/engine/EngineConfig.h:102-200`).
- Step pipeline: forward -> loss + backward -> optimizer -> background ->
  bilagrid/PPISP -> densify (`src/engine/EngineTrainStep.cpp:141-310`).
- Densification/pruning schedule and limits: `src/engine/EngineConfig.h:209-290`,
  scheduler `src/engine/EngineDensify.cpp:156`.
- Evaluation: re-parse the held-out split with the same normalization, render
  one view per step, report `l1/psnr/ssim` and color-corrected variants,
  optionally LPIPS, write `metrics.json`
  (`src/app/TrainerCore.cpp:1994-2349`, `src/app/EvalMetrics.h:1-45`).
- Progress: `on_step` after every completed step with the engine mutex
  released; callbacks carry step, latency, splat count and loss map
  (`src/app/TrainerCore.h:178-186,219-227`,
  `src/app/TrainerCore.cpp:1861-1871`).

## 10. UI surface

Top-level screens (`src/app/gui/GuiApp.h:120`): `Home`, `NewDataset`, `Train`,
`Viewer`, `Batch`, `Mesh`; dispatch `src/app/gui/GuiApp.cpp:3329-3334`.
Major panels: config editor, viewport/compare/image compare, splat viewer,
SfM runner, mesh runner, batch, mask editor, edit tools, render session.
The GUI reads session progress from `TrainRunner` and submits commands through
`TrainRunner`; it does not save checkpoints itself.

The automation surface (off unless enabled) exposes `/ui/state`, tree, click,
move, drag, scroll, key, text, wait, drop and screenshot over a loopback HTTP
server (`src/app/gui/Automation.cpp:734-746`,
`src/app/gui/GuiApp.cpp:3105-3145`).

## 11. Reader/writer inventory

| artifact | direction | format | reference |
|---|---|---|---|
| dataset cameras/images/points | read | COLMAP text/binary | `src/data/parsers/ColmapParser.cpp` |
| dataset meta | read/write (edits only) | Nerfstudio `transforms.json` | `src/data/parsers/NerfstudioParser.cpp`, `src/data/SparseEdit.cpp` |
| dataset meta | read | Metashape XML/PSX | `src/data/parsers/MetashapeParser.cpp` |
| images / masks / depth / normals | read | PNG/JPEG/BMP/TGA/EXR/TIFF | `src/data/DataManager.cpp:447-660` |
| gauge | read | `gauge.txt` key/value | `src/data/parsers/DatasetCommon.cpp:682-707` |
| scene transform | write/read | `scene_transform.json` | `src/data/SceneTransform.cpp` |
| training config | write/read | flat `config.json` | `src/app/TrainerCore.cpp:759-768`, `src/config/TrainConfigJson.h` |
| splat | read/write | PLY ASCII/binary LE | `src/checkpoint/SplatPly.cpp` |
| checkpoint | write/read | `step-*.ckpt/{splat.ply,state.tar}` | `src/engine/EngineCheckpoint.cpp` |
| seed / laser cloud | read/write | PLY / E57 / LAS | `src/data/PointCloudFile.cpp`, `src/data/E57Reader.cpp` |
| eval metrics | write | `metrics.json` | `src/app/TrainerCore.cpp:2330-2350` |
| eval images | write | PNG | `src/app/TrainerCore.cpp:2183-2196` |
| mesh exports | write | PLY/OBJ(+MTL)/glTF/GLB/STL | `src/mesh/MeshExport.h` |
| GUI log / crash | write | text | `src/app/gui/GuiApp.cpp:562-574`, `src/app/CrashLog.cpp:464-469` |

## 12. Observable outputs and exit codes

| trigger | observable |
|---|---|
| CLI startup | device table with `*` on the selected device |
| dataset loaded | `Cameras: N (M after splitting), seed points: K (train_frame_scale=...)` |
| run started | `Output directory:`, `Resuming from:` when resuming, optional viewer URL |
| every 100 steps and last step | `step i/N (p%)  splats S  [elapsed | ETA]` plus configured metrics |
| checkpoint | `Checkpoint saved to: <path>` |
| success | `Training complete. Steps: N   Time: T`, eval lines, `Eval metrics written to ...` |
| CLI error | `error: <message>` on stderr, exit 1 |
| usage error / unknown tool | usage text, exit 2 |
| SIGINT during train | graceful stop, final checkpoint (unless `steps_per_save == 0`), exit 130 |
| GUI failure | `TrainError` phase with raw message; no process exit |
| crash | `<config dir>/crash.log` |

## 13. Error, cancellation and partial-failure matrix

| situation | reference behavior | rewrite requirement |
|---|---|---|
| invalid dataset/config | `std::runtime_error` before GPU work; CLI exit 1; GUI `LoadError` | typed error with stable code, same user-visible message content |
| unsupported flag | refused up front with `--<flag> is not supported yet` | typed `Unsupported` error, never silently ignored |
| unreadable image mid-run | `DataDecodeError`; CLI throws, GUI pauses for retry/stop | typed `DataDecode` error, retryable, session stays in a defined state |
| GPU submit/device loss | sticky backend error, step throws `GPU backend error at step N` | typed `GpuDeviceLost`/`GpuSubmit` error + diagnostic event |
| SIGINT | cooperative stop at step boundary; save; exit 130 | cancellation token checked at defined points; final checkpoint idempotent |
| GUI stop | `stop_requested`, optional save, skip eval, process survives | same session command, terminal event, saved result visible |
| checkpoint corrupt/old | actionable message, refuse resume unless resumable | typed error naming missing/invalid member and required action |
| write failure | exception; checkpoint dir may be partial | atomic temp+rename; failed write leaves prior checkpoint intact |
| crash | no completion marker, partial step dir remains | completion marker or atomic directory swap; resume ignores incomplete |

## 14. Compatibility invariants to preserve

1. COLMAP and Nerfstudio placement/pose conventions, intrinsics order and
   distortion tier slot order.
2. `scene_transform.json` field spellings and the `train_from_world` /
   `world_from_train` pair, including the georeference block.
3. Splat PLY property order and raw value meaning (log scale, logit opacity,
   wxyz quaternion, DC coefficient), plus the filter rules when exporting.
4. Checkpoint directory naming `step-%09d.ckpt` and the `splat.ply` +
   `state.tar` pair; `state.json` field names and `format_version` 1.
5. `config.json` flat flag keys; unknown keys tolerated, missing keys default.
6. CLI flag names, `-`/`_` equivalence, bools taking a value, exit codes
   0/1/2/130, and the stable log lines listed above.
7. Unified behavior between CLI and GUI through one session.

## 15. Gaps and uncertainties

- `format_version`/`quant_codec` are write-only in the reference; the rewrite
  must validate them or document that it keeps the same tolerance.
- `config.json` has no schema version; the rewrite adds one in P1-F02 with a
  compatibility reader for unversioned files.
- Parsing, post-split and preload have no cancellation in the reference. The
  rewrite contract makes them cooperative at file/step boundaries.
- Checkpoint writes are not atomic in the reference. The rewrite must add
  atomic replacement (P1-F05) while still reading existing checkpoints.
- `academic-baseline` is mentioned in a reference comment but is not in the
  preset table; the table is authoritative.
- The GUI's auxiliary editors (mask/edit/render/pick sheets) were inventoried
  only at the screen/panel boundary, not widget by widget.
- CUDA was not built or exercised in this environment; see
  [reference-baseline.md](reference-baseline.md) section 5.
