# P2-F08: wgpu surface present verification

Status: **未开始** — planning record only. No implementation tip exists.
Owner: `spirula-gpu`. Added by the 2026-10-27 plan review to close the last
open Phase 2 exit condition. Builds on
[device-context.md](device-context.md), [gpu-resources.md](gpu-resources.md),
[gpu-diagnostics.md](gpu-diagnostics.md); consumes the honest unsupported gate
from [gpu-smoke.md](gpu-smoke.md) without reinterpreting it as a pass.

**This document is a contract sketch, not evidence.** Nothing below is
implemented, measured, or verified. Do not cite it as progress.

## Why this feature exists

P2-F06's acceptance wording asked for "a minimal surface test reproducible on
supported devices", but what P2-F06 actually delivered is an honest
**unsupported** gate that refuses to forge a window. No plan ID owned the
actual present verification, so the Phase 2 exit condition ("surface present
still unverified") had no owner and could hang indefinitely. P2-F06's wording
was narrowed on 2026-10-27; P2-F08 takes over the responsibility.

The distinction matters because two adjacent slices are easy to misread:

| Slice | What it proves | What it does **not** prove |
|---|---|---|
| P6-F07 | readback → host `PresentedFrame` → Slint `Image` upload; software paint visible | anything about a GPU surface, swapchain, or compositor |
| P2-F06 | headless compute reaches correct bytes; refuses to fake a window | any present success |
| **P2-F08** | a real window surface presents frames | zero-copy, or cross-platform parity beyond what each platform records |

Software paint is not present. Slint `renderer-software` output is not present.
A headless compute pass with a correct readback is not present.

## Required contract (to be frozen in this feature)

The implementation must decide and record, before writing code:

- **Surface creation.** Which window handle type per platform, which owner
  creates it (session-owned `GpuContext`, consistent with P2-F02 — no global
  singleton, no auto-rebuild on loss), and how surface lifetime relates to the
  existing `DeviceHandle` identity/generation rules.
- **Capability negotiation.** `adapter.get_surface_capabilities`, accepted
  `TextureFormat` set, present mode policy (which mode is required vs optional,
  and what the fallback is), alpha mode, and the size/resize path. This must
  extend the P2-F01 required/optional split rather than silently assuming a
  format exists.
- **Present evidence.** What counts as proof that present happened: a
  post-present readback of known content, or compositor-visible output
  observed through a documented, repeatable procedure. State plainly which
  platforms each method works on and which stay unverified.
- **Failure semantics.** Typed errors for surface creation failure, surface
  configuration mismatch, present failure, `SurfaceError::Lost`/`Outdated`
  handling, and resize during present. Reuse the `SP-GPU-*` catalog and the
  P2-F05 error-scope/lost-latch discipline; do not invent parallel codes.
- **Resource accounting.** Present-frame bytes and sync points must be
  reported through the P1-F08 report format, kept separate from the P2-F03
  staging/readback budget.

## Acceptance

1. Real-window present succeeds on the target development machine. If a given
   host cannot present, record an exact per-platform `unsupported` with
   command, environment, driver and reason — never a silent pass. That
   `unsupported` record alone does **not** complete this feature and does
   **not** automatically lower or close the Phase 2 exit.
2. Capability negotiation is observable: chosen format, present mode, alpha
   mode, and what was rejected.
3. Present failure, surface loss, and resize produce typed errors and do not
   leak resources (`after_drop current = 0`, no retained submissions).
4. Resource and sync measurements are recorded per the P1-F08 format.
5. No claim of zero-copy. No claim that software paint equals present. No
   claim of cross-platform parity beyond the platforms actually exercised.
6. Docs state clearly which platforms were verified, which were not, and —
   only after a separate reviewed decision if present remains impossible —
   what written Phase 2 exit adjustment (if any) is attached to that evidence.

## Effect on the Phase 2 exit

This feature is the Phase 2 exit gate. Completing P2-F01–P2-F07 does not close
it. An `unsupported` host record is honesty, not a pass: it must not be read
as feature completion, and it must not silently lower the exit bar. If every
target host truly cannot present, the only non-hanging path is an **explicit,
separately reviewed** written exit decision that cites the unsupported
evidence — never “all IDs marked done while the exit stays forever open,” and
never software paint as a substitute.

## Non-claims

- No implementation, no measurement, no host evidence yet.
- Not zero-copy; not shared-texture; not a UI feature (P6-F03 owns windows and
  navigation, P6-F04 owns real preview).
- Does not change P2-F01–P2-F07 contracts beyond consuming them.
- Does not claim present verified; does not treat P2-F06 unsupported as a
  present pass.
