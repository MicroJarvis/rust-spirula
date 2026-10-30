# Phase 3 documentation

| Feature | Document | Status |
|---|---|---|
| P3-F01 camera projection / coordinates | [camera.md](camera.md) | 已完成 |
| P3-F02 image orientation / mask | [image-orient-mask.md](image-orient-mask.md) | 已完成 |
| P3-F03 COLMAP dataset read | [colmap.md](colmap.md) | 已完成 |
| P3-F04 Nerfstudio/Metashape | [nerfstudio-metashape.md](nerfstudio-metashape.md) | 进行中 (HOLD) |
| P3-F05..F08 | (not started) | 未开始 |

P3-F01 delivered C-DATA-11..14 (code tip `37b7a73…`; READY
`/tmp/spirula-p3-f01-review/REVIEW-37b7a73.md`; landed on main). Orthographic
remains AD-P3-F01-01 unsupported (`SP-INP-004`). CPU host goldens stay
authoritative vs later GPU tolerances. Exact-fit active source `-1` keeps
original source metadata.

**P3-F02 已完成并落地 main** (code tip
`6f2b79ca5da9af5d8b14f5b2e2cd9b16cdacbc90`; merge
`da0ea365f1a77d820c26e25390b6111aaf7587f2`). C-DATA-07..10 in `spirula-io` plus
admitted session prepare commands. Plan IDs total **75** (P2-F08 remains
未开始). Independent READY evidence on tip `6f2b79c…`:

- focused IO: `image_formats_aux` 9/9 + `image_orient_mask` 11/11
- `image_ops_e2e` 14/14
- after-start cancel regressions:
  `wide_row_bilinear_cancel_after_start_within_250ms`,
  `area_downsample_cancel_after_start_within_250ms`
- independent wide probe (`wide_mask_cancel`): `Canceled`,
  `cancel_observation_ms≈0`
- golden `cmp` `fixtures/images/cpp_orient_mask_golden.json` vs regenerated
- `cargo run -p spirula-xtask -- verify` → 8/8; `git diff --check` clean

Honest Non-claim/boundary (does **not** block READY): third-party `image`
decode is non-preemptible mid-call (runtime §5 per-image decode checkpoint);
owned prepare-path CPU uses bounded-chunk `CancelCheckpoint::tick` (≤250 ms
gap). Drop drain deadline ≠ proven hard worker join — see
[image-orient-mask.md](image-orient-mask.md).

**P3-F03 已完成并落地 main** (merge `a35823f…`). C-DATA-01..03 / C-DATA-06; see
[colmap.md](colmap.md). Remaining honest gap: dense `fit_model` single LM call
non-preemptible.

**P3-F04 进行中 (HOLD)** — tip pending capacity-cliff JSON DOM bound fix
(no merge). Nerfstudio `transforms.json` + self-contained PLY + Metashape
XML (+ `.psx` typed unsupported without zip). Lens adapt pins C++ 650–745
(no fake COLMAP model remaps). See
[nerfstudio-metashape.md](nerfstudio-metashape.md). Does not include P3-F05
center/normalize, P3-F06 splat, or P3-F08 cache.
