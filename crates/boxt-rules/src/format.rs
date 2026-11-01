use std::fmt;

use boxt_core::verdict::Tier;
use serde::Deserialize;
use serde::de::{self, Deserializer, IgnoredAny, Visitor};

#[derive(Debug, Deserialize)]
pub(crate) struct Header {
    pub schema: Option<Scalar>,
}

pub(crate) type Keys = std::collections::BTreeMap<String, IgnoredAny>;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawRule {
    #[serde(rename = "target", default, deserialize_with = "present")]
    _target: bool,
    #[serde(rename = "targets", default, deserialize_with = "present")]
    _targets: bool,
    #[serde(rename = "tier", default, deserialize_with = "present")]
    _tier: bool,
    #[serde(rename = "tiers", default, deserialize_with = "present")]
    _tiers: bool,
    #[serde(rename = "schema")]
    _schema: IgnoredAny,
    pub id: String,
    pub behaviour: String,
    pub title: Option<String>,
    pub fix: Option<String>,
    pub evidence: Option<RawEvidence>,
    #[serde(rename = "match")]
    pub matches: Option<RawMatch>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawEvidence {
    pub found: Option<RawEvidenceEntry>,
    pub suspected: Option<RawEvidenceEntry>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawEvidenceEntry {
    pub id: String,
    pub text: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawMatch {
    pub found: Option<RawNode>,
    pub suspected: Option<RawNode>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawNode {
    pub kind: Option<String>,
    pub field: Option<String>,
    pub equals: Option<Literal>,
    pub one_of: Option<Vec<Literal>>,
    pub pattern: Option<String>,
    pub path_under: Option<String>,
    pub number: Option<RawNumber>,
    pub guard_contains: Option<String>,
    pub co_occurs: Option<RawCoOccurs>,
    pub all: Option<Vec<RawNode>>,
    pub any: Option<Vec<RawNode>>,
    pub not: Option<Box<RawNode>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawNumber {
    pub op: NumberOp,
    pub value: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize)]
pub enum NumberOp {
    #[serde(rename = "<")]
    Lt,
    #[serde(rename = "<=")]
    Le,
    #[serde(rename = "=")]
    Eq,
    #[serde(rename = ">=")]
    Ge,
    #[serde(rename = ">")]
    Gt,
}

impl NumberOp {
    pub const ALL: [NumberOp; 5] = [
        NumberOp::Lt,
        NumberOp::Le,
        NumberOp::Eq,
        NumberOp::Ge,
        NumberOp::Gt,
    ];

    pub const fn symbol(self) -> &'static str {
        match self {
            NumberOp::Lt => "<",
            NumberOp::Le => "<=",
            NumberOp::Eq => "=",
            NumberOp::Ge => ">=",
            NumberOp::Gt => ">",
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawCoOccurs {
    pub scope: CoScope,
    pub all: Vec<RawNode>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoScope {
    Function,
    File,
    Package,
}

impl CoScope {
    pub const ALL: [CoScope; 3] = [CoScope::Function, CoScope::File, CoScope::Package];

    pub const fn name(self) -> &'static str {
        match self {
            CoScope::Function => "function",
            CoScope::File => "file",
            CoScope::Package => "package",
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawProfile {
    #[serde(rename = "schema")]
    _schema: IgnoredAny,
    pub id: String,
    pub version: String,
    #[serde(default)]
    pub allowed: Vec<String>,
    #[serde(default)]
    pub controls: Vec<RawControl>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawControl {
    pub id: String,
    #[serde(default)]
    pub behaviours: Vec<RawBehaviourTier>,
    pub not_detectable: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawBehaviourTier {
    pub behaviour: String,
    pub tier: Tier,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawManifest {
    pub corpus_version: Option<Scalar>,
    #[serde(default)]
    pub behaviours: Vec<RawBehaviour>,
    #[serde(default)]
    pub windows_guard_macros: Vec<String>,
    #[serde(default)]
    pub dynamic_load_symbols: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawBehaviour {
    pub id: String,
    pub dimension: String,
}

fn present<'de, D: Deserializer<'de>>(deserializer: D) -> Result<bool, D::Error> {
    IgnoredAny::deserialize(deserializer)?;
    Ok(true)
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Literal {
    Text(String),
    Integer(i64),
    Bool(bool),
}

impl fmt::Display for Literal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Literal::Text(s) => f.write_str(s),
            Literal::Integer(n) => write!(f, "{n}"),
            Literal::Bool(b) => write!(f, "{b}"),
        }
    }
}

struct LiteralVisitor;

impl<'de> Visitor<'de> for LiteralVisitor {
    type Value = Literal;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a string, an integer or a boolean")
    }

    fn visit_bool<E: de::Error>(self, v: bool) -> Result<Literal, E> {
        Ok(Literal::Bool(v))
    }

    fn visit_i64<E: de::Error>(self, v: i64) -> Result<Literal, E> {
        Ok(Literal::Integer(v))
    }

    fn visit_u64<E: de::Error>(self, v: u64) -> Result<Literal, E> {
        i64::try_from(v)
            .map(Literal::Integer)
            .map_err(|_| E::invalid_value(de::Unexpected::Unsigned(v), &self))
    }

    fn visit_str<E: de::Error>(self, v: &str) -> Result<Literal, E> {
        Ok(Literal::Text(v.to_owned()))
    }

    fn visit_string<E: de::Error>(self, v: String) -> Result<Literal, E> {
        Ok(Literal::Text(v))
    }
}

impl<'de> Deserialize<'de> for Literal {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(LiteralVisitor)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Scalar(pub String);

struct ScalarVisitor;

impl<'de> Visitor<'de> for ScalarVisitor {
    type Value = Scalar;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a version string")
    }

    fn visit_str<E: de::Error>(self, v: &str) -> Result<Scalar, E> {
        Ok(Scalar(v.to_owned()))
    }

    fn visit_string<E: de::Error>(self, v: String) -> Result<Scalar, E> {
        Ok(Scalar(v))
    }
}

impl<'de> Deserialize<'de> for Scalar {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_str(ScalarVisitor)
    }
}
