# COLMAP bake/fit C++ pin golden (P3-F03)

Host-only dumper for bake/fit + `camhost::ray_pixel` + pose against pin
`9a24f09927944a795e7ba1bd8243a625bf419479`.

**Does not commit C++ sources.** `extract_ref.py` pulls exact ColmapParser
bake/fit bodies and model/fit support into **gitignored** `.build/`;
`build.sh` compiles them with CameraMath / SourceCamera / DistortionFit from
the same pin.

```bash
# Optional: override discovery of spirula-studio
export SPIRULA_REFERENCE_REPO=/path/to/spirula-studio

./tools/bake-pin-golden/build.sh
./tools/bake-pin-golden/bin/spirula-bake-pin-golden \
  > fixtures/datasets/colmap-goldens/bake-pin/expected.txt
# then refresh fixtures/manifest.json digests
```

Output rows:
- `id model tier fx fy cx cy source fit_max_px d[8]`
- `ray id` three pixel pairs from C++ `camhost::ray_pixel` (source when fitted,
  else baked `project_ray`) on fixed unfitted rays
- `pose …` from pin qvec/tvec via extracted `colmap_to_c2w`

Rust holdout (`pin_bake_host_golden_holdout`) compares Rust **baked**
`FrameCamera::ray_to_pixel` against those C++ ray pixels with **absolute**
pixel tolerances — not both sides via under-test `source_project`.

`bin/` and `.build/` are gitignored.
