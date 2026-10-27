#!/usr/bin/env bash
# Build host CameraMath golden dumper against pinned reference sources (read-only).
# Does not modify the reference repo. No CUDA / direct Vulkan.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
if [[ -n "${SPIRULA_REFERENCE_REPO:-}" ]]; then
  REF="$(cd "$SPIRULA_REFERENCE_REPO" && pwd)"
elif [[ -f "$HOME/Projects/spirula-studio/src/data/CameraMath.cpp" ]]; then
  REF="$HOME/Projects/spirula-studio"
elif [[ -f "/Users/tfjiang/Projects/spirula-studio/src/data/CameraMath.cpp" ]]; then
  REF="/Users/tfjiang/Projects/spirula-studio"
else
  echo "SPIRULA_REFERENCE_REPO not found" >&2
  exit 1
fi
OUT_DIR="$SCRIPT_DIR/bin"
mkdir -p "$OUT_DIR"
PIN="$(git -C "$REF" rev-parse HEAD)"
echo "reference=$REF pin=$PIN" >&2
if [[ "$PIN" != "9a24f09927944a795e7ba1bd8243a625bf419479" ]]; then
  echo "error: expected pin 9a24f099…, got $PIN" >&2
  exit 1
fi
c++ -std=c++17 -O2 -I"$REF/src" \
  -c "$REF/src/data/SourceCamera.cpp" -o "$OUT_DIR/SourceCamera.o"
c++ -std=c++17 -O2 -I"$REF/src" \
  -c "$REF/src/data/CameraMath.cpp" -o "$OUT_DIR/CameraMath.o"
c++ -std=c++17 -O2 -I"$REF/src" \
  "$SCRIPT_DIR/main.cpp" "$OUT_DIR/CameraMath.o" "$OUT_DIR/SourceCamera.o" \
  -o "$OUT_DIR/spirula-camera-golden"
echo "built $OUT_DIR/spirula-camera-golden" >&2
