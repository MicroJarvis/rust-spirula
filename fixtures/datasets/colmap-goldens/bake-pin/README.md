# COLMAP bake/fit host pin golden (P3-F03)

C++ pin bake/fit + ray + pose holdout for
`9a24f09927944a795e7ba1bd8243a625bf419479`. **No C++ sources** are checked in.

| File | Role |
|---|---|
| `inputs.json` | 18 model parameter vectors (640×480) |
| `expected.txt` | Rows: `id model tier fx fy cx cy source fit_max_px d[8]`; `ray id` pixel triples from C++ `camhost::ray_pixel`; `pose …` |
| `provenance.txt` | Pin SHA + extract note |

## Regenerate (host-only C++ oracle)

```bash
export SPIRULA_REFERENCE_REPO=/path/to/spirula-studio   # optional
./tools/bake-pin-golden/build.sh
./tools/bake-pin-golden/bin/spirula-bake-pin-golden \
  > fixtures/datasets/colmap-goldens/bake-pin/expected.txt
# then refresh fixtures/manifest.json digests via xtask / fixtures checker
```

`tools/bake-pin-golden/extract_ref.py` writes reference sources only under
**gitignored** `.build/`. Do not use under-test Rust `bake_and_fit` /
`source_project` to overwrite this golden.

Rust holdout (`pin_bake_host_golden_holdout`) compares baked
`FrameCamera::ray_to_pixel` against the C++ ray pixels with absolute pixel
tolerances.
