#!/usr/bin/env bash
# Extract pin sources into gitignored .build/, compile golden dumper. Never commit .build/ or bin/.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
BUILD_DIR="${SPIRULA_IMAGE_GOLDEN_BUILD:-$SCRIPT_DIR/.build}"
OUT_DIR="$SCRIPT_DIR/bin"
PIN="${SPIRULA_REFERENCE_PIN:-9a24f09927944a795e7ba1bd8243a625bf419479}"
mkdir -p "$BUILD_DIR" "$OUT_DIR"
python3 "$SCRIPT_DIR/extract_ref.py" --out "$BUILD_DIR"
c++ -std=c++17 -O2 -I"$BUILD_DIR" "$SCRIPT_DIR/main.cpp" -o "$OUT_DIR/spirula-image-orient-golden"
echo "built $OUT_DIR/spirula-image-orient-golden (pin sources under $BUILD_DIR, gitignored)" >&2
