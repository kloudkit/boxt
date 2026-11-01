use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};

use boxt_core::SortedVec;
use boxt_core::Text;
use boxt_core::fact::{Fact, FactKind, FieldValue, FileKind};
use boxt_core::location::{Frame, FrameKind, Location, Scope, ScriptName, normalize_value_path};
use boxt_core::verdict::{Confidence, Evidence, Finding, Observation};
use serde::Serialize;

use crate::format::{CoScope, Literal, NumberOp};
use crate::validate::{CommonField, Corpus, FieldPath, FieldTest, Matcher, Rule, RuleMatch, Test};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Matched {
    pub findings: SortedVec<Finding>,
    pub observations: SortedVec<Observation>,
}

pub fn evaluate(corpus: &Corpus, facts: &[Fact]) -> Matched {
    let mut eval = Eval {
        facts,
        groups: Groups::new(facts),
        co_occurs: BTreeMap::new(),
    };
    let mut findings = Vec::new();
    for rule in &corpus.rules {
        eval.rule(rule, &mut findings);
    }
    let mut observations: Vec<Observation> = facts
        .iter()
        .filter_map(|fact| match fact {
            Fact::Import {
                file_kind: FileKind::Binary,
                location,
                symbol,
                ..
            } if corpus
                .manifest
                .dynamic_load_symbols
                .iter()
                .any(|s| s.as_bytes() == symbol.as_bytes()) =>
            {
                Some(Observation::DynamicLoad {
                    location: location.clone(),
                    symbol: symbol.clone(),
                })
            }
            _ => None,
        })
        .collect();
    observations.sort();
    observations.dedup();
    Matched {
        findings: SortedVec::from(findings),
        observations: SortedVec::from(observations),
    }
}

type FileIdentity<'a> = (u32, &'a [Frame], Option<&'a [u8]>, Option<ScriptName>);

fn file_identity(location: &Location) -> FileIdentity<'_> {
    let script = match location.scope {
        Scope::Script(name) => Some(name),
        _ => None,
    };
    (
        location.input.index,
        &location.frames,
        location.path.as_ref().map(|p| p.as_bytes()),
        script,
    )
}

fn package_frames(location: &Location) -> Option<&[Frame]> {
    let innermost = location
        .frames
        .iter()
        .rposition(|f| matches!(f.kind, FrameKind::Rpm | FrameKind::Deb))?;
    Some(&location.frames[..=innermost])
}

#[derive(Default)]
struct ScopeGroups {
    of: Vec<Option<usize>>,
    members: Vec<Vec<usize>>,
}

impl ScopeGroups {
    fn build<'a, K: Ord>(facts: &'a [Fact], key: impl Fn(&'a Location) -> Option<K>) -> Self {
        let mut ids: BTreeMap<K, usize> = BTreeMap::new();
        let mut groups = ScopeGroups::default();
        for (i, fact) in facts.iter().enumerate() {
            let id = key(fact.location()).map(|k| {
                let next = ids.len();
                let id = *ids.entry(k).or_insert(next);
                if id == groups.members.len() {
                    groups.members.push(Vec::new());
                }
                groups.members[id].push(i);
                id
            });
            groups.of.push(id);
        }
        groups
    }
}

struct Groups {
    function: ScopeGroups,
    file: ScopeGroups,
    package: ScopeGroups,
}

impl Groups {
    fn new(facts: &[Fact]) -> Self {
        Groups {
            function: ScopeGroups::build(facts, |l| match &l.scope {
                Scope::Function {
                    qualified_name,
                    signature,
                } => Some((
                    file_identity(l),
                    qualified_name.as_bytes(),
                    signature.as_bytes(),
                )),
                _ => None,
            }),
            file: ScopeGroups::build(facts, |l| Some(file_identity(l))),
            package: ScopeGroups::build(facts, |l| {
                package_frames(l).map(|frames| (l.input.index, frames))
            }),
        }
    }

    fn get(&self, scope: CoScope) -> &ScopeGroups {
        match scope {
            CoScope::Function => &self.function,
            CoScope::File => &self.file,
            CoScope::Package => &self.package,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum HitKey {
    Fact(usize),
    Group(CoScope, usize),
}

struct Eval<'a> {
    facts: &'a [Fact],
    groups: Groups,
    co_occurs: BTreeMap<(usize, usize), bool>,
}

impl Eval<'_> {
    fn rule(&mut self, rule: &Rule, out: &mut Vec<Finding>) {
        let mut found_keys = BTreeSet::new();
        let mut found_anchors = BTreeSet::new();
        let mut found_covered = BTreeSet::new();
        for confidence in [Confidence::Found, Confidence::Suspected] {
            for m in rule.matches.iter().filter(|m| m.confidence == confidence) {
                for (key, at) in self.hits(&m.matcher) {
                    let members = match key {
                        HitKey::Fact(i) => vec![i],
                        HitKey::Group(scope, group) => {
                            self.groups.get(scope).members[group].clone()
                        }
                    };
                    if confidence == Confidence::Found {
                        found_keys.insert(key);
                        found_anchors.insert(at);
                        found_covered.extend(members.iter().copied());
                    } else {
                        let suppressed = found_keys.contains(&key)
                            || match key {
                                HitKey::Fact(i) => found_covered.contains(&i),
                                HitKey::Group(..) => {
                                    members.iter().any(|j| found_anchors.contains(j))
                                }
                            };
                        if suppressed {
                            continue;
                        }
                    }
                    out.push(finding(rule, m, &self.facts[at]));
                }
            }
        }
    }

    fn hits(&mut self, matcher: &Matcher) -> Vec<(HitKey, usize)> {
        let Test::CoOccurs { scope, all } = &matcher.test else {
            return (0..self.facts.len())
                .filter(|&i| self.holds(matcher, i))
                .map(|i| (HitKey::Fact(i), i))
                .collect();
        };
        let mut out = Vec::new();
        for group in 0..self.groups.get(*scope).members.len() {
            let members = self.groups.get(*scope).members[group].clone();
            if !members
                .iter()
                .any(|&i| kind_fits(matcher.kind, &self.facts[i]))
            {
                continue;
            }
            let mut first: Option<usize> = None;
            let mut every = true;
            for sub in all {
                let witness = members.iter().copied().find(|&j| self.holds(sub, j));
                match witness {
                    Some(j) => first = Some(first.map_or(j, |f| f.min(j))),
                    None => {
                        every = false;
                        break;
                    }
                }
            }
            if let (true, Some(at)) = (every, first) {
                out.push((HitKey::Group(*scope, group), at));
            }
        }
        out
    }

    fn holds(&mut self, matcher: &Matcher, i: usize) -> bool {
        let fact = &self.facts[i];
        if !kind_fits(matcher.kind, fact) {
            return false;
        }
        match &matcher.test {
            Test::Exists => true,
            Test::Field { field, test } => resolve(fact, field).is_some_and(|v| v.passes(test)),
            Test::GuardContains(name) => fact.guards().iter().any(|chain| {
                chain.frames.iter().any(|frame| {
                    frame
                        .identifiers
                        .iter()
                        .any(|id| id.as_bytes() == name.as_bytes())
                })
            }),
            Test::All(list) => list.iter().all(|m| self.holds(m, i)),
            Test::Any(list) => list.iter().any(|m| self.holds(m, i)),
            Test::Not(inner) => !self.holds(inner, i),
            Test::CoOccurs { scope, all } => {
                let Some(group) = self.groups.get(*scope).of[i] else {
                    return false;
                };
                let key = (std::ptr::from_ref(matcher) as usize, group);
                if let Some(&known) = self.co_occurs.get(&key) {
                    return known;
                }
                let members = self.groups.get(*scope).members[group].clone();
                let result = all
                    .iter()
                    .all(|sub| members.iter().any(|&j| self.holds(sub, j)));
                self.co_occurs.insert(key, result);
                result
            }
        }
    }
}

fn kind_fits(kind: Option<FactKind>, fact: &Fact) -> bool {
    kind.is_none_or(|k| fact.kind() == k)
}

fn finding(rule: &Rule, m: &RuleMatch, at: &Fact) -> Finding {
    Finding {
        location: at.location().clone(),
        rule: rule.id.clone(),
        behaviour: rule.behaviour.clone(),
        confidence: m.confidence,
        evidence: Evidence {
            definition: m.evidence.id.clone(),
        },
        guards: at.guards().to_vec(),
        fix: rule.fix.clone(),
    }
}

enum Value<'a> {
    Text(Cow<'a, [u8]>),
    Path(Text),
    Integer(u64),
    Bool(bool),
}

fn scope_text(scope: &Scope) -> Option<Cow<'_, [u8]>> {
    match scope {
        Scope::File => None,
        Scope::Function { qualified_name, .. } => Some(Cow::Borrowed(qualified_name.as_bytes())),
        Scope::Symbol(s) | Scope::ConfigKey(s) => Some(Cow::Borrowed(s.as_bytes())),
        Scope::UnitSection { section, key } => {
            let mut out = section.as_bytes().to_vec();
            out.push(b'.');
            out.extend_from_slice(key.as_bytes());
            Some(Cow::Owned(out))
        }
        Scope::Script(name) => Some(Cow::Borrowed(name.name().as_bytes())),
    }
}

fn resolve<'a>(fact: &'a Fact, path: &FieldPath) -> Option<Value<'a>> {
    match path {
        FieldPath::Common(CommonField::FileKind) => Some(Value::Text(Cow::Borrowed(
            fact.file_kind().name().as_bytes(),
        ))),
        FieldPath::Common(CommonField::LocationPath) => {
            let path = fact.location().path.as_ref()?;
            Some(Value::Path(
                normalize_value_path(path.as_text()).as_text().clone(),
            ))
        }
        FieldPath::Common(CommonField::LocationLine) => {
            fact.location().line.map(|l| Value::Integer(u64::from(l)))
        }
        FieldPath::Common(CommonField::LocationScope) => {
            scope_text(&fact.location().scope).map(Value::Text)
        }
        FieldPath::Kind { name, index } => match (fact.field(name)?, index) {
            (FieldValue::TextList(list), Some(i)) => {
                let item = list.get(usize::try_from(*i).ok()?)?;
                Some(Value::Text(Cow::Borrowed(item.as_bytes())))
            }
            (FieldValue::Text(Cow::Borrowed(t)), None) => {
                Some(Value::Text(Cow::Borrowed(t.as_bytes())))
            }
            (FieldValue::Text(Cow::Owned(t)), None) => {
                Some(Value::Text(Cow::Owned(t.into_bytes())))
            }
            (FieldValue::Path(p), None) => Some(Value::Path(
                normalize_value_path(p.as_text()).as_text().clone(),
            )),
            (FieldValue::Integer(n), None) => Some(Value::Integer(n)),
            (FieldValue::Bool(b), None) => Some(Value::Bool(b)),
            _ => None,
        },
    }
}

impl Value<'_> {
    fn bytes(&self) -> Option<&[u8]> {
        match self {
            Value::Text(t) => Some(t),
            Value::Path(p) => Some(p.as_bytes()),
            Value::Integer(_) | Value::Bool(_) => None,
        }
    }

    fn equals(&self, literal: &Literal) -> bool {
        match (self, literal) {
            (Value::Text(v), Literal::Text(s)) => v.as_ref() == s.as_bytes(),
            (Value::Path(v), Literal::Text(s)) => {
                v.as_bytes() == normalize_value_path(&Text::from(s.as_str())).as_bytes()
            }
            (Value::Integer(v), Literal::Integer(n)) => i128::from(*v) == i128::from(*n),
            (Value::Bool(v), Literal::Bool(b)) => v == b,
            _ => false,
        }
    }

    fn passes(&self, test: &FieldTest) -> bool {
        match test {
            FieldTest::Equals(literal) => self.equals(literal),
            FieldTest::OneOf(list) => list.iter().any(|l| self.equals(l)),
            FieldTest::Pattern(p) | FieldTest::PathUnder(p) => {
                self.bytes().is_some_and(|b| p.is_match(b))
            }
            FieldTest::Number { op, value } => {
                let n = match self {
                    Value::Integer(n) => Some(i128::from(*n)),
                    Value::Text(t) => c_integer(t),
                    Value::Path(_) | Value::Bool(_) => None,
                };
                n.is_some_and(|n| compare(*op, n, i128::from(*value)))
            }
        }
    }
}

fn compare(op: NumberOp, left: i128, right: i128) -> bool {
    match op {
        NumberOp::Lt => left < right,
        NumberOp::Le => left <= right,
        NumberOp::Eq => left == right,
        NumberOp::Ge => left >= right,
        NumberOp::Gt => left > right,
    }
}

pub(crate) fn c_integer(bytes: &[u8]) -> Option<i128> {
    let s = std::str::from_utf8(bytes).ok()?;
    let (negative, s) = match s.as_bytes().first() {
        Some(b'-') => (true, &s[1..]),
        Some(b'+') => (false, &s[1..]),
        _ => (false, s),
    };
    let s = s.trim_end_matches(['u', 'U', 'l', 'L']);
    let (radix, digits) = if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        (16, hex)
    } else if s.len() > 1 && s.starts_with('0') {
        (8, &s[1..])
    } else {
        (10, s)
    };
    if digits.is_empty() || !digits.chars().all(|c| c.is_digit(radix)) {
        return None;
    }
    let magnitude = i128::try_from(u128::from_str_radix(digits, radix).ok()?).ok()?;
    Some(if negative { -magnitude } else { magnitude })
}

#[cfg(test)]
mod tests {
    use super::c_integer;

    #[test]
    fn c_integer_literals_parse_in_every_base() {
        assert_eq!(c_integer(b"0"), Some(0));
        assert_eq!(c_integer(b"80"), Some(80));
        assert_eq!(c_integer(b"0x50"), Some(80));
        assert_eq!(c_integer(b"0X1f"), Some(31));
        assert_eq!(c_integer(b"0120"), Some(80));
        assert_eq!(c_integer(b"80u"), Some(80));
        assert_eq!(c_integer(b"80UL"), Some(80));
        assert_eq!(c_integer(b"0x50ull"), Some(80));
        assert_eq!(c_integer(b"-1"), Some(-1));
        assert_eq!(c_integer(b"+7"), Some(7));
        assert_eq!(c_integer(b"PORT"), None);
        assert_eq!(c_integer(b""), None);
        assert_eq!(c_integer(b"0x"), None);
        assert_eq!(c_integer(b"08"), None);
        assert_eq!(c_integer(b"0x+5"), None);
        assert_eq!(c_integer(b"1.5"), None);
        assert_eq!(c_integer(b"u"), None);
        assert_eq!(c_integer(b"\"80\""), None);
        assert_eq!(
            c_integer(b"99999999999999999999999999999999999999999"),
            None
        );
    }
}
