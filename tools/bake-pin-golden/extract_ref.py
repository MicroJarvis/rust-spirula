#!/usr/bin/env python3
"""Extract pin ColmapParser bake/fit bodies + model/fit support into a gitignored build dir.

Never commits C++ sources. Pin 9a24f099… — host-only compile under .build/.
"""
from __future__ import annotations

import argparse
import json
import os
import subprocess
from pathlib import Path

PIN = "9a24f09927944a795e7ba1bd8243a625bf419479"

REF_FILES = [
    "src/core/CameraModel.h",
    "src/data/CameraMath.h",
    "src/data/CameraMath.cpp",
    "src/data/SourceCamera.h",
    "src/data/SourceCamera.cpp",
    "src/data/DistortionFit.h",
    "src/data/DistortionFit.cpp",
]

# Same 18 param vectors as fixtures/datasets/colmap-goldens/bake-pin/inputs.json
PARAMS = [
    [500, 320, 240],
    [500, 510, 320, 240],
    [500, 320, 240, 0.01],
    [500, 320, 240, 0.01, -0.002],
    [500, 510, 320, 240, 0.01, -0.002, 0.0001, -0.0002],
    [500, 510, 320, 240, 0.01, -0.002, 0.0003, -0.00004],
    [500, 510, 320, 240, 0.01, -0.002, 0.0001, -0.0002, 0.0003, 0.00004, -0.000005, 0.000001],
    [500, 510, 320, 240, 0.9],
    [500, 320, 240, 0.01],
    [500, 320, 240, 0.01, -0.002],
    [500, 510, 320, 240, 0.01, -0.002, 0.0001, -0.0002, 0.0003, -0.00004, 0.00005, -0.00006],
    [
        500,
        510,
        320,
        240,
        0.01,
        -0.002,
        0.0003,
        -0.00004,
        0.000005,
        -0.000001,
        0.0001,
        -0.0002,
        0.00005,
        -0.00006,
        0.00001,
        -0.00002,
    ],
    [500, 320, 240, 0.01],
    [500, 510, 320, 240, 0.01],
    [500, 320, 240],
    [500, 510, 320, 240],
    [500, 510, 320, 240, 0.6, 1.1],
    [640, 480],
]

RAYS = [(0.0, 0.0, 1.0), (0.2, -0.1, 1.0), (-0.15, 0.25, 1.0)]


def resolve_ref(explicit: str | None) -> Path:
    if explicit:
        return Path(explicit).resolve()
    here = Path(__file__).resolve().parent
    for cand in (
        here.parents[3] / "spirula-studio",
        here.parents[2] / "spirula-studio",
        Path.home() / "Projects" / "spirula-studio",
    ):
        if (cand / "src/data/parsers/ColmapParser.cpp").is_file():
            return cand.resolve()
    raise SystemExit("set SPIRULA_REFERENCE_REPO to spirula-studio checkout")


def git_show(repo: Path, path: str) -> str:
    return subprocess.check_output(
        ["git", "-C", str(repo), "show", f"{PIN}:{path}"], text=True
    )


def block(src: str, start: str, semi: bool = False) -> str:
    i = src.index(start)
    b = src.index("{", i)
    depth = 0
    for j in range(b, len(src)):
        if src[j] == "{":
            depth += 1
        elif src[j] == "}":
            depth -= 1
            if not depth:
                return src[i : j + 1] + (";" if semi else "")
    raise ValueError(start)


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

    for name in REF_FILES:
        p = out / "ref" / name
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_text(git_show(ref, name))

    s = git_show(ref, "src/data/parsers/ColmapParser.cpp")
    h = git_show(ref, "src/data/DatasetParser.h")

    parts = [
        "#include <algorithm>\n"
        "#include <array>\n"
        "#include <cmath>\n"
        "#include <cstdio>\n"
        "#include <cstring>\n"
        "#include <map>\n"
        "#include <string>\n"
        "#include <stdexcept>\n"
        "#include <vector>\n"
        '#include "core/CameraModel.h"\n'
        '#include "data/CameraMath.h"\n'
        '#include "data/DistortionFit.h"\n'
        '#include "data/SourceCamera.h"\n'
        "constexpr double kPi=3.14159265358979323846;"
    ]
    for t in ["ColmapCamera", "ColmapImage", "RedistortSource"]:
        parts.append(block(h, "struct " + t + " {", True))
    parts += [
        block(s, "struct ColmapModelInfo {", True),
        block(s, "const std::map<int, ColmapModelInfo>& colmap_model_table()"),
        block(s, "int colmap_model_id("),
        block(s, "struct BakedIntrins {", True),
        block(s, "BakedIntrins bake_colmap_intrins("),
        block(s, "struct FitReport {", True),
        block(s, "void fit_colmap_source("),
        block(s, "void qvec2rotmat("),
        block(s, "void colmap_to_c2w("),
    ]

    (out / "inputs.json").write_text(json.dumps(PARAMS))

    parts.append("int main(){ std::map<std::string,FitReport> reports;")
    for i, p in enumerate(PARAMS):
        plist = ",".join(map(str, p))
        parts.append(
            "{ ColmapCamera c; c.camera_id="
            + str(i)
            + "; c.model=colmap_model_table().at("
            + str(i)
            + ").name; c.width=640;c.height=480;c.params={"
            + plist
            + "};"
            "auto b=bake_colmap_intrins(c);"
            "if(b.source.source_model>=0)fit_colmap_source(c,b,reports);"
            'printf("'
            + str(i)
            + ' %d %d %.17g %.17g %.17g %.17g %d %.17g",'
            "(int)b.model,(int)b.distortion,(double)b.fx,(double)b.fy,"
            "(double)b.cx,(double)b.cy,b.source.source_model,"
            "(double)b.source.fit_max_px);"
            "for(auto x:b.dist)printf(\" %.17g\",(double)x);puts(\"\");"
            # C++ corresponding path for independent unfitted rays (source when
            # fitted, else baked project_ray) — not under-test Rust regen.
            "camhost::Camera cam; cam.model=(int)b.model; cam.tier=(int)b.distortion;"
            "cam.width=640; cam.height=480;"
            "cam.fx=b.fx; cam.fy=b.fy; cam.cx=b.cx; cam.cy=b.cy;"
            "for(int di=0;di<8;++di) cam.dist[di]=b.dist[di];"
            "cam.source_model=b.source.source_model;"
            "std::memcpy(cam.source_params,b.source.params,sizeof(cam.source_params));"
            'printf("ray '
            + str(i)
            + '");'
            + "".join(
                f"{{ double ray[3]={{{x},{y},{z}}}; double px[2];"
                f"if(!camhost::ray_pixel(cam,ray,px)){{px[0]=px[1]=nan(\"\");}}"
                f'printf(" %.17g %.17g",px[0],px[1]); }}'
                for (x, y, z) in RAYS
            )
            + 'puts("");'
            "}"
        )
    parts.append(
        "ColmapImage im;im.qvec={0.9238795325112867,0,0.3826834323650898,0};"
        "im.tvec={1,2,3};double pose[12];colmap_to_c2w(im,pose);"
        'printf("pose");for(auto x:pose)printf(" %.17g",x);puts("");}'
    )
    (out / "main.cpp").write_text("\n".join(parts))
    (out / "provenance.txt").write_text(
        f"pin={PIN}\n"
        "Extracted exact function bodies from git show "
        f"{PIN}:src/data/parsers/ColmapParser.cpp; type declarations from "
        "DatasetParser.h; model/fit support compiled unchanged from pin. "
        "Output rows: id model tier fx fy cx cy source fit_max_px d[8]; "
        "ray id px0 py0 px1 py1 px2 py2 via camhost::ray_pixel; pose.\n"
    )
    print(f"extracted pin sources under {out}", flush=True)


if __name__ == "__main__":
    main()
