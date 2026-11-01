mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::{self, Write as _};
use std::path::PathBuf;

use boxt_core::Text;
use boxt_core::fact::{
    COMMON_FIELDS, Directive, FACT_KINDS, Fact, FactError, FactKind, FileKind, LineRule, PathRule,
};
use boxt_core::limits::LimitName;
use boxt_core::location::{FrameKind, Scope, ScopeKind, ScriptName};
use boxt_core::shell::RedirectOp;
use boxt_core::version::{RULE_SCHEMA_VERSION, SchemaVersion};
use serde::Serialize;
use serde::ser::{self, Serializer};
use serde_json::Value;

use common::{sample_fact, sample_fact_with, scopes};

#[test]
fn table_is_in_enum_order() {
    assert_eq!(FACT_KINDS.len(), FactKind::ALL.len());
    for (spec, kind) in FACT_KINDS.iter().zip(FactKind::ALL) {
        assert_eq!(spec.kind, kind);
        assert_eq!(FactKind::from_name(spec.name), Some(kind));
        assert!(!spec.sources.is_empty(), "{}", spec.name);
        let unique: BTreeSet<FileKind> = spec.sources.iter().map(|r| r.file_kind).collect();
        assert_eq!(unique.len(), spec.sources.len(), "{}", spec.name);
        assert!(
            spec.sources.iter().all(|r| !r.scopes.is_empty()),
            "{}",
            spec.name
        );
    }
}

#[test]
fn table_names_and_fields_match_the_enum() {
    for kind in FactKind::ALL {
        let fact = sample_fact(kind);
        assert_eq!(fact.kind(), kind);
        let json = serde_json::to_value(&fact).unwrap();
        let outer = json.as_object().unwrap();
        assert_eq!(outer.keys().collect::<Vec<_>>(), vec![kind.name()]);
        let object = outer[kind.name()].as_object().unwrap();
        let serialized: BTreeSet<&str> = object.keys().map(String::as_str).collect();
        let declared: BTreeSet<&str> = COMMON_FIELDS
            .iter()
            .copied()
            .chain(kind.fields().iter().map(|f| f.name))
            .collect();
        assert_eq!(serialized, declared, "{}", kind.name());
        assert_eq!(
            serde_json::to_value(kind).unwrap(),
            Value::from(kind.name())
        );
    }
}

#[test]
fn field_types_fit_the_serialized_values() {
    use boxt_core::fact::FieldType;
    for kind in FactKind::ALL {
        let json = serde_json::to_value(sample_fact(kind)).unwrap();
        let object = json[kind.name()].as_object().unwrap();
        for field in kind.fields() {
            let value = &object[field.name];
            let fits = match field.ty {
                FieldType::Text | FieldType::Path => value.is_string() || value.is_null(),
                FieldType::Integer => value.is_u64() || value.is_null(),
                FieldType::Bool => value.is_boolean(),
                FieldType::TextList => value
                    .as_array()
                    .is_some_and(|a| a.iter().all(Value::is_string)),
                FieldType::Guards | FieldType::Records => value
                    .as_array()
                    .is_some_and(|a| a.iter().all(Value::is_object)),
            };
            assert!(fits, "{} {} {value}", kind.name(), field.name);
        }
    }
}

#[test]
fn every_listed_field_resolves_on_a_sample_of_its_kind() {
    use boxt_core::fact::FieldValue;
    for kind in FactKind::ALL {
        let fact = sample_fact(kind);
        let empty = sample_fact_with(kind, false);
        let json = serde_json::to_value(&empty).unwrap();
        for field in kind.fields() {
            let value = fact
                .field(field.name)
                .unwrap_or_else(|| panic!("{} {} does not resolve", kind.name(), field.name));
            assert_eq!(value.ty(), field.ty, "{} {}", kind.name(), field.name);
            let serialized = &serde_json::to_value(&fact).unwrap()[kind.name()][field.name];
            let resolved = match &value {
                FieldValue::Text(t) => Some(serde_json::to_value(t.as_ref()).unwrap()),
                FieldValue::Path(p) => Some(serde_json::to_value(p).unwrap()),
                FieldValue::Integer(n) => Some(Value::from(*n)),
                FieldValue::Bool(b) => Some(Value::from(*b)),
                FieldValue::TextList(l) => Some(serde_json::to_value(l).unwrap()),
                FieldValue::Guards(_)
                | FieldValue::Assignments(_)
                | FieldValue::Redirections(_) => None,
            };
            if let Some(resolved) = resolved {
                assert_eq!(&resolved, serialized, "{} {}", kind.name(), field.name);
            }
            let absent = json[kind.name()][field.name].is_null();
            assert_eq!(
                empty.field(field.name).is_none(),
                absent,
                "{} {}",
                kind.name(),
                field.name
            );
        }
        assert_eq!(fact.field("file_kind"), None);
        assert_eq!(fact.field("location"), None);
        assert_eq!(fact.field("no_such_field"), None);
    }
}

#[test]
fn a_field_value_is_the_fact_value() {
    use boxt_core::fact::FieldValue;
    let call = sample_fact(FactKind::Call);
    assert_eq!(
        call.field("callee"),
        Some(FieldValue::Text(std::borrow::Cow::Owned(Text::from(
            "setuid"
        ))))
    );
    let script = sample_fact(FactKind::PackageScript);
    assert_eq!(
        script.field("name"),
        Some(FieldValue::Text(std::borrow::Cow::Owned(Text::from(
            "post_install"
        ))))
    );
    let file = sample_fact(FactKind::PackageFile);
    assert_eq!(file.field("mode"), Some(FieldValue::Integer(0o104755)));
}

#[test]
fn file_kind_names_are_their_serialized_names() {
    for kind in common::file_kinds() {
        assert_eq!(
            serde_json::to_value(kind).unwrap(),
            Value::from(kind.name())
        );
    }
    assert_eq!(common::file_kinds(), FileKind::ALL);
}

#[test]
fn every_fact_carries_an_allowed_file_kind() {
    for kind in FactKind::ALL {
        let fact = sample_fact(kind);
        assert_eq!(fact.validate(), Ok(()), "{}", kind.name());
        assert!(kind.file_kinds().contains(&fact.file_kind()));
    }
    let wrong = Fact::Import {
        file_kind: FileKind::Source,
        location: common::location("a.c", 1, boxt_core::location::Scope::File),
        symbol: boxt_core::Text::from("dlopen"),
        version: None,
    };
    assert_eq!(
        wrong.validate(),
        Err(FactError::FileKindNotAllowed {
            kind: FactKind::Import,
            file_kind: FileKind::Source,
        })
    );
}

#[test]
fn no_fact_kind_names_a_behaviour() {
    let behaviours = [
        "dynamic_load",
        "fixed_user_id",
        "forking",
        "needs_root",
        "setuid",
        "privileged_port",
    ];
    for spec in FACT_KINDS {
        assert!(!behaviours.contains(&spec.name), "{}", spec.name);
    }
}

type Shapes = BTreeSet<String>;
type Fields = Vec<(String, Shapes)>;

enum Payload {
    Unit,
    Newtype(Shapes),
    Struct(Fields),
}

enum Container {
    Struct(Fields),
    Newtype(Shapes),
    Enum(BTreeMap<(u32, String), Payload>),
}

#[derive(Default)]
struct Registry {
    types: BTreeMap<String, Container>,
}

fn merge_fields(into: &mut Fields, from: Fields) {
    for (name, shapes) in from {
        match into.iter_mut().find(|(n, _)| *n == name) {
            Some((_, existing)) => existing.extend(shapes),
            None => into.push((name, shapes)),
        }
    }
}

impl Registry {
    fn record_struct(&mut self, name: &str, fields: Fields) {
        match self
            .types
            .entry(name.to_owned())
            .or_insert_with(|| Container::Struct(Vec::new()))
        {
            Container::Struct(existing) => merge_fields(existing, fields),
            _ => panic!("{name} traced as two kinds of type"),
        }
    }

    fn record_newtype(&mut self, name: &str, shape: String) {
        match self
            .types
            .entry(name.to_owned())
            .or_insert_with(|| Container::Newtype(Shapes::new()))
        {
            Container::Newtype(existing) => {
                existing.insert(shape);
            }
            _ => panic!("{name} traced as two kinds of type"),
        }
    }

    fn record_variant(&mut self, name: &str, index: u32, variant: &str, payload: Payload) {
        let Container::Enum(variants) = self
            .types
            .entry(name.to_owned())
            .or_insert_with(|| Container::Enum(BTreeMap::new()))
        else {
            panic!("{name} traced as two kinds of type");
        };
        let slot = variants
            .entry((index, variant.to_owned()))
            .or_insert(Payload::Unit);
        match (slot, payload) {
            (_, Payload::Unit) => {}
            (slot @ Payload::Unit, payload) => *slot = payload,
            (Payload::Newtype(existing), Payload::Newtype(shapes)) => existing.extend(shapes),
            (Payload::Struct(existing), Payload::Struct(fields)) => merge_fields(existing, fields),
            _ => panic!("{name}::{variant} traced with two payload forms"),
        }
    }
}

#[derive(Debug)]
struct TraceError(String);

impl fmt::Display for TraceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for TraceError {}

impl ser::Error for TraceError {
    fn custom<T: fmt::Display>(msg: T) -> Self {
        TraceError(msg.to_string())
    }
}

struct Tracer<'a>(&'a mut Registry);

struct Compound<'a> {
    reg: &'a mut Registry,
    open: &'static str,
    close: &'static str,
    shapes: Shapes,
    variant: Option<(&'static str, u32, &'static str)>,
}

struct MapTracer<'a> {
    reg: &'a mut Registry,
    keys: Shapes,
    values: Shapes,
    schema_key: Option<String>,
    schema_fields: Fields,
}

const SCHEMA_MAP_KEYS: &[&str] = &["bytes_base64"];

struct StructTracer<'a> {
    reg: &'a mut Registry,
    name: &'static str,
    fields: Fields,
    variant: Option<(u32, &'static str)>,
}

fn shape_of<T: ?Sized + Serialize>(reg: &mut Registry, value: &T) -> Result<String, TraceError> {
    value.serialize(Tracer(reg))
}

impl<'a> Serializer for Tracer<'a> {
    type Ok = String;
    type Error = TraceError;
    type SerializeSeq = Compound<'a>;
    type SerializeTuple = Compound<'a>;
    type SerializeTupleStruct = Compound<'a>;
    type SerializeTupleVariant = Compound<'a>;
    type SerializeMap = MapTracer<'a>;
    type SerializeStruct = StructTracer<'a>;
    type SerializeStructVariant = StructTracer<'a>;

    fn serialize_bool(self, _: bool) -> Result<String, TraceError> {
        Ok("bool".into())
    }
    fn serialize_i8(self, _: i8) -> Result<String, TraceError> {
        Ok("i8".into())
    }
    fn serialize_i16(self, _: i16) -> Result<String, TraceError> {
        Ok("i16".into())
    }
    fn serialize_i32(self, _: i32) -> Result<String, TraceError> {
        Ok("i32".into())
    }
    fn serialize_i64(self, _: i64) -> Result<String, TraceError> {
        Ok("i64".into())
    }
    fn serialize_u8(self, _: u8) -> Result<String, TraceError> {
        Ok("u8".into())
    }
    fn serialize_u16(self, _: u16) -> Result<String, TraceError> {
        Ok("u16".into())
    }
    fn serialize_u32(self, _: u32) -> Result<String, TraceError> {
        Ok("u32".into())
    }
    fn serialize_u64(self, _: u64) -> Result<String, TraceError> {
        Ok("u64".into())
    }
    fn serialize_f32(self, _: f32) -> Result<String, TraceError> {
        Ok("f32".into())
    }
    fn serialize_f64(self, _: f64) -> Result<String, TraceError> {
        Ok("f64".into())
    }
    fn serialize_char(self, _: char) -> Result<String, TraceError> {
        Ok("char".into())
    }
    fn serialize_str(self, _: &str) -> Result<String, TraceError> {
        Ok("string".into())
    }
    fn serialize_bytes(self, _: &[u8]) -> Result<String, TraceError> {
        Ok("bytes".into())
    }
    fn serialize_none(self) -> Result<String, TraceError> {
        Ok("null".into())
    }
    fn serialize_some<T: ?Sized + Serialize>(self, value: &T) -> Result<String, TraceError> {
        value.serialize(self)
    }
    fn serialize_unit(self) -> Result<String, TraceError> {
        Ok("unit".into())
    }
    fn serialize_unit_struct(self, name: &'static str) -> Result<String, TraceError> {
        Ok(name.into())
    }
    fn serialize_unit_variant(
        self,
        name: &'static str,
        index: u32,
        variant: &'static str,
    ) -> Result<String, TraceError> {
        self.0.record_variant(name, index, variant, Payload::Unit);
        Ok(name.into())
    }
    fn serialize_newtype_struct<T: ?Sized + Serialize>(
        self,
        name: &'static str,
        value: &T,
    ) -> Result<String, TraceError> {
        let shape = shape_of(self.0, value)?;
        self.0.record_newtype(name, shape);
        Ok(name.into())
    }
    fn serialize_newtype_variant<T: ?Sized + Serialize>(
        self,
        name: &'static str,
        index: u32,
        variant: &'static str,
        value: &T,
    ) -> Result<String, TraceError> {
        let shape = shape_of(self.0, value)?;
        self.0
            .record_variant(name, index, variant, Payload::Newtype([shape].into()));
        Ok(name.into())
    }
    fn serialize_seq(self, _: Option<usize>) -> Result<Compound<'a>, TraceError> {
        Ok(Compound {
            reg: self.0,
            open: "[",
            close: "]",
            shapes: Shapes::new(),
            variant: None,
        })
    }
    fn serialize_tuple(self, _: usize) -> Result<Compound<'a>, TraceError> {
        Ok(Compound {
            reg: self.0,
            open: "(",
            close: ")",
            shapes: Shapes::new(),
            variant: None,
        })
    }
    fn serialize_tuple_struct(
        self,
        _: &'static str,
        len: usize,
    ) -> Result<Compound<'a>, TraceError> {
        self.serialize_tuple(len)
    }
    fn serialize_tuple_variant(
        self,
        name: &'static str,
        index: u32,
        variant: &'static str,
        _: usize,
    ) -> Result<Compound<'a>, TraceError> {
        Ok(Compound {
            reg: self.0,
            open: "(",
            close: ")",
            shapes: Shapes::new(),
            variant: Some((name, index, variant)),
        })
    }
    fn serialize_map(self, _: Option<usize>) -> Result<MapTracer<'a>, TraceError> {
        Ok(MapTracer {
            reg: self.0,
            keys: Shapes::new(),
            values: Shapes::new(),
            schema_key: None,
            schema_fields: Vec::new(),
        })
    }
    fn serialize_struct(
        self,
        name: &'static str,
        _: usize,
    ) -> Result<StructTracer<'a>, TraceError> {
        Ok(StructTracer {
            reg: self.0,
            name,
            fields: Vec::new(),
            variant: None,
        })
    }
    fn serialize_struct_variant(
        self,
        name: &'static str,
        index: u32,
        variant: &'static str,
        _: usize,
    ) -> Result<StructTracer<'a>, TraceError> {
        Ok(StructTracer {
            reg: self.0,
            name,
            fields: Vec::new(),
            variant: Some((index, variant)),
        })
    }
}

impl Compound<'_> {
    fn element<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), TraceError> {
        let shape = shape_of(self.reg, value)?;
        self.shapes.insert(shape);
        Ok(())
    }

    fn finish(self) -> Result<String, TraceError> {
        let shape = format!("{}{}{}", self.open, union(&self.shapes), self.close);
        match self.variant {
            Some((name, index, variant)) => {
                self.reg
                    .record_variant(name, index, variant, Payload::Newtype([shape].into()));
                Ok(name.into())
            }
            None => Ok(shape),
        }
    }
}

impl ser::SerializeSeq for Compound<'_> {
    type Ok = String;
    type Error = TraceError;
    fn serialize_element<T: ?Sized + Serialize>(&mut self, v: &T) -> Result<(), TraceError> {
        self.element(v)
    }
    fn end(self) -> Result<String, TraceError> {
        self.finish()
    }
}

impl ser::SerializeTuple for Compound<'_> {
    type Ok = String;
    type Error = TraceError;
    fn serialize_element<T: ?Sized + Serialize>(&mut self, v: &T) -> Result<(), TraceError> {
        self.element(v)
    }
    fn end(self) -> Result<String, TraceError> {
        self.finish()
    }
}

impl ser::SerializeTupleStruct for Compound<'_> {
    type Ok = String;
    type Error = TraceError;
    fn serialize_field<T: ?Sized + Serialize>(&mut self, v: &T) -> Result<(), TraceError> {
        self.element(v)
    }
    fn end(self) -> Result<String, TraceError> {
        self.finish()
    }
}

impl ser::SerializeTupleVariant for Compound<'_> {
    type Ok = String;
    type Error = TraceError;
    fn serialize_field<T: ?Sized + Serialize>(&mut self, v: &T) -> Result<(), TraceError> {
        self.element(v)
    }
    fn end(self) -> Result<String, TraceError> {
        self.finish()
    }
}

impl ser::SerializeMap for MapTracer<'_> {
    type Ok = String;
    type Error = TraceError;
    fn serialize_key<T: ?Sized + Serialize>(&mut self, key: &T) -> Result<(), TraceError> {
        if let Ok(Value::String(name)) = serde_json::to_value(key)
            && SCHEMA_MAP_KEYS.contains(&name.as_str())
        {
            self.schema_key = Some(name);
            return Ok(());
        }
        let shape = shape_of(self.reg, key)?;
        self.keys.insert(shape);
        Ok(())
    }
    fn serialize_value<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), TraceError> {
        let shape = shape_of(self.reg, value)?;
        match self.schema_key.take() {
            Some(name) => self.schema_fields.push((name, [shape].into())),
            None => {
                self.values.insert(shape);
            }
        }
        Ok(())
    }
    fn end(self) -> Result<String, TraceError> {
        if self.keys.is_empty() && !self.schema_fields.is_empty() {
            return Ok(fields_text(&self.schema_fields));
        }
        Ok(format!(
            "map<{}, {}>",
            union(&self.keys),
            union(&self.values)
        ))
    }
}

impl StructTracer<'_> {
    fn field<T: ?Sized + Serialize>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<(), TraceError> {
        let shape = shape_of(self.reg, value)?;
        self.fields.push((key.to_owned(), [shape].into()));
        Ok(())
    }

    fn finish(self) -> Result<String, TraceError> {
        match self.variant {
            Some((index, variant)) => {
                self.reg
                    .record_variant(self.name, index, variant, Payload::Struct(self.fields));
            }
            None => self.reg.record_struct(self.name, self.fields),
        }
        Ok(self.name.into())
    }
}

impl ser::SerializeStruct for StructTracer<'_> {
    type Ok = String;
    type Error = TraceError;
    fn serialize_field<T: ?Sized + Serialize>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<(), TraceError> {
        self.field(key, value)
    }
    fn end(self) -> Result<String, TraceError> {
        self.finish()
    }
}

impl ser::SerializeStructVariant for StructTracer<'_> {
    type Ok = String;
    type Error = TraceError;
    fn serialize_field<T: ?Sized + Serialize>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<(), TraceError> {
        self.field(key, value)
    }
    fn end(self) -> Result<String, TraceError> {
        self.finish()
    }
}

fn union(shapes: &Shapes) -> String {
    let mut parts: Vec<&str> = shapes
        .iter()
        .map(String::as_str)
        .filter(|s| *s != "null")
        .collect();
    if shapes.contains("null") {
        parts.push("null");
    }
    parts.join(" | ")
}

fn fields_text(fields: &Fields) -> String {
    let parts: Vec<String> = fields
        .iter()
        .map(|(name, shapes)| format!("{name}: {}", union(shapes)))
        .collect();
    format!("{{{}}}", parts.join(", "))
}

fn traced_facts() -> Vec<Fact> {
    let mut facts = Vec::new();
    for kind in FactKind::ALL {
        for filled in [true, false] {
            let fact = sample_fact_with(kind, filled);
            for file_kind in kind.file_kinds() {
                let mut json = serde_json::to_value(&fact).unwrap();
                json[kind.name()]["file_kind"] = serde_json::to_value(file_kind).unwrap();
                facts.push(serde_json::from_value(json).unwrap());
            }
        }
    }
    for scope in scopes() {
        let mut json = serde_json::to_value(sample_fact(FactKind::Call)).unwrap();
        json["call"]["location"]["scope"] = serde_json::to_value(scope).unwrap();
        facts.push(serde_json::from_value(json).unwrap());
    }
    let mut raw = serde_json::to_value(sample_fact(FactKind::StringLiteral)).unwrap();
    raw["string_literal"]["value"] =
        serde_json::to_value(Text::from_bytes(vec![0xff, 0x00])).unwrap();
    facts.push(serde_json::from_value(raw).unwrap());
    facts
}

struct VariantNames(Option<&'static [&'static str]>);

impl<'de> serde::Deserializer<'de> for &mut VariantNames {
    type Error = serde::de::value::Error;

    fn deserialize_any<V: serde::de::Visitor<'de>>(self, _: V) -> Result<V::Value, Self::Error> {
        Err(serde::de::Error::custom("not an enum"))
    }

    fn deserialize_enum<V: serde::de::Visitor<'de>>(
        self,
        _: &'static str,
        variants: &'static [&'static str],
        _: V,
    ) -> Result<V::Value, Self::Error> {
        self.0 = Some(variants);
        Err(serde::de::Error::custom("variants captured"))
    }

    serde::forward_to_deserialize_any! {
        bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string bytes byte_buf
        option unit unit_struct newtype_struct seq tuple tuple_struct map struct identifier
        ignored_any
    }
}

fn serde_variants<T: serde::de::DeserializeOwned>() -> &'static [&'static str] {
    let mut names = VariantNames(None);
    let _ = T::deserialize(&mut names);
    names.0.expect("an externally tagged enum")
}

fn unit_names<T: Serialize>(all: &[T]) -> Vec<String> {
    all.iter()
        .map(|v| {
            serde_json::to_value(v)
                .unwrap()
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect()
}

#[test]
fn every_scope_variant_has_a_fixture() {
    let names: Vec<&str> = ScopeKind::ALL.iter().map(|k| k.name()).collect();
    assert_eq!(names, serde_variants::<Scope>());
    for kind in ScopeKind::ALL {
        assert_eq!(common::scope_of(kind).kind(), kind);
    }
    let fixtures: BTreeSet<ScopeKind> = scopes().iter().map(Scope::kind).collect();
    assert_eq!(fixtures.len(), ScopeKind::ALL.len());
}

#[test]
fn closed_enum_all_lists_every_variant() {
    assert_eq!(unit_names(&FileKind::ALL), serde_variants::<FileKind>());
    assert_eq!(unit_names(&FactKind::ALL), serde_variants::<FactKind>());
    assert_eq!(unit_names(&Directive::ALL), serde_variants::<Directive>());
    assert_eq!(unit_names(&ScopeKind::ALL), serde_variants::<ScopeKind>());
    assert_eq!(unit_names(&ScriptName::ALL), serde_variants::<ScriptName>());
    for name in ScriptName::ALL {
        assert_eq!(
            serde_json::to_value(name).unwrap(),
            Value::from(name.name())
        );
    }
    assert_eq!(unit_names(&PathRule::ALL), serde_variants::<PathRule>());
    for rule in PathRule::ALL {
        assert_eq!(
            serde_json::to_value(rule).unwrap(),
            Value::from(rule.name())
        );
    }
    assert_eq!(unit_names(&LineRule::ALL), serde_variants::<LineRule>());
    for rule in LineRule::ALL {
        assert_eq!(
            serde_json::to_value(rule).unwrap(),
            Value::from(rule.name())
        );
    }
    assert_eq!(unit_names(&FrameKind::ALL), serde_variants::<FrameKind>());
    assert_eq!(unit_names(&RedirectOp::ALL), serde_variants::<RedirectOp>());
    assert_eq!(unit_names(&LimitName::ALL), serde_variants::<LimitName>());
}

fn trace() -> Registry {
    let mut reg = Registry::default();
    for fact in traced_facts() {
        shape_of(&mut reg, &fact).unwrap();
    }
    reg
}

fn render() -> String {
    let reg = trace();
    let Some(Container::Enum(fact_variants)) = reg.types.get("Fact") else {
        panic!("Fact was not traced as an enum");
    };
    let mut out = String::new();
    writeln!(out, "rule_schema_version: {RULE_SCHEMA_VERSION}").unwrap();
    writeln!(out, "common: {}", COMMON_FIELDS.join(", ")).unwrap();
    for spec in FACT_KINDS {
        writeln!(out).unwrap();
        writeln!(out, "{}", spec.name).unwrap();
        for rule in spec.sources {
            let scopes: Vec<&str> = rule.scopes.iter().map(|k| k.name()).collect();
            writeln!(
                out,
                "  from {}: scopes {}; path {}; line {}",
                rule.file_kind.name(),
                scopes.join(", "),
                rule.path.name(),
                rule.line.name()
            )
            .unwrap();
        }
        let fields: Vec<String> = spec
            .fields
            .iter()
            .map(|f| format!("{} ({})", f.name, f.ty.name()))
            .collect();
        writeln!(out, "  fields: {}", fields.join(", ")).unwrap();
        let Some(Payload::Struct(fields)) =
            fact_variants.get(&(spec.kind as u32, spec.name.to_owned()))
        else {
            panic!("{} was not traced", spec.name);
        };
        writeln!(out, "  shape: {}", fields_text(fields)).unwrap();
    }
    assert_eq!(fact_variants.len(), FACT_KINDS.len());
    writeln!(out).unwrap();
    writeln!(out, "types").unwrap();
    for (name, container) in reg.types.iter().filter(|(n, _)| *n != "Fact") {
        let body = match container {
            Container::Struct(fields) => fields_text(fields),
            Container::Newtype(shapes) => union(shapes),
            Container::Enum(variants) => {
                let parts: Vec<String> = variants
                    .iter()
                    .map(|((_, variant), payload)| match payload {
                        Payload::Unit => variant.clone(),
                        Payload::Newtype(shapes) => format!("{variant}({})", union(shapes)),
                        Payload::Struct(fields) => format!("{variant} {}", fields_text(fields)),
                    })
                    .collect();
                parts.join(" | ")
            }
        };
        writeln!(out, "  {name} = {body}").unwrap();
    }
    out
}

#[test]
fn the_snapshot_holds_no_fixture_values() {
    let rendered = render();
    for value in [
        "setuid",
        "/var/run/app.pid",
        "glibc",
        "forking",
        "_WIN32",
        "501",
    ] {
        assert!(!rendered.contains(value), "{value}");
    }
}

fn snapshot_path(version: SchemaVersion) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("snapshots")
        .join(format!("fact-kinds-v{version}.txt"))
}

#[test]
fn fact_kinds_match_the_snapshot_for_this_rule_schema_version() {
    let current = render();
    let path = snapshot_path(RULE_SCHEMA_VERSION);
    let Ok(recorded) = std::fs::read_to_string(&path) else {
        if std::env::var_os("BOXT_WRITE_SNAPSHOT").is_some() {
            std::fs::write(&path, &current).unwrap();
            return;
        }
        panic!(
            "no snapshot for rule schema version {RULE_SCHEMA_VERSION} at {}; \
             rerun with BOXT_WRITE_SNAPSHOT=1 to record it:\n{current}",
            path.display()
        );
    };
    assert!(
        recorded == current,
        "the Fact kinds changed without a RULE_SCHEMA_VERSION bump. \
         Bump RULE_SCHEMA_VERSION in src/version.rs (the minor for an added kind or \
         field, the major for anything else), then rerun with \
         BOXT_WRITE_SNAPSHOT=1 to record fact-kinds-v{{new}}.txt. \
         Recorded snapshots are never edited.\n--- recorded\n{recorded}\n--- current\n{current}"
    );
}

#[test]
fn recorded_snapshots_are_never_ahead_of_the_version() {
    let dir = snapshot_path(RULE_SCHEMA_VERSION)
        .parent()
        .unwrap()
        .to_path_buf();
    for entry in std::fs::read_dir(dir).unwrap() {
        let name = entry.unwrap().file_name().into_string().unwrap();
        let version: SchemaVersion = name
            .strip_prefix("fact-kinds-v")
            .and_then(|s| s.strip_suffix(".txt"))
            .and_then(|s| s.parse().ok())
            .unwrap_or_else(|| panic!("unexpected snapshot file {name}"));
        assert!(version <= RULE_SCHEMA_VERSION, "{name}");
    }
}
