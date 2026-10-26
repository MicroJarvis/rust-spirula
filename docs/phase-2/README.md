# Phase 2 documentation

| Feature | Document | Status |
|---|---|---|
| P2-F01 adapter selection / negotiation | [gpu-adapter.md](gpu-adapter.md) | 已完成 |
| P2-F02 device / queue / context | [device-context.md](device-context.md) | 已完成 |
| P2-F03 buffer / texture / readback | [gpu-resources.md](gpu-resources.md) | 已完成 |
| P2-F04 shader / pipeline contract | [gpu-pipelines.md](gpu-pipelines.md) | 已完成 |
| P2-F05 GPU errors / diagnostic events | [gpu-diagnostics.md](gpu-diagnostics.md) | 当前 |
| P2-F06 headless GPU smoke | (not started) | 未开始 |

P2-F06+ are not started until P2-F05 is independently approved and docs-closed.
Phase 2 exit conditions remain open. Application / train / UI are not wired
through this phase’s diagnose-only GPU boundary (`--compute` is not readiness).
P2-F05 diagnostics stay GPU-local (P6-F01 owns application session history).
