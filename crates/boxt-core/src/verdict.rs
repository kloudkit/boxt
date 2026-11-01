use std::fmt;

use serde::{Deserialize, Serialize};

use crate::fact::{FileKind, GuardChain};
use crate::location::Location;
use crate::outcome::{Sm3, VerdictCounts};
use crate::sorted::SortedVec;
use crate::text::Text;
use crate::version::Versions;

type StdResult<T, E> = std::result::Result<T, E>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tier {
    Blocked,
    NeedsWindows,
    NeedsCodeChange,
    ReadyWithConfig,
    Ready,
}

impl Tier {
    pub const WORST_TO_BEST: [Tier; 5] = [
        Tier::Blocked,
        Tier::NeedsWindows,
        Tier::NeedsCodeChange,
        Tier::ReadyWithConfig,
        Tier::Ready,
    ];

    pub fn worst(self, other: Tier) -> Tier {
        self.min(other)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Confidence {
    Found,
    Suspected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdError {
    Empty,
    EmptySegment,
    BadChar,
    BadHyphen,
    TooFewSegments,
}

impl fmt::Display for IdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            IdError::Empty => "id is empty",
            IdError::EmptySegment => "id has an empty segment",
            IdError::BadChar => "id holds a character other than a-z, 0-9, '-' or '/'",
            IdError::BadHyphen => "id segment starts or ends with '-' or has '--'",
            IdError::TooFewSegments => "id needs the form <dimension>/<name>",
        })
    }
}

impl std::error::Error for IdError {}

pub const ID_SEGMENT_PATTERN: &str = "[a-z0-9]+(-[a-z0-9]+)*";

pub fn validate_id(id: &str, min_segments: usize) -> StdResult<(), IdError> {
    if id.is_empty() {
        return Err(IdError::Empty);
    }
    let mut segments = 0;
    for segment in id.split('/') {
        segments += 1;
        if segment.is_empty() {
            return Err(IdError::EmptySegment);
        }
        if !segment
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
        {
            return Err(IdError::BadChar);
        }
        if segment.starts_with('-') || segment.ends_with('-') || segment.contains("--") {
            return Err(IdError::BadHyphen);
        }
    }
    if segments < min_segments {
        return Err(IdError::TooFewSegments);
    }
    Ok(())
}

macro_rules! id_newtype {
    ($name:ident, $min:expr) => {
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(try_from = "String", into = "String")]
        pub struct $name(String);

        impl $name {
            pub fn new(id: impl Into<String>) -> StdResult<Self, IdError> {
                let id = id.into();
                validate_id(&id, $min)?;
                Ok($name(id))
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }

            pub fn is_user(&self) -> bool {
                self.0.starts_with("user/")
            }
        }

        impl TryFrom<String> for $name {
            type Error = IdError;

            fn try_from(id: String) -> StdResult<Self, IdError> {
                $name::new(id)
            }
        }

        impl TryFrom<&str> for $name {
            type Error = IdError;

            fn try_from(id: &str) -> StdResult<Self, IdError> {
                $name::new(id)
            }
        }

        impl From<$name> for String {
            fn from(id: $name) -> String {
                id.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}

id_newtype!(BehaviourId, 1);
id_newtype!(DimensionId, 1);
id_newtype!(TargetId, 1);
id_newtype!(ControlId, 1);
id_newtype!(RuleId, 2);
id_newtype!(EvidenceId, 1);

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct BehaviourTier {
    pub behaviour: BehaviourId,
    pub tier: Tier,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Control {
    pub id: ControlId,
    pub behaviours: SortedVec<BehaviourTier>,
    pub not_detectable: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Profile {
    pub id: TargetId,
    pub version: String,
    pub controls: SortedVec<Control>,
    pub allowed: SortedVec<BehaviourId>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
struct RawRuleCoverage {
    rule: RuleId,
    file_kinds: SortedVec<SortedVec<FileKind>>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "RawRuleCoverage", into = "RawRuleCoverage")]
pub struct RuleCoverage {
    rule: RuleId,
    file_kinds: SortedVec<SortedVec<FileKind>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoverageError {
    EmptyAlternative,
    DuplicateAlternative,
    DuplicateFileKind,
}

impl fmt::Display for CoverageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            CoverageError::EmptyAlternative => "rule coverage has an empty alternative",
            CoverageError::DuplicateAlternative => "rule coverage repeats an alternative",
            CoverageError::DuplicateFileKind => "a coverage alternative repeats a file kind",
        })
    }
}

impl std::error::Error for CoverageError {}

impl TryFrom<RawRuleCoverage> for RuleCoverage {
    type Error = CoverageError;

    fn try_from(raw: RawRuleCoverage) -> StdResult<Self, CoverageError> {
        RuleCoverage::new(raw.rule, raw.file_kinds)
    }
}

impl From<RuleCoverage> for RawRuleCoverage {
    fn from(c: RuleCoverage) -> Self {
        RawRuleCoverage {
            rule: c.rule,
            file_kinds: c.file_kinds,
        }
    }
}

impl RuleCoverage {
    pub fn new(
        rule: RuleId,
        file_kinds: SortedVec<SortedVec<FileKind>>,
    ) -> StdResult<Self, CoverageError> {
        for alternative in file_kinds.iter() {
            if alternative.is_empty() {
                return Err(CoverageError::EmptyAlternative);
            }
            if alternative.windows(2).any(|w| w[0] == w[1]) {
                return Err(CoverageError::DuplicateFileKind);
            }
        }
        if file_kinds.windows(2).any(|w| w[0] == w[1]) {
            return Err(CoverageError::DuplicateAlternative);
        }
        Ok(RuleCoverage { rule, file_kinds })
    }

    pub fn rule(&self) -> &RuleId {
        &self.rule
    }

    pub fn file_kinds(&self) -> &SortedVec<SortedVec<FileKind>> {
        &self.file_kinds
    }

    pub fn can_fire(&self, read: &[FileKind]) -> bool {
        self.file_kinds
            .iter()
            .any(|all| all.iter().all(|kind| read.contains(kind)))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ControlCoverage {
    pub target: TargetId,
    pub control: ControlId,
    pub rules: SortedVec<RuleCoverage>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct BehaviourCoverage {
    pub behaviour: BehaviourId,
    pub rules: SortedVec<RuleCoverage>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Coverage {
    pub controls: SortedVec<ControlCoverage>,
    pub behaviours: SortedVec<BehaviourCoverage>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Evidence {
    pub definition: EvidenceId,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Finding {
    pub location: Location,
    pub rule: RuleId,
    pub behaviour: BehaviourId,
    pub confidence: Confidence,
    pub evidence: Evidence,
    pub guards: Vec<GuardChain>,
    pub fix: String,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Observation {
    WindowsBranch {
        location: Location,
        guards: Vec<GuardChain>,
    },
    DynamicLoad {
        location: Location,
        symbol: Text,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct TierRange {
    pub found: Tier,
    pub with_suspected: Tier,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct TargetVerdict {
    pub target: TargetId,
    pub range: TierRange,
    pub found: u64,
    pub suspected: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Result {
    pub versions: Versions,
    pub counts: VerdictCounts,
    pub sm3: Sm3,
    pub macro_tables_cut: SortedVec<u32>,
    pub host_dependent: bool,
    pub targets: SortedVec<TargetVerdict>,
    pub findings: SortedVec<Finding>,
    pub observations: SortedVec<Observation>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiers_run_worst_to_best() {
        let mut sorted = Tier::WORST_TO_BEST;
        sorted.sort();
        assert_eq!(sorted, Tier::WORST_TO_BEST);
        assert_eq!(
            Tier::Ready.worst(Tier::NeedsCodeChange),
            Tier::NeedsCodeChange
        );
        assert_eq!(Tier::Blocked.worst(Tier::Ready), Tier::Blocked);
    }

    #[test]
    fn ids_are_kebab_case() {
        assert!(BehaviourId::new("needs-root").is_ok());
        assert!(TargetId::new("openshift-restricted-v2").is_ok());
        assert!(RuleId::new("identity/needs-root").is_ok());
        assert!(RuleId::new("user/site-rule").unwrap().is_user());
        assert_eq!(RuleId::new("needs-root"), Err(IdError::TooFewSegments));
        assert_eq!(BehaviourId::new("Needs-Root"), Err(IdError::BadChar));
        assert_eq!(BehaviourId::new("needs_root"), Err(IdError::BadChar));
        assert_eq!(BehaviourId::new("-x"), Err(IdError::BadHyphen));
        assert_eq!(BehaviourId::new("a--b"), Err(IdError::BadHyphen));
        assert_eq!(BehaviourId::new("a//b"), Err(IdError::EmptySegment));
        assert_eq!(BehaviourId::new(""), Err(IdError::Empty));
        assert!(serde_json::from_str::<TargetId>(r#""Docker""#).is_err());
    }
}
