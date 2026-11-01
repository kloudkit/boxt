use std::fmt;
use std::num::NonZeroU32;

use serde::{Deserialize, Serialize};

use crate::location::{Frame, RelPath, Scope};
use crate::text::Text;
use crate::verdict::{BehaviourId, Finding};

pub const FINDING_ID_HEX_DIGITS: usize = 12;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Identity {
    pub behaviour: BehaviourId,
    pub frames: Vec<Frame>,
    pub path: Option<RelPath>,
    pub scope: Scope,
}

impl Identity {
    pub fn of(finding: &Finding) -> Identity {
        Identity {
            behaviour: finding.behaviour.clone(),
            frames: finding.location.frames.clone(),
            path: finding.location.path.clone(),
            scope: finding.location.scope.clone(),
        }
    }

    pub fn canonical_text(&self) -> Text {
        let mut out = Vec::new();
        field(&mut out, b"behaviour", self.behaviour.as_str().as_bytes());
        for frame in &self.frames {
            let mut value = frame.kind.name().as_bytes().to_vec();
            value.push(b':');
            value.extend_from_slice(frame.name.as_bytes());
            field(&mut out, b"frame", &value);
        }
        field(
            &mut out,
            b"path",
            self.path.as_ref().map_or(&[][..], RelPath::as_bytes),
        );
        field(&mut out, b"scope", self.scope.kind().name().as_bytes());
        match &self.scope {
            Scope::File => {}
            Scope::Function {
                qualified_name,
                signature,
            } => {
                field(&mut out, b"name", qualified_name.as_bytes());
                field(&mut out, b"signature", signature.as_bytes());
            }
            Scope::Symbol(text) | Scope::ConfigKey(text) => {
                field(&mut out, b"text", text.as_bytes());
            }
            Scope::UnitSection { section, key } => {
                field(&mut out, b"section", section.as_bytes());
                field(&mut out, b"key", key.as_bytes());
            }
            Scope::Script(name) => field(&mut out, b"text", name.name().as_bytes()),
        }
        out.pop();
        Text::from_bytes(out)
    }
}

fn field(out: &mut Vec<u8>, label: &[u8], value: &[u8]) {
    out.extend_from_slice(label);
    out.push(b' ');
    for &b in value {
        match b {
            b'\\' => out.extend_from_slice(b"\\\\"),
            b'\n' => out.extend_from_slice(b"\\n"),
            _ => out.push(b),
        }
    }
    out.push(b'\n');
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct FindingId(String);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FindingIdError {
    NoOrdinal,
    BadDigest,
    BadOrdinal,
}

impl fmt::Display for FindingIdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            FindingIdError::NoOrdinal => "a finding id is <12 hex digits>-<n>",
            FindingIdError::BadDigest => "a finding id starts with 12 lowercase hex digits",
            FindingIdError::BadOrdinal => "a finding id ends in a decimal number from 1",
        })
    }
}

impl std::error::Error for FindingIdError {}

impl FindingId {
    pub fn new(digest: &[u8; 32], ordinal: NonZeroU32) -> FindingId {
        let mut id = String::with_capacity(FINDING_ID_HEX_DIGITS + 11);
        for b in &digest[..FINDING_ID_HEX_DIGITS / 2] {
            id.push_str(&format!("{b:02x}"));
        }
        id.push('-');
        id.push_str(&ordinal.to_string());
        FindingId(id)
    }

    pub fn parse(id: &str) -> Result<FindingId, FindingIdError> {
        let (digest, ordinal) = id.split_once('-').ok_or(FindingIdError::NoOrdinal)?;
        if digest.len() != FINDING_ID_HEX_DIGITS
            || !digest
                .bytes()
                .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
        {
            return Err(FindingIdError::BadDigest);
        }
        let canonical = !ordinal.is_empty()
            && !ordinal.starts_with('0')
            && ordinal.bytes().all(|c| c.is_ascii_digit());
        if !canonical || ordinal.parse::<NonZeroU32>().is_err() {
            return Err(FindingIdError::BadOrdinal);
        }
        Ok(FindingId(id.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn digest(&self) -> &str {
        &self.0[..FINDING_ID_HEX_DIGITS]
    }

    pub fn ordinal(&self) -> NonZeroU32 {
        self.0[FINDING_ID_HEX_DIGITS + 1..]
            .parse()
            .unwrap_or(NonZeroU32::MIN)
    }
}

impl TryFrom<String> for FindingId {
    type Error = FindingIdError;

    fn try_from(id: String) -> Result<Self, FindingIdError> {
        FindingId::parse(&id)
    }
}

impl From<FindingId> for String {
    fn from(id: FindingId) -> String {
        id.0
    }
}

impl fmt::Display for FindingId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::location::{FrameKind, InputRef, Location, ScriptName};
    use crate::verdict::{Confidence, Evidence, EvidenceId, RuleId};

    fn finding(index: u32, given: &str, line: Option<u32>, scope: Scope) -> Finding {
        Finding {
            location: Location {
                input: InputRef {
                    index,
                    given: Text::from(given),
                },
                frames: vec![Frame {
                    kind: FrameKind::Rpm,
                    name: Text::from("app"),
                }],
                path: Some(RelPath::try_from("usr/bin/a\\b").unwrap()),
                line,
                scope,
            },
            rule: RuleId::new("identity/needs-root").unwrap(),
            behaviour: BehaviourId::new("needs-root").unwrap(),
            confidence: Confidence::Found,
            evidence: Evidence {
                definition: EvidenceId::new("call").unwrap(),
            },
            guards: Vec::new(),
            fix: String::new(),
        }
    }

    #[test]
    fn identity_ignores_input_and_line() {
        let scope = Scope::Function {
            qualified_name: Text::from("ns::f"),
            signature: Text::from("(int)"),
        };
        let a = Identity::of(&finding(0, "/a", Some(3), scope.clone()));
        let b = Identity::of(&finding(4, "../b", Some(90), scope));
        assert_eq!(a, b);
        assert_eq!(
            a.canonical_text().as_bytes(),
            b"behaviour needs-root\nframe rpm:app\npath usr/bin/a\\\\b\nscope function\nname ns::f\nsignature (int)"
        );
    }

    #[test]
    fn canonical_text_escapes_and_separates_fields() {
        let mut no_path = Identity::of(&finding(
            0,
            "/a",
            None,
            Scope::Script(ScriptName::PostInstall),
        ));
        no_path.path = None;
        no_path.frames[0].name = Text::from("a\nb");
        assert_eq!(
            no_path.canonical_text().as_bytes(),
            b"behaviour needs-root\nframe rpm:a\\nb\npath \nscope script\ntext post_install"
        );
        let symbol = Identity::of(&finding(0, "/a", None, Scope::Symbol(Text::from("dlopen"))));
        let key = Identity::of(&finding(
            0,
            "/a",
            None,
            Scope::ConfigKey(Text::from("dlopen")),
        ));
        assert_ne!(symbol.canonical_text(), key.canonical_text());
    }

    #[test]
    fn every_scope_kind_has_pinned_canonical_text() {
        let text = |scope: Scope| {
            let mut identity = Identity::of(&finding(0, "/a", None, scope));
            identity.frames.clear();
            identity.canonical_text().as_bytes().to_vec()
        };
        assert_eq!(
            text(Scope::File),
            b"behaviour needs-root\npath usr/bin/a\\\\b\nscope file"
        );
        assert_eq!(
            text(Scope::Symbol(Text::from("dlopen"))),
            b"behaviour needs-root\npath usr/bin/a\\\\b\nscope symbol\ntext dlopen"
        );
        assert_eq!(
            text(Scope::ConfigKey(Text::from("server.port"))),
            b"behaviour needs-root\npath usr/bin/a\\\\b\nscope config_key\ntext server.port"
        );
        assert_eq!(
            text(Scope::UnitSection {
                section: Text::from("Service"),
                key: Text::from("User"),
            }),
            b"behaviour needs-root\npath usr/bin/a\\\\b\nscope unit_section\nsection Service\nkey User"
        );
    }

    #[test]
    fn finding_ids_are_twelve_hex_digits_and_an_ordinal() {
        let id = FindingId::new(&[0xab; 32], NonZeroU32::new(2).unwrap());
        assert_eq!(id.as_str(), "abababababab-2");
        assert_eq!(id.digest(), "abababababab");
        assert_eq!(id.ordinal().get(), 2);
        assert_eq!(FindingId::parse("abababababab-2"), Ok(id.clone()));
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, "\"abababababab-2\"");
        assert_eq!(serde_json::from_str::<FindingId>(&json).unwrap(), id);
        for bad in [
            "abababababab",
            "ABABABABABAB-1",
            "ababababab-1",
            "abababababab-0",
            "abababababab-01",
            "abababababab-",
            "abababababab-x",
        ] {
            assert!(FindingId::parse(bad).is_err(), "{bad}");
        }
    }
}
