use spirula_core::{
    ChoiceKind, ErrorCode, FieldType, JsonKind, JsonNumber, LoadedTrainConfig,
    TRAIN_CONFIG_FIELD_COUNT, TRAIN_CONFIG_FIELDS, TRAIN_CONFIG_SCHEMA_VERSION, TrainConfig,
    field_catalog_json, field_spec,
};
use std::path::PathBuf;

fn get<'a>(value: &'a spirula_core::JsonValue, key: &str) -> &'a spirula_core::JsonValue {
    let JsonKind::Object(pairs) = &value.kind else {
        panic!("expected object");
    };
    &pairs
        .iter()
        .find(|(name, _)| name == key)
        .unwrap_or_else(|| panic!("missing {key}"))
        .1
}

fn array(value: &spirula_core::JsonValue) -> &[spirula_core::JsonValue] {
    let JsonKind::Array(items) = &value.kind else {
        panic!("expected array");
    };
    items
}

fn string(value: &spirula_core::JsonValue) -> &str {
    let JsonKind::String(text) = &value.kind else {
        panic!("expected string");
    };
    text
}

fn flag(value: &spirula_core::JsonValue) -> bool {
    let JsonKind::Bool(flag) = value.kind else {
        panic!("expected bool");
    };
    flag
}

fn number(value: &spirula_core::JsonValue) -> f64 {
    let JsonKind::Number(JsonNumber::Finite(number)) = value.kind else {
        panic!("expected number");
    };
    number
}

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures")
}

fn read(path: &str) -> String {
    std::fs::read_to_string(fixtures().join(path))
        .unwrap_or_else(|error| panic!("reading {path}: {error}"))
}

#[test]
fn golden_catalog_and_defaults_cover_all_251_fields() {
    let catalog = field_catalog_json();
    let defaults = TrainConfig::default().to_current_json();
    assert_eq!(read("config/catalog.json"), catalog);
    assert_eq!(read("config/defaults.json"), defaults);

    let loaded = LoadedTrainConfig::parse(&defaults).unwrap();
    assert_eq!(loaded.schema_version(), Some(TRAIN_CONFIG_SCHEMA_VERSION));
    assert!(loaded.unknown_fields().is_empty());
    assert!(loaded.config.same_values(&TrainConfig::default()));
    assert_eq!(loaded.to_preserved_json(), defaults);
    assert_eq!(TRAIN_CONFIG_FIELDS.len(), 251);
    assert_eq!(TRAIN_CONFIG_FIELD_COUNT, 251);

    let parsed = spirula_core::parse_json(&catalog).unwrap();
    let fields = array(get(&parsed, "fields"));
    assert_eq!(number(get(&parsed, "field_count")), 251.0);
    assert_eq!(fields.len(), 251);
    for (spec, row) in TRAIN_CONFIG_FIELDS.iter().zip(fields) {
        assert_eq!(string(get(row, "name")), spec.name);
        assert_eq!(string(get(row, "section")), spec.section);
        assert_eq!(string(get(row, "tier")), spec.tier);
        assert_eq!(string(get(row, "choices")), spec.choices);
        assert_eq!(flag(get(row, "empty_is_unset")), spec.empty_is_unset);
        let type_name = match spec.value_type {
            FieldType::String => "string",
            FieldType::Int => "int",
            FieldType::Bool => "bool",
            FieldType::Float => "float",
            FieldType::OptionalBool => "optional_bool",
            FieldType::OptionalFloat => "optional_float",
            FieldType::OptionalInt => "optional_int",
            FieldType::Vec3Int => "vec3_int",
            FieldType::Vec3Float => "vec3_float",
        };
        assert_eq!(string(get(row, "type")), type_name);
    }
    assert!(defaults.contains("\"outlier_threshold\":Infinity"));
    assert!(defaults.contains("\"max_world_size\":Infinity"));
    assert!(loaded.config.outlier_threshold.is_infinite());
    assert!(loaded.config.max_world_size.is_infinite());
}

#[test]
fn legacy_checkpoint_config_roundtrips_without_rewriting() {
    let path = fixtures().join("checkpoints/tiny-run/config.json");
    let original = std::fs::read_to_string(&path).unwrap();
    let loaded = LoadedTrainConfig::parse(&original).unwrap();
    assert_eq!(loaded.schema_version(), None);
    assert_eq!(loaded.config.data, "fixtures/datasets/tiny-colmap");
    assert_eq!(loaded.config.output_dir_name, "tiny-run");
    assert_eq!(loaded.config.steps_per_save, 1);
    assert!(loaded.config.save_only_latest_checkpoint);
    assert_eq!(loaded.config.scene_center, "none");
    assert_eq!(loaded.config.num_iterations, 30000);
    let unknown: Vec<_> = loaded
        .unknown_fields()
        .into_iter()
        .map(|field| (field.name, field.raw))
        .collect();
    assert_eq!(
        unknown,
        vec![
            ("preset".to_owned(), "\"3dgs\"".to_owned()),
            ("steps".to_owned(), "1".to_owned()),
        ]
    );
    assert_eq!(loaded.to_preserved_json(), original);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
}

#[test]
fn preserved_save_keeps_unknowns_and_edits_to_missing_fields() {
    let original = read("checkpoints/tiny-run/config.json");
    let mut loaded = LoadedTrainConfig::parse(&original).unwrap();
    loaded.config.num_iterations = 4;
    let saved = loaded.to_preserved_json();
    assert!(!saved.contains("schema_version"));
    assert!(saved.contains("\"preset\":\"3dgs\""));
    assert!(saved.contains("\"steps\":1"));
    assert!(saved.contains("\"num_iterations\":4"));
    assert!(!saved.contains("\"sh_degree\""));

    let again = LoadedTrainConfig::parse(&saved).unwrap();
    assert_eq!(again.schema_version(), None);
    assert_eq!(again.config.num_iterations, 4);
    assert_eq!(again.config.data, "fixtures/datasets/tiny-colmap");
    assert_eq!(again.unknown_fields().len(), 2);

    let migrated = loaded.to_current_json();
    let current = LoadedTrainConfig::parse(&migrated).unwrap();
    assert_eq!(current.schema_version(), Some(1));
    assert_eq!(current.config.num_iterations, 4);
    assert_eq!(current.unknown_fields().len(), 2);
    assert!(migrated.contains("\"schema_version\":1"));
}

#[test]
fn nested_unknown_values_and_infinity_strings_survive() {
    let text = concat!(
        r#"{"resume":"Infinity","outlier_threshold":Infinity,"extra":{"nested":[-Infinity,{"word":"NaN"}],"a":1,"a":2}}"#,
        "\n"
    );
    let mut loaded = LoadedTrainConfig::parse(text).unwrap();
    assert_eq!(loaded.config.resume, "Infinity");
    assert!(loaded.config.outlier_threshold.is_infinite());
    assert!(loaded.config.outlier_threshold.is_sign_positive());
    assert_eq!(loaded.to_preserved_json(), text);

    loaded.config.viewer_port = 9;
    let saved = loaded.to_preserved_json();
    let again = LoadedTrainConfig::parse(&saved).unwrap();
    assert_eq!(again.config.resume, "Infinity");
    assert_eq!(again.config.viewer_port, 9);
    assert_eq!(again.schema_version(), None);
    let extra = again
        .unknown_fields()
        .into_iter()
        .find(|field| field.name == "extra")
        .unwrap();
    assert!(extra.raw.contains("-Infinity"));
    assert!(extra.raw.contains("\"word\":\"NaN\""));
    assert!(extra.raw.contains("\"a\":1"));
    assert!(extra.raw.contains("\"a\":2"));
}

#[test]
fn missing_and_null_follow_the_reference_field_rules() {
    let loaded = LoadedTrainConfig::parse(
        r#"{"num_iterations":null,"data":null,"apply_loss_for_mask":null,"scale_init":null}"#,
    )
    .unwrap();
    assert_eq!(loaded.config.num_iterations, 30000);
    assert_eq!(loaded.config.data, "");
    assert_eq!(loaded.config.apply_loss_for_mask, None);
    assert_eq!(loaded.config.scale_init, None);
    assert_eq!(loaded.config.output_dir_prefix, "outputs");
}

#[test]
fn invalid_shapes_ranges_versions_and_enums_use_stable_codes() {
    let cases = [
        (
            r#"{"num_iterations":"30000"}"#,
            ErrorCode::MalformedRecord,
            "num_iterations",
        ),
        (
            r#"{"num_iterations":1.5}"#,
            ErrorCode::MalformedRecord,
            "num_iterations",
        ),
        (
            r#"{"num_iterations":2147483648}"#,
            ErrorCode::ValueOutOfRange,
            "num_iterations",
        ),
        (
            r#"{"save_only_latest_checkpoint":1}"#,
            ErrorCode::MalformedRecord,
            "save_only_latest_checkpoint",
        ),
        (
            r#"{"background_color":[0,0]}"#,
            ErrorCode::MalformedRecord,
            "background_color",
        ),
        (
            r#"{"scene_center":"sideways"}"#,
            ErrorCode::UnsupportedVersion,
            "scene_center",
        ),
        (
            r#"{"scene_center":""}"#,
            ErrorCode::UnsupportedVersion,
            "scene_center",
        ),
        (
            r#"{"data_format":"nope"}"#,
            ErrorCode::UnsupportedVersion,
            "data_format",
        ),
        (
            r#"{"schema_version":2}"#,
            ErrorCode::UnsupportedVersion,
            "schema_version",
        ),
        (
            r#"{"schema_version":"1"}"#,
            ErrorCode::MalformedRecord,
            "schema_version",
        ),
        (
            r#"{"schema_version":null}"#,
            ErrorCode::MalformedRecord,
            "schema_version",
        ),
        (
            r#"{"data":"a","data":"b"}"#,
            ErrorCode::MalformedRecord,
            "data",
        ),
        ("[]", ErrorCode::MalformedRecord, "document"),
    ];
    for (text, code, field) in cases {
        let error = LoadedTrainConfig::parse(text).expect_err(text);
        assert_eq!(error.code(), code, "{text}");
        assert_eq!(
            error.context_value(spirula_core::ContextKey::Field),
            Some(field),
            "{text}"
        );
    }
    assert!(LoadedTrainConfig::parse(r#"{"data_format":""}"#).is_ok());
    assert!(LoadedTrainConfig::parse(r#"{"data_format":"colmap"}"#).is_ok());
    let spec = field_spec("data_format").unwrap();
    assert!(spec.empty_is_unset);
    assert!(matches!(spec.choice_kind, ChoiceKind::Closed(_)));
}

#[test]
fn nonfinite_tokens_roundtrip_and_nan_matches() {
    let text = r#"{"outlier_threshold":-Infinity,"max_world_size":NaN,"means_lr":Infinity}"#;
    let loaded = LoadedTrainConfig::parse(text).unwrap();
    assert!(
        loaded.config.outlier_threshold.is_infinite()
            && loaded.config.outlier_threshold.is_sign_negative()
    );
    assert!(loaded.config.max_world_size.is_nan());
    assert!(loaded.config.means_lr.is_infinite());
    let saved = loaded.to_preserved_json();
    assert_eq!(saved, text);
}
