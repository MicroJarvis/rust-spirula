# C++ camera golden harness (P3-F01)

Builds a **host-only** dumper against pinned Spirula Studio sources
(`9a24f099…`) via `git archive` of seven files (CameraModel.h, CameraMath,
SourceCamera, DistortionFit). Never patches the reference working tree. No
CUDA / direct Vulkan.

```bash
# Optional: override discovery of <main-repo>/../spirula-studio
export SPIRULA_REFERENCE_REPO=/path/to/spirula-studio
# Optional pin override (default 9a24f099…)
# export SPIRULA_REFERENCE_PIN=9a24f09927944a795e7ba1bd8243a625bf419479

./tools/camera-golden/build.sh
./tools/camera-golden/bin/spirula-camera-golden \
  > fixtures/camera/cpp-golden/camera_math.json
```

`build.sh` refuses a dirty pin path when HEAD equals the pin but the seven
files differ from the committed tree, and always compiles the archived pin
export under `bin/ref-pin/` (not the live checkout).

Schema v2 JSON covers: model×tier math grid, domain/boundary rays (incl.
wide negative-Z), Newton residual vs `valid_distortion`, panorama seam /
no-pole-slack normalize, SourceCamera samples, DistortionFit auto+empty with
holdout metrics and active/original source ids.

Rust holdout: `cargo test -p spirula-core --test camera_math_golden`.
Artifacts under `bin/` are gitignored — do not commit them.
