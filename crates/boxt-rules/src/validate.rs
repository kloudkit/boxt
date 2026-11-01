use std::cmp::Ordering;
use std::collections::BTreeSet;
use std::hash::{Hash, Hasher};

use boxt_core::SortedVec;
use boxt_core::fact::{COMMON_FIELDS, FactKind, FieldType};
use boxt_core::location::RelPath;
use boxt_core::pattern::{self, Pattern, PatternMode};
use boxt_core::verdict::{
    BehaviourId, BehaviourTier, Confidence, Control, ControlId, DimensionId, EvidenceId, IdError,
    Profile, RuleId, TargetId,
};
use boxt_core::version::{PROFILE_FORMAT_VERSION, RULE_SCHEMA_VERSION, SchemaVersion};

use crate::error::{CorpusError, Problem};
use crate::format::{
    CoScope, Header, Keys, Literal, NumberOp, RawEvidenceEntry, RawManifest, RawNode, RawProfile,
    RawRule,
};
use crate::load::parse;

pub const COMMON_TEST_FIELDS: &[(CommonField, &str, FieldType)] = &[
    (CommonField::FileKind, "file_kind", FieldType::Text),
    (CommonField::LocationPath, "location.path", FieldType::Path),
    (
        CommonField::LocationLine,
        "location.line",
        FieldType::Integer,
    ),
    (
        CommonField::LocationScope,
        "location.scope",
        FieldType::Text,
    ),
];

pub const TARGET_KEYS: &[&str] = &["target", "targets"];
pub const TIER_KEYS: &[&str] = &["tier", "tiers"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CommonField {
    FileKind,
    LocationPath,
    LocationLine,
    LocationScope,
}

impl CommonField {
    pub fn name(self) -> &'static str {
        COMMON_TEST_FIELDS
            .iter()
            .find(|(f, _, _)| *f == self)
            .map(|(_, n, _)| *n)
            .unwrap_or_default()
    }

    pub fn root(self) -> &'static str {
        let name = self.name();
        name.split('.').next().unwrap_or(name)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum FieldPath {
    Common(CommonField),
    Kind {
        name: &'static str,
        index: Option<u32>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum FieldTest {
    Equals(Literal),
    OneOf(Vec<Literal>),
    Pattern(Pattern),
    PathUnder(Pattern),
    Number { op: NumberOp, value: i64 },
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Test {
    Exists,
    Field { field: FieldPath, test: FieldTest },
    GuardContains(String),
    CoOccurs { scope: CoScope, all: Vec<Matcher> },
    All(Vec<Matcher>),
    Any(Vec<Matcher>),
    Not(Box<Matcher>),
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Matcher {
    pub kind: Option<FactKind>,
    pub test: Test,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EvidenceDef {
    pub id: EvidenceId,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RuleMatch {
    pub confidence: Confidence,
    pub evidence: EvidenceDef,
    pub matcher: Matcher,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Rule {
    pub id: RuleId,
    pub file: RelPath,
    pub behaviour: BehaviourId,
    pub title: String,
    pub fix: String,
    pub matches: Vec<RuleMatch>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Behaviour {
    pub id: BehaviourId,
    pub dimension: DimensionId,
}

#[derive(Debug, Clone)]
pub struct CorpusVersion {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
    pub pre: Vec<String>,
    pub build: Vec<String>,
}

impl CorpusVersion {
    pub fn parse(s: &str) -> Option<CorpusVersion> {
        let (rest, build) = match s.split_once('+') {
            Some((r, b)) => (r, Some(b)),
            None => (s, None),
        };
        let (core, pre) = match rest.split_once('-') {
            Some((c, p)) => (c, Some(p)),
            None => (rest, None),
        };
        let mut parts = core.split('.');
        let major = numeric(parts.next()?)?;
        let minor = numeric(parts.next()?)?;
        let patch = numeric(parts.next()?)?;
        if parts.next().is_some() {
            return None;
        }
        let pre = match pre {
            Some(p) => identifiers(p, true)?,
            None => Vec::new(),
        };
        let build = match build {
            Some(b) => identifiers(b, false)?,
            None => Vec::new(),
        };
        Some(CorpusVersion {
            major,
            minor,
            patch,
            pre,
            build,
        })
    }

    pub fn zero() -> CorpusVersion {
        CorpusVersion {
            major: 0,
            minor: 0,
            patch: 0,
            pre: Vec::new(),
            build: Vec::new(),
        }
    }

    pub fn precedence(&self, other: &CorpusVersion) -> Ordering {
        let core =
            (self.major, self.minor, self.patch).cmp(&(other.major, other.minor, other.patch));
        if core != Ordering::Equal {
            return core;
        }
        match (self.pre.is_empty(), other.pre.is_empty()) {
            (true, true) => return Ordering::Equal,
            (true, false) => return Ordering::Greater,
            (false, true) => return Ordering::Less,
            (false, false) => {}
        }
        for (a, b) in self.pre.iter().zip(&other.pre) {
            let ord = match (a.parse::<u64>(), b.parse::<u64>()) {
                (Ok(x), Ok(y)) => x.cmp(&y),
                (Ok(_), Err(_)) => Ordering::Less,
                (Err(_), Ok(_)) => Ordering::Greater,
                (Err(_), Err(_)) => a.as_bytes().cmp(b.as_bytes()),
            };
            if ord != Ordering::Equal {
                return ord;
            }
        }
        self.pre.len().cmp(&other.pre.len())
    }
}

impl PartialEq for CorpusVersion {
    fn eq(&self, other: &Self) -> bool {
        self.precedence(other) == Ordering::Equal
    }
}

impl Eq for CorpusVersion {}

impl PartialOrd for CorpusVersion {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for CorpusVersion {
    fn cmp(&self, other: &Self) -> Ordering {
        self.precedence(other)
    }
}

impl Hash for CorpusVersion {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.major.hash(state);
        self.minor.hash(state);
        self.patch.hash(state);
        self.pre.hash(state);
    }
}

impl std::fmt::Display for CorpusVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)?;
        if !self.pre.is_empty() {
            write!(f, "-{}", self.pre.join("."))?;
        }
        if !self.build.is_empty() {
            write!(f, "+{}", self.build.join("."))?;
        }
        Ok(())
    }
}

fn numeric(s: &str) -> Option<u64> {
    let canonical = s == "0" || (!s.is_empty() && !s.starts_with('0'));
    if !canonical || !s.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    s.parse().ok()
}

fn identifiers(s: &str, pre: bool) -> Option<Vec<String>> {
    let mut out = Vec::new();
    for id in s.split('.') {
        if id.is_empty() || !id.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-') {
            return None;
        }
        if pre && id.bytes().all(|c| c.is_ascii_digit()) && numeric(id).is_none() {
            return None;
        }
        out.push(id.to_owned());
    }
    Some(out)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    pub corpus_version: CorpusVersion,
    pub behaviours: Vec<Behaviour>,
    pub windows_guard_macros: Vec<String>,
    pub dynamic_load_symbols: Vec<String>,
}

impl Manifest {
    pub fn empty() -> Manifest {
        Manifest {
            corpus_version: CorpusVersion::zero(),
            behaviours: Vec::new(),
            windows_guard_macros: Vec::new(),
            dynamic_load_symbols: Vec::new(),
        }
    }

    pub fn behaviour(&self, id: &BehaviourId) -> Option<&Behaviour> {
        self.behaviours
            .binary_search_by(|b| b.id.cmp(id))
            .ok()
            .map(|i| &self.behaviours[i])
    }

    fn merged(base: &Manifest, user: Manifest) -> Manifest {
        let mut behaviours = base.behaviours.clone();
        behaviours.extend(user.behaviours);
        behaviours.sort();
        let join = |a: &[String], b: Vec<String>| {
            let mut out = a.to_vec();
            out.extend(b.into_iter().filter(|s| !a.contains(s)));
            out
        };
        Manifest {
            corpus_version: base.corpus_version.clone(),
            behaviours,
            windows_guard_macros: join(&base.windows_guard_macros, user.windows_guard_macros),
            dynamic_load_symbols: join(&base.dynamic_load_symbols, user.dynamic_load_symbols),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    Corpus,
    User,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Corpus {
    pub origin: Origin,
    pub manifest: Manifest,
    pub user_version: Option<CorpusVersion>,
    pub rules: Vec<Rule>,
    pub profiles: Vec<Profile>,
}

pub(crate) enum Role {
    Manifest,
    Registry,
    Rule,
    Profile,
}

pub(crate) struct SourceFile {
    pub path: RelPath,
    pub role: Role,
    pub text: Result<String, Problem>,
}

struct Sink {
    errors: Vec<CorpusError>,
}

impl Sink {
    fn push(&mut self, file: &RelPath, problem: Problem) {
        self.errors.push(CorpusError {
            file: file.clone(),
            problem,
        });
    }
}

fn id<T>(
    sink: &mut Sink,
    file: &RelPath,
    key: &'static str,
    value: &str,
    make: fn(String) -> Result<T, IdError>,
) -> Option<T> {
    match make(value.to_owned()) {
        Ok(v) => Some(v),
        Err(error) => {
            sink.push(
                file,
                Problem::BadId {
                    key,
                    value: value.to_owned(),
                    error,
                },
            );
            None
        }
    }
}

fn blank(s: &Option<String>) -> bool {
    s.as_deref().is_none_or(|s| s.trim().is_empty())
}

fn check_version(sink: &mut Sink, file: &RelPath, text: &str, supported: SchemaVersion) -> bool {
    let header: Header = match parse(text) {
        Ok(h) => h,
        Err(problem) => {
            sink.push(file, problem);
            return false;
        }
    };
    let Some(raw) = header.schema else {
        sink.push(file, Problem::NoSchemaVersion);
        return false;
    };
    let Ok(found) = raw.0.parse::<SchemaVersion>() else {
        sink.push(file, Problem::BadSchemaVersion(raw.0));
        return false;
    };
    if let Err(e) = supported.check(found) {
        sink.push(file, Problem::UnsupportedVersion(e));
        return false;
    }
    true
}

pub(crate) fn validate(
    files: Vec<SourceFile>,
    origin: Origin,
    base: Option<&Corpus>,
) -> Result<Corpus, Vec<CorpusError>> {
    let mut sink = Sink { errors: Vec::new() };
    let mut manifest_file = None;
    let mut rule_files = Vec::new();
    let mut profile_files = Vec::new();
    for file in files {
        let text = match file.text {
            Ok(t) => t,
            Err(problem) => {
                sink.push(&file.path, problem);
                if matches!(file.role, Role::Manifest) && origin == Origin::Corpus {
                    manifest_file = Some(None);
                }
                continue;
            }
        };
        match (file.role, origin) {
            (Role::Manifest, Origin::Corpus) | (Role::Registry, Origin::User) => {
                manifest_file = Some(Some((file.path, text)));
            }
            (Role::Manifest, Origin::User) | (Role::Registry, Origin::Corpus) => {
                sink.push(&file.path, Problem::UnexpectedFile);
            }
            (Role::Rule, _) => rule_files.push((file.path, text)),
            (Role::Profile, _) => profile_files.push((file.path, text)),
        }
    }

    let mut own = Manifest::empty();
    let mut user_version = None;
    match manifest_file {
        Some(Some((path, text))) => {
            if let Some((m, v)) = manifest_from(&mut sink, &path, &text, origin) {
                own = m;
                if origin == Origin::User {
                    user_version = v;
                }
            }
        }
        Some(None) => {}
        None if origin == Origin::Corpus => sink.push(&manifest_path(), Problem::NoManifest),
        None => {}
    }
    let manifest = match (origin, base) {
        (Origin::User, Some(base)) => Manifest::merged(&base.manifest, own),
        _ => own,
    };

    let registry = Registry {
        ids: manifest.behaviours.iter().map(|b| b.id.clone()).collect(),
        origin,
    };
    let taken: BTreeSet<String> = base
        .map(|b| {
            b.rules
                .iter()
                .map(|r| r.id.to_string())
                .chain(b.profiles.iter().map(|p| p.id.to_string()))
                .collect()
        })
        .unwrap_or_default();
    let ids = Ids { origin, taken };

    let mut rules = Vec::new();
    for (path, text) in &rule_files {
        if let Some(rule) = rule_from(&mut sink, path, text, &registry, &ids) {
            rules.push(rule);
        }
    }
    rules.sort_by(|a, b| a.id.cmp(&b.id).then_with(|| a.file.cmp(&b.file)));
    for pair in rules.windows(2) {
        if pair[0].id == pair[1].id {
            sink.push(&pair[1].file, Problem::DuplicateId(pair[1].id.to_string()));
        }
    }

    let mut profiles: Vec<(RelPath, Profile)> = Vec::new();
    for (path, text) in &profile_files {
        if let Some(profile) = profile_from(&mut sink, path, text, &registry, &ids) {
            profiles.push((path.clone(), profile));
        }
    }
    profiles.sort_by(|a, b| a.1.id.cmp(&b.1.id).then_with(|| a.0.cmp(&b.0)));
    for pair in profiles.windows(2) {
        if pair[0].1.id == pair[1].1.id {
            sink.push(&pair[1].0, Problem::DuplicateId(pair[1].1.id.to_string()));
        }
    }

    if sink.errors.is_empty() {
        Ok(Corpus {
            origin,
            manifest,
            user_version,
            rules,
            profiles: profiles.into_iter().map(|(_, p)| p).collect(),
        })
    } else {
        sink.errors.sort_by(|a, b| a.file.cmp(&b.file));
        Err(sink.errors)
    }
}

fn manifest_path() -> RelPath {
    RelPath::new(boxt_core::Text::from("manifest.yaml")).expect("valid path")
}

struct Registry {
    ids: Vec<BehaviourId>,
    origin: Origin,
}

impl Registry {
    fn check(&self, sink: &mut Sink, file: &RelPath, value: &str) -> Option<BehaviourId> {
        let b = id(sink, file, "behaviour", value, BehaviourId::new)?;
        if b.is_user() && self.origin == Origin::Corpus {
            sink.push(file, Problem::UserBehaviourOutsideUserRegistry(b));
            return None;
        }
        if self.ids.binary_search(&b).is_err() {
            sink.push(file, Problem::UnknownBehaviour(b));
            return None;
        }
        Some(b)
    }
}

struct Ids {
    origin: Origin,
    taken: BTreeSet<String>,
}

impl Ids {
    fn check(&self, sink: &mut Sink, file: &RelPath, id: &str) -> bool {
        let mut ok = true;
        if self.origin == Origin::User && !id.starts_with("user/") {
            sink.push(file, Problem::NotUserId(id.to_owned()));
            ok = false;
        }
        if self.taken.contains(id) {
            sink.push(file, Problem::CollidesWithCorpus(id.to_owned()));
            ok = false;
        }
        ok
    }
}

fn manifest_from(
    sink: &mut Sink,
    file: &RelPath,
    text: &str,
    origin: Origin,
) -> Option<(Manifest, Option<CorpusVersion>)> {
    let raw: RawManifest = match parse(text) {
        Ok(r) => r,
        Err(problem) => {
            sink.push(file, problem);
            return None;
        }
    };
    let before = sink.errors.len();
    let corpus_version = match (&raw.corpus_version, origin) {
        (Some(v), _) => {
            let parsed = CorpusVersion::parse(&v.0);
            if parsed.is_none() {
                sink.push(file, Problem::BadCorpusVersion(v.0.clone()));
            }
            parsed
        }
        (None, Origin::Corpus) => {
            sink.push(file, Problem::MissingKey("corpus_version".to_owned()));
            None
        }
        (None, Origin::User) => None,
    };
    let mut behaviours = Vec::new();
    for b in &raw.behaviours {
        let bid = id(sink, file, "id", &b.id, BehaviourId::new);
        let dim = id(sink, file, "dimension", &b.dimension, DimensionId::new);
        if let (Some(bid), Some(dimension)) = (bid, dim) {
            match (origin, bid.is_user()) {
                (Origin::Corpus, true) => {
                    sink.push(file, Problem::UserBehaviourOutsideUserRegistry(bid));
                    continue;
                }
                (Origin::User, false) => {
                    sink.push(file, Problem::NotUserBehaviour(bid));
                    continue;
                }
                _ => {}
            }
            behaviours.push(Behaviour { id: bid, dimension });
        }
    }
    behaviours.sort();
    for pair in behaviours.windows(2) {
        if pair[0].id == pair[1].id {
            sink.push(file, Problem::DuplicateId(pair[1].id.to_string()));
        }
    }
    for (key, list) in [
        ("windows_guard_macros", &raw.windows_guard_macros),
        ("dynamic_load_symbols", &raw.dynamic_load_symbols),
    ] {
        let mut seen = BTreeSet::new();
        for entry in list {
            if entry.trim().is_empty() {
                sink.push(file, Problem::BlankEntry(key));
            } else if !seen.insert(entry.as_str()) {
                sink.push(
                    file,
                    Problem::DuplicateEntry {
                        key,
                        value: entry.clone(),
                    },
                );
            }
        }
    }
    if sink.errors.len() != before {
        return None;
    }
    let manifest = Manifest {
        corpus_version: match origin {
            Origin::Corpus => corpus_version.clone()?,
            Origin::User => CorpusVersion::zero(),
        },
        behaviours,
        windows_guard_macros: raw.windows_guard_macros,
        dynamic_load_symbols: raw.dynamic_load_symbols,
    };
    Some((manifest, corpus_version))
}

fn rule_from(
    sink: &mut Sink,
    file: &RelPath,
    text: &str,
    registry: &Registry,
    ids: &Ids,
) -> Option<Rule> {
    let before = sink.errors.len();
    if let Ok(keys) = parse::<Keys>(text) {
        for key in TARGET_KEYS {
            if keys.contains_key(*key) {
                sink.push(file, Problem::NamesTarget(key));
            }
        }
        for key in TIER_KEYS {
            if keys.contains_key(*key) {
                sink.push(file, Problem::NamesTier(key));
            }
        }
    }
    if !check_version(sink, file, text, RULE_SCHEMA_VERSION) {
        return None;
    }
    let raw: RawRule = match parse(text) {
        Ok(r) => r,
        Err(problem) => {
            sink.push(file, problem);
            return None;
        }
    };
    let rule_id = id(sink, file, "id", &raw.id, RuleId::new);
    if rule_id.is_some() {
        ids.check(sink, file, &raw.id);
    }
    let behaviour = registry.check(sink, file, &raw.behaviour);
    if blank(&raw.title) {
        sink.push(file, Problem::NoTitle);
    }
    if blank(&raw.fix) {
        sink.push(file, Problem::NoFix);
    }

    let (found_ev, suspected_ev) = match raw.evidence {
        Some(e) => (e.found, e.suspected),
        None => (None, None),
    };
    let (found_m, suspected_m) = match raw.matches {
        Some(m) => (m.found, m.suspected),
        None => (None, None),
    };
    if found_m.is_none() && suspected_m.is_none() {
        sink.push(file, Problem::NoMatch);
    }
    let mut matches = Vec::new();
    let mut evidence_ids = BTreeSet::new();
    for (confidence, node, evidence) in [
        (Confidence::Found, found_m, found_ev),
        (Confidence::Suspected, suspected_m, suspected_ev),
    ] {
        let had_evidence = evidence.is_some();
        let evidence = evidence.and_then(|e| evidence_from(sink, file, confidence, e));
        if let Some(e) = &evidence
            && !evidence_ids.insert(e.id.clone())
        {
            sink.push(file, Problem::DuplicateId(e.id.to_string()));
        }
        let Some(node) = node else {
            if had_evidence {
                sink.push(file, Problem::EvidenceWithoutMatch(confidence));
            }
            continue;
        };
        if !had_evidence {
            sink.push(file, Problem::ConfidenceWithoutEvidence(confidence));
        }
        let matcher = matcher_from(sink, file, node, None);
        if let (Some(evidence), Some(matcher)) = (evidence, matcher) {
            matches.push(RuleMatch {
                confidence,
                evidence,
                matcher,
            });
        }
    }
    if sink.errors.len() != before {
        return None;
    }
    Some(Rule {
        id: rule_id?,
        file: file.clone(),
        behaviour: behaviour?,
        title: raw.title?.trim().to_owned(),
        fix: raw.fix?.trim().to_owned(),
        matches,
    })
}

fn evidence_from(
    sink: &mut Sink,
    file: &RelPath,
    confidence: Confidence,
    raw: RawEvidenceEntry,
) -> Option<EvidenceDef> {
    let eid = id(sink, file, "evidence id", &raw.id, EvidenceId::new);
    if blank(&raw.text) {
        sink.push(file, Problem::NoEvidenceText(confidence));
        return None;
    }
    Some(EvidenceDef {
        id: eid?,
        text: raw.text?.trim().to_owned(),
    })
}

fn kind_named(sink: &mut Sink, file: &RelPath, name: &str) -> Option<FactKind> {
    let kind = FactKind::from_name(name);
    if kind.is_none() {
        sink.push(file, Problem::UnknownFactKind(name.to_owned()));
    }
    kind
}

pub fn field_path(kind: FactKind, field: &str) -> Option<(FieldPath, FieldType)> {
    if let Some((common, _, ty)) = COMMON_TEST_FIELDS.iter().find(|(_, n, _)| *n == field) {
        return Some((FieldPath::Common(*common), *ty));
    }
    let (base, index) = match field.split_once('.') {
        Some((base, index)) => {
            let canonical = index == "0" || (!index.is_empty() && !index.starts_with('0'));
            if !canonical || !index.bytes().all(|c| c.is_ascii_digit()) {
                return None;
            }
            (base, Some(index.parse::<u32>().ok()?))
        }
        None => (field, None),
    };
    if COMMON_FIELDS.contains(&base) {
        return None;
    }
    let spec = kind.field(base)?;
    let ty = match (spec.ty, index) {
        (_, None) => spec.ty,
        (FieldType::TextList, Some(_)) => FieldType::Text,
        (_, Some(_)) => return None,
    };
    Some((
        FieldPath::Kind {
            name: spec.name,
            index,
        },
        ty,
    ))
}

fn literal_fits(literal: &Literal, ty: FieldType) -> bool {
    matches!(
        (literal, ty),
        (Literal::Text(_), FieldType::Text | FieldType::Path)
            | (Literal::Integer(_), FieldType::Integer)
            | (Literal::Bool(_), FieldType::Bool)
    )
}

fn text_value(sink: &mut Sink, file: &RelPath, key: &'static str, value: String) -> Option<String> {
    if value.trim().is_empty() {
        sink.push(file, Problem::BlankValue(key));
        return None;
    }
    Some(value)
}

fn compiled(
    sink: &mut Sink,
    file: &RelPath,
    key: &'static str,
    result: Result<Pattern, pattern::PatternError>,
) -> Option<Pattern> {
    match result {
        Ok(p) => Some(p),
        Err(e) => {
            sink.push(
                file,
                Problem::BadPattern {
                    key,
                    message: e.to_string(),
                },
            );
            None
        }
    }
}

fn field_test(
    sink: &mut Sink,
    file: &RelPath,
    raw: &mut RawNode,
    field: &str,
    ty: FieldType,
) -> Option<FieldTest> {
    let misfit = |sink: &mut Sink, test: &'static str| {
        sink.push(
            file,
            Problem::TestDoesNotFit {
                field: field.to_owned(),
                field_type: ty,
                test,
            },
        );
        None
    };
    if let Some(v) = raw.equals.take() {
        if !literal_fits(&v, ty) {
            return misfit(sink, "equals");
        }
        Some(FieldTest::Equals(v))
    } else if let Some(list) = raw.one_of.take() {
        if list.is_empty() {
            sink.push(file, Problem::EmptyList("one_of"));
            return None;
        }
        if !list.iter().all(|v| literal_fits(v, ty)) {
            return misfit(sink, "one_of");
        }
        Some(FieldTest::OneOf(list))
    } else if let Some(p) = raw.pattern.take() {
        let mode = match ty {
            FieldType::Path => PatternMode::Path,
            FieldType::Text => PatternMode::Text,
            _ => return misfit(sink, "pattern"),
        };
        let p = text_value(sink, file, "pattern", p)?;
        Some(FieldTest::Pattern(compiled(
            sink,
            file,
            "pattern",
            pattern::compile(&p, mode),
        )?))
    } else if let Some(p) = raw.path_under.take() {
        if ty != FieldType::Path {
            return misfit(sink, "path_under");
        }
        let p = text_value(sink, file, "path_under", p)?;
        Some(FieldTest::PathUnder(compiled(
            sink,
            file,
            "path_under",
            pattern::path_under(&p),
        )?))
    } else {
        let n = raw.number.take()?;
        let common = COMMON_TEST_FIELDS.iter().any(|(_, n, _)| *n == field);
        let fits = match ty {
            FieldType::Integer => true,
            FieldType::Text => !common,
            _ => false,
        };
        if !fits {
            return misfit(sink, "number");
        }
        Some(FieldTest::Number {
            op: n.op,
            value: n.value,
        })
    }
}

fn matcher_from(
    sink: &mut Sink,
    file: &RelPath,
    mut raw: RawNode,
    inherited: Option<FactKind>,
) -> Option<Matcher> {
    let before = sink.errors.len();
    let own = match &raw.kind {
        Some(name) => Some(kind_named(sink, file, name)?),
        None => None,
    };
    let kind = own.or(inherited);

    let field_tests = [
        raw.equals.is_some(),
        raw.one_of.is_some(),
        raw.pattern.is_some(),
        raw.path_under.is_some(),
        raw.number.is_some(),
    ]
    .iter()
    .filter(|b| **b)
    .count();
    let other_tests = [
        raw.guard_contains.is_some(),
        raw.co_occurs.is_some(),
        raw.all.is_some(),
        raw.any.is_some(),
        raw.not.is_some(),
    ]
    .iter()
    .filter(|b| **b)
    .count();
    if field_tests + other_tests > 1 {
        sink.push(file, Problem::SeveralTests);
        return None;
    }
    if raw.field.is_some() && field_tests == 0 {
        sink.push(file, Problem::FieldWithoutTest);
        return None;
    }
    if raw.field.is_none() && field_tests == 1 {
        sink.push(file, Problem::TestWithoutField);
        return None;
    }

    let children =
        |sink: &mut Sink, list: Vec<RawNode>, key: &'static str| -> Option<Vec<Matcher>> {
            if list.is_empty() {
                sink.push(file, Problem::EmptyList(key));
                return None;
            }
            let mut out = Vec::new();
            let mut ok = true;
            for child in list {
                match matcher_from(sink, file, child, kind) {
                    Some(m) => out.push(m),
                    None => ok = false,
                }
            }
            ok.then_some(out)
        };

    let test = if let Some(field) = raw.field.take() {
        let Some(k) = kind else {
            sink.push(file, Problem::NoKind);
            return None;
        };
        let Some((path, ty)) = field_path(k, &field) else {
            sink.push(file, Problem::UnknownField { kind: k, field });
            return None;
        };
        let test = field_test(sink, file, &mut raw, &field, ty)?;
        Test::Field { field: path, test }
    } else if let Some(g) = raw.guard_contains {
        let Some(k) = kind else {
            sink.push(file, Problem::NoKind);
            return None;
        };
        if !k.fields().iter().any(|f| f.ty == FieldType::Guards) {
            sink.push(
                file,
                Problem::TestDoesNotFit {
                    field: "guards".to_owned(),
                    field_type: FieldType::Guards,
                    test: "guard_contains",
                },
            );
            return None;
        }
        Test::GuardContains(text_value(sink, file, "guard_contains", g)?)
    } else if let Some(co) = raw.co_occurs {
        Test::CoOccurs {
            scope: co.scope,
            all: children(sink, co.all, "co_occurs.all")?,
        }
    } else if let Some(list) = raw.all {
        Test::All(children(sink, list, "all")?)
    } else if let Some(list) = raw.any {
        Test::Any(children(sink, list, "any")?)
    } else if let Some(not) = raw.not {
        Test::Not(Box::new(matcher_from(sink, file, *not, kind)?))
    } else if own.is_some() {
        Test::Exists
    } else {
        sink.push(file, Problem::NoTest);
        return None;
    };
    if sink.errors.len() != before {
        return None;
    }
    Some(Matcher { kind: own, test })
}

fn profile_from(
    sink: &mut Sink,
    file: &RelPath,
    text: &str,
    registry: &Registry,
    ids: &Ids,
) -> Option<Profile> {
    if !check_version(sink, file, text, PROFILE_FORMAT_VERSION) {
        return None;
    }
    let raw: RawProfile = match parse(text) {
        Ok(r) => r,
        Err(problem) => {
            sink.push(file, problem);
            return None;
        }
    };
    let before = sink.errors.len();
    let target = id(sink, file, "id", &raw.id, TargetId::new);
    if target.is_some() {
        ids.check(sink, file, &raw.id);
    }
    if raw.version.trim().is_empty() {
        sink.push(file, Problem::BlankValue("version"));
    }
    let mut allowed = Vec::new();
    for a in &raw.allowed {
        if let Some(b) = registry.check(sink, file, a) {
            allowed.push(b);
        }
    }
    let mut seen_allowed = BTreeSet::new();
    for b in &allowed {
        if !seen_allowed.insert(b.clone()) {
            sink.push(file, Problem::DuplicateId(b.to_string()));
        }
    }
    let mut controls = Vec::new();
    let mut control_ids = BTreeSet::new();
    for c in raw.controls {
        let Some(cid) = id(sink, file, "control id", &c.id, ControlId::new) else {
            continue;
        };
        if !control_ids.insert(cid.clone()) {
            sink.push(file, Problem::DuplicateId(cid.to_string()));
        }
        let mark = c
            .not_detectable
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty());
        match (c.behaviours.is_empty(), mark.is_some()) {
            (true, false) => {
                sink.push(file, Problem::ControlWithoutBehaviours(cid));
                continue;
            }
            (false, true) => {
                sink.push(file, Problem::ControlTieredAndNotDetectable(cid));
                continue;
            }
            _ => {}
        }
        let mut behaviours = Vec::new();
        let mut seen = BTreeSet::new();
        for bt in c.behaviours {
            if let Some(b) = registry.check(sink, file, &bt.behaviour) {
                if !seen.insert(b.clone()) {
                    sink.push(file, Problem::DuplicateId(b.to_string()));
                }
                if seen_allowed.contains(&b) {
                    sink.push(file, Problem::AllowedAndTiered(b.clone()));
                }
                behaviours.push(BehaviourTier {
                    behaviour: b,
                    tier: bt.tier,
                });
            }
        }
        controls.push(Control {
            id: cid,
            behaviours: SortedVec::from(behaviours),
            not_detectable: mark,
        });
    }
    if sink.errors.len() != before {
        return None;
    }
    Some(Profile {
        id: target?,
        version: raw.version.trim().to_owned(),
        controls: SortedVec::from(controls),
        allowed: SortedVec::from(allowed),
    })
}
