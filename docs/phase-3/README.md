# Phase 3 documentation

| Feature | Document | Status |
|---|---|---|
| P3-F01 camera projection / coordinates | [camera.md](camera.md) | 已完成 |
| P3-F02 image orientation / mask | [image-orient-mask.md](image-orient-mask.md) | 进行中 (HOLD) |
| P3-F03..F08 | (not started) | 未开始 |

P3-F01 delivered C-DATA-11..14 (code tip `37b7a73…`; READY
`/tmp/spirula-p3-f01-review/REVIEW-37b7a73.md`; landed on main). Orthographic
remains AD-P3-F01-01 unsupported (`SP-INP-004`). CPU host goldens stay
authoritative vs later GPU tolerances. Exact-fit active source `-1` keeps
original source metadata.

This branch (`codex/p3-f02`) implements C-DATA-07..10 (PNG/JPEG/BMP/TGA/TIFF/EXR,
mask flip/offset, depth/normal, EXIF) in `spirula-io` plus admitted session
commands. Plan IDs total **75** (P2-F08 added on main). Status **进行中** /
**HOLD** until independent review — full matrix scope is not shrunk in docs.
Current plan work is **P3-F02**.
