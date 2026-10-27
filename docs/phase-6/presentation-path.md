# P6-F07: Slint / wgpu display-path validation

Status: **已完成** (tip `82adae0eebbb1f1a0f20ddfec1d7dd824ae7006c`; independent
READY `/tmp/spirula-p6-f07-review/REVIEW-final-82adae0.md`; parent fullverify
8/8 `/tmp/spirula-codex-p6-f07-82adae0-independent-verify.log`). Owners:
`spirula-ui` / `spirula-application` / `spirula-gpu`. Builds on P2-F03
resources, P2-F05 diagnostics, P2-F06 smoke honesty, P1-F07 session ownership,
P1-F08 measurement vocabulary. **Not** P6-F03 navigation / full shell;
train/preview remain unwired. Phase 2 exit stays open (surface present still
unverified). **Software renderer path must not close the wgpu surface present
gate.**

## Architecture decision (minimal)

**AD-P6-F07-01 — application may depend on `spirula-gpu` for presentation
generation only.**

| Rule | Decision |
|---|---|
| `spirula-application` → `spirula-gpu` | **Allowed** (narrow path edge) so the session-owned presentation owner can open a `GpuContext` and produce host frames |
| `spirula-application` → `wgpu*` | **Forbidden** (unchanged) |
| `spirula-ui` → `spirula-gpu` / `wgpu*` | **Forbidden** (unchanged) |
| `spirula-core` presentation values | Host RGBA + metrics only — **no** `wgpu` types |
| Shared GPU texture into Slint | **Not available** in this tip → explicit **readback → host copy → Slint `Image` upload** fallback; costs recorded |
| Slint software renderer | Allowed for headless / window probe; **does not** claim wgpu surface present verification |

Synced: [architecture.md](../architecture.md),
[architecture-rules.md](../agent/architecture-rules.md),
[runtime-contracts.md](../phase-0/runtime-contracts.md) §6.1,
`spirula-xtask` policy allowlist + tests.

## Reference behavior (C++)

Pinned: `9a24f09927944a795e7ba1bd8243a625bf419479`.

Observed GUI preview path (not a invented `glGetTexImage` chain):

| C++ locus | What happens | Rust binding in this slice |
|---|---|---|
| `src/app/gui/render/FrameRenderer.cpp:259-304` (`Slot` + `~Slot` / dtor / `destroy_gl`) | Worker stop on slot/dtor; GL texture pool + FBO teardown (`glDeleteTextures`, clear `_pool_w/_pool_h`) | Application presentation shutdown joins worker, closes `GpuContext`, clears latest-frame slot; accounted `current_bytes==0` and pending/retained queues empty |
| `src/app/gui/render/FrameRenderer.cpp:695-716` (`target`) | On size change, delete pool textures and recreate; `glTexImage2D(..., nullptr)` allocates RGBA storage | Resize bumps `size_generation`; next produce uses new WxH; late frames from prior generation are discarded (must not overwrite new-size slot) |
| `src/app/gui/render/FrameRenderer.cpp:747-752` (`poll`) | Worker RGBA result uploaded with `glPixelStorei(GL_UNPACK_ALIGNMENT,1)` + `glTexImage2D(..., res.rgba8.data())` | **Deviation**: no ImGui/GL. wgpu texture upload → readback → tight host RGBA (`ReadbackToHost`), then Slint `Image` host upload |
| `src/app/gui/render/RenderSession.cpp:1851` | Corner preview: `dl->AddImage((ImTextureID)_frames.texture(), …)` | Slint `Image` property assign (**set-image**); not wgpu surface present |
| `src/app/gui/render/RenderTimeline.cpp:413` | Camera view: `dl->AddImage((ImTextureID)_frames.texture(), …)` | Same host-image path; timeline chrome out of scope |
| `viewer/` WebGL | Separate web path | Out of scope |

There is **no** `glGetTexImage` in `FrameRenderer.cpp` on this pin; host pixels
reach GL via worker `rgba8` + `glTexImage2D` upload, and ImGui samples the GL
texture with `AddImage`.

## Binding decisions

### Versions / device (recorded at runtime)

- `wgpu` crate version: workspace pin (`WGPU_VERSION` from `spirula-gpu`).
- Slint: `1.18` with `renderer-software` (headless + window probe).
- Adapter/backend/device name: captured on presentation open (same negotiate
  path as P2-F01/F02).
- Thread ownership: **`GpuContext::open` and all submit/wait/readback run on
  the presentation worker only.** `PresentationOwner::start` returns while
  status is `Opening`; UI polls `status` / timer ticks — never blocks the UI
  thread waiting for GPU open or frames.

### Frame contract (`spirula-core`)

`PresentedFrame`: `width`, `height`, tightly packed RGBA8 bytes, monotonic
`frame_id`, `size_generation`, known copy-byte counters (logical + staging row
pitch), sync labels, produce/deliver timestamps (host `Instant` deltas where
measurable). CPU reference pattern: `R=x%256, G=y%256, B=(x+y)%256, A=255`.
Corner samples (TL/TR/BL/BR) must match that pattern. Staging row pitch follows
wgpu `COPY_BYTES_PER_ROW_ALIGNMENT` (256); tight host RGBA has no padding.

### GPU path (`spirula-gpu`)

`produce_probe_rgba_frame(ctx, width, height, frame_id, size_generation, cancel)`:

1. Create RGBA8 texture (copy_src|copy_dst).
2. Upload CPU pattern (staging).
3. Readback → map → tight RGBA.
4. Exact match vs CPU pattern or typed fail.
5. **Fail reclaim (P2-F06 semantics):** on Err after resources exist, drop
   ticket, `abandon_unsubmitted_pending`. Distinguish **already-reaped
   terminal** wait errors (post-submit Validation / OOM: wait reaps then
   returns typed `SP-GPU-003` / `SP-GPU-004`) from **still in-flight**
   timeout/cancel. Only wait a handle that is still accepted; clear
   `locals.submission` when wait already reaped. Preserve the original typed
   produce error when reclaim is clean (do not overwrite OOM with
   `reclaim wait failed`). If an in-flight reclaim wait cannot finish within
   its deadline → **close the context** (ledger invalidate) and report that
   honestly — do **not** claim Live `current==0` after discarding the only
   handle. When reclaim succeeds on a still-Live context: `current_bytes == 0`,
   pending empty, `retained_submission_count == 0` (exact), then retryable.
6. Wait uses short slices with a **total deadline** (not `wait(..., None)`);
   `cancel=None` still bounds wall time; cancel token remains observable when
   present. Timeout inject keeps the submission for the next slice/reclaim.
7. Path tag: `ReadbackToHost` (not shared-texture; not surface present).

Illegal sizes: `0` or `>4096` → typed `ValueOutOfRange` before GPU work.

### Application presentation owner

- Bounded **latest-frame** slot (capacity 1): producer overwrites; slow
  consumer never blocks the GPU worker; dropped/overwritten frames counted.
  Slot host-byte ceiling = one frame (`width*height*4` for current size).
- **Resize is shared-state only** (never enqueued). Bumps `size_generation`,
  updates WxH, **clears the slot immediately**. Prevents: (a) stale Resize
  cmd replay rolling size back when the cmd queue is full; (b) taking a
  cached prior-generation frame after resize. Produce stamps generation;
  stale finishes are dropped; `take_latest` also rejects mismatched generation.
- Produce cmd channel capacity 1 (coalesce). Deterministic barriers:
  `wait_ready`, `wait_produce_epoch`, `request_and_wait_frame` (no sleep races).
- Cancel/shutdown cooperative; in-flight produce respects `CancelToken`.
- After `close`: status `Closed`; `request_frame` / `resize` rejected (no
  resurrect). Fresh `start` allowed.
- Stall consumer: ≥1000 produce attempts without take → overwrite/drop counts
  advance; Stop/cancel/shutdown still responds (not blocked by UI).
- No train/preview commands; no empty `spirula-train` shell.

### UI (`spirula-ui`)

- Depends only on `spirula-core` + `spirula-application` (+ Slint).
- `image_from_presented_frame` = host pixel copy only (`host-pixel-copy`);
  **does not** claim set-image or software-paint.
- `mark_property_assigned` only after a real component `set_*`.
- `mark_software_paint_verified` only from a verified paint path
  (`RenderingState::AfterRendering`). If the notifier is unsupported, paint
  stays **unverified** — never mark before the window draws.
- Probe binary: worker opens GPU; UI timer polls status/frames via
  [`ProbeDriver`](../../crates/spirula-ui/src/probe_driver.rs) — **resize only
  when wanted size changes**; initial frame is `request` only; while awaiting a
  generation, never re-resize (fixes gen spin / blank image / taken=0).
  Auto-cycles sizes after first assign (64×48, 65×3, 63×2, 128×72). Tech
  buttons also set want. Callbacks hold `Weak`; close `take`s owner then
  shutdown (exit 0). Software renderer ≠ wgpu surface present.
- No navigation chrome (P6-F03).

### Measurement honesty

| Metric | Status |
|---|---|
| Known host copies (GPU readback buffer → tight RGBA; RGBA → Slint pixels) | **Measured** (per-frame logical / bpr / staging / UI upload bytes) |
| Staging row pitch / padded staging bytes | **Measured** |
| Sync points (`submit`, `wait`, `map`, `host-pixel-copy`, `set-image`) | Labeled when taken |
| Software paint via Slint rendering notifier | **Unverified** on tip host (`Unsupported`); CUA human-visible software draw recorded separately — **not** a notifier pass |
| Produce → deliver latency (host clocks) | **Measured** when both timestamps exist |
| GPU→display present latency / compositor | **Unmeasurable** here — recorded as unavailable |
| Process-wide CPU total peak | **Not measured** in this tip |
| Zero-copy / shared texture | **Not claimed** |
| wgpu surface present | **Unverified** — P2-F06 honest unsupported gate remains open |

## Acceptance

### Headless (required for tip)

1. Exact RGBA for sizes **1×1**, **63×2**, **65×3**: staging row pitch + padded
   bytes match wgpu 256-align; tight host length `w*h*4`; corner RGBA matches
   CPU probe. **Met.**
2. Size `0` and oversize (`>4096`) → typed fail; no GPU leak. **Met.**
3. Latest-wins; cancel; failure path; cleanup **`current_bytes == 0`** and
   **pending/retained == 0** (exact); post-submit Validation/OOM preserve typed
   codes with Live retry. **Met.**
4. Resize: late frames from prior `size_generation` discarded; delivered frame
   matches new size only. **Met.**
5. Close: no resurrect (`request_frame` / take after shutdown fails or empty).
   **Met.**
6. Stall UI: ≥1000 produces without take → overwrite/drop counts; slot bytes ≤
   one current frame; Stop/cancel/shutdown not blocked. **Met.**
7. Policy: `application→gpu` only; UI/core forbid `wgpu*`. **Met.**

### Real window (required host evidence)

8. Run `spirula-presentation-probe` on a real host window; record that the
   probe image is **drawn** (not merely set-image). Software renderer allowed.
   **Met** via parent CUA on `/tmp/SpirulaPresentationProbe.app` (bundle id
   `local.spirula.presentation-probe`) — see window evidence log. App
   `software_paint` flag stays **unverified** when the rendering notifier is
   `Unsupported`; CUA-visible software draw is recorded separately and does
   **not** close wgpu surface present.
9. Docs must still state wgpu surface present is unverified. **Met** (this
   page; Phase 2 exit remains open).

### Plan hygiene

10. 74 plan IDs; P6-F03 remains 未开始; Phase 2 exit open. **Met.**

## Window evidence log

| Date | Host | Command | Result |
|---|---|---|---|
| 2026-10-27 | macOS aarch64 | `cargo run -p spirula-ui --bin spirula-presentation-probe` | early launch; incomplete |
| 2026-10-27 | macOS aarch64 | `/tmp/SpirulaPresentationProbe.app` (`local.spirula.presentation-probe`) | gen-spin firm: blank / taken=0; CPU resource diag pid 55193; fixed in `82adae0` |
| 2026-10-27 | macOS aarch64 | same `.app` @ `82adae0` (ProbeDriver + Weak close) | **PASS (CUA)** — 64×48 frame74/taken74/gen37 visible (produce_to_take ~20.73ms; logical12288 / bpr256 / staging12288 / UIcopy12288); click 65×3 → frame84/taken84/gen42 strip visible (~23.58ms; logical780 / bpr512 / staging1536 / UIcopy780); AX zoom 960×784→3024×1736 image+controls remain visible (~17.90ms sample); close → process gone, `/tmp/spirula-presentation-probe-logs/latest.exit=0`, stderr `probe: clean shutdown (owner released)`. App `software_paint` unverified (notifier Unsupported); CUA human-visible software draw recorded. **Not** wgpu surface present. |

## Non-claims

- Not P6-F03 shell/navigation; not train/preview.
- Not wgpu surface present pass; not zero-copy UI.
- Not three-platform UI matrix; macOS host evidence primary.
- Not Phase 2 exit closure.
- Not process-wide CPU total peak measurement.
- Not a Slint rendering-notifier software-paint automation pass when
  notifier is `Unsupported`.
