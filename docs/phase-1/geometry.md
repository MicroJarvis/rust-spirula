# P1-F04: math, camera, and scene types

`spirula-core` owns CPU `f64` vectors, matrices, quaternions, one pinhole
camera, a similarity, and splat value checks. It does not read files, talk to
the GPU, or fit lenses.

## Reference behavior

Pinned sources: `src/data/CameraMath.h`, `src/core/CameraModel.h`,
`src/data/SceneTransform.h`, `src/data/SceneTransform.cpp`,
`src/checkpoint/SplatPly.h`, and the Phase 0 fixtures under `fixtures/`.

- `c2w` is an OpenGL camera-to-world pose: +X right, +Y up, −Z forward.
  The rotation is row-major and acts on column vectors:
  `p_world = R p_gl + t`, so `p_gl = R^T (p_world - t)`.
- A point is in front only when the OpenGL camera-space `z` is strictly
  negative. `z >= 0` (the plane and everything behind it) is rejected.
- Normalized image coordinates are `(x / -z, -y / -z)`. That is the OpenCV
  camera frame (+Z forward, +Y down) without a second matrix.
- The OpenCV tier used by the fixture is `k1 k2 p1 p2`. The other four slots
  of the reference's 8-wide row are zero and are not a second model.
  `r2 = u^2 + v^2`, `radial = 1 + k1 r2 + k2 r2^2`,
  `u' = u radial + 2 p1 u v + p2 (r2 + 2 u^2)`,
  `v' = v radial + p1 (r2 + 2 v^2) + 2 p2 u v`,
  then `pixel = (fx u' + cx, fy v' + cy)`.
- Scene similarity, row-major `R`: `p_train = scale * R p_world + t`.
  The inverse is `R_w = R^T`, `scale_w = 1/scale`,
  `t_w = -(1/scale) R^T t`.
  `translation_before_rotation = R^T t / scale`.
- Quaternions in the reference splat file and in `quaternion_wxyz` are
  `(w, x, y, z)`. `quaternion_xyzw` is the same rotation with the components
  swapped. A 90 degree rotation about +Z is
  `wxyz = (√2/2, 0, 0, √2/2)` and the row-major matrix
  `[0, -1, 0, 1, 0, 0, 0, 0, 1]`.
- Stored splat values are raw: log scale, logit opacity, `(w, x, y, z)`, and
  DC. Activated scale is `exp(log scale)`, activated opacity is the logistic
  of the logit, and base color is `dc * C0 + 0.5` with
  `C0 = 0.28209479177387814`.

## Binding decisions

- Vectors, matrices, and quaternions that enter the API must be finite.
  A quaternion with positive length is normalized. A zero quaternion is
  `SP-INP-003`.
- `Mat3` is row-major. `try_inverse` returns `SP-INP-003` when the
  determinant is non-finite or smaller than `1e-12` in magnitude.
- Poses and scene rotations must be proper rotations: `R^T R = I` and
  `det(R) = 1`, each within `1e-9`. Reflections and non-orthogonal matrices
  are `SP-INP-001`.
- Focal lengths and similarity scale must be finite and strictly positive
  (`SP-INP-003`). Width and height must be non-zero (`SP-INP-003`).
- Behind-camera projection returns `SP-INP-003` with field `camera_z` and
  expected `< 0`. It does not return a pixel.
- Splat log-scales must stay finite after `exp`. Opacity is stored as a
  finite logit; the logistic saturates toward 0 or 1 for extreme logits.
- Golden camera pixels must match within `1e-9` px.

## Deferred

Full lens models (fisheye, equisolid, equirectangular, thin-prism, undistort
Newton, ray generation) stay in P3-F01. Scene JSON, centering modes, and
georeference stay in P3-F05. Splat PLY layout stays in P3-F06. This slice
checks values; it does not parse those files.
