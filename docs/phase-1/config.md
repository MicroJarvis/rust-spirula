# P1-F02: training config contract

This note records the C++ reference behavior for `config.json` and the
validation and serialization rules `spirula-core` implements. `TrainConfig`
has all 251 `SS_CONFIG_FIELDS` rows from the pinned reference
`src/config/TrainConfig.h`. Load and save are string APIs; they do not touch
the filesystem.

Reference sources: `src/config/TrainConfig.h`, `src/config/TrainConfigJson.h`,
`src/data/JsonField.h`, `src/data/Json.h`.

## Reference behavior

- One flat key per flag, spelled as the struct member. Section and tier are
  presentational and are not stored in JSON.
- Writers emit keys in table order. A full reference write emits every field.
- A missing key leaves the field at its default. Load starts from whatever
  config the caller already has, normally the defaults.
- Unknown keys are ignored. They are not an error. The reference full write
  drops them, because it emits only table fields.
- `null` on `std::string` stores an empty string. `null` on `std::optional<T>`
  disengages it. `null` on bool, int, float and the length-3 arrays leaves the
  current value unchanged.
- An empty `std::string` is written as `null`, not `""`. An engaged optional
  writes its inner value. A disengaged optional writes `null`.
- Floats accept and emit the Python `json` tokens `Infinity`, `-Infinity` and
  `NaN`. `outlier_threshold` and `max_world_size` default to `+Infinity`.
- A JSON string whose characters are `Infinity`, `-Infinity` or `NaN` is a
  string, not a number.
- Choice cells are string constraints, not C++ enums. `choices == ""` is
  free-form. `choices == "none"` is free-form and displays empty as `none`.
  `<folder>`, `<file>` and `<data>/...` are path strings. Any other cell is a
  `|`-separated closed set. The reference JSON loader does not check the set.
- Wrong JSON types are mostly coerced by the reference accessors (`as_int`
  yields 0, `as_bool` yields false, `as_string` yields empty). Arrays of the
  wrong length throw. There is no schema version and no numeric range table.
- `steps` is not a flag and is not an alias of `num_iterations`. `preset` is
  not a flag. `fixtures/checkpoints/tiny-run/config.json` keeps those keys.

## Rust validation

These differences are intentional. They do not add flags or renumber errors.

| condition | code | reference |
|---|---|---|
| malformed JSON, wrong shape, non-integral int, duplicate root key, null or non-integer `schema_version`, unescaped U+0000..U+001F in a string | `SP-FMT-002` | coerce, or throw an untyped `runtime_error` for arrays |
| integer outside `i32` | `SP-INP-003` | cast through `double` |
| nonempty closed choice not in its set, including a value assigned in memory and then written | `SP-FMT-003` | accept any string |
| `schema_version` other than the exact integer 1 | `SP-FMT-003` | field does not exist |

- Missing `schema_version` is legacy and stays accepted. There is no
  deprecation date.
- `schema_version: 1` is the current document. The integer must be exact.
- Integer fields and `schema_version` use the numeric lexeme. A value is an
  integer when its decimal digits and exponent place it on an integer,
  including `1.0`, `1e0`, and `10e-1`. Leading zeros in the exponent do not
  change it: `1e000000000000000000000` is 1. `1.0000000000000001` is not an integer,
  even though `f64` rounds it to `1`. The same rule covers exponent
  cancellation such as `21474836470e-1`. An exact integer outside `i32` is
  `SP-INP-003`. An exact integer other than 1 in `schema_version` is
  `SP-FMT-003`.
- JSON strings reject raw U+0000 through U+001F. The escapes `\b`, `\f`, `\n`,
  `\r`, `\t`, and `\u00XX` stay valid and decode to those characters.
- An empty JSON string is a valid closed choice only when the reference
  default is empty (`FieldSpec::empty_is_unset`). `data_format: ""` stays
  valid; `scene_center: ""` does not. That restriction is only for closed
  choices. Free, free-form and path fields accept `""` even when their
  reference default is nonempty. `null` on every string field stores an
  empty string and stays valid, including `scene_center: null`.
- Writers encode every empty string as `null`. `TrainConfig::validate`,
  `to_current_json`, and `to_preserved_json` accept that encoding, so a
  document loaded from `null` can be saved. A nonempty closed choice outside
  its set is still rejected on write. Free, free-form, and path strings stay
  writable. `None` optionals stay writable and are still emitted as `null`.
- Free, free-form and path strings are not enum-checked.
- No extra numeric domain ranges. The reference table does not define them.
- Float fields accept finite numbers and the three non-finite tokens. They
  reject strings, including the string `"Infinity"`.
- Duplicate keys at the root are rejected so a field is not dropped. Nested
  duplicates inside an unknown value stay inside that preserved value.

## Serialization

`spirula-core` exposes string and document APIs only. It does not open,
rename or replace files.

- A document this crate originates carries `schema_version` 1 and then every
  known field in table order. Empty strings and disengaged optionals are
  `null`. Non-finite floats use the Python tokens.
- Loading an external document remembers its original text, the keys it
  contained, and every unknown value, including nested objects and arrays.
- Saving that document writes known keys that were present from the typed
  values. Unknown keys are preserved, including nested values.
- A field that was absent is written when its typed value differs from the
  value captured at load. An absent field that is still unchanged stays
  absent. Edits to previously missing fields are therefore kept, and untouched
  defaults are not inserted.
- That save does not insert `schema_version` into a legacy document.
- If the typed values still match the loaded values, save returns the original
  text unchanged. Floats match by `f32` bit pattern: `+0` and `-0` are
  different, and a sign-only edit is written as `-0`. NaN still matches NaN.
- Promoting a document to the current form is explicit: `schema_version` 1,
  every known field, then the preserved unknown keys. That call returns text
  when `validate` succeeds. It does not rewrite a file.

## Filesystem (P1-F05)

`spirula-io` finishes the filesystem contract. Details are in
[atomic.md](atomic.md).

- `load_train_config` reads bytes, UTF-8-decodes (`SP-FMT-002` on invalid
  UTF-8 with `path`), then parses with `LoadedTrainConfig::parse` (path
  attached on parse errors).
- `LoadedTrainConfig::source_text` is the exact loaded bytes. Save and migrate
  hold the destination lease while checking that text still matches the live
  file; a concurrent rewrite is `SP-SESS-001`.
- `save_train_config` atomically replaces the path with preserved JSON after
  validating the bytes parse and `TrainConfig::validate`.
- `migrate_train_config` validates current-form JSON first, creates
  `{path}.bak` with atomic no-clobber (existing backups are never replaced),
  then publishes under the same destination lease. The reference still opens
  `config.json` directly (`TrainerCore.cpp:759-768`); the rewrite does not.

The Phase 0 checkpoint `config.json` is a legacy compatibility input. Readers
accept it as written. Fixture files under `fixtures/` are not rewritten by
these APIs.

## Planned execution validation

The compatibility reader above remains unchanged. P5-F08 adds a separate
capability-validated `TrainPlan`; accepting any of the 251 fields does not
mean its algorithm is implemented. Finite numeric ranges, unsupported-option
errors, resource/capability checks and resolved digest belong to that layer.
P4-F01 applies the same principle to render requests; P6-F03 only exposes
implemented options. Acceptance is in
[development-gates.md](../development-gates.md).
