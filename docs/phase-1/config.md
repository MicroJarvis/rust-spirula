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
| closed choice not in its set, including a value assigned in memory and then written | `SP-FMT-003` | accept any string |
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
- An empty string is a valid closed choice only when the reference default is
  empty (`FieldSpec::empty_is_unset`). `data_format: ""` stays valid;
  `scene_center: ""` does not.
- `TrainConfig::validate` checks every closed choice on the typed value.
  `to_current_json` and `to_preserved_json` return `Result` and call it, so a
  public field assignment cannot persist a document the reader rejects.
  Free, free-form, and path strings, including empty, stay writable. `None`
  optionals stay writable and are still emitted as `null`.
- Free, free-form and path strings are not enum-checked.
- No extra numeric domain ranges. The reference table does not define them.
- Float fields accept finite numbers and the three non-finite tokens. They
  reject strings, including the string `"Infinity"`.
- Duplicate keys at the root are rejected so a field is not dropped. Nested
  duplicates inside an unknown value stay inside that preserved value.

## Serialization

`spirula-core` exposes string and document APIs only. It does not open,
rename or replace files. Backup and atomic replacement belong to P1-F05.

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

The Phase 0 checkpoint `config.json` is a legacy compatibility input. Readers
accept it as written. Nothing in this feature changes that file so a test can
pass.
