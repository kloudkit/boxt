use boxt_core::fact::{FACT_KINDS, FactKindSpec, FieldType};
use boxt_core::verdict::ID_SEGMENT_PATTERN;
use boxt_core::version::{RULE_SCHEMA_VERSION, SchemaVersion};
use serde_json::{Map, Value, json};

use crate::format::{CoScope, NumberOp};
use crate::validate::{COMMON_TEST_FIELDS, TARGET_KEYS, TIER_KEYS};

const FIELD_TESTS: [&str; 5] = ["equals", "one_of", "pattern", "path_under", "number"];
const COMBINATORS: [&str; 4] = ["co_occurs", "all", "any", "not"];
const CONFIDENCES: [&str; 2] = ["found", "suspected"];

pub fn schema_file_name(version: SchemaVersion) -> String {
    format!("rule-v{version}.schema.json")
}

pub fn rule_schema() -> String {
    generate(FACT_KINDS, RULE_SCHEMA_VERSION)
}

fn text() -> Value {
    json!({ "type": "string", "pattern": "\\S" })
}

pub fn generate(kinds: &[FactKindSpec], version: SchemaVersion) -> String {
    let kind_names: Vec<&str> = kinds.iter().map(|k| k.name).collect();
    let mut defs = Map::new();

    defs.insert(
        "evidence_entry".into(),
        json!({
            "type": "object",
            "additionalProperties": false,
            "required": ["id", "text"],
            "properties": {
                "id": { "$ref": "#/$defs/id" },
                "text": text()
            }
        }),
    );
    defs.insert(
        "id".into(),
        json!({
            "type": "string",
            "pattern": format!("^{ID_SEGMENT_PATTERN}(/{ID_SEGMENT_PATTERN})*$")
        }),
    );
    defs.insert(
        "rule_id".into(),
        json!({
            "type": "string",
            "pattern": format!("^{ID_SEGMENT_PATTERN}(/{ID_SEGMENT_PATTERN})+$")
        }),
    );
    defs.insert(
        "node".into(),
        json!({
            "type": "object",
            "required": ["kind"],
            "properties": { "kind": { "enum": kind_names } },
            "allOf": kinds.iter().map(|k| json!({
                "if": { "properties": { "kind": { "const": k.name } } },
                "then": { "$ref": format!("#/$defs/node_{}", k.name) }
            })).collect::<Vec<_>>()
        }),
    );
    defs.insert("node_root".into(), node_body(None));
    defs.insert("child_root".into(), child("#/$defs/node_root"));
    for k in kinds {
        defs.insert(format!("node_{}", k.name), node_body(Some(k)));
        defs.insert(
            format!("child_{}", k.name),
            child(&format!("#/$defs/node_{}", k.name)),
        );
    }

    let mut properties = Map::new();
    properties.insert(
        "schema".into(),
        json!({ "anyOf": [
            { "const": version.to_string() },
            { "const": f64::from(version.major) + f64::from(version.minor) / 10f64.powi(digits(version.minor)) }
        ] }),
    );
    properties.insert("id".into(), json!({ "$ref": "#/$defs/rule_id" }));
    properties.insert("behaviour".into(), json!({ "$ref": "#/$defs/id" }));
    properties.insert("title".into(), text());
    properties.insert("fix".into(), text());
    properties.insert(
        "evidence".into(),
        json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "found": { "$ref": "#/$defs/evidence_entry" },
                "suspected": { "$ref": "#/$defs/evidence_entry" }
            }
        }),
    );
    properties.insert(
        "match".into(),
        json!({
            "type": "object",
            "additionalProperties": false,
            "minProperties": 1,
            "properties": {
                "found": { "$ref": "#/$defs/child_root" },
                "suspected": { "$ref": "#/$defs/child_root" }
            }
        }),
    );

    let mut pairing = Vec::new();
    for c in CONFIDENCES {
        for (from, to) in [("match", "evidence"), ("evidence", "match")] {
            pairing.push(json!({
                "if": { "required": [from], "properties": { from: { "required": [c] } } },
                "then": { "required": [to], "properties": { to: { "required": [c] } } }
            }));
        }
    }

    let forbidden: Vec<&str> = TARGET_KEYS.iter().chain(TIER_KEYS).copied().collect();
    let schema = json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "$id": format!("boxt:rule-v{version}"),
        "title": format!("boxt rule, schema {version}"),
        "type": "object",
        "additionalProperties": false,
        "required": ["schema", "id", "behaviour", "title", "fix", "evidence", "match"],
        "properties": properties,
        "not": { "anyOf": forbidden.iter().map(|k| json!({ "required": [k] })).collect::<Vec<_>>() },
        "allOf": pairing,
        "$defs": defs
    });
    let mut out = serde_json::to_string_pretty(&schema).expect("schema serializes");
    out.push('\n');
    out
}

fn digits(n: u32) -> i32 {
    if n == 0 { 1 } else { n.ilog10() as i32 + 1 }
}

fn child(body: &str) -> Value {
    json!({
        "if": { "type": "object", "required": ["kind"] },
        "then": { "$ref": "#/$defs/node" },
        "else": { "$ref": body }
    })
}

fn fields_of(kind: &FactKindSpec, types: &[FieldType]) -> Option<Value> {
    let names: Vec<&str> = COMMON_TEST_FIELDS
        .iter()
        .filter(|(_, _, ty)| types.contains(ty))
        .map(|(_, n, _)| *n)
        .chain(
            kind.fields
                .iter()
                .filter(|f| types.contains(&f.ty))
                .map(|f| f.name),
        )
        .collect();
    let lists: Vec<&str> = kind
        .fields
        .iter()
        .filter(|f| f.ty == FieldType::TextList)
        .map(|f| f.name)
        .collect();
    let mut any = Vec::new();
    if !names.is_empty() {
        any.push(json!({ "enum": names }));
    }
    if types.contains(&FieldType::Text) && !lists.is_empty() {
        any.push(json!({
            "type": "string",
            "pattern": format!("^({})\\.(0|[1-9][0-9]*)$", lists.join("|"))
        }));
    }
    (!any.is_empty()).then(|| json!({ "anyOf": any }))
}

fn literal_type(ty: FieldType) -> Option<&'static str> {
    match ty {
        FieldType::Text | FieldType::Path => Some("string"),
        FieldType::Integer => Some("integer"),
        FieldType::Bool => Some("boolean"),
        _ => None,
    }
}

fn node_body(kind: Option<&FactKindSpec>) -> Value {
    let child = match kind {
        Some(k) => format!("#/$defs/child_{}", k.name),
        None => "#/$defs/child_root".to_owned(),
    };
    let mut properties = Map::new();
    let mut branches = Vec::new();
    let mut tests: Vec<&str> = Vec::new();
    if let Some(k) = kind {
        properties.insert("kind".into(), json!({ "const": k.name }));
        let all_types = [
            FieldType::Text,
            FieldType::Path,
            FieldType::Integer,
            FieldType::Bool,
        ];
        if let Some(any) = fields_of(k, &all_types) {
            properties.insert("field".into(), any);
        }
        properties.insert("equals".into(), json!({}));
        properties.insert("one_of".into(), json!({ "type": "array", "minItems": 1 }));
        properties.insert("pattern".into(), text());
        properties.insert("path_under".into(), text());
        properties.insert(
            "number".into(),
            json!({
                "type": "object",
                "additionalProperties": false,
                "required": ["op", "value"],
                "properties": {
                    "op": { "enum": NumberOp::ALL.iter().map(|o| o.symbol()).collect::<Vec<_>>() },
                    "value": { "type": "integer" }
                }
            }),
        );
        for test in ["equals", "one_of"] {
            let mut alternatives = Vec::new();
            for group in [
                &[FieldType::Text, FieldType::Path][..],
                &[FieldType::Integer],
                &[FieldType::Bool],
            ] {
                let (Some(fields), Some(ty)) = (fields_of(k, group), literal_type(group[0])) else {
                    continue;
                };
                let value = if test == "equals" {
                    json!({ "type": ty })
                } else {
                    json!({ "items": { "type": ty } })
                };
                alternatives.push(json!({ "properties": { "field": fields, test: value } }));
            }
            if !alternatives.is_empty() {
                branches.push(json!({ "required": [test, "field"], "anyOf": alternatives }));
            }
        }
        for (test, group) in [
            ("pattern", &[FieldType::Text, FieldType::Path][..]),
            ("path_under", &[FieldType::Path]),
            ("number", &[FieldType::Integer]),
        ] {
            if let Some(fields) = fields_of(k, group) {
                branches.push(json!({
                    "required": [test, "field"],
                    "properties": { "field": fields }
                }));
            }
        }
        tests.extend(FIELD_TESTS);
        if k.fields.iter().any(|f| f.ty == FieldType::Guards) {
            properties.insert("guard_contains".into(), text());
            branches
                .push(json!({ "required": ["guard_contains"], "not": { "required": ["field"] } }));
            tests.push("guard_contains");
        }
    }
    properties.insert(
        "co_occurs".into(),
        json!({
            "type": "object",
            "additionalProperties": false,
            "required": ["scope", "all"],
            "properties": {
                "scope": { "enum": CoScope::ALL.iter().map(|s| s.name()).collect::<Vec<_>>() },
                "all": { "type": "array", "minItems": 1, "items": { "$ref": child } }
            }
        }),
    );
    properties.insert(
        "all".into(),
        json!({ "type": "array", "minItems": 1, "items": { "$ref": child } }),
    );
    properties.insert(
        "any".into(),
        json!({ "type": "array", "minItems": 1, "items": { "$ref": child } }),
    );
    properties.insert("not".into(), json!({ "$ref": child }));
    for c in COMBINATORS {
        branches.push(json!({ "required": [c], "not": { "required": ["field"] } }));
    }
    tests.extend(COMBINATORS);
    if kind.is_some() {
        let mut excluded: Vec<Value> = tests.iter().map(|t| json!({ "required": [t] })).collect();
        excluded.push(json!({ "required": ["field"] }));
        branches.push(json!({ "required": ["kind"], "not": { "anyOf": excluded } }));
    }
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": properties,
        "oneOf": branches
    })
}
