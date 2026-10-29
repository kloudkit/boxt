use std::fmt;

use serde::{Deserialize, Serialize};

use crate::fact::FileKind;
use crate::limits::LimitName;
use crate::settings::{Encoding, PathPattern};
use crate::text::Text;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Outcome {
    pub file: FileOutcome,
    pub file_kind: Option<FileKind>,
    pub host_dependent: bool,
}

impl Outcome {
    pub fn new(file: FileOutcome, file_kind: Option<FileKind>) -> Self {
        Outcome {
            file,
            file_kind,
            host_dependent: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileOutcome {
    Read,
    NotChecked {
        reason: NotCheckedReason,
        details: Details,
    },
    Aggregate {
        reason: NotCheckedReason,
        count: Option<u64>,
        at_least: u64,
    },
    OutOfScope {
        cause: OutOfScopeCause,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotCheckedReason {
    NoReader,
    EncodingUndeclared,
    InvalidForDeclaredEncoding,
    Unparseable,
    UnsupportedInterpreter,
    NoDecoder,
    LimitExceeded(LimitName),
    Fault(FaultKind),
    RepeatedFault,
    SpecialFile(SpecialKind),
    Symlink(LinkTarget),
    UnsafeEntryName,
    NotInCompileDatabase,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FaultKind {
    Crash,
    Panic,
    BadFrame,
    Exited,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpecialKind {
    Fifo,
    Socket,
    BlockDevice,
    CharDevice,
    ReparsePoint,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LinkTarget {
    File,
    Directory,
    Special(SpecialKind),
    Missing,
    Loop,
    Unresolved,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Details {
    None,
    Measured { value: u64, limit: u64 },
    Encoding { declared: Encoding },
    Interpreter { interpreter: Text },
    EntryName { name: Text },
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutOfScopeCause {
    Pattern(PathPattern),
    OwnFile(OwnFileRole),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OwnFileRole {
    ProjectFile,
    Output,
    CompileDatabase,
    WaiverFile,
    UserRules,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Count {
    Exact(u64),
    AtLeast(u64),
}

impl Count {
    pub fn value(self) -> u64 {
        match self {
            Count::Exact(n) | Count::AtLeast(n) => n,
        }
    }
}

impl fmt::Display for Count {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Count::Exact(n) => write!(f, "{n}"),
            Count::AtLeast(n) => write!(f, "at least {n}"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct VerdictCounts {
    pub read: u64,
    pub not_checked: Count,
}

pub fn verdict_counts<'a, I>(outcomes: I) -> VerdictCounts
where
    I: IntoIterator<Item = &'a FileOutcome>,
{
    let mut read = 0u64;
    let mut not_checked = 0u64;
    let mut bounded = false;
    for outcome in outcomes {
        match outcome {
            FileOutcome::Read => read += 1,
            FileOutcome::NotChecked { .. } => not_checked += 1,
            FileOutcome::Aggregate { count: Some(n), .. } => not_checked += n,
            FileOutcome::Aggregate {
                count: None,
                at_least,
                ..
            } => {
                not_checked += (*at_least).max(1);
                bounded = true;
            }
            FileOutcome::OutOfScope { .. } => {}
        }
    }
    VerdictCounts {
        read,
        not_checked: if bounded {
            Count::AtLeast(not_checked)
        } else {
            Count::Exact(not_checked)
        },
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Sm3 {
    pub numerator: u64,
    pub denominator: u64,
}

pub fn sm3_share<'a, I>(files: I) -> Option<Sm3>
where
    I: IntoIterator<Item = (&'a Outcome, bool)>,
{
    let mut numerator = 0u64;
    let mut denominator = 0u64;
    for (outcome, unreadable_region) in files {
        if outcome.file_kind != Some(FileKind::Source) {
            continue;
        }
        let (n, hit) = match &outcome.file {
            FileOutcome::OutOfScope { .. }
            | FileOutcome::NotChecked {
                reason: NotCheckedReason::NotInCompileDatabase,
                ..
            }
            | FileOutcome::Aggregate {
                reason: NotCheckedReason::NotInCompileDatabase,
                ..
            } => continue,
            FileOutcome::Read => (1, unreadable_region),
            FileOutcome::NotChecked { .. } => (1, true),
            FileOutcome::Aggregate {
                count, at_least, ..
            } => (count.unwrap_or((*at_least).max(1)), true),
        };
        denominator += n;
        if hit {
            numerator += n;
        }
    }
    (denominator > 0).then_some(Sm3 {
        numerator,
        denominator,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn not_checked(reason: NotCheckedReason) -> FileOutcome {
        FileOutcome::NotChecked {
            reason,
            details: Details::None,
        }
    }

    #[test]
    fn out_of_scope_adds_nothing() {
        let mut outcomes = vec![
            FileOutcome::OutOfScope {
                cause: OutOfScopeCause::Pattern(PathPattern {
                    base: Text::from("/repo"),
                    pattern: Text::from("third_party/**"),
                }),
            };
            5000
        ];
        outcomes.extend([FileOutcome::Read, FileOutcome::Read, FileOutcome::Read]);
        let counts = verdict_counts(&outcomes);
        assert_eq!(counts.read, 3);
        assert_eq!(counts.not_checked, Count::Exact(0));
    }

    #[test]
    fn aggregate_of_unknown_count_is_at_least_one() {
        let outcomes = [FileOutcome::Aggregate {
            reason: NotCheckedReason::RepeatedFault,
            count: None,
            at_least: 0,
        }];
        let counts = verdict_counts(&outcomes);
        assert_eq!(counts.not_checked, Count::AtLeast(1));
        assert_eq!(counts.not_checked.to_string(), "at least 1");
    }

    #[test]
    fn mixed_counts() {
        let outcomes = [
            FileOutcome::Read,
            not_checked(NotCheckedReason::NoReader),
            FileOutcome::Aggregate {
                reason: NotCheckedReason::LimitExceeded(LimitName::TotalEntries),
                count: Some(40),
                at_least: 40,
            },
            FileOutcome::Aggregate {
                reason: NotCheckedReason::RepeatedFault,
                count: None,
                at_least: 3,
            },
        ];
        let counts = verdict_counts(&outcomes);
        assert_eq!(counts.read, 1);
        assert_eq!(counts.not_checked, Count::AtLeast(44));
        assert_eq!(counts.not_checked.to_string(), "at least 44");
    }

    fn source(file: FileOutcome) -> Outcome {
        Outcome::new(file, Some(FileKind::Source))
    }

    #[test]
    fn sm3_zero_denominator_is_none() {
        assert_eq!(sm3_share(std::iter::empty()), None);
        let config = Outcome::new(FileOutcome::Read, Some(FileKind::Config));
        assert_eq!(sm3_share([(&config, true)]), None);
        let left_out = source(FileOutcome::Aggregate {
            reason: NotCheckedReason::NotInCompileDatabase,
            count: Some(50),
            at_least: 50,
        });
        assert_eq!(sm3_share([(&left_out, false)]), None);
    }

    #[test]
    fn v2_sm3_other_aggregates_count_in_both() {
        let read = source(FileOutcome::Read);
        let unknown = source(FileOutcome::Aggregate {
            reason: NotCheckedReason::RepeatedFault,
            count: None,
            at_least: 3,
        });
        let unknown_zero = source(FileOutcome::Aggregate {
            reason: NotCheckedReason::RepeatedFault,
            count: None,
            at_least: 0,
        });
        let counted = source(FileOutcome::Aggregate {
            reason: NotCheckedReason::LimitExceeded(LimitName::TotalEntries),
            count: Some(5),
            at_least: 5,
        });
        assert_eq!(
            sm3_share([
                (&read, false),
                (&unknown, false),
                (&unknown_zero, false),
                (&counted, false)
            ]),
            Some(Sm3 {
                numerator: 9,
                denominator: 10
            })
        );
    }

    #[test]
    fn e15_sm3_excludes_not_in_compile_database_in_both_forms() {
        let read = source(FileOutcome::Read);
        let single = source(not_checked(NotCheckedReason::NotInCompileDatabase));
        let aggregate = source(FileOutcome::Aggregate {
            reason: NotCheckedReason::NotInCompileDatabase,
            count: Some(7),
            at_least: 7,
        });
        assert_eq!(sm3_share([(&single, true)]), None);
        assert_eq!(
            sm3_share([(&read, false), (&single, true), (&aggregate, true)]),
            Some(Sm3 {
                numerator: 0,
                denominator: 1
            })
        );
    }

    #[test]
    fn sm3_counts_unread_and_regions() {
        let clean = source(FileOutcome::Read);
        let region = source(FileOutcome::Read);
        let skipped = source(not_checked(NotCheckedReason::EncodingUndeclared));
        let excluded = source(FileOutcome::OutOfScope {
            cause: OutOfScopeCause::OwnFile(OwnFileRole::Output),
        });
        let left_out = source(FileOutcome::Aggregate {
            reason: NotCheckedReason::NotInCompileDatabase,
            count: Some(50),
            at_least: 50,
        });
        let binary = Outcome::new(
            not_checked(NotCheckedReason::NoReader),
            Some(FileKind::Binary),
        );
        let share = sm3_share([
            (&clean, false),
            (&clean, false),
            (&region, true),
            (&skipped, false),
            (&excluded, true),
            (&left_out, false),
            (&binary, true),
        ]);
        assert_eq!(
            share,
            Some(Sm3 {
                numerator: 2,
                denominator: 4
            })
        );
    }
}
