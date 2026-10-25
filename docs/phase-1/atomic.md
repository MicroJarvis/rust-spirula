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

- An empty `parent()` (bare relatives such as `config.json`) is normalized to
  `.` for temps, locks, `read_dir`, and directory sync.
- Temp and lock basenames keep the destination's native `OsString` identity.
  Matching uses encoded-byte prefixes, not `to_string_lossy`, so distinct
  non-UTF-8 names (for example `a\xFF` vs `a\xFE`) cannot share a recovery
  prefix.
- A publish takes an exclusive lease file `{file_name}.spirula-lock` with
  `create_new`, then creates a unique sibling
  `{file_name}.spirula-tmp.{pid}.{seq}` with `create_new` and **keeps that
  file handle** for the write/sync path (no close/reopen). Colliding temp
  names retry with a new sequence. Temps stay in the destination parent so
  replace stays same-filesystem.
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
- Concurrent publishers serialize on the lease (`SP-SESS-003` /
  `publish_lease_held` while another publish or recovery is active). Callers
  may retry. The last successful replace wins; readers see only complete
  previous or complete new payloads, never a torn mix.
- `recover_publish_orphans` takes the same exclusive lease. While a writer is
  paused mid-publish, recovery returns `SP-SESS-003` and deletes nothing.
  When the lease is free, it removes only strict
  `{file}.spirula-tmp.{decimal_pid}.{decimal_seq}` names. Arbitrary
  similarly-prefixed user files and the live destination are left alone.
- `TempHarness` allocates with `create_dir` and retries on `AlreadyExists`. It
  never adopts or deletes a preexisting directory; `Drop` removes only the
  root this harness created. Children must stay relative, must not include
  `..` / root / `Prefix` (including Windows drive-relative `C:foo`), and
  owned creation walks ancestors with `symlink_metadata` so a symlink inside
  the root cannot redirect writes outside.
- Config load/save/migrate live here. Save/migrate require
  `loaded.source_text()` to match the live file (`SP-SESS-001` on stale
  snapshots). Migration validates the new document before any backup write,
  creates `{path}.bak` only when absent (no-clobber), then publishes. Checkpoint
  bundles are out of scope.

## Deferred

Checkpoint directory swap, `state.tar` layout, completion markers, and resume
gates stay in P3-F07 / P5-F05. Dataset and image writers adopt this primitive
when those features land.
