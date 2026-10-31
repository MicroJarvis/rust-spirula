# Checkpoint metadata (P3-F07)

P3-F07 reads the legacy reference layout
`step-*.ckpt/{state.tar,splat.ply}`. It is a compatibility reader only: it does
not write generations or restore a new process (those behaviours belong to
P5-F05/P5-F07).

`state.tar` must be a POSIX ustar archive with unique, relative, regular-file
members, valid header checksums, complete member padding, and a two-zero-block
trailer. Extra zero padding after the trailer is accepted for the reference
fixture; non-zero bytes after the trailer are corrupt. Member paths reject
absolute paths, `..`, `.`/empty components, repeated separators, and Windows
backslashes. Missing/truncated members use `SP-CKPT-003`; malformed headers,
duplicate/unsafe members, and invalid payloads use `SP-CKPT-002`.

`state.json` requires `format_version: 1`, `full_resume` (0 or 1), step/count
fields, SH fields, and `primitive`. Unknown versions return `SP-FMT-003`.
The required `world.means.npy` and `world.opacities.npy` blocks are validated as
NPY v1, v2, or v3 little-endian `<f4`, C-order arrays. Header shape arithmetic
must match the exact payload byte length, every value must be finite, and the
first dimension of both required arrays must equal `cur_num_splats`. Unknown
`.npy` members are retained as metadata and parsed with the same format,
length, finiteness, cancellation, and memory-budget checks.

`full_resume: 1` is reported as `FullResume` only when the complete legacy
metadata set and `splat.ply` are present. `full_resume: 0` is `ModelOnly` and
`require_full_resume` returns `SP-CKPT-001`. A PLY without `state.tar` is
incomplete and never claims full resume. The reader observes cooperative
cancellation at archive entries and NPY payload chunks; cumulative bytes are
charged against the existing bounded load-memory budget.

The fixture and focused tests cover full/model-only classification, unknown
versions, missing blocks, truncation, path traversal, duplicate members,
non-finite values, count mismatches, PLY-only inputs, cancellation, and the
stable checkpoint error codes.
