#!/usr/bin/env bash
# Host-only P3-F02 golden dumper (ImageOrient + cpu_resize). Pin via git archive.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
OUT_DIR="$SCRIPT_DIR/bin"
PIN="${SPIRULA_REFERENCE_PIN:-9a24f09927944a795e7ba1bd8243a625bf419479}"
REPO_TOP="$(git -C "$SCRIPT_DIR" rev-parse --show-toplevel)"
MAIN_REPO="$(cd "$(git -C "$SCRIPT_DIR" rev-parse --path-format=absolute --git-common-dir)/.." && pwd)"

resolve_ref() {
  if [[ -n "${SPIRULA_REFERENCE_REPO:-}" ]]; then
    cd "$SPIRULA_REFERENCE_REPO" && pwd
    return
  fi
  local cand
  for cand in "$MAIN_REPO/../spirula-studio" "$REPO_TOP/../spirula-studio" "$REPO_TOP/../../../spirula-studio"; do
    if [[ -f "$cand/src/core/ImageOrient.h" ]]; then
      cd "$cand" && pwd
      return
    fi
  done
  echo "error: set SPIRULA_REFERENCE_REPO" >&2
  exit 1
}

REF="$(resolve_ref)"
git -C "$REF" cat-file -t "${PIN}^{commit}" >/dev/null
EXTRACT="$OUT_DIR/ref-pin"
rm -rf "$EXTRACT"
mkdir -p "$EXTRACT" "$OUT_DIR"
git -C "$REF" archive --format=tar "$PIN" -- src/core/ImageOrient.h | tar -x -C "$EXTRACT"
echo "reference=$REF pin=$PIN" >&2
c++ -std=c++17 -O2 -I"$EXTRACT/src" "$SCRIPT_DIR/main.cpp" -o "$OUT_DIR/spirula-image-orient-golden"
echo "built $OUT_DIR/spirula-image-orient-golden" >&2
