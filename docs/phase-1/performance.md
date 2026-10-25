# P1-F08: resource and performance report infrastructure

`spirula-core` owns the versioned report contracts. `spirula-io` owns JSON
serialization, content hashing, and atomic publish/reload. A tiny CPU sample
binary generates and reads a report. There is no train/GPU/UI runtime and no
parallel application state.

## Reference behavior

Pinned sources: `src/dense/MemoryReport.cpp` (JSON memory snapshot via
`.part` + `replace_file`) and development-gates §5 (accounted vs device-wide
metrics, per-slice baselines before Phase 9 aggregation).

The C++ dense memory report mixes host process RSS, planned demand, and
optional device fields into one `memory.json`. P1-F08 does **not** copy that
shape. It records:

- application-**accounted** allocated / current / peak bytes from an explicit
  checked counter (`current <= peak <= allocated`);
- optional **resource byte budget** with preflight rejection before allocate;
- phase wall time in **nanoseconds** (with optional `work_count` for batched
  samples) plus optional time budget;
- fixture and config content SHA-256, plus source revision/digest or an honest
  unavailable reason and dirty/clean/unavailable working-tree status;
- explicit **unavailable** reasons for OS process peaks, device-wide GPU
  memory, general GPU counters, **GPU timestamp-query timing** (separate from
  absolute wall-clock timestamps), and absolute wall-clock timestamps.

Guessed or proxy numbers must never be labeled as measured.

## Binding decisions

- Schema id `spirula-perf-report`, version `1`. Version integers are parsed
  with a checked `u64→u32` conversion and must equal the schema version
  exactly (`SP-FMT-003`). Values such as `4294967297` that would wrap under a
  truncating cast are rejected.
- Provenance includes fixture/config SHA-256, `source_revision`
  (`digest` or `unavailable` + reason), and `dirty` (`clean` / `dirty` /
  `unavailable` + reason). Environment records OS, arch, and rustc (or an
  explicit unavailable note).
- `memory.accounted` is the only measured host memory section in F08.
  `memory.resource_budget_bytes` is optional; when set,
  `PerfReport::check_budgets` fails if peak or cumulative allocated exceeds
  it. `AccountedBytesTracker::preflight` rejects over-budget allocates without
  mutating counters. Tracker `allocate` / `free` use checked arithmetic and
  leave state unchanged on overflow/underflow.
- Fields `os_process_peak`, `device_memory`, `gpu`, `gpu_timestamp_query`, and
  `wall_clock_timestamp` are objects with `status: "unavailable"` and a
  non-empty `reason`. GPU timestamp-query timing is never folded into absolute
  wall-clock timestamp fields.
- Each phase records `name`, `wall_time_ns`, `source` (for example
  `std::time::Instant::elapsed`), optional `budget_ns`, and optional
  `work_count`. `check_budgets` fails with `SP-INP-003` on time or resource
  overflow after revalidating the report.
- `PerfReport::validate` (also called from save/serialize) rechecks **all**
  publicly mutable nested fields so blanked environment strings or mutated
  memory/phase values cannot be persisted for a reader to reject later.
- Persistence uses `spirula-io` atomic publish (unique sibling temp + replace).
- The sample binary is `spirula-perf-sample` in `spirula-io`. It preflights a
  known byte budget, times batched allocate/free work in nanoseconds,
  publishes, reloads, and compares. It does not query OS RSS or GPU APIs.

## Template for later features

Copy this checklist into each feature’s measurement note (do not invent
values):

1. Feature id and report version.
2. Fixture / config SHA-256; source revision digest or unavailable reason;
   dirty/clean/unavailable status.
3. Environment (OS, arch, rustc/tooling) and device/driver when measured.
4. Accounted allocated/current/peak, counter source, and resource byte budget /
   preflight result.
5. Separate OS / device-wide peaks only when actually sampled; otherwise
   unavailable + reason.
6. Phase list: name, wall_time_ns, source, budget_ns, work_count, overflow.
7. GPU counters and GPU timestamp-query timing: measured or unavailable
   reasons (kept distinct from absolute wall-clock timestamps).
8. Commands used to generate and reload the report.

## Commands

```bash
# Unit contracts (version wrap / budget / mutation / overflow)
cargo test -p spirula-core perf -- --test-threads=1

# Persistence round-trip and invalid JSON/version/mutation
cargo test -p spirula-io perf -- --test-threads=1

# Generate + read a report
cargo run -p spirula-io --bin spirula-perf-sample -- /tmp/spirula-perf-report.json

# Full gate
cargo run -p spirula-xtask -- verify
```

## Explicit non-claims

- No OS RSS / working-set sampling.
- No GPU adapter, device memory, GPU timestamp-query, or absolute wall-clock
  timestamp measurement.
- No training, render, or application-session orchestration.
- Phase 9 still owns cross-feature baseline aggregation (P9-F02).
