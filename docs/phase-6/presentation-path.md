# P6-F07: Slint / wgpu display-path validation

Status: **当前 / HOLD-MERGE** (candidate tip pending independent review; do not merge).
Owners: `spirula-ui` / `spirula-application` / `spirula-gpu`.
Builds on P2-F03 resources, P2-F05 diagnostics, P2-F06 smoke honesty, P1-F07
session ownership, P1-F08 measurement vocabulary. **Not** P6-F03 navigation /
full shell; train/preview remain unwired. Phase 2 exit stays open (surface
present still unverified).

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

| C++ surface | Behavior recorded | Rust binding in this slice |
|---|---|---|
| GUI preview blit (`FrameRenderer` / live preview GL texture → `glGetTexImage` / ImGui `AddImage`) | GPU renders to texture; host/UI samples or blits for display | **Deviation**: no ImGui/GL; no splat preview. Micro RGBA probe via wgpu texture upload→readback→host, then Slint `Image` |
| Web viewer (`viewer/`) | Separate WebGL path | Out of scope |
| Device/context ownership | Engine owns GPU; GUI consumes textures | Application-owned presentation slot; UI never opens GPU |

## Binding decisions

### Versions / device (recorded at runtime)

- `wgpu` crate version: workspace pin (`WGPU_VERSION` from `spirula-gpu`).
- Slint: `1.18` with `renderer-software` (headless + window probe).
- Adapter/backend/device name: captured on presentation open (same negotiate
  path as P2-F01/F02).
- Thread ownership: GPU open/submit/wait/readback on the presentation worker;
  UI thread only consumes host `PresentedFrame` / Slint `Image`.

### Frame contract (`spirula-core`)

`PresentedFrame`: `width`, `height`, tightly packed RGBA8 bytes, monotonic
`frame_id`, known copy-byte counters, sync labels, produce/deliver timestamps
(host `Instant` deltas where measurable). CPU reference pattern:
`R=x%256, G=y%256, B=(x+y)%256, A=255`.

### GPU path (`spirula-gpu`)

`produce_probe_rgba_frame(ctx, width, height, cancel)`:

1. Create RGBA8 texture (copy_src|copy_dst).
2. Upload CPU pattern (staging).
3. Readback → map → tight RGBA.
4. Exact match vs CPU pattern or typed fail.
5. Drop resources; report `after_drop` accounted bytes when context still live.
6. Path tag: `ReadbackToHost` (not shared-texture; not surface present).

### Application presentation owner

- Bounded **latest-frame** slot (capacity 1): producer overwrites; slow
  consumer never blocks the GPU worker; dropped/overwritten frames counted.
- Cancel/shutdown cooperative; in-flight produce respects `CancelToken`.
- Resize: next produce uses new dimensions; old slot replaced.
- Repeat start after shutdown is allowed (fresh context).
- No train/preview commands; no empty `spirula-train` shell.

### UI (`spirula-ui`)

- Depends only on `spirula-core` + `spirula-application` (+ Slint).
- Converts `PresentedFrame` → `slint::Image` (host pixel upload = additional
  known copy).
- Distinguishes **set-image** (property assign) from **draw/present** (window
  show / software paint). Software renderer evidence ≠ wgpu surface present.
- Minimal probe UI (image + status text). No navigation chrome (P6-F03).

### Measurement honesty

| Metric | Status |
|---|---|
| Known host copies (GPU readback buffer → tight RGBA; RGBA → Slint pixels) | Reported (bytes) |
| Sync points (`submit`, `wait`, `map`) | Labeled |
| Produce → deliver latency (host clocks) | Reported when both timestamps exist |
| GPU→display present latency / compositor | **Unmeasurable** here — recorded as unavailable |
| Zero-copy / shared texture | **Not claimed** |

## Acceptance (HOLD-MERGE)

1. Headless: produce exact RGBA; presentation latest-wins; cancel; illegal size;
   failure path; slow consumer drop count; cleanup current≈0 after drop.
2. UI conversion to Slint `Image` without `wgpu` in UI/core public types.
3. Real window probe binary optional for local CUA; software renderer allowed;
   docs do **not** claim surface present verified.
4. Policy allows `application→gpu` only; forbids UI/core→wgpu.
5. 74 plan IDs; P6-F03 remains 未开始; Phase 2 exit open.

## Non-claims

- Not P6-F03 shell/navigation; not train/preview.
- Not wgpu surface present pass; not zero-copy UI.
- Not three-platform UI matrix; macOS host evidence primary.
- Not Phase 2 exit closure.
