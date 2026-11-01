use std::fmt;

use serde::{Deserialize, Serialize};

use crate::fact::FileKind;
use crate::limits::LimitName;
use crate::message::{CanonicalKey, CollectedFile, EntryPath};
use crate::settings::{Encoding, PathPattern};
use crate::sorted::SortedVec;
use crate::text::Text;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Outcome {
    pub file: FileOutcome,
    pub file_kind: Option<FileKind>,
    pub host_dependent: bool,
    pub lines: Option<LineCount>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct LineCount {
    pub lines: u64,
    pub complete: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutcomeError {
    OutOfScope,
    NotInCompileDatabase,
    HostDependent,
    EmptyCount,
    CountBelowAtLeast,
    AggregateOnEntryZero,
    NoLineCount,
    LinesOnAggregate,
    PartialCountOnRead,
    LinesAboveBytes { lines: u64, bytes: u64 },
}

impl fmt::Display for OutcomeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            OutcomeError::OutOfScope => "a worker may not send an out-of-scope outcome",
            OutcomeError::NotInCompileDatabase => {
                "a worker may not send a not-in-compile-database outcome"
            }
            OutcomeError::HostDependent => "a worker may not mark an outcome host-dependent",
            OutcomeError::EmptyCount => "an aggregate's count is 0",
            OutcomeError::CountBelowAtLeast => "an aggregate's count is below its at_least",
            OutcomeError::AggregateOnEntryZero => "an aggregate sits on a container's entry 0",
            OutcomeError::NoLineCount => "a read source file has no line count",
            OutcomeError::LinesOnAggregate => "an aggregate carries a line count",
            OutcomeError::PartialCountOnRead => "a read file's line count is incomplete",
            OutcomeError::LinesAboveBytes { lines, bytes } => {
                return write!(f, "a line count of {lines} is above the {bytes} bytes read");
            }
        })
    }
}

impl std::error::Error for OutcomeError {}

impl Outcome {
    pub fn validate_from_worker(&self, entry: &EntryPath) -> Result<(), OutcomeError> {
        if self.host_dependent {
            return Err(OutcomeError::HostDependent);
        }
        if self.file == FileOutcome::Read
            && self.file_kind == Some(FileKind::Source)
            && self.lines.is_none()
        {
            return Err(OutcomeError::NoLineCount);
        }
        if matches!(self.file, FileOutcome::Aggregate { .. }) && self.lines.is_some() {
            return Err(OutcomeError::LinesOnAggregate);
        }
        if self.file == FileOutcome::Read && self.lines.is_some_and(|c| !c.complete) {
            return Err(OutcomeError::PartialCountOnRead);
        }
        match &self.file {
            FileOutcome::OutOfScope { .. } => Err(OutcomeError::OutOfScope),
            FileOutcome::NotChecked {
                reason: NotCheckedReason::NotInCompileDatabase,
                ..
            }
            | FileOutcome::Aggregate {
                reason: NotCheckedReason::NotInCompileDatabase,
                ..
            } => Err(OutcomeError::NotInCompileDatabase),
            FileOutcome::Aggregate { .. } if entry.last() == Some(0) => {
                Err(OutcomeError::AggregateOnEntryZero)
            }
            FileOutcome::Aggregate { count: Some(0), .. } => Err(OutcomeError::EmptyCount),
            FileOutcome::Aggregate {
                count: Some(n),
                at_least,
                ..
            } if n < at_least => Err(OutcomeError::CountBelowAtLeast),
            _ => Ok(()),
        }
    }

    pub fn new(file: FileOutcome, file_kind: Option<FileKind>) -> Self {
        Outcome {
            file,
            file_kind,
            host_dependent: false,
            lines: None,
        }
    }

    pub fn with_lines(self, lines: u64) -> Self {
        Outcome {
            lines: Some(LineCount {
                lines,
                complete: true,
            }),
            ..self
        }
    }

    pub fn with_partial_lines(self, lines: u64) -> Self {
        Outcome {
            lines: Some(LineCount {
                lines,
                complete: false,
            }),
            ..self
        }
    }

    pub fn validate_lines(&self, bytes: u64) -> Result<(), OutcomeError> {
        match self.lines {
            Some(count) if count.lines > bytes => Err(OutcomeError::LinesAboveBytes {
                lines: count.lines,
                bytes,
            }),
            _ => Ok(()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LineUnits {
    Bytes,
    Utf16Le,
    Utf16Be,
}

#[derive(Debug, Clone)]
pub struct LineCounter {
    declared: Option<Encoding>,
    units: Option<LineUnits>,
    head: Vec<u8>,
    odd: Option<u8>,
    ends: u64,
    any: bool,
    last_lf: bool,
}

impl LineCounter {
    pub fn new(declared: Option<Encoding>) -> Self {
        LineCounter {
            declared,
            units: None,
            head: Vec::with_capacity(2),
            odd: None,
            ends: 0,
            any: false,
            last_lf: false,
        }
    }

    pub fn feed(&mut self, mut bytes: &[u8]) {
        if self.units.is_none() {
            let take = (2 - self.head.len()).min(bytes.len());
            self.head.extend_from_slice(&bytes[..take]);
            bytes = &bytes[take..];
            if self.head.len() < 2 {
                return;
            }
            self.decide();
        }
        self.count(bytes);
    }

    pub fn finish(mut self) -> u64 {
        if self.units.is_none() {
            self.decide();
        }
        if self.odd.take().is_some() {
            self.any = true;
            self.last_lf = false;
        }
        self.ends
            .saturating_add(u64::from(self.any && !self.last_lf))
    }

    fn decide(&mut self) {
        let head = std::mem::take(&mut self.head);
        let bom_le = head.as_slice() == [0xFF, 0xFE];
        let bom_be = head.as_slice() == [0xFE, 0xFF];
        let (units, bom) = match self.declared {
            Some(Encoding::Utf16Le | Encoding::WindowsCodePage(1200)) => {
                (LineUnits::Utf16Le, bom_le)
            }
            Some(Encoding::Utf16Be | Encoding::WindowsCodePage(1201)) => {
                (LineUnits::Utf16Be, bom_be)
            }
            None if bom_le => (LineUnits::Utf16Le, true),
            None if bom_be => (LineUnits::Utf16Be, true),
            _ => (LineUnits::Bytes, false),
        };
        self.units = Some(units);
        if !bom {
            self.count(&head);
        }
    }

    fn count(&mut self, bytes: &[u8]) {
        match self.units {
            Some(LineUnits::Bytes) | None => {
                let Some(&last) = bytes.last() else {
                    return;
                };
                let lf = bytes.iter().filter(|&&b| b == b'\n').count() as u64;
                self.ends = self.ends.saturating_add(lf);
                self.any = true;
                self.last_lf = last == b'\n';
            }
            Some(units) => {
                for &b in bytes {
                    let Some(first) = self.odd.take() else {
                        self.odd = Some(b);
                        continue;
                    };
                    let unit = match units {
                        LineUnits::Utf16Le => u16::from_le_bytes([first, b]),
                        _ => u16::from_be_bytes([first, b]),
                    };
                    self.any = true;
                    self.last_lf = unit == 0x000A;
                    if self.last_lf {
                        self.ends = self.ends.saturating_add(1);
                    }
                }
            }
        }
    }
}

pub fn line_count(bytes: &[u8], declared: Option<Encoding>) -> u64 {
    let mut counter = LineCounter::new(declared);
    counter.feed(bytes);
    counter.finish()
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
    Unreadable(IoKind),
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
pub enum IoKind {
    NotFound,
    PermissionDenied,
    IsADirectory,
    Other,
}

impl IoKind {
    pub fn from_io(kind: std::io::ErrorKind) -> IoKind {
        match kind {
            std::io::ErrorKind::NotFound => IoKind::NotFound,
            std::io::ErrorKind::PermissionDenied => IoKind::PermissionDenied,
            std::io::ErrorKind::IsADirectory => IoKind::IsADirectory,
            _ => IoKind::Other,
        }
    }
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
            FileOutcome::Read => read = read.saturating_add(1),
            FileOutcome::NotChecked { .. } => not_checked = not_checked.saturating_add(1),
            FileOutcome::Aggregate { count: Some(n), .. } => {
                not_checked = not_checked.saturating_add(*n);
            }
            FileOutcome::Aggregate {
                count: None,
                at_least,
                ..
            } => {
                not_checked = not_checked.saturating_add((*at_least).max(1));
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
pub struct FileShare {
    pub numerator: u64,
    pub denominator: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct LineShare {
    pub numerator: u64,
    pub denominator: u64,
    pub uncounted_files: u64,
    pub incomplete_files: u64,
}

impl LineShare {
    pub fn is_lower_bound(&self) -> bool {
        self.uncounted_files > 0 || self.incomplete_files > 0
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Sm3 {
    pub lines: Option<LineShare>,
    pub files: Option<FileShare>,
    pub uncounted: SortedVec<CanonicalKey>,
    pub incomplete: SortedVec<CanonicalKey>,
}

fn region_lines(file: &CollectedFile, lines: u64) -> u64 {
    let mut spans: Vec<(u64, u64)> = file
        .regions
        .iter()
        .map(|r| (u64::from(r.first_line), u64::from(r.last_line).min(lines)))
        .filter(|&(first, last)| first >= 1 && first <= last)
        .collect();
    spans.sort_unstable();
    let mut total = 0u64;
    let mut covered = 0u64;
    for (first, last) in spans {
        let from = first.max(covered.saturating_add(1));
        if from <= last {
            total = total.saturating_add(last - from + 1);
        }
        covered = covered.max(last);
    }
    total
}

pub fn sm3_share<'a, I>(files: I) -> Sm3
where
    I: IntoIterator<Item = &'a CollectedFile>,
{
    let mut file_num = 0u64;
    let mut file_den = 0u64;
    let mut line_num = 0u64;
    let mut line_den = 0u64;
    let mut uncounted_files = 0u64;
    let mut incomplete_files = 0u64;
    let mut uncounted = Vec::new();
    let mut incomplete = Vec::new();
    for file in files {
        let outcome = &file.outcome;
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
            FileOutcome::Read => (1, file.has_unreadable_region()),
            FileOutcome::NotChecked { .. } => (1, true),
            FileOutcome::Aggregate {
                count, at_least, ..
            } => (count.unwrap_or((*at_least).max(1)), true),
        };
        if n == 0 {
            continue;
        }
        file_den = file_den.saturating_add(n);
        if hit {
            file_num = file_num.saturating_add(n);
        }
        let count = match (&outcome.file, outcome.lines) {
            (FileOutcome::Read, Some(count)) => {
                line_num = line_num.saturating_add(region_lines(file, count.lines));
                count
            }
            (FileOutcome::NotChecked { .. }, Some(count)) => {
                line_num = line_num.saturating_add(count.lines);
                count
            }
            _ => {
                uncounted_files = uncounted_files.saturating_add(n);
                uncounted.push(file.key.clone());
                continue;
            }
        };
        line_den = line_den.saturating_add(count.lines);
        if !count.complete {
            incomplete_files = incomplete_files.saturating_add(1);
            incomplete.push(file.key.clone());
        }
    }
    let any_lines = line_den > 0 || uncounted_files > 0 || incomplete_files > 0;
    Sm3 {
        lines: any_lines.then_some(LineShare {
            numerator: line_num,
            denominator: line_den,
            uncounted_files,
            incomplete_files,
        }),
        files: (file_den > 0).then_some(FileShare {
            numerator: file_num,
            denominator: file_den,
        }),
        uncounted: SortedVec::from(uncounted),
        incomplete: SortedVec::from(incomplete),
    }
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

    use crate::location::{InputRef, Location, RelPath, Scope};
    use crate::message::{CanonicalKey, Region, RegionReason};

    fn source(file: FileOutcome) -> Outcome {
        Outcome::new(file, Some(FileKind::Source))
    }

    fn collected(outcome: &Outcome, regions: &[(u32, u32)]) -> CollectedFile {
        let path = RelPath::try_from("src/main.c").unwrap();
        CollectedFile {
            key: CanonicalKey::at(0, path.clone(), EntryPath::job()),
            location: Location {
                input: InputRef {
                    index: 0,
                    given: Text::from("/repo"),
                },
                frames: vec![],
                path: Some(path),
                line: None,
                scope: Scope::File,
            },
            outcome: outcome.clone(),
            regions: regions
                .iter()
                .map(|&(first_line, last_line)| Region {
                    first_line,
                    last_line,
                    reason: RegionReason::Unparseable,
                })
                .collect(),
            notes: vec![],
        }
    }

    fn share(files: &[(&Outcome, &[(u32, u32)])]) -> Sm3 {
        let files: Vec<CollectedFile> = files.iter().map(|(o, r)| collected(o, r)).collect();
        let sm3 = sm3_share(&files);
        Sm3 {
            uncounted: SortedVec::new(),
            incomplete: SortedVec::new(),
            ..sm3
        }
    }

    fn files(numerator: u64, denominator: u64) -> Option<FileShare> {
        Some(FileShare {
            numerator,
            denominator,
        })
    }

    fn lines(numerator: u64, denominator: u64, uncounted_files: u64) -> Option<LineShare> {
        Some(LineShare {
            numerator,
            denominator,
            uncounted_files,
            incomplete_files: 0,
        })
    }

    const REGION: &[(u32, u32)] = &[(1, 2)];

    #[test]
    fn sm3_zero_denominator_is_none() {
        let none = Sm3::default();
        assert_eq!(sm3_share(std::iter::empty()), none);
        let config = Outcome::new(FileOutcome::Read, Some(FileKind::Config)).with_lines(9);
        assert_eq!(share(&[(&config, REGION)]), none);
        let left_out = source(FileOutcome::Aggregate {
            reason: NotCheckedReason::NotInCompileDatabase,
            count: Some(50),
            at_least: 50,
        });
        assert_eq!(share(&[(&left_out, &[])]), none);
        let empty = source(FileOutcome::Read).with_lines(0);
        assert_eq!(
            share(&[(&empty, &[])]),
            Sm3 {
                lines: None,
                files: files(0, 1),
                ..Sm3::default()
            }
        );
    }

    #[test]
    fn v2_sm3_other_aggregates_count_in_both() {
        let read = source(FileOutcome::Read).with_lines(10);
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
            share(&[
                (&read, &[]),
                (&unknown, &[]),
                (&unknown_zero, &[]),
                (&counted, &[])
            ]),
            Sm3 {
                lines: lines(0, 10, 9),
                files: files(9, 10),
                ..Sm3::default()
            }
        );
    }

    #[test]
    fn e15_sm3_excludes_not_in_compile_database_in_both_forms() {
        let read = source(FileOutcome::Read).with_lines(4);
        let single = source(not_checked(NotCheckedReason::NotInCompileDatabase)).with_lines(7);
        let aggregate = source(FileOutcome::Aggregate {
            reason: NotCheckedReason::NotInCompileDatabase,
            count: Some(7),
            at_least: 7,
        });
        assert_eq!(
            share(&[(&single, REGION)]),
            Sm3 {
                lines: None,
                files: None,
                ..Sm3::default()
            }
        );
        assert_eq!(
            share(&[(&read, &[]), (&single, REGION), (&aggregate, REGION)]),
            Sm3 {
                lines: lines(0, 4, 0),
                files: files(0, 1),
                ..Sm3::default()
            }
        );
    }

    #[test]
    fn sm3_counts_unread_and_regions() {
        let clean = source(FileOutcome::Read).with_lines(10);
        let region = source(FileOutcome::Read).with_lines(20);
        let skipped = source(not_checked(NotCheckedReason::EncodingUndeclared)).with_lines(30);
        let excluded = source(FileOutcome::OutOfScope {
            cause: OutOfScopeCause::OwnFile(OwnFileRole::Output),
        })
        .with_lines(40);
        let left_out = source(FileOutcome::Aggregate {
            reason: NotCheckedReason::NotInCompileDatabase,
            count: Some(50),
            at_least: 50,
        });
        let binary = Outcome::new(
            not_checked(NotCheckedReason::NoReader),
            Some(FileKind::Binary),
        )
        .with_lines(60);
        assert_eq!(
            share(&[
                (&clean, &[]),
                (&clean, &[]),
                (&region, &[(3, 5)]),
                (&skipped, &[]),
                (&excluded, REGION),
                (&left_out, &[]),
                (&binary, REGION),
            ]),
            Sm3 {
                lines: lines(33, 70, 0),
                files: files(2, 4),
                ..Sm3::default()
            }
        );
    }

    #[test]
    fn regions_count_their_union_clamped_to_the_file() {
        let file = source(FileOutcome::Read).with_lines(10);
        for (regions, expected) in [
            (&[(3, 5), (4, 9)][..], 7),
            (&[(4, 9), (3, 5)][..], 7),
            (&[(1, 1), (1, 1), (10, 10)][..], 2),
            (&[(2, 3), (5, 6)][..], 4),
            (&[(8, 12)][..], 3),
            (&[(11, 20)][..], 0),
            (&[(1, u32::MAX)][..], 10),
        ] {
            let got = share(&[(&file, regions)]).lines.unwrap().numerator;
            assert_eq!(got, expected, "{regions:?}");
        }
    }

    #[test]
    fn line_count_counts_lf_ends_plus_an_unterminated_last_line() {
        for (bytes, expected) in [
            (&b""[..], 0),
            (b"\n", 1),
            (b"a", 1),
            (b"a\nb", 2),
            (b"a\nb\n", 2),
            (b"a\r\nb\r\n", 2),
            (b"a\r\nb", 2),
            (b"a\rb\r", 1),
            (b"\r", 1),
            (b"\n\n\n", 3),
            (b"\xEF\xBB\xBFa\n", 1),
        ] {
            assert_eq!(line_count(bytes, None), expected, "{bytes:?}");
            assert_eq!(
                line_count(bytes, Some(Encoding::Utf8)),
                expected,
                "{bytes:?}"
            );
        }
    }

    fn utf16(text: &str, le: bool, bom: bool) -> Vec<u8> {
        let mut out = Vec::new();
        let units = bom.then_some(0xFEFF).into_iter().chain(text.encode_utf16());
        for unit in units {
            out.extend(if le {
                unit.to_le_bytes()
            } else {
                unit.to_be_bytes()
            });
        }
        out
    }

    #[test]
    fn utf16_counts_code_units_after_the_bom() {
        for (text, expected) in [
            ("", 0),
            ("a", 1),
            ("a\nb", 2),
            ("a\r\nb\r\n", 2),
            ("a\rb", 1),
            ("\u{0A0A}", 1),
            ("\u{0A00}\u{000A}", 1),
        ] {
            for le in [true, false] {
                let declared = Some(if le {
                    Encoding::Utf16Le
                } else {
                    Encoding::Utf16Be
                });
                let with_bom = utf16(text, le, true);
                assert_eq!(line_count(&with_bom, None), expected, "{text:?} {le}");
                assert_eq!(line_count(&with_bom, declared), expected, "{text:?} {le}");
                assert_eq!(
                    line_count(&utf16(text, le, false), declared),
                    expected,
                    "{text:?} {le}"
                );
            }
        }
        assert_eq!(line_count(&[0xFF, 0xFE, 0x0A], None), 1);
        let page = utf16("a\nb", true, false);
        assert_eq!(line_count(&page, Some(Encoding::WindowsCodePage(1200))), 2);
        let page = utf16("a\nb", false, false);
        assert_eq!(line_count(&page, Some(Encoding::WindowsCodePage(1201))), 2);
        assert_eq!(line_count(&[0xFF, 0xFE, 0x0A, 0x00, 0x61], None), 2);
        assert_eq!(line_count(&[0x0A, 0x00], None), 2);
        assert_eq!(line_count(&[0x61, 0x00, 0x0A, 0x00], None), 2);
    }

    #[test]
    fn line_counter_streams_in_any_split() {
        let samples = [
            (b"a\nb\r\nc".to_vec(), None),
            (utf16("x\ny\nz", true, true), None),
            (utf16("x\ny\n", false, false), Some(Encoding::Utf16Be)),
            (vec![0xFE], None),
            (vec![0xFF, 0xFE], None),
        ];
        for (bytes, declared) in samples {
            let whole = line_count(&bytes, declared);
            for chunk in 1..=bytes.len().max(1) {
                let mut counter = LineCounter::new(declared);
                for piece in bytes.chunks(chunk) {
                    counter.feed(piece);
                    counter.feed(&[]);
                }
                assert_eq!(counter.finish(), whole, "{bytes:?} by {chunk}");
            }
        }
    }

    #[test]
    fn outcomes_from_workers_carry_their_line_count() {
        let child = EntryPath::from([1]);
        assert_eq!(
            source(FileOutcome::Read).validate_from_worker(&child),
            Err(OutcomeError::NoLineCount)
        );
        assert_eq!(
            source(FileOutcome::Read)
                .with_lines(0)
                .validate_from_worker(&child),
            Ok(())
        );
        assert_eq!(
            source(not_checked(NotCheckedReason::Unparseable)).validate_from_worker(&child),
            Ok(())
        );
        assert_eq!(
            Outcome::new(FileOutcome::Read, Some(FileKind::Config)).validate_from_worker(&child),
            Ok(())
        );
        let aggregate = source(FileOutcome::Aggregate {
            reason: NotCheckedReason::RepeatedFault,
            count: None,
            at_least: 1,
        });
        assert_eq!(aggregate.validate_from_worker(&child), Ok(()));
        assert_eq!(
            aggregate.with_lines(3).validate_from_worker(&child),
            Err(OutcomeError::LinesOnAggregate)
        );
    }
}
