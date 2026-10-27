# Phase 3 documentation

| Feature | Document | Status |
|---|---|---|
| P3-F01 camera projection / coordinates | (tip on `codex/p3-f01`, **not merged** to this baseline) | main plan still 当前 |
| P3-F02 image orientation / mask | [image-orient-mask.md](image-orient-mask.md) | 进行中 |
| P3-F03..F08 | (not started) | 未开始 |

This branch (`codex/p3-f02`) is cut from main `dfb17ab` and does **not** include
unmerged P3-F01 camera work. P3-F02 implements C-DATA-07..10 (PNG/JPEG/BMP/TGA/
TIFF/EXR, mask flip/offset, depth/normal, EXIF) in `spirula-io` plus admitted
session commands (wiring not deferred). 74 plan IDs unchanged; status **进行中**.
