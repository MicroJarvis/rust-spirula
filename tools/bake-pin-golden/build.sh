#!/usr/bin/env bash
# Extract pin ColmapParser bake/fit into gitignored .build/, compile host-only dumper.
# Never commits C++ sources. Artifacts under bin/ and .build/ are gitignored.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
BUILD_DIR="${SPIRULA_BAKE_PIN_BUILD:-$SCRIPT_DIR/.build}"
OUT_DIR="$SCRIPT_DIR/bin"
PIN="${SPIRULA_REFERENCE_PIN:-9a24f09927944a795e7ba1bd8243a625bf419479}"
mkdir -p "$BUILD_DIR" "$OUT_DIR"

python3 "$SCRIPT_DIR/extract_ref.py" --out "$BUILD_DIR"

CXX="${CXX:-c++}"
INC=(-std=c++17 -O2 -I"$BUILD_DIR/ref/src")
"$CXX" "${INC[@]}" -c "$BUILD_DIR/ref/src/data/SourceCamera.cpp" -o "$OUT_DIR/SourceCamera.o"
"$CXX" "${INC[@]}" -c "$BUILD_DIR/ref/src/data/CameraMath.cpp" -o "$OUT_DIR/CameraMath.o"
"$CXX" "${INC[@]}" -c "$BUILD_DIR/ref/src/data/DistortionFit.cpp" -o "$OUT_DIR/DistortionFit.o"
"$CXX" "${INC[@]}" \
  "$BUILD_DIR/main.cpp" \
  "$OUT_DIR/CameraMath.o" "$OUT_DIR/SourceCamera.o" "$OUT_DIR/DistortionFit.o" \
  -o "$OUT_DIR/spirula-bake-pin-golden"

echo "built $OUT_DIR/spirula-bake-pin-golden (pin=$PIN; sources under $BUILD_DIR, gitignored)" >&2
