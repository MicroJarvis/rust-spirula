# COLMAP bake/fit host pin golden (P3-F03)

Host-only extract from pin `9a24f09927944a795e7ba1bd8243a625bf419479`
(`ColmapParser.cpp` bake + fit). **No C++ sources** are checked in.

| File | Role |
|---|---|
| `inputs.json` | 18 model parameter vectors (640×480) |
| `expected.txt` | Host rows: `id model tier fx fy cx cy source fit_max_px d[8]` + `pose …` |
| `provenance.txt` | Pin SHA + extraction notes |

Regenerate outside the repo (see `/tmp/spirula-p3-f03-pin-golden/`), then copy
only these host artifacts and refresh `fixtures/manifest.json`.
