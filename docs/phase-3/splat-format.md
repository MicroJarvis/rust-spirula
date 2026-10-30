# P3-F06 — Gaussian splat PLY format

**Status:** 已完成（workspace 门禁通过，待主线审核合并）。

The reader and writer live in `spirula-io::ply` and reuse
`spirula-core::{GaussianSplat, SplatSet}`. The PLY property set is the format
identity; the PLY itself has no version field and no Rust-only envelope is
introduced.

## Contract

The accepted vertex properties are exactly, in order:

```text
x y z | nx ny nz | f_dc_0 f_dc_1 f_dc_2 | opacity | scale_0 scale_1 scale_2 | rot_0 rot_1 rot_2 rot_3
```

Readers accept ASCII and `binary_little_endian`. Each property must be scalar
floating point. Writers emit binary little-endian `float32`; normals are
written as zero because the domain model does not carry normals. Raw values
remain compatible with the reference:

- `scale_*` are log scales;
- `opacity` is a logit;
- `rot_*` is a normalized `(w, x, y, z)` quaternion;
- `f_dc_*` is `(base_color - 0.5) / SH_C0`.

`read_ply_splats_budgeted` uses the existing cumulative file/memory budget and
cooperative cancellation checkpoints. The parser rejects missing or reordered
properties, list properties, non-finite values, empty sets, truncated or
trailing bodies, and output values that cannot be represented as finite
`float32`. `write_ply_splats_budgeted` performs checked size arithmetic and
pre-reserves the output before publishing.

Writes use the existing temporary-file plus atomic-replace path. Validation or
cancellation leaves an existing destination unchanged.

## Evidence

`fixtures/splats/tiny_splat.ply`, `render_splat.ply`, `seed_points.ply` and
`golden/splats.json` remain the compatibility fixtures. The focused
`splat_ply` integration tests cover raw decode, bounds, ASCII input, binary
round-trip, malformed order/empty input, cancellation, output budgets and
finite `float32` conversion.

## Boundaries

This slice does not implement checkpoint bundles, SH bands above the fixture's
degree-0 property set, training cache, render kernels, or edited/chunked
export. Those remain P3-F07, P3-F08, P4 and P7 work respectively.
