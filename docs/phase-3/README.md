# Phase 3 documentation

| Feature | Document | Status |
|---|---|---|
| P3-F01 camera projection / coordinates | (tip on `codex/p3-f01`, **not merged** to this baseline) | main plan still 当前 |
| P3-F02 image orientation / mask | [image-orient-mask.md](image-orient-mask.md) | 进行中 |
| P3-F03..F08 | (not started) | 未开始 |

This branch (`codex/p3-f02`) is cut from main `dfb17ab` and does **not** include
unmerged P3-F01 camera work. P3-F02 implements C-DATA-10 / C-DATA-08 (partial
C-DATA-07 PNG/JPEG) in `spirula-io` plus a minimal `spirula-application`
session entry. Format scope stays PNG+JPEG; mask EDT stays out. 74 plan IDs
unchanged.
