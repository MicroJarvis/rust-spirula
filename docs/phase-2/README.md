# Phase 2 documentation

| Feature | Document | Status |
|---|---|---|
| P2-F01 adapter selection / negotiation | [gpu-adapter.md](gpu-adapter.md) | 已完成 |
| P2-F02 device / queue / context | [device-context.md](device-context.md) | 已完成 |
| P2-F03 buffer / texture / readback | [gpu-resources.md](gpu-resources.md) | 已完成 |
| P2-F04 shader / pipeline contract | [gpu-pipelines.md](gpu-pipelines.md) | 已完成 |
| P2-F05 GPU errors / diagnostic events | [gpu-diagnostics.md](gpu-diagnostics.md) | 已完成 |
| P2-F06 headless GPU smoke | [gpu-smoke.md](gpu-smoke.md) | 已完成 |
| P2-F07 minimal CLI package / isolated launch | (not started) | 当前 |

P2-F07 is **当前** after P2-F06 docs-closure. Phase 2 exit conditions remain
open (packaging + remaining Phase 2 exit evidence). Application / train / UI
are not wired through this phase’s diagnose/smoke GPU boundary. P2-F05
diagnostics stay GPU-local (P6-F01 owns application session history). Real host
OOM remains unsupported; the 60 s stall watchdog stays deferred. Surface smoke
in F06 stayed an honest **unsupported** gate (no forged window; present not
verified).
