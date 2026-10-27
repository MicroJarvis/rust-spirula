# P3-F01: camera projection and coordinate conventions

Status: **已完成**. Code tip `37b7a734fe0f7126a51fa86bd9ec5d2ece6dd758`;
independent READY `/tmp/spirula-p3-f01-review/REVIEW-37b7a73.md`. Owners:
`spirula-core` (pure `f64` math), `spirula-io` (fixture / serialization only).
Builds on P1-F04 pinhole + OpenCV prefix. **Not** dataset parsers (P3-F03+),
UI, train, or GPU projection.

Pinned C++ reference: `9a24f09927944a795e7ba1bd8243a625bf419479`
(`CameraModel.h`, `CameraMath.h/.cpp`, `SourceCamera.h/.cpp`,
`DistortionFit.h/.cpp`).

## Capability binding

| Matrix | Scope delivered |
|---|---|
| C-DATA-11 | `CameraModel::{Pinhole,Fisheye,Equisolid,Equirectangular}` |
| C-DATA-12 | `DistortionTier::{None,OpenCv,ThinPrism}`; 8-wide slots; demote/promote |
| C-DATA-13 | `SourceCamera` forward project; `fit_camera` / `fit_camera_auto` (Chebyshev + LM + Lawson; ladder `FF→3F→33→03→01→00`; auto halfFOV 60°/70°); `kExactFitPx=0.1` redistort; active source `-1` vs original metadata |
| C-DATA-14 | OpenGL c2w (existing pose) + CV +Z rays / normalized coords; distort / undistort Newton / generate_ray / project_ray; `FrameCamera` panorama wrap, `pixel_ray` / `projection_jacobian` / `ray_pixel` (source = forward truth) |

## AD-P3-F01-01 — orthographic

Plan wording lists **正交**, but the pinned C++ `CameraModelType` has **no**
orthographic model (search of `CameraModel.h` / `CameraMath.*` /
`SourceCamera.*` / `DistortionFit.*` / host projection slang: zero hits).
Orthographic appears only as GUI/viewer viewport emulation, not a training
projection path.

**Decision:** orthographic is an **explicit unsupported** model request
(`SP-INP-004` / typed reject). It is **not** silently dropped from the
acceptance list and **not** invented as a Rust-only camera without a reference
numeric contract. Revisit only with a pinned reference behavior + fixtures.
**74 plan IDs unchanged.**

## AD-P3-F01-02 — CPU math vs GPU tolerance

C++ host `camhost` / `srccam` / `dsfit` are the CPU golden authority. Rust
matches host `f64`/`f32`-as-stored semantics within documented tolerances
(`1e-9` relative/abs for closed-form; Newton residuals as in reference). GPU /
WGSL tolerances stay for later render/train features — do not loosen CPU
goldens to match GPU.

## Architecture

- `spirula-core`: models, tiers, lens math, rays, source project, fit, pose.
  No IO/GPU/Slint.
- `spirula-io`: camera catalog JSON with typed validation; preserve original
  source params for rollback; no silent rewrite of original coefficients.
- Shared test entry: load C++-generated goldens under
  `fixtures/camera/cpp-golden/` and holdout cases; Rust must not self-mint the
  only authority numbers.
- C++ harness: `tools/camera-golden/` compiles **git-archive pin export**
  (repo-relative `../spirula-studio` via `git-common-dir`, or
  `SPIRULA_REFERENCE_REPO`); never patches the reference repo; no CUDA /
  direct Vulkan; `bin/` gitignored.

## CameraMath review notes (parity)

- `undistort_lens`: 8 Newton iters, FD step `1e-5`, det break `1e-12`, final
  residual **`< 0.01`**. Distinct from `valid_distortion` Jacobian bound
  `0.25 < min(det,j00,j11) < 4` + outward dot ≥ 0. Finite u/v and coeff rows
  are checked before any identity short-circuit.
- Wide `project_ray` uses `atan2(rxy, z)` (negative Z allowed for fisheye /
  equisolid past 90°).
- Panorama: `normalize_pixel` has **no** pole slack; `ray_in_frame` allows
  ±1 px vertical; full-longitude tol `abs(2πfx−width)≤1e-5·width`;
  `pixel_difference` wraps by `2πfx` (799↔1 at width 800 → Δx = −2).
- `pixel_ray`: fitted model is inverse **seed** only; `source_model ≥ 0`
  makes `ray_pixel` / refine use SourceCamera as ground truth.

## DistortionFit review notes (parity)

- Domain: first growing branch only (128 az, 48 coarse, 40 bisect).
- Report on independent uniform `(2n+1)` grid + 512 near-rim holdout — not
  training residual alone.
- Ladder masks `0xFF → 0x3F → 0x33 → 0x03 → 0x01 → 0x00`; auto picks pinhole
  iff halfFOV < 60°, fish always, equisolid iff > 70°; stop when max ≤ 0.1 px.
- Exact fit clears **active** redistort source to `-1` while
  `FittedWithSource::original_source_model` + `source_params` stay.

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
`SP-INP-001`). Never return NaN pixels/rays or pretend success. Catalog load
rejects zero/overflow size, non-positive/nonfinite focals, wrapped/unknown
source ids, and `source_params` that are not exactly 16 finite `f32` values.

## Compatibility

Keep P1 `PinholeCamera` / `CameraPose` / `OpenCvDistortion` public API and
existing `fixtures/golden/camera.json` numbers. New models are additive.

## Non-claims

- Not P3 dataset parsers; not redistort GPU kernels; not Slint; not train.
- Not Phase 2 surface present; not orthographic numeric support.
