# P2-F08: wgpu surface present verification

Status: **未开始** — planning record only. No implementation tip exists.
Owner: `spirula-gpu`. Added by the 2026-10-27 plan review to close the last
open Phase 2 exit condition. Builds on
[device-context.md](device-context.md), [gpu-resources.md](gpu-resources.md),
[gpu-diagnostics.md](gpu-diagnostics.md); consumes the honest unsupported gate
from [gpu-smoke.md](gpu-smoke.md) without reinterpreting it as a pass.

**This document is a contract sketch, not evidence.** Nothing below is
implemented, measured, or verified. Do not cite it as progress.

Pinned public API: resolved crate **`wgpu` 30.0.1** (workspace pin). Descriptions
below follow that crate's public surface; do not invent return values or
compositor signals that the API does not expose.

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
| **P2-F08** | a real window surface presents frames **and** those frames are observably visible (or a supported platform presentation/compositor feedback path documents that visibility) | zero-copy; cross-platform parity beyond recorded platforms; that texture readback alone equals present |

Software paint is not present. Slint `renderer-software` output is not present.
A headless compute pass with a correct readback is not present.
**A surface-texture readback (even after `Queue::present`) validates pixels; it
does not prove the window/compositor displayed them.** Readback alone cannot
close P2-F08.

## Three evidence layers (keep separate)

P2-F08 must record these as distinct claims. Passing an earlier layer never
implies a later one.

| Layer | What is checked | What it is **not** |
|---|---|---|
| **A. Pixel validation** | Known pattern written into a GPU texture (surface texture and/or a related staging/copy path) matches on readback / CPU compare | Proof that a window showed those pixels |
| **B. Present API submission** | `Surface::get_current_texture` yielded a presentable frame; work was `Queue::submit`ted; `Queue::present` was called on that `SurfaceTexture` | Proof of compositor visibility; a typed `Result` from `present` (see API notes — it returns `()`) |
| **C. Actual visibility** | Repeatable known-pattern **real-window** visibility evidence, **or** a **supported** platform presentation/compositor feedback path whose limits are documented | Optional; inventing unobservable feedback; treating Layer A or B as Layer C |

Closing P2-F08 on a host requires **Layer C** for that host (plus honest Layer A/B
records). Layer A and B are necessary engineering checks; they are insufficient
exit evidence by themselves.

## wgpu 30.0.1 API notes (resolved local source)

Inspected under the cargo registry tree for `wgpu` **30.0.1** (and
`wgpu-types` **30.0.1**). Contract text must use these names:

- **Capabilities:** `Surface::get_capabilities(&adapter) -> SurfaceCapabilities`.
  There is no public `Adapter::get_surface_capabilities` on this pin. Prefer
  `Surface::get_default_config` only as a starting point; still record the full
  negotiated format / present-mode / alpha-mode set.
- **Acquire frame:** `Surface::get_current_texture() -> CurrentSurfaceTexture`
  (not `Result<_, SurfaceError>`). Public variants to policy against:

  | `CurrentSurfaceTexture` | Policy sketch (to freeze in implementation) |
  |---|---|
  | `Success(SurfaceTexture)` | Presentable; proceed with draw → submit → `Queue::present` |
  | `Suboptimal(SurfaceTexture)` | Still presentable; record suboptimal; re-`configure` soon for optimal path |
  | `Timeout` | No texture; skip frame / retry later; typed skip or retry signal — not a silent pass |
  | `Occluded` | No texture; window occluded/minimized; skip until visible; do not invent a present success |
  | `Outdated` | No texture; re-`Surface::configure` and retry |
  | `Lost` | No texture; **surface** lost — recreate via `Instance::create_surface` + configure **unless** the **device** is lost (see below) |
  | `Validation` | No texture; a validation error was raised and caught by an error scope / uncaptured handler; attend to that error before retry |

- **Surface loss vs device loss:** `CurrentSurfaceTexture::Lost` docs require
  distinguishing cases. If `Device::set_device_lost_callback` / P2-F05 lost latch
  shows the **device** is lost, recreate device and dependent resources (P2-F02
  owner path). Otherwise recreate the **surface** only, then configure. Do not
  collapse surface lost into device lost or the reverse.
- **Present call:** `Queue::present(&self, surface_texture: SurfaceTexture)`
  returns **`()`**. It schedules presentation after submit; it does **not**
  return `Result` or a public present status. On the wgpu-core backend, internal
  `surface_present` errors are routed through the device error sink
  (`handle_error_nolabel` / error scopes / `on_uncaptured_error`) — the `Ok`
  status from the core call is discarded by the public wrapper. **Do not claim
  an observable `Queue::present` return value.** Document present-path failures
  only via: prior `CurrentSurfaceTexture` outcomes, configure/create errors
  (`CreateSurfaceError` / configure validation), and error-scope / uncaptured /
  P2-F05 diagnostic paths that are actually wired and tested.
- **Unobservable:** Do not invent compositor ACKs, frame callbacks, or
  present-success booleans that wgpu 30.0.1 does not expose on this path.
  `Adapter::get_presentation_timestamp` is a clock helper, not proof that a
  frame became visible.

## Required contract (to be frozen in this feature)

The implementation must decide and record, before writing code:

- **Surface creation.** Which window handle type per platform, which owner
  creates it (session-owned `GpuContext`, consistent with P2-F02 — no global
  singleton, no auto-rebuild on loss), and how surface lifetime relates to the
  existing `DeviceHandle` identity/generation rules. Creation failures use
  `CreateSurfaceError` (public), mapped into `SP-GPU-*` without parallel codes.
- **Capability negotiation.** Call `Surface::get_capabilities(&adapter)`;
  record accepted `TextureFormat` set, present-mode policy (required vs
  optional and fallback), alpha mode, and the size/resize path. Extend the
  P2-F01 required/optional split; never assume a format exists.
- **Layer A — pixel validation.** Known pattern (exact RGBA corners or full
  buffer) checked by readback/CPU compare where the copy path is valid. State
  plainly that this layer does **not** close present.
- **Layer B — present API submission.** Observable acquire outcome
  (`CurrentSurfaceTexture`), submit, and `Queue::present` invocation; typed
  handling for Timeout/Occluded/Outdated/Lost/Validation; Suboptimal recorded.
  Present-path device errors only where error scopes / uncaptured handlers
  actually observe them.
- **Layer C — actual visibility.** One of:
  1. **Repeatable known-pattern real-window visibility** — documented procedure
     (for example CUA / accessibility / human log) that the same known pattern
     is visible in a real window after present, with command, host, and limits; or
  2. **Supported platform presentation/compositor feedback** — only if a
     platform API used by the implementation yields an observable signal that
     the project can record and re-run; document the API, what it proves, and
     what it does **not** prove.
  Platforms without Layer C stay unverified for present; they may carry an
  honest `unsupported` or "visibility not recorded" note — never a silent pass.
- **Failure semantics.** Map create/configure/acquire/lost/device-lost into
  typed `SP-GPU-*` using P2-F05 error-scope/lost-latch discipline. Resize during
  present follows Outdated/Lost policy. Do not invent a public `SurfaceError`
  return from `get_current_texture` on this pin.
- **Resource accounting.** Present-frame bytes and sync points use the P1-F08
  report format, kept separate from the P2-F03 staging/readback budget. Readback
  used for Layer A remains budgeted and labeled as pixel validation, not as
  visibility evidence.

## Acceptance

1. On the target development machine, Layers A–C succeed with a real window, **or**
   an exact per-platform `unsupported` / visibility-limits record with command,
   environment, driver and reason — never a silent pass. That `unsupported`
   record alone does **not** complete this feature and does **not**
   automatically lower or close the Phase 2 exit.
2. Capability negotiation is observable via `Surface::get_capabilities`: chosen
   format, present mode, alpha mode, and what was rejected.
3. `CurrentSurfaceTexture` policy is implemented and tested for
   Success / Suboptimal / Timeout / Occluded / Outdated / Lost / Validation;
   surface lost vs device lost are distinct; resources do not leak
   (`after_drop current = 0`, no retained submissions).
4. Docs state that `Queue::present` returns `()` and that present failures are
   only claimed where error-scope / uncaptured / diagnostic paths actually
   observe them — no invented present `Result`.
5. **Readback alone does not close P2-F08.** Layer C visibility evidence (or a
   documented supported compositor-feedback path with limits) is required for a
   present-pass claim.
6. Resource and sync measurements are recorded per the P1-F08 format.
7. No claim of zero-copy. No claim that software paint equals present. No
   claim of cross-platform parity beyond the platforms actually exercised.
8. Docs state which platforms completed Layer C, which only reached A/B, which
   are unsupported, and — only after a separate reviewed decision if present
   remains impossible — what written Phase 2 exit adjustment (if any) is
   attached to that evidence.

## Effect on the Phase 2 exit

This feature is the Phase 2 exit gate. Completing P2-F01–P2-F07 does not close
it. An `unsupported` host record is honesty, not a pass: it must not be read
as feature completion, and it must not silently lower the exit bar. If every
target host truly cannot present or cannot supply Layer C evidence, the only
non-hanging path is an **explicit, separately reviewed** written exit decision
that cites that evidence — never “all IDs marked done while the exit stays
forever open,” never software paint as a substitute, and never texture
readback as a substitute for visibility.

## Non-claims

- No implementation, no measurement, no host evidence yet.
- Not zero-copy; not shared-texture; not a UI feature (P6-F03 owns windows and
  navigation, P6-F04 owns real preview).
- Does not change P2-F01–P2-F07 contracts beyond consuming them.
- Does not claim present verified; does not treat P2-F06 unsupported as a
  present pass; does not treat Layer A readback or Layer B `Queue::present`
  invocation as Layer C visibility.
