# COLMAP bake/fit host pin golden (P3-F03)

Host-only bake/fit + unfitted-ray + pose holdout for pin
`9a24f09927944a795e7ba1bd8243a625bf419479`. **No C++ sources** are checked in.

| File | Role |
|---|---|
| `inputs.json` | 18 model parameter vectors (640×480) |
| `expected.txt` | Rows: `id model tier fx fy cx cy source fit_max_px d[8]`; `ray id` pixel triples; `pose …` |
| `provenance.txt` | Pin SHA + regen command |

## Regenerate (in-repo)

```bash
cargo run -p spirula-io --example regen_bake_pin -- \
  fixtures/datasets/colmap-goldens/bake-pin
# then refresh fixtures/manifest.json digests via xtask / fixtures checker
```

See also `tools/fixtures/README.md`.
