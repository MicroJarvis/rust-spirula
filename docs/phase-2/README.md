# Phase 2 documentation

| Feature | Document | Status |
|---|---|---|
| P2-F01 adapter selection / negotiation | [gpu-adapter.md](gpu-adapter.md) | 已完成 |
| P2-F02 device / queue / context | [device-context.md](device-context.md) | 已完成 |
| P2-F03 buffer / texture / readback | [gpu-resources.md](gpu-resources.md) | 已完成 |
| P2-F04 shader / pipeline contract | [gpu-pipelines.md](gpu-pipelines.md) | 已完成 |
| P2-F05 GPU errors / diagnostic events | [gpu-diagnostics.md](gpu-diagnostics.md) | 已完成 |
| P2-F06 headless GPU smoke | [gpu-smoke.md](gpu-smoke.md) | 已完成 |
| P2-F07 minimal CLI package / isolated launch | [cli-package.md](cli-package.md) | 已完成 |
| P2-F08 wgpu surface present verification | [gpu-surface-present.md](gpu-surface-present.md) | 未开始 |

P2-F01–P2-F07 are **已完成**. Phase 2 exit conditions remain **open**: surface
present is still **not** verified (F06 honest unsupported gate). Completing
every implemented Phase 2 feature ID does **not** close the Phase 2 exit —
including after early **P6-F07** (已完成; see
[../phase-6/presentation-path.md](../phase-6/presentation-path.md)), which
validated readback→Slint host frames without claiming wgpu surface present.
**P2-F08 owns the remaining exit condition** (added 2026-10-27 plan review;
see [project-plan.md](../project-plan.md) §4 and
[gpu-surface-present.md](gpu-surface-present.md)): a real-window surface,
`Surface::get_capabilities` negotiation, `CurrentSurfaceTexture` policy, and
**Layer C** visibility evidence (repeatable known-pattern real-window proof
or supported platform compositor feedback with documented limits). Texture
readback validates pixels only; `Queue::present` returns `()` and does not
by itself prove compositor visibility — neither closes P2-F08 alone. F06's
acceptance wording was narrowed accordingly — its surface deliverable is the
honest unsupported gate, and it never counted as a present pass. If the
target host cannot provide a real window or Layer C evidence, P2-F08 must
record per-platform unsupported evidence (command, environment, reason);
that record does **not** mark P2-F08 complete and does **not** automatically
lower or close the Phase 2 exit — any written exit adjustment needs a
separate review and must not substitute unsupported, software paint, or
readback for present verification. Plan current work is **P3-F01**. Phase 2
diagnose/smoke remain separate from presentation; train/preview/P6-F03 stay
unwired. P2-F05 diagnostics stay GPU-local (P6-F01 owns application session
history). Real host OOM remains unsupported; the 60 s stall watchdog stays
deferred. Packaging honesty: macOS tested; Linux NOREPLACE path implemented
but not host-tested here; Windows publish unsupported.
