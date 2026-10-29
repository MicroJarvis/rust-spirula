# Host-only fixture regenerators (P3-F03)

No C++ sources are copied into this tree. Bake/fit goldens are regenerated
from in-repo Rust (`spirula-io::colmap::bake_and_fit`, `source_project`,
`FrameCamera::ray_to_pixel`, `colmap_to_c2w`).

## Bake-pin golden

```bash
cargo run -p spirula-io --example regen_bake_pin -- \
  fixtures/datasets/colmap-goldens/bake-pin
```

Writes `expected.txt`:
- bake rows: `id model tier fx fy cx cy source fit_max_px d[8]`
- `ray id` unfitted-source (or exact-bake) pixel triples
- `pose …` from pin qvec/tvec via `colmap_to_c2w`

Pin reference SHA (behavior only): `9a24f09927944a795e7ba1bd8243a625bf419479`.
