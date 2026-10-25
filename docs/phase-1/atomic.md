# P1-F05: atomic publish and temp harness

`spirula-io` owns same-directory temporary siblings, completeness checks,
atomic replace, orphan recovery, the reusable temp-directory harness, and
config filesystem save/migration. It does not define checkpoint bundle layout
or completion markers (P5-F05).

## Reference behavior

Pinned sources: `src/core/AtomicFile.h`, `src/app/TrainerCore.cpp:759-768`,
`src/dense/MemoryReport.cpp:50-58`, and the Phase 0 inventory notes on
non-atomic checkpoints.

- `AtomicFile.h::replace_file` publishes a finished temporary onto the
  destination. POSIX uses `std::filesystem::rename`. Windows uses
  `MoveFileExW` with `MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH`.
  There is no delete-destination-then-rename sequence.
- Dense memory reports write `destination + ".part"`, flush, then call
  `replace_file`. Concurrent readers may make publish fail; the next write
  retries. The shared `.part` name is not exclusive.
- `TrainerCore::save_config_json` opens `config.json` with `fopen(..., "w")`
  and writes directly. Scene transform JSON does the same. Those paths are
  intentional reference gaps: a crash mid-write can leave a truncated file.
- Checkpoint directories are also non-atomic in the reference and have no
  completion marker. This feature supplies the file-level primitive only;
  bundle schema and markers stay in P5-F05 / P3-F07.

## Binding decisions

- A publish creates a unique sibling
  `{file_name}.spirula-tmp.{pid}-{seq}` with `create_new`. Colliding names
  retry with a new sequence. Temps always live in the destination's parent
  directory so replace stays same-filesystem.
- Bytes are written, flushed, and `sync_all`'d on the temp. The writer reports
  an expected length; a mismatch is `SP-FMT-004` and aborts before replace.
  Optional validators run on the payload before the destination is touched.
- Replace uses the platform primitive above. Failure before replace leaves the
  previous destination bytes intact and deletes the temp. There is no
  delete-old-then-rename window on any supported platform.
- Parent-directory sync after replace defaults on for Unix and off for
  Windows. Directory handles do not give the same durability story on
  Windows, so the default stays honest. When sync is requested and fails, the
  destination already holds the new bytes; the error is `SP-IO-004` with
  `actual=postpublish_directory_sync` and a local `source`. Callers must treat
  that as post-publish durability failure, not as an untouched destination.
- Typed IO errors carry `path` and a local `source`. Cancel checks return
  `SP-CAN-001` and clean the temp without touching the destination.
- Concurrent publishers each own a temp. The last successful replace wins.
  Readers may observe only complete previous or complete new payloads for a
  single-file destination, not a torn mix of two writers' bytes.
- `recover_publish_orphans(destination)` deletes only siblings matching the
  destination's `.spirula-tmp.` prefix. It never modifies the live file.
- `TempHarness` creates an isolated directory under the process temp root and
  removes it on drop. Children must stay relative and must not escape the
  root.
- Config load/save/migrate live here. Save writes preserved JSON atomically.
  Explicit migration writes `{path}.bak` with the pre-migration bytes first,
  then publishes current-form JSON. Checkpoint bundles are out of scope.

## Deferred

Checkpoint directory swap, `state.tar` layout, completion markers, and resume
gates stay in P3-F07 / P5-F05. Dataset and image writers adopt this primitive
when those features land.
