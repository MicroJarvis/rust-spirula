# Host-only fixture regenerators (P3-F03)

Bake/fit goldens are a **C++ pin oracle**. No C++ sources are copied into this
tree; `tools/bake-pin-golden` extracts/compiles them under a gitignored
`.build/` directory.

## Bake-pin golden

```bash
export SPIRULA_REFERENCE_REPO=/path/to/spirula-studio   # optional
./tools/bake-pin-golden/build.sh
./tools/bake-pin-golden/bin/spirula-bake-pin-golden \
  > fixtures/datasets/colmap-goldens/bake-pin/expected.txt
```

Writes `expected.txt`:
- bake rows: `id model tier fx fy cx cy source fit_max_px d[8]`
- `ray id` three pixel pairs from C++ `camhost::ray_pixel` (source when fitted,
  else baked) on fixed unfitted rays
- `pose …` from pin qvec/tvec via extracted `colmap_to_c2w`

Pin reference SHA: `9a24f09927944a795e7ba1bd8243a625bf419479`.

See `tools/bake-pin-golden/README.md`. The `spirula-io` example
`regen_bake_pin` refuses to overwrite this golden from under-test Rust.
