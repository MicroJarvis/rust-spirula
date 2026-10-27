#!/usr/bin/env bash
# Build host camera golden dumpers against a pinned C++ reference export.
# Never patches the reference working tree; compiles from `git archive` of the pin.
# No CUDA / direct Vulkan. Artifacts under bin/ are gitignored.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
OUT_DIR="$SCRIPT_DIR/bin"
PIN="${SPIRULA_REFERENCE_PIN:-9a24f09927944a795e7ba1bd8243a625bf419479}"

REPO_TOP="$(git -C "$SCRIPT_DIR" rev-parse --show-toplevel)"
# Main checkout root (worktree-safe): parent of the shared .git directory.
MAIN_REPO="$(cd "$(git -C "$SCRIPT_DIR" rev-parse --path-format=absolute --git-common-dir)/.." && pwd)"

resolve_ref() {
  if [[ -n "${SPIRULA_REFERENCE_REPO:-}" ]]; then
    cd "$SPIRULA_REFERENCE_REPO" && pwd
    return
  fi
  local cand
  for cand in \
    "$MAIN_REPO/../spirula-studio" \
    "$REPO_TOP/../spirula-studio" \
    "$REPO_TOP/../../spirula-studio" \
    "$REPO_TOP/../../../spirula-studio"; do
    if [[ -f "$cand/src/data/CameraMath.cpp" ]]; then
      cd "$cand" && pwd
      return
    fi
  done
  echo "error: set SPIRULA_REFERENCE_REPO to the spirula-studio checkout" >&2
  echo "  (default: <main-repo>/../spirula-studio via git-common-dir)" >&2
  exit 1
}

REF="$(resolve_ref)"
if ! git -C "$REF" cat-file -t "${PIN}^{commit}" >/dev/null 2>&1; then
  echo "error: pin $PIN not found in $REF" >&2
  exit 1
fi

REF_FILES=(
  src/core/CameraModel.h
  src/data/CameraMath.h
  src/data/CameraMath.cpp
  src/data/SourceCamera.h
  src/data/SourceCamera.cpp
  src/data/DistortionFit.h
  src/data/DistortionFit.cpp
)

# Refuse a dirty pin path in the live tree when HEAD matches pin but contents differ.
if [[ "$(git -C "$REF" rev-parse HEAD)" == "$PIN" ]]; then
  dirty="$(git -C "$REF" status --porcelain -- "${REF_FILES[@]}")"
  if [[ -n "$dirty" ]]; then
    echo "error: reference HEAD is pin $PIN but these files are dirty:" >&2
    echo "$dirty" >&2
    echo "  clean the tree or point SPIRULA_REFERENCE_REPO at a clean checkout" >&2
    exit 1
  fi
fi

EXTRACT="$OUT_DIR/ref-pin"
rm -rf "$EXTRACT"
mkdir -p "$EXTRACT" "$OUT_DIR"
git -C "$REF" archive --format=tar "$PIN" -- "${REF_FILES[@]}" | tar -x -C "$EXTRACT"

echo "reference=$REF pin=$PIN (git archive, not live tree)" >&2

CXX="${CXX:-c++}"
INC=(-std=c++17 -O2 -I"$EXTRACT/src")

"$CXX" "${INC[@]}" -c "$EXTRACT/src/data/SourceCamera.cpp" -o "$OUT_DIR/SourceCamera.o"
"$CXX" "${INC[@]}" -c "$EXTRACT/src/data/CameraMath.cpp" -o "$OUT_DIR/CameraMath.o"
"$CXX" "${INC[@]}" -c "$EXTRACT/src/data/DistortionFit.cpp" -o "$OUT_DIR/DistortionFit.o"
"$CXX" "${INC[@]}" \
  "$SCRIPT_DIR/main.cpp" \
  "$OUT_DIR/CameraMath.o" "$OUT_DIR/SourceCamera.o" "$OUT_DIR/DistortionFit.o" \
  -o "$OUT_DIR/spirula-camera-golden"

echo "built $OUT_DIR/spirula-camera-golden" >&2
