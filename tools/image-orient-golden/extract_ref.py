#!/usr/bin/env python3
"""Extract real pin sources into a gitignored build dir (no committed copies)."""
from __future__ import annotations

import argparse
import os
import subprocess
from pathlib import Path

PIN = "9a24f09927944a795e7ba1bd8243a625bf419479"


def resolve_ref(explicit: str | None) -> Path:
    if explicit:
        return Path(explicit).resolve()
    here = Path(__file__).resolve().parent
    for cand in (
        here.parents[3] / "spirula-studio",
        here.parents[2] / "spirula-studio",
        Path.home() / "Projects" / "spirula-studio",
    ):
        if (cand / "src/data/DataManager.cpp").is_file():
            return cand.resolve()
    raise SystemExit("set SPIRULA_REFERENCE_REPO to spirula-studio checkout")


def git_show(repo: Path, path: str) -> bytes:
    return subprocess.check_output(["git", "-C", str(repo), "show", f"{PIN}:{path}"])


def extract_function_block(src: str, start_marker: str, end_marker: str) -> str:
    i = src.find(start_marker)
    if i < 0:
        raise SystemExit(f"missing marker {start_marker!r}")
    j = src.find(end_marker, i)
    if j < 0:
        raise SystemExit(f"missing end marker {end_marker!r} after {start_marker!r}")
    return src[i:j]


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", type=Path, required=True)
    ap.add_argument("--ref", default=os.environ.get("SPIRULA_REFERENCE_REPO"))
    args = ap.parse_args()
    ref = resolve_ref(args.ref)
    out: Path = args.out
    out.mkdir(parents=True, exist_ok=True)

    subprocess.check_call(
        ["git", "-C", str(ref), "cat-file", "-t", f"{PIN}^{{commit}}"],
        stdout=subprocess.DEVNULL,
    )

    dm = git_show(ref, "src/data/DataManager.cpp").decode()
    # Templates: bilinear through nearest (inclusive of helpers used by goldens).
    resize_blob = extract_function_block(
        dm,
        "template<typename T, int C>\ninline void cpu_bilinear_resize",
        "\n\n// One cached RGB row",
    )
    # turn_normal_vectors for C-DATA-09 goldens
    turn_blob = extract_function_block(
        dm,
        "void turn_normal_vectors(uint8_t* px, size_t n, int turns_cw)",
        "\n\n// Straight alpha over",
    )

    orient_h = git_show(ref, "src/core/ImageOrient.h")
    edt_h = git_show(ref, "src/core/DistanceTransform.h")
    exif_h = git_show(ref, "src/sfm/core/Exif.h").decode()
    # Pull only ExifTransform + exifTransform (no TIFF reader deps).
    xf = extract_function_block(
        exif_h,
        "struct ExifTransform {",
        "\n\n// Up, in the camera frame",
    )

    (out / "ImageOrient.h").write_bytes(orient_h)
    (out / "DistanceTransform.h").write_bytes(edt_h)
    (out / "exif_transform.inc").write_text(
        "// extracted from sfm/core/Exif.h @ " + PIN + "\n" + xf + "\n",
        encoding="utf-8",
    )
    (out / "cpu_resize.inc").write_text(
        "// extracted from data/DataManager.cpp @ " + PIN + "\n"
        + "#include <cmath>\n#include <cstdint>\n#include <limits>\n#include <type_traits>\n"
        + resize_blob
        + "\n",
        encoding="utf-8",
    )
    (out / "turn_normal.inc").write_text(
        "// extracted from data/DataManager.cpp @ " + PIN + "\n"
        + "#include <cstddef>\n#include <cstdint>\n"
        + turn_blob
        + "\n",
        encoding="utf-8",
    )
    (out / "PIN.txt").write_text(PIN + "\n" + f"reference={ref}\n", encoding="utf-8")
    print(f"extracted pin={PIN} into {out}", flush=True)


if __name__ == "__main__":
    main()
