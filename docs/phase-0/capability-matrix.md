# P0-F03: capability matrix

Every capability observed in the reference tree (`9a24f099`) is listed with
its reference source, input, output, compatibility assumption, verification
method, owner crate and current state. "Owner" is the Rust crate that must
implement the capability, not the crate that merely consumes it.

States: `未开始` (no Rust implementation), `进行中`, `已完成`, `阻塞`. All
rows began as `未开始` in Phase 0; subsequent implementations are recorded
below. The target feature in the verification column is the completion gate.
The 2026-10-25 prospective additions follow [development gates](../development-gates.md)
and remain `未开始`.

Reference citations are `path:line` in the reference checkout. Fixture names
refer to [fixtures.md](fixtures.md).

## Data and formats

| ID | capability | reference | input | output | compatibility assumption | verification (target) | owner | state |
|---|---|---|---|---|---|---|---|---|
| C-DATA-01 | dataset auto-detection | `src/data/parsers/ColmapParser.cpp:1019-1062` | directory | format id | `transforms.json` > COLMAP > Metashape; COLMAP wins with fallback | fixture dataset + P3-F03 | `spirula-io` | 未开始 |
| C-DATA-02 | COLMAP cameras/images/points text | `ColmapParser.cpp:616-795` | `cameras.txt`, `images.txt`, `points3D.txt` | `ParsedDataset` | 18 model names, `.bin` preferred, points optional | `fixtures/dataset/tiny-colmap` + P3-F03 | `spirula-io` | 未开始 |
| C-DATA-03 | COLMAP binary records | `ColmapParser.cpp:616-621` | `.bin` | same as text | byte layout identical to COLMAP; no rewrite | P3-F03 golden | `spirula-io` | 未开始 |
| C-DATA-04 | Nerfstudio `transforms.json` | `NerfstudioParser.cpp:419-560` | JSON + images + optional PLY | `ParsedDataset` | `applied_transform` inversion; per-frame intrinsics | fixture transforms + P3-F04 | `spirula-io` | 未开始 |
| C-DATA-05 | Metashape XML/PSX | `MetashapeParser.cpp:385-785` | XML/ZIP tree + PLY | `ParsedDataset` | converted through a Nerfstudio-shaped meta; component selection | P3-F04 | `spirula-io` | 未开始 |
| C-DATA-06 | train/eval split | `DatasetCommon.cpp:196-260` | camera names + config | index lists | `all/fraction/interval/filename`; empty eval legal, empty train error | P3-F03 unit tests | `spirula-io` | 未开始 |
| C-DATA-07 | image decode (PNG/JPEG/BMP/TGA/EXR/TIFF) | `DataManager.cpp:447-518`, `DataManager.h:14-22` | file | RGB array | 8/16-bit PNG, float EXR/TIFF, no external codec runtime | fixture image + P3-F02 | `spirula-io` | 进行中 |
| C-DATA-08 | masks and alpha | `DataManager.cpp:521-615`, `ImageProbe.cpp:60` | aux files / alpha | binary mask | white keeps, alpha >= 128 keeps, flip/offset optional | P3-F02 fixtures | `spirula-io` | 进行中 |
| C-DATA-09 | depth/normal aux maps | `DataManager.cpp:616-660` | PNG/float files | arrays | 16-bit PNG depth, float maps, size mismatch resized later | P3-F02 | `spirula-io` | 未开始 |
| C-DATA-10 | EXIF orientation | `DatasetCommon.cpp:45-70`, `ImageOrient.h` | JPEG EXIF | quarter turns | `none/orient/apply`; rotation applies to all aux maps | P3-F02 | `spirula-io` | 进行中 |
| C-DATA-11 | camera models | `core/CameraModel.h:21-26` | model + params | typed camera | PINHOLE/FISHEYE/EQUISOLID/EQUIRECTANGULAR | P3-F01 golden | `spirula-core` | 未开始 |
| C-DATA-12 | distortion tiers | `core/CameraModel.h:28-50` | coefficients | tier + 8-wide row | slot order per tier; zero extension; cheapest exact tier | P3-F01 golden | `spirula-core` | 未开始 |
| C-DATA-13 | unrepresentable lens fitting | `data/DistortionFit.h`, `data/SourceCamera.h` | source params | fitted tier + redistort | fit never rejects a dataset; exact below 0.1 px is left alone | P3-F01 numeric fixture | `spirula-core` | 未开始 |
| C-DATA-14 | projection/coordinate conventions | `data/CameraMath.h:1-42`, `core/CameraDistortion.cuh` | camera + point | pixel/ray | OpenGL c2w, CV +Z rays, normalized image coords | P3-F01 fixture numbers | `spirula-core` | 进行中 |
| C-DATA-15 | scene transform serialization | `data/SceneTransform.cpp:244-331` | centering + similarity | `scene_transform.json` | all matrix/quaternion/Euler spellings; georeference block | `fixtures/scene` + P3-F05 | `spirula-core`/`spirula-io` | 进行中 |
| C-DATA-16 | scene-center modes | `data/SceneCenter.h`, `DatasetCommon.cpp:523-545` | camera/point statistics | translation | `none/point-median/camera-median/camera-focus/point-mean/camera-mean`, `auto` resolves | P3-F05 unit tests | `spirula-core` | 未开始 |
| C-DATA-17 | seed point cloud PLY | `NerfstudioParser.cpp:235-276` | PLY | points + RGB | requires x/y/z + RGB, ascii/BE; double coordinates | `fixtures/splats/points.ply` + P3-F05 | `spirula-io` | 未开始 |
| C-DATA-18 | splat PLY read/write | `checkpoint/SplatPly.cpp` | PLY | splat scene | property order/raw meaning; ASCII read, binary LE write | `fixtures/splats/tiny_splat.ply` + P3-F06 | `spirula-io` | 未开始 |
| C-DATA-19 | checkpoint read | `engine/EngineCheckpoint.cpp`, `core/CheckpointIO.h` | `state.tar` + `.npy` | state blocks | ustar + NPY v1-v3; `state.json` fields and `format_version` 1 | `fixtures/checkpoints` + P3-F07 | `spirula-io` | 未开始 |
| C-DATA-20 | checkpoint write | `EngineCheckpoint.cpp:354-390` | state blocks | `step-%09d.ckpt` | same layout; rewrite adds atomic replacement and completion marker | P5-F05 fault injection + P5-F07 new-process resume | `spirula-io` | 未开始 |
| C-DATA-21 | resume prerequisites | `checkpoint/Resume.cpp:78-140` | run dir/checkpoint | config + step | classify model-only/full resume; full state/dataset/plan required; retain scene_center legacy rule | P3-F07/P5-F05 | `spirula-io` | 未开始 |
| C-DATA-22 | config read/write | `config/TrainConfig.h`, `config/TrainConfigJson.h` | flat JSON | typed config | flat flag keys, unknown ignored, missing defaults, no version today | P1-F02 + `fixtures/config` | `spirula-core` | 已完成 |
| C-DATA-23 | atomic persistence | `core/AtomicFile.h:15-23` | bytes | file replace | temp + rename; reference checkpoints are not atomic | P1-F05 (crash/corrupt tests) | `spirula-io` | 已完成 |
| C-DATA-24 | mesh export formats | `mesh/MeshExport.h` | mesh | PLY/OBJ/glTF/GLB/STL | format set and color modes; no overwrite of input | P7-F04 round trip | `spirula-io` | 未开始 |
| C-DATA-25 | bounded training batch loader | `data/DataManager.cpp`; Brush `brush-dataset/scene_loader.rs` as secondary design reference | images + sampling state | ordered batches/cache | no new runtime dependency; bounded decode/prefetch/cache bytes | P3-F08 budget/cancel/order tests | `spirula-io` | 未开始 |
| C-DATA-26 | edited splat view and chunked export | `checkpoint/SplatPly.cpp`; OOOSplat export view as secondary design reference | selection/transform + model | PLY chunks | preserve reference coordinates/raw attributes; explicit compression memory limits | P7-F07 independent readback | `spirula-io` | 未开始 |

## Rendering and GPU

| ID | capability | reference | input | output | compatibility assumption | verification (target) | owner | state |
|---|---|---|---|---|---|---|---|---|
| C-REN-01 | scene/primitive representation | `primitives/Primitive.cuh`, `engine/Engine.h` | splats | device arrays | means/quats wxyz/log scales/logit opacity/DC/SH | P4-F01 fixtures | `spirula-render` | 进行中 |
| C-REN-02 | camera parameter binding | `engine/Engine.h:75-84` | camera + view matrices | GPU uniforms | intrinsics fx/fy/cx/cy; value-parameterized model/tier | P4-F01/P4-F02 | `spirula-render` | 未开始 |
| C-REN-03 | projection forward | `kernels/projection/*`, `EngineForward.cpp:160-330` | camera + splats | screen splats | 3dgs/mip/3dgut; packed/unpacked | P4-F03 CPU/GPU parity | `spirula-render` | 未开始 |
| C-REN-04 | tile intersection and binning | `kernels/tile/IntersectTile.cu:420-580` | screen splats | (tile, splat) counts | 8x8 tiles, macro tile 8-128, int32 coarsening | P4-F03 tolerance fixture | `spirula-render` | 未开始 |
| C-REN-05 | depth-key sort | `IntersectTile.cu:517-548` | tile/splat pairs | sorted pairs + offsets | `(tile_id << 32) \| depth`, deterministic ordering | P4-F04 determinism test | `spirula-render` | 未开始 |
| C-REN-06 | rasterize and composite | `kernels/raster/RasterizationEval3DFwd_kernel.cuh:234-325` | sorted pairs | RGB, T, depth | front-to-back, early-out 1e-4, ray depth | P4-F05 golden image | `spirula-render` | 未开始 |
| C-REN-07 | depth and median depth | `RasterizationEval3DFwd_kernel.cuh:262-322` | sorted pairs | depth buffers | expected ray depth; median via log-space 1/2 crossing | P4-F05 tolerance | `spirula-render` | 未开始 |
| C-REN-08 | appearance chain | `EngineForward.cpp:478-489` | rendered RGB | final RGB | background, PPISP, color space, bilateral grid in fixed order | P4-F05 golden | `spirula-render` | 未开始 |
| C-REN-09 | readback and preview | `EngineQuery.cpp:50-118`, `Engine.h:620-657` | GPU buffers | host image | f16 expanded on host; viewer blit to uint8 | P4-F06 application/CLI preview | `spirula-render`/`spirula-gpu` | 未开始 |
| C-REN-10 | CPU reference renderer | (new; reference parity tools) | scene + camera | image + tolerance | each GPU stage has CPU reference or deterministic fixture | P4-F01/P4-F03 | `spirula-render` | 未开始 |
| C-REN-11 | training forward saved state | `engine/EngineForward.cpp`, `kernels/raster/*` | scene/camera | float image + indexed intermediates | explicit saved/recomputed layout, lifetime and budget | P4-F07 forward parity | `spirula-render` | 未开始 |
| C-REN-12 | raster/compositing VJP | `kernels/raster/*`, `engine/EngineLoss.cpp` | image gradient + forward state | projected gradients | production thresholds recorded; smooth numerical probes separate | P4-F08 finite differences | `spirula-render` | 未开始 |
| C-REN-13 | projection/covariance/SH VJP | `kernels/projection/*`, `engine/EngineForward.cpp` | projected gradients + parameters | 3D/quat/scale/SH/opacity gradients | parameterization and viewdir-to-mean chain explicit | P4-F09 parameter-wise checks | `spirula-render` | 未开始 |
| C-REN-14 | gradient aggregation/refinement stats | reference backward families; Brush `brush-render/bwd` as secondary design reference | tile/visible gradients | full gradients + stats | capability-aware CAS/reduction, finite values and mapped indices | P4-F10 end-to-end backward | `spirula-render`/`spirula-gpu` | 未开始 |
| C-GPU-01 | adapter selection and limits | `backend/api/BackendRuntime.h`, `backend/vulkan/VulkanContext.h:44-46` | requested features | adapter + capabilities | score discrete > integrated > other, VRAM tie-break when known; wgpu VRAM/uuid unavailable honest; required floors preserved in ResolvedLimits | P2-F01 headless | `spirula-gpu` | 已完成 |
| C-GPU-02 | device/queue lifecycle | `backend/vulkan/VulkanContext.cpp`, `EngineState.cpp` | process | context | session-owned GpuContext (no global singleton); reset_scene≠close; lost latch→owner poll/reopen; app SessionState not updated from GPU | P2-F02 host+synthetic lifecycle | `spirula-gpu` | 已完成 |
| C-GPU-03 | buffer/texture/readback | `backend/api/BackendRuntime.h:1-140` | bytes/images | GPU + back | staging+explicit wait; RGBA8 pitch; accounted scoped bytes (not VRAM); labels; pending/inflight hold; ticket RAII; submission reap | P2-F03 host+synthetic round trip / diagnose --resources | `spirula-gpu` | 已完成 |
| C-GPU-04 | shader pipeline contract | `backend/vulkan/VulkanPipelines.*` | WGSL/shader entry | pipeline | stable key (name+version+entry+spec+bind_layout_version); affine v1/v2 same-ABI epochs; SP-GPU-005 path=`shaders/<name>.wgsl`; diagnose `--compute` | P2-F04 | `spirula-gpu` | 已完成 |
| C-GPU-05 | GPU errors and device loss | `VulkanContext.cpp:694-742`, `TrainerCore.cpp:1633-1636` | submit/wait | typed error + GPU-local diagnostics | error scopes→SP-GPU-001..005; Lost+reject; bounded ring; upload/readback/map metadata; post-submit waitable handle + latched wait Err; real OOM unsupported; no app session; stall watchdog deferred | P2-F05 | `spirula-gpu` | 已完成 |
| C-GPU-06 | headless smoke test | `backend/vulkan/tests/vk_*` | device | pass/fail/skip/unsupported | affine compute smoke (reuse F04/F05) + Fail reclaim; honest surface unsupported gate (present not verified); device/backend/features/limits + cleanup current0; packaging P2-F07 | P2-F06 | `spirula-gpu` | 已完成 |

## Training

| ID | capability | reference | input | output | compatibility assumption | verification (target) | owner | state |
|---|---|---|---|---|---|---|---|---|
| C-TRN-01 | training worker and scheduling | `TrainerCore.cpp:1227-1893` | config + dataset | events + checkpoint | loading/ready/training/evaluating/done/failed | P1-F07 application owner + P5-F01 step tests | `spirula-train` | 未开始 |
| C-TRN-02 | per-pixel loss | `EngineLoss.cpp:929-963` | render + GT | loss dict | L1/L2/YUV, SSIM/FLIP/DeltaE, depth/normal/alpha, regularizers | P5-F02 micro fixture | `spirula-train` | 未开始 |
| C-TRN-03 | optimizer | `EngineOptim.cpp:667,768`, `TrainerCore.cpp:182-192` | gradients | updated state | Adam beta/eps, scheduled LR + warmup, fused path optional | P5-F02 determinism test | `spirula-train` | 未开始 |
| C-TRN-04 | densification/pruning | `EngineConfig.h:209-290`, `EngineDensify.cpp:156` | step + stats | grown/pruned set | schedules, caps, opacity floor, relocation | P5-F03 reproducibility | `spirula-train` | 未开始 |
| C-TRN-05 | evaluation and metrics | `TrainerCore.cpp:1994-2349`, `EvalMetrics.h` | eval split | `metrics.json` | l1/psnr/ssim + color-corrected variants, optional LPIPS | P5-F04/P5-F07 | `spirula-train` | 未开始 |
| C-TRN-06 | progress events | `TrainerCore.h:178-186,219-227` | step | typed event | step/latency/splats/metrics, called outside engine lock | P5-F04/P6-F01 | `spirula-train` | 未开始 |
| C-TRN-07 | stop/pause/save-on-stop | `TrainerCore.h:270-280`, `TrainerCore.cpp:1764-1893` | command | final checkpoint | cooperative stop at step boundary, 130 on SIGINT | P5-F06 cancellation tests | `spirula-train` | 未开始 |
| C-TRN-08 | divergence detection | `TrainerCore.cpp:1644-1660` | loss/latency | warning event | non-finite or `rgb_loss > 1e3` logged once | P5-F06 | `spirula-train` | 未开始 |
| C-TRN-09 | validated executable TrainPlan | (new; reference config compatibility retained) | legacy config + dataset/GPU capabilities | resolved plan + digest | unsupported options fail; config acceptance is not execution support | P5-F08 validation/digest tests | `spirula-train`/`spirula-application` | 未开始 |

## UI

| ID | capability | reference | input | output | compatibility assumption | verification (target) | owner | state |
|---|---|---|---|---|---|---|---|---|
| C-UI-01 | window and screen navigation | `gui/GuiApp.h:120`, `GuiApp.cpp:3329-3334` | commands | screens | Home/NewDataset/Train/Viewer/Batch/Mesh | P6-F03 Slint behavior test | `spirula-ui` | 未开始 |
| C-UI-02 | config editor and validation | `gui/ConfigUI.h`, `config/TrainConfig.h` | config table | edited config | tiers/basic/advanced/expert and translated labels | P5-F08/P6-F03 supported-capability checks | `spirula-ui` | 未开始 |
| C-UI-03 | dataset load and preview | `gui/GuiApp.cpp:8605-9271`, `EngineDataManager.cpp:581` | dataset path | preview | loads through application/session; no direct IO/GPU | P6-F04 | `spirula-ui` | 未开始 |
| C-UI-04 | training start/stop/resume | `gui/TrainRunner.cpp:124-149`, `GuiApp.cpp:11598-11606` | commands | status + metrics | same session as CLI; stop may save | P6-F05 | `spirula-ui` | 未开始 |
| C-UI-05 | log and error feedback | `gui/GuiApp.cpp:10696-10699`, `TrainRunner.cpp:160-180` | events | log panel/modal | raw error text plus typed code; retry on data errors | P6-F03/P6-F06 | `spirula-ui` | 未开始 |
| C-UI-06 | GUI automation surface | `gui/Automation.cpp:734-746` | HTTP/UI events | state/tree/screenshot | loopback, off by default | P6-F06 (optional) | `spirula-ui` | 未开始 |
| C-UI-07 | shared session with CLI | `GuiApp.cpp:1391-1410` | application API | same events | CLI and UI submit the same commands | P6-F06 parity test | `spirula-application` | 未开始 |
| C-UI-08 | measured Slint presentation path | (new; reference viewer blit is behavior input) | application image/opaque presentation | displayed frame | no public wgpu in UI; shared/readback fallback cost recorded | P6-F07 copies/latency/lifetime | `spirula-ui`/`spirula-application`/`spirula-gpu` | 已完成 |

## CLI and application

| ID | capability | reference | input | output | compatibility assumption | verification (target) | owner | state |
|---|---|---|---|---|---|---|---|---|
| C-CLI-01 | flat flag parsing | `cli/main.cpp:82-135,499-527` | argv | `TrainConfig` | flag names, `-`/`_` equal, bools take values | P6-F02 golden parse | `spirula-cli` | 未开始 |
| C-CLI-02 | presets | `config/TrainConfig.h:440-600` | preset name | config | 7 named presets + GUI preset files | P6-F02/P6-F03 | `spirula-core`/`spirula-cli` | 未开始 |
| C-CLI-03 | train command flow | `cli/main.cpp:468-642` | args | run + logs | load -> setup -> train -> eval; `--help`/`--help-all` | P6-F02 | `spirula-cli` | 未开始 |
| C-CLI-04 | exit codes and error text | `cli/main.cpp:626-642`, `Main.cpp:266-273` | outcome | 0/1/2/130 | `error: <msg>` on stderr; usage 2; SIGINT 130 | P6-F02 | `spirula-cli` | 未开始 |
| C-CLI-05 | device selection flag | `cli/main.cpp:287-349` | index/name/uuid/auto | device | prints a table, warns on known-bad devices | P6-F02/P2-F01 | `spirula-cli`/`spirula-gpu` | 未开始 |
| C-CLI-06 | resume command | `cli/main.cpp:539-542`, `checkpoint/Resume.h` | run dir/checkpoint | resumed session | explicit flags override checkpoint config | P6-F02/P5-F05 | `spirula-cli` | 未开始 |
| C-CLI-07 | subcommands other than train | `Main.cpp:71-106`, `Tools.h:52-62` | argv | tool output | sfm/sam/mesh/geometry/dense/e57/lidar/partition/encode later | P8/P7 target features | `spirula-cli` | 未开始 |
| C-APP-01 | command/event session API | (new; reference is `TrainerSession` + GUI worker) | commands | events | one application owner, bounded history/snapshots, source seq gaps and resync | P1-F07/P6-F01 | `spirula-application` | 进行中（P1-F07 fake probe owner；真实业务 P6-F01） |
| C-APP-02 | cancellation and progress | (new; reference atomics) | token | events | cooperative, idempotent cancel, defined terminal event | P1-F03/P1-F07/P1-F09/P5-F06/P6-F01 | `spirula-core`/`spirula-application` | 进行中（P1-F03+P1-F07+P1-F09 SIGINT；训练取消 P5-F06） |

## Cross-cutting

| ID | capability | reference | input | output | compatibility assumption | verification (target) | owner | state |
|---|---|---|---|---|---|---|---|---|
| C-X-01 | error taxonomy | `runtime_error` everywhere; CLI text | failure | typed error | stable codes, category, retryability, context | P1-F01 | `spirula-core` | 已完成 |
| C-X-02 | logging and diagnostics | `TrainerCore.cpp:775-779`, `Log.h` | events | text | stable message content for scripts | P6-F02/P9-F05 | `spirula-application` | 未开始 |
| C-X-03 | catalog versioning | none in reference config/checkpoint | file | version + migration | add versions; keep unversioned readers | P1-F02/P3-F07 | `spirula-core`/`spirula-io` | 进行中 |
| C-X-04 | performance baseline | `docs/testing.md` (reference) | workload | metrics | per-slice resource/time records, quality/convergence and resume | P1-F08/P9-F02 | workspace | 进行中（P1-F08 报告格式/CPU 样本已完成；逐功能基线与 P9 汇总未开始） |
| C-X-05 | cross-platform adapter coverage | CI matrix | platform | report | macOS/Linux/Windows records, skip reasons | P9-F01 | `spirula-gpu` | 未开始 |
| C-X-06 | minimal CLI package | (new) | clean checkout | isolated runnable package | `spirula-xtask package`/`check`/`run`: locked archive build, MANIFEST+NOTICE+allowlist, canonical `bin/<name>`, one-shot NOREPLACE rename; macOS tested (Metal smoke); Linux implemented untested; Windows publish unsupported; no signed installer (P9-F04); preview/train samples later (P4-F06/P5-F07) | P2-F07/P4-F06/P5-F07 | `spirula-xtask` | 已完成 |
| C-X-07 | pinned tool/process management | reference auxiliary commands; OOOSplat engine/process policies as secondary reference | versioned runtime artifact/request | verified installation + validated output | existing stack rules remain; bounded logs, process-tree cancel and rollback | P8-F07 isolated tests + platform acceptance | `spirula-io`/`spirula-application` | 未开始 |

## Owners and phases summary

| owner crate | capability IDs | first target phase |
|---|---|---|
| `spirula-core` | C-DATA-11..16, C-DATA-22, C-CLI-02, C-APP-02, C-X-01, C-X-03 | P1-F01, P3-F01 |
| `spirula-io` | C-DATA-01..10, C-DATA-17..26, C-X-03, C-X-07 | P1-F05, P3-F02..F07 |
| `spirula-gpu` | C-GPU-01..06, C-REN-14, C-UI-08, C-CLI-05, C-X-05 | P2-F01..F06 |
| `spirula-render` | C-REN-01..14 | P4-F01..F10 |
| `spirula-train` | C-TRN-01..09 | P5-F08 validation first, P5-F01..F07 |
| `spirula-ui` | C-UI-01..06, C-UI-08 | P6-F07 early, P6-F03..F06 |
| `spirula-cli` | C-CLI-01..07 | P1-F09 early, P6-F02 |
| `spirula-application` | C-APP-01..02, C-TRN-09, C-UI-07..08, C-X-02, C-X-07 | P1-F07 early, P6-F01 |
| workspace/`spirula-xtask` | C-X-04, C-X-06 | P1-F08/P2-F07, P9-F02/P9-F04 |

## Gaps recorded by this matrix

- `C-X-01` is implemented in `spirula-core` (P1-F01): the 26-code catalog,
  categories, retryability, context keys, CLI text and exit codes. Other
  implementations and pending portions are recorded below.
- `C-DATA-22` is implemented in `spirula-core` (P1-F02): the 251-field
  config table, `schema_version` 1, and legacy reads. `C-X-03` stays
  `进行中` because checkpoint `format_version` validation is still P3-F07.
- `C-APP-02` has the core token and progress/terminal payloads (P1-F03),
  application channels (P1-F07), and CLI SIGINT (P1-F09). Actual training
  checkpoints and complete UI integration remain P5-F06/P6-F01, so the row
  stays `进行中`.
- `C-DATA-14` has the P0 pinhole path: OpenGL `c2w`, OpenCV `k1 k2 p1 p2`,
  and behind-camera rejection (P1-F04). Fisheye, equisolid, equirectangular,
  thin-prism, and ray generation stay in P3-F01 (unmerged tip), so the row
  stays `进行中`. `C-DATA-11` through `C-DATA-13` stay `未开始` on this
  baseline.
- `C-DATA-07` / `C-DATA-08` / `C-DATA-10` are **进行中** under P3-F02
  ([phase-3/image-orient-mask.md](../phase-3/image-orient-mask.md)): PNG/JPEG
  **header-only** size probe with encoded/pixel budgets before decode; JPEG
  EXIF Orientation 1..8 (full SHORT `u16`, LE/BE, bad IFD rejected); pixel bake
  with AD-P3-F02-01 turns-only / full policies; mask NonZero via stbi gray
  (alpha ignored) + nearest; Alpha LA/RGBA via `cpu_resize` then ≥128; C++
  pixel goldens + admitted `spirula-application` session commands (real IO
  e2e). EXR/TIFF/BMP/TGA, depth/normal, and mask boundary EDT stay out of
  this tip (plan scope; not shrunk for READY).
- `C-DATA-15` has the similarity `p_train = scale * R * p_world + t` and its
  inverse (P1-F04). Scene JSON, centering modes, and georeference stay in
  P3-F05, so the row stays `进行中`. `C-DATA-16` stays `未开始`.
- `C-REN-01` has CPU splat checks for finite means, `wxyz` quaternions, log
  scales, and logit opacity (P1-F04). Device arrays and SH bands stay in
  P4-F01, and the PLY layout stays in P3-F06, so the row stays `进行中`.
- `C-DATA-23` is implemented in `spirula-io` (P1-F05): unique sibling temps,
  completeness checks, POSIX rename / Windows `MoveFileExW` replace, orphan
  recovery, and the temp harness. Config save/migration with `{path}.bak`
  finishes the P1-F02 filesystem contract. Checkpoint bundle schema and
  completion markers remain P5-F05, so `C-DATA-20` stays `未开始`.
- The workspace verify gate is implemented by `spirula-xtask` (P1-F06):
  inherited lints, architecture edges, declared feature combinations, and
  compile steps pinned to the rustc host via `--target`. That gate is not
  `C-X-06` (minimal CLI package; P2-F07 → `已完成`, owner `spirula-xtask`).
  Multi-platform matrices remain `C-X-05` / P9-F01.
- The reference's `config.json` and checkpoint have no validated version
  field. Config now adds `schema_version` and keeps an unversioned reader.
  Checkpoint validation is unchanged.
- The reference has no CPU reference renderer for every stage; `C-REN-10`
  records the work the rewrite must add, not a reference feature.
- Auxiliary pipelines (SfM, SAM, video, dense, mesh, partitioning) are listed as
  `C-CLI-07` only at their entry boundary; they get their own matrix rows
  when Phase 7/8 starts; C-X-07 now separately tracks their shared tool/process
  foundation, not completed SfM/video algorithms.

- The new saved-state/VJP/aggregation rows remain unimplemented; existing CPU
  math/config/atomic tests do not validate GPU training. Per-stage budgets and
  full-resume tests remain target gates in
  [development-gates.md](../development-gates.md). Initial CLI packages are
  `C-X-06` / P2-F07（已完成）. `C-UI-08` / P6-F07 tip recorded per-frame copy
  bytes and host produce→deliver latency (CUA window draw); process-wide CPU
  total peak was **not** measured; software rendering notifier may stay
  unverified (`Unsupported`) while CUA-visible software draw is recorded
  separately — still **not** wgpu surface present.
