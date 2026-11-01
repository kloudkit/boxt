mod common;

use boxt_core::fact::{FACT_KINDS, FactKindSpec, Field, FieldType};
use boxt_core::version::{RULE_SCHEMA_VERSION, SchemaVersion};
use boxt_rules::{generate_schema, rule_schema, schema_file_name};

use common::corpus_dir;

fn schema_path(version: SchemaVersion) -> std::path::PathBuf {
    corpus_dir().join("schema").join(schema_file_name(version))
}

#[test]
fn the_rule_schema_matches_the_committed_file_for_this_version() {
    let current = rule_schema();
    let path = schema_path(RULE_SCHEMA_VERSION);
    let Ok(recorded) = std::fs::read_to_string(&path) else {
        if std::env::var_os("BOXT_WRITE_SNAPSHOT").is_some() {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, &current).unwrap();
            return;
        }
        panic!(
            "no rule schema for rule schema version {RULE_SCHEMA_VERSION} at {}; \
             rerun with BOXT_WRITE_SNAPSHOT=1 to record it",
            path.display()
        );
    };
    assert!(
        recorded == current,
        "the rule schema changed without a RULE_SCHEMA_VERSION bump: the committed \
         {} differs from the generated schema for version {RULE_SCHEMA_VERSION}. Bump \
         RULE_SCHEMA_VERSION in boxt-core, then rerun with BOXT_WRITE_SNAPSHOT=1 to record \
         the new version's file. Recorded schema files are never edited.\n--- generated\n{current}",
        path.display()
    );
}

#[test]
fn recorded_schemas_are_never_ahead_of_the_version() {
    let dir = schema_path(RULE_SCHEMA_VERSION)
        .parent()
        .unwrap()
        .to_path_buf();
    for entry in std::fs::read_dir(dir).unwrap() {
        let name = entry.unwrap().file_name().into_string().unwrap();
        let version: SchemaVersion = name
            .strip_prefix("rule-v")
            .and_then(|s| s.strip_suffix(".schema.json"))
            .and_then(|s| s.parse().ok())
            .unwrap_or_else(|| panic!("unexpected schema file {name}"));
        assert!(version <= RULE_SCHEMA_VERSION, "{name}");
    }
}

#[test]
fn an_added_fact_field_changes_the_schema() {
    let base = generate_schema(FACT_KINDS, RULE_SCHEMA_VERSION);
    let mut kinds: Vec<FactKindSpec> = FACT_KINDS.to_vec();
    let mut fields: Vec<Field> = kinds[0].fields.to_vec();
    fields.push(Field {
        name: "new_field",
        ty: FieldType::Text,
    });
    kinds[0].fields = Box::leak(fields.into_boxed_slice());
    let changed = generate_schema(&kinds, RULE_SCHEMA_VERSION);
    assert_ne!(base, changed);
    assert!(changed.contains("new_field"));

    let mut retyped: Vec<FactKindSpec> = FACT_KINDS.to_vec();
    let mut fields: Vec<Field> = retyped[0].fields.to_vec();
    fields[0].ty = FieldType::Integer;
    retyped[0].fields = Box::leak(fields.into_boxed_slice());
    assert_ne!(base, generate_schema(&retyped, RULE_SCHEMA_VERSION));
}

#[test]
fn the_schema_names_every_fact_kind_and_field() {
    let schema: serde_json::Value = serde_json::from_str(&rule_schema()).unwrap();
    let defs = schema["$defs"].as_object().unwrap();
    let kinds: Vec<&str> = schema["$defs"]["node"]["properties"]["kind"]["enum"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(kinds, FACT_KINDS.iter().map(|k| k.name).collect::<Vec<_>>());
    for spec in FACT_KINDS {
        let names =
            &defs[&format!("node_{}", spec.name)]["properties"]["field"]["anyOf"][0]["enum"];
        for field in spec.fields.iter().filter(|f| {
            !matches!(
                f.ty,
                FieldType::TextList | FieldType::Guards | FieldType::Records
            )
        }) {
            assert!(
                names.as_array().unwrap().iter().any(|v| v == field.name),
                "{} {}",
                spec.name,
                field.name
            );
        }
    }
    assert_eq!(
        schema["properties"]["schema"]["anyOf"][0]["const"],
        RULE_SCHEMA_VERSION.to_string()
    );
}
