# Phase 2 documentation

| Feature | Document | Status |
|---|---|---|
| P2-F01 adapter selection / negotiation | [gpu-adapter.md](gpu-adapter.md) | 已完成 |
| P2-F02 device / queue / context | [device-context.md](device-context.md) | 已完成 |
| P2-F03 buffer / texture / readback | [gpu-resources.md](gpu-resources.md) | 已完成 |
| P2-F04 shader / pipeline contract | [gpu-pipelines.md](gpu-pipelines.md) | 已完成 |
| P2-F05 GPU errors / diagnostic events | (not started) | 当前 |

P2-F06+ (surface smoke, package) are not started.
Phase 2 exit conditions remain open. Application / train / UI are not wired
through this phase’s diagnose-only GPU boundary (`--compute` is not readiness).
