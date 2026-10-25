# P1-F08: resource and performance report infrastructure

`spirula-core` owns the versioned report contracts. `spirula-io` owns JSON
serialization, content hashing, atomic publish/reload, and the CPU sample
workload. A tiny CPU sample binary generates and reads a report. There is no
train/GPU/UI runtime and no parallel application state.

## Reference behavior

Pinned sources: `src/dense/MemoryReport.cpp` (JSON memory snapshot via
`.part` + `replace_file`) and development-gates §5 (accounted vs device-wide
metrics, per-slice baselines before Phase 9 aggregation).

The C++ dense memory report mixes host process RSS, planned demand, and
optional device fields into one `memory.json`. P1-F08 does **not** copy that
shape. It records:

- application-**accounted** allocated / current / peak bytes from an explicit
  checked counter (`current <= peak <= allocated`), with `source` naming the
  counted scope (the CPU sample counts only the per-iteration work buffer);
- optional **scoped resident** resource byte budget (live current/peak of that
  accounted scope only) with preflight rejection before allocate using
  `SP-GPU-004` / `AllocationFailed`; cumulative `allocated_bytes` remains a
  volume counter and is not a capacity limit; the budget is **not** process
  RSS / working-set enforcement;
- phase wall time in **nanoseconds** (with optional `work_count` for batched
  samples) plus optional time budget;
- fixture and config content SHA-256 of the **executed** fixture bytes and the
  **parsed** config JSON that defines the workload, plus source revision/digest
  or an honest unavailable reason and dirty/clean/unavailable working-tree
  status;
- explicit **unavailable** reasons for OS process peaks, device-wide GPU
  memory, general GPU counters, **GPU timestamp-query timing** (separate from
  absolute wall-clock timestamps), and absolute wall-clock timestamps.

Guessed or proxy numbers must never be labeled as measured.

## Binding decisions

- Schema id `spirula-perf-report`, version `1`. Version integers are parsed
  with a checked `u64→u32` conversion and must equal the schema version
  exactly (`SP-FMT-003`). Values such as `4294967297` that would wrap under a
  truncating cast are rejected.
- Provenance includes fixture/config SHA-256 of the payloads that the sample
  actually executes/parses, `source_revision` (`digest` or `unavailable` +
  reason), and `dirty` (`clean` / `dirty` / `unavailable` + reason). The sample
  keeps source/dirty unavailable (it does not invent a CWD git identity).
  Environment records OS, arch, and rustc (or an explicit unavailable note).
- `memory.accounted` is the only measured host memory section in F08. Its
  `source` must state the scope. The sample uses
  `explicit_counter:per_iteration_work_buffer_only`: fixture bytes, config JSON,
  report serialization, and allocator metadata may exist outside that counter.
- `memory.resource_budget_bytes` is an optional **scoped resident** cap: when
  set, `PerfReport::check_budgets` and `AccountedBytesTracker::preflight` fail
  with `AllocationFailed` only if accounted live current/peak would exceed it.
  Repeated allocate/free under the same cap is allowed even when cumulative
  allocated volume grows. An 8192-byte sample cap is not a claim about total
  process RSS.
- Tracker `allocate` / `free` use checked arithmetic and leave state unchanged
  on overflow/underflow.
- Fields `os_process_peak`, `device_memory`, `gpu`, `gpu_timestamp_query`, and
  `wall_clock_timestamp` are objects with `status: "unavailable"` and a
  non-empty `reason`. GPU timestamp-query timing is never folded into absolute
  wall-clock timestamp fields.
- Each phase records `name`, `wall_time_ns`, `source` (for example
  `std::time::Instant::elapsed`), optional `budget_ns`, and optional
  `work_count`. `check_budgets` fails with `SP-INP-003` on time overflow and
  `SP-GPU-004` on scoped resident resource overflow after revalidating the
  report.
- `PerfReport::validate` (also called from save/serialize) rechecks **all**
  publicly mutable nested fields so blanked environment strings or mutated
  memory/phase values cannot be persisted for a reader to reject later.
- Persistence uses `spirula-io` atomic publish (unique sibling temp + replace).
- The sample workload (`spirula-io::run_measured_workload`) hashes an 8192-byte
  patterned fixture and a config JSON (`chunk_bytes=8192`, `iterations=8`,
  `resource_budget_bytes=8192`). It allocates a real work buffer, copies the
  opaque fixture via safe `Read` + `black_box`, folds an observable checksum
  (no `unsafe`), frees, and repeats. Release builds must keep that
  allocation/read path; debug-only evidence is not sufficient.

## Template for later features

Copy this checklist into each feature’s measurement note (do not invent
values):

1. Feature id and report version.
2. Fixture / config SHA-256 of the **executed** payloads; source revision
   digest or unavailable reason; dirty/clean/unavailable status.
3. Environment (OS, arch, rustc/tooling) and device/driver when measured.
4. Accounted allocated/current/peak, counter source, and resident resource byte
   budget / preflight result (volume counters named separately if limited).
5. Separate OS / device-wide peaks only when actually sampled; otherwise
   unavailable + reason.
6. Phase list: name, wall_time_ns, source, budget_ns, work_count, overflow.
7. GPU counters and GPU timestamp-query timing: measured or unavailable
   reasons (kept distinct from absolute wall-clock timestamps).
8. Commands used to generate and reload the report, including a **release**
   sample run when the measurement claims CPU work.

## Commands

```bash
# Unit contracts (version wrap / budget / mutation / overflow / reuse)
cargo test -p spirula-core perf -- --test-threads=1

# Persistence + workload (fixture/config identity, resident reuse)
cargo test -p spirula-io -- --test-threads=1

# Release workload regressions (optimizer must not delete the sample)
cargo test -p spirula-io --release sample_workload -- --test-threads=1

# Generate + read (debug)
cargo run -p spirula-io --bin spirula-perf-sample -- /tmp/spirula-perf-report.json

# Generate + read (release) — required evidence for the sample
cargo run -p spirula-io --release --bin spirula-perf-sample -- /tmp/spirula-perf-report-release.json

# Optional: confirm release asm still calls the allocator in the timed path
cargo rustc --release -p spirula-io --bin spirula-perf-sample -- --emit=asm

# Full gate
cargo run -p spirula-xtask -- verify
```

## Explicit non-claims

- No OS RSS / working-set sampling.
- No GPU adapter, device memory, GPU timestamp-query, or absolute wall-clock
  timestamp measurement.
- No training, render, or application-session orchestration.
- No claim that cumulative allocated volume is a resident capacity budget.
- No claim that the sample’s scoped work-buffer budget equals process RSS or
  enforces total process memory.
- Phase 9 still owns cross-feature baseline aggregation (P9-F02).
