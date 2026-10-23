# P0-F01: pinned C++ reference baseline

Status: tracked by [../project-plan.md](../project-plan.md). This document is
the single record of *which* C++ tree the Rust rewrite is compared against and
*how* that tree is built, tested and diagnosed.

## 1. Pinned identity

| item | value |
|---|---|
| upstream repository | `https://github.com/harry7557558/spirula-studio.git` |
| pinned commit | `9a24f09927944a795e7ba1bd8243a625bf419479` |
| short commit | `9a24f099` |
| commit date (UTC) | 2026-10-22T05:12:35Z (`2026-10-22 01:12:35 -0400`) |
| commit subject | `training quality: parameter tuning` |
| application version string | `2026.10.16` (`cmake/SsOptions.cmake:66`) |
| reference language standard | C++17 (`cmake/SsOptions.cmake:264-265`) |
| license | GPL-3.0 |

The reference checkout is local and is **not** a dependency of this workspace.
By default the tooling looks for it in `../spirula-studio` relative to this
repository; set `SPIRULA_REFERENCE_REPO` to point somewhere else:

```bash
export SPIRULA_REFERENCE_REPO=/path/to/spirula-studio
```

The baseline is the **committed** tree at the pinned commit. Local untracked
files (for example a developer `CMakePresets.json`) are not part of it. Any
tracked modification makes the checkout invalid as evidence; verify with:

```bash
bash tools/reference-baseline.sh check
```

`check` fails when the checkout is missing, the commit differs, or tracked
files are dirty, and prints the local tool versions. It does not build anything.

## 2. Toolchain and dependency versions

These are the versions the reference build selects. "Pinned" means the build
fetches this exact version; "minimum" means the build accepts anything at or
above the stated version.

| component | version | source |
|---|---|---|
| CMake | >= 3.18, Ninja generator | `CMakeLists.txt:1`, `build_develop.bash` |
| C++ compiler | C++17, any of GCC/Clang/MSVC | `cmake/SsOptions.cmake:264-265` |
| CUDA backend | CUDA toolkit found by `find_package(CUDAToolkit REQUIRED)`; CUDA standard 17; architecture list from `nvidia-smi` or `-DCMAKE_CUDA_ARCHITECTURES`/`TORCH_CUDA_ARCH_LIST` | `cmake/SsBackendCuda.cmake:9`, `:26-52`, `:149` |
| Vulkan backend (Linux/Windows) | Vulkan SDK/loader; headers >= 277, tested with 1.4.357 | `cmake/SsVulkan.cmake:70-80` |
| Vulkan backend (macOS) | MoltenVK `1.4.2`, statically linked by default (`SS_MACOS_VULKAN=static`) | `cmake/SsVulkan.cmake:15-16` |
| Slang (`slangc`) | `2026.12.0.1` on Linux/Windows, `2026.12.1` on macOS (only release with macOS assets) | `cmake/SsSlang.cmake:6-12` |
| Python (codegen/lints) | Python 3; optional because generated files are committed | `build_develop.bash:17-24` |
| GUI dependencies | GLFW `3.4`, Dear ImGui `v1.92.8` (FetchContent, GUI builds only) | `cmake/SsApps.cmake:196-203` |
| OpenMP (macOS) | Homebrew `libomp` keg, passed as `-DOpenMP_ROOT` | `build_develop.bash:93-111` |
| macOS system packages | `cmake`, `ninja`, `libomp` | `docs/build.md` (reference) |
| CI Linux | `ubuntu-24.04`, packages `ninja-build libvulkan-dev libgl1-mesa-dev xorg-dev libwayland-dev libwayland-bin wayland-protocols libxkbcommon* libxrandr-dev libxinerama-dev libxcursor-dev libxi-dev spirv-tools` | `.github/workflows/ci.yml` |
| CI macOS | `macos-15`, `brew install cmake ninja libomp` | `.github/workflows/ci.yml` |

The reference build has no package manifest that pins transitive libraries:
CUDA, Vulkan, Slang and MoltenVK are provisioned as above, and the Vulkan
SPIR-V blobs are compiled at build time and never committed.

## 3. Build entry points

Always build through the reference dev scripts, which run codegen, the source
lints, then CMake configure + build:

```bash
# Linux/macOS, Vulkan backend (verified by upstream CI)
bash build_develop.bash -DSS_BACKEND=vulkan

# Linux/macOS, CUDA backend
bash build_develop.bash -DSS_BACKEND=cuda

# Vulkan without a window (no GLFW/GL downloads)
bash build_develop.bash -DSS_BACKEND=vulkan -DSS_BUILD_GUI=OFF

# Vulkan + parity tests on the CUDA branch is opt-in; the Vulkan branch builds them
bash build_develop.bash -DSS_BACKEND=cuda -DSS_BUILD_BACKEND_TESTS=ON

# Windows: locates Visual Studio with vswhere, then calls vcvars64
build_develop.bat -DSS_BACKEND=vulkan
```

Build trees are per backend: `build/` on macOS, `build_cuda/` and
`build_vulkan/` on Linux/Windows. Selecting a tree with `-B` is not supported;
use `SS_BACKEND` instead. The reference documentation is
`docs/build.md` in that checkout.

## 4. Test and failure-log entry points

The native parity tests are the reference suite (`docs/testing.md` in the
reference checkout). There is no single `cargo test` equivalent:

| entry | purpose | command |
|---|---|---|
| parity tools | CUDA-vs-Vulkan numerical comparison, dump then compare | `./build_cuda/<tool> dump ref.bin` then `./build_vulkan/<tool> compare ref.bin` |
| engine tests | end-to-end render/train/reset invariants | `./build_vulkan/engine_render_parity`, `engine_train_parity`, `engine_reset_state`, `gt_bilagrid_sentinels` |
| Vulkan smoke tests | runtime, pipeline, sort/scan | `./build_vulkan/vk_runtime_smoke`, `vk_pipeline_smoke`, `vk_sortscan_test` |
| CTest subset | `hevc_sps_crop_test` and other tests registered with CTest | `ctest --test-dir build_vulkan -L headless` |
| CLI smoke | training CLI still starts | `./build_vulkan/spirula train --help` |

The parity tools live in `src/backend/tests/`, `src/backend/tests/engine/`,
`src/backend/vulkan/tests/`, `src/sfm/tests/`, `src/nn/tests/` and
`src/sam/tests/`. Each `.cpp` builds one executable of the same base name in
the backend tree. Comparison is tolerance-based; see `docs/testing.md` for
the per-test gates.

Failure evidence is captured in three places:

- CLI failures print `error: <message>` on stderr and exit `1` (usage errors
  exit `2`, SIGINT exits `130`);
- the GUI writes `<config dir>/crash.log`;
- a build or test log can be captured by the baseline script into
  `artifacts/reference-baseline/<UTC timestamp>/`, which is gitignored.

Capture an environment and command log (no build is run):

```bash
bash tools/reference-baseline.sh capture
# writes artifacts/reference-baseline/<timestamp>/{environment.txt,git.txt,README.txt}
```

To record an actual build/test run, tee the commands from section 3 and 4 into
the same directory, for example:

```bash
bash build_develop.bash -DSS_BACKEND=vulkan 2>&1 | tee artifacts/reference-baseline/<ts>/build.log
./build/spirula train --help            2>&1 | tee artifacts/reference-baseline/<ts>/smoke.log
```

## 5. Environment snapshot recorded for Phase 0

Recorded on the development machine used for this phase (not part of the
pinned baseline; see [exit-report.md](exit-report.md) for the full record):

| item | value |
|---|---|
| OS | macOS 27.0.1, arm64 |
| CMake | 4.3.0 |
| Ninja | not installed |
| Apple Clang | 21.0.0 |
| Python | 3.11.2 |
| CUDA / nvcc | not installed |
| Slang / Vulkan SDK | not installed |
| Rust | 1.99.0 (workspace toolchain) |

Because Ninja, CUDA and the Vulkan SDK/Slang are absent, the reference build
and parity suite were **not** run in this phase. The exact commands above are
the reproducible entry points; the skip and its reason are recorded in the
Phase 0 exit report. A feature that depends on C++ parity evidence must run
them on a machine where the toolchain exists before it is marked complete.

## 6. Drift policy

- The reference commit is pinned. Moving to a newer reference commit is its
  own change: record the new commit, re-run the inventory/matrix, and call out
  behavior differences in the affected feature records.
- Do not copy reference source into this repository; only record behavior,
  formats and golden values.
- When a fixture or contract is derived from the reference, cite the reference
  `path:line` in the fixture description or capability matrix.
