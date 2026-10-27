# C++ camera-math golden harness (P3-F01)

Builds a **host-only** dumper against the pinned Spirula Studio checkout
(`9a24f099…`). Compiles reference `CameraMath.cpp` + `SourceCamera.cpp`
**read-only** (no edits under the reference repo). No CUDA / direct Vulkan.

```bash
export SPIRULA_REFERENCE_REPO=/path/to/spirula-studio   # optional if default path works
./tools/camera-golden/build.sh
./tools/camera-golden/bin/spirula-camera-golden \
  > fixtures/camera/cpp-golden/camera_math.json
```

Rust holdout: `cargo test -p spirula-core --test camera_math_golden`.
Artifacts under `bin/` are gitignored.
