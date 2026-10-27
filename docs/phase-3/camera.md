# P3-F01: camera projection and coordinate conventions

Status: **当前 / HOLD-MERGE** (tip in progress). Owners: `spirula-core`
(pure `f64` math), `spirula-io` (fixture / serialization only). Builds on
P1-F04 pinhole + OpenCV prefix. **Not** dataset parsers (P3-F03+), UI, train,
or GPU projection.

Pinned C++ reference: `9a24f09927944a795e7ba1bd8243a625bf419479`
(`CameraModel.h`, `CameraMath.h/.cpp`, `SourceCamera.h/.cpp`,
`DistortionFit.h/.cpp`).

## Capability binding

| Matrix | Scope in this tip |
|---|---|
| C-DATA-11 | `CameraModel::{Pinhole,Fisheye,Equisolid,Equirectangular}` |
| C-DATA-12 | `DistortionTier::{None,OpenCv,ThinPrism}`; 8-wide slots; demote/promote |
| C-DATA-13 | `SourceCamera` forward project + `kExactFitPx=0.1` redistort decision; `fit_camera` / `fit_camera_auto` port (fixture-backed) |
| C-DATA-14 | OpenGL c2w (existing pose) + CV +Z rays / normalized coords; distort / undistort Newton / generate_ray / project_ray / panorama wrap helpers |

## AD-P3-F01-01 — orthographic

Plan wording lists **正交**, but the pinned C++ `CameraModelType` has **no**
orthographic model (search of `CameraModel.h` / `CameraMath.*` /
`SourceCamera.*` / `DistortionFit.*` / host projection slang: zero hits).
Orthographic appears only as GUI/viewer viewport emulation, not a training
projection path.

**Decision for this tip:** orthographic is an **explicit unsupported** model
request (`SP-INP-004` / typed reject). It is **not** silently dropped from the
acceptance list and **not** invented as a Rust-only camera without a reference
numeric contract. Revisit only with a pinned reference behavior + fixtures.

## AD-P3-F01-02 — CPU math vs GPU tolerance

C++ host `camhost` / `srccam` / `dsfit` are the CPU golden authority. Rust
matches host `f64`/`f32`-as-stored semantics within documented tolerances
(`1e-9` relative/abs for closed-form; Newton residuals as in reference). GPU /
WGSL tolerances stay for later render/train features — do not loosen CPU
goldens to match GPU.

## Architecture

- `spirula-core`: models, tiers, lens math, rays, source project, fit, pose.
  No IO/GPU/Slint.
- `spirula-io`: read/write camera fixture JSON and preserve source params for
  rollback; no silent rewrite of original coefficients.
- Shared test entry: load C++-generated goldens under
  `fixtures/camera/cpp-golden/` and holdout cases; Rust must not self-mint the
  only authority numbers.
- C++ harness: `tools/camera-golden/` compiles **read-only** reference `.cpp`
  files; never patches the reference repo; no CUDA / direct Vulkan.

## Coefficient slots (per-tier)

| Tier | Width used | Order |
|---|---|---|
| None | 0 | all zeros |
| OpenCv | 4 | `k1 k2 p1 p2` |
| ThinPrism | 8 | `k1 k2 k3 k4 p1 p2 sx1 sy1` |

Demote/promote match `CameraModel.h` (exact zero only). Equirectangular only
pairs with `None` for compiled kernels; other pairs may exist as data but are
rejected by `is_compiled_pair` when asked for engine-ready cameras.

## Failure honesty

Non-finite, overflow, zero-length rays, Newton non-convergence, folded
Jacobian, behind-camera / out-of-domain → typed `DomainError` (`SP-INP-003` or
`SP-INP-001`). Never return NaN pixels/rays or pretend success.

## Compatibility

Keep P1 `PinholeCamera` / `CameraPose` / `OpenCvDistortion` public API and
existing `fixtures/golden/camera.json` numbers. New models are additive.

## Non-claims

- Not P3 dataset parsers; not redistort GPU kernels; not Slint; not train.
- Not Phase 2 surface present; not orthographic numeric support.
- Not marking P3-F01 **已完成** until independent READY + full C-DATA-11..14
  evidence (this doc stays HOLD meanwhile).
