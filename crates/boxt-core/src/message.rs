use std::fmt;
use std::io::{self, Read};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::compile_db::CompileUnit;
use crate::fact::{Fact, FileKind};
use crate::limits::{LimitName, Limits};
use crate::location::{Frame, InputRef, Location, RelPath, Scope};
use crate::outcome::{Details, FaultKind, FileOutcome, NotCheckedReason, Outcome};
use crate::settings::WorkerSettings;
use crate::text::Text;

#[derive(Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EntryPath(pub Vec<u32>);

impl EntryPath {
    pub fn job() -> Self {
        EntryPath(Vec::new())
    }

    pub fn child(&self, index: u32) -> Self {
        let mut v = self.0.clone();
        v.push(index);
        EntryPath(v)
    }

    pub fn as_slice(&self) -> &[u32] {
        &self.0
    }

    pub fn depth(&self) -> usize {
        self.0.len()
    }

    pub fn is_job(&self) -> bool {
        self.0.is_empty()
    }

    pub fn is_prefix_of(&self, other: &EntryPath) -> bool {
        other.0.starts_with(&self.0)
    }

    pub fn parent(&self) -> Option<EntryPath> {
        self.0
            .split_last()
            .map(|(_, parent)| EntryPath(parent.to_vec()))
    }

    pub fn last(&self) -> Option<u32> {
        self.0.last().copied()
    }
}

impl From<&[u32]> for EntryPath {
    fn from(v: &[u32]) -> Self {
        EntryPath(v.to_vec())
    }
}

impl<const N: usize> From<[u32; N]> for EntryPath {
    fn from(v: [u32; N]) -> Self {
        EntryPath(v.to_vec())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobKind {
    Route,
    CompileDatabase,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobFile {
    InInput(RelPath),
    Setting(Text),
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Job {
    pub kind: JobKind,
    pub input: InputRef,
    pub file: JobFile,
    pub resume_after: Option<EntryPath>,
}

impl Job {
    pub fn path(&self) -> Option<&RelPath> {
        match &self.file {
            JobFile::InInput(path) => Some(path),
            JobFile::Setting(_) => None,
        }
    }

    pub fn is_well_formed(&self) -> bool {
        matches!(
            (self.kind, &self.file),
            (JobKind::Route, JobFile::InInput(_)) | (JobKind::CompileDatabase, JobFile::Setting(_))
        ) && (self.kind == JobKind::Route || self.resume_after.is_none())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CollectorMsg {
    Settings(WorkerSettings),
    Job(Job),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RegionReason {
    Unparseable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Region {
    pub first_line: u32,
    pub last_line: u32,
    pub reason: RegionReason,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegionError {
    LineZero,
    Inverted,
}

impl fmt::Display for RegionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            RegionError::LineZero => "a region starts on line 0; lines are 1-based",
            RegionError::Inverted => "a region ends before it starts",
        })
    }
}

impl std::error::Error for RegionError {}

impl Region {
    pub fn validate(&self) -> Result<(), RegionError> {
        if self.first_line == 0 {
            return Err(RegionError::LineZero);
        }
        if self.last_line < self.first_line {
            return Err(RegionError::Inverted);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Note {
    MissingSymbols,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerMsg {
    Opened {
        entry: EntryPath,
        frames: Vec<Frame>,
        path: Option<RelPath>,
        scope: Scope,
        file_kind: Option<FileKind>,
    },
    Facts {
        entry: EntryPath,
        facts: Vec<Fact>,
    },
    Regions {
        entry: EntryPath,
        regions: Vec<Region>,
    },
    Notes {
        entry: EntryPath,
        notes: Vec<Note>,
    },
    EntryDone {
        entry: EntryPath,
        outcome: Outcome,
        bytes: u64,
    },
    CompileUnits {
        entries: Vec<CompileUnit>,
    },
    JobDone {
        outcome: Option<Outcome>,
    },
}

impl WorkerMsg {
    pub fn entry(&self) -> Option<&EntryPath> {
        match self {
            WorkerMsg::Opened { entry, .. }
            | WorkerMsg::Facts { entry, .. }
            | WorkerMsg::Regions { entry, .. }
            | WorkerMsg::Notes { entry, .. }
            | WorkerMsg::EntryDone { entry, .. } => Some(entry),
            WorkerMsg::CompileUnits { .. } | WorkerMsg::JobDone { .. } => None,
        }
    }

    pub fn opened_location(&self, input: &InputRef) -> Option<Location> {
        match self {
            WorkerMsg::Opened {
                frames,
                path,
                scope,
                ..
            } => Some(Location {
                input: input.clone(),
                frames: frames.clone(),
                path: path.clone(),
                line: None,
                scope: scope.clone(),
            }),
            _ => None,
        }
    }

    pub fn ends_container(&self) -> Option<EntryPath> {
        match self {
            WorkerMsg::EntryDone {
                entry,
                outcome:
                    Outcome {
                        file: FileOutcome::Aggregate { .. },
                        ..
                    },
                ..
            } if entry.last().is_some_and(|i| i > 0) => entry.parent(),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct CanonicalKey {
    pub input_index: u32,
    pub job_path: RelPath,
    pub entry: EntryPath,
    pub location: Option<Location>,
}

impl CanonicalKey {
    pub fn for_fact(job: &Job, entry: &EntryPath, fact: &Fact) -> Option<Self> {
        Some(CanonicalKey {
            input_index: job.input.index,
            job_path: job.path()?.clone(),
            entry: entry.clone(),
            location: Some(fact.location().clone()),
        })
    }

    pub fn for_outcome(job: &Job, entry: &EntryPath) -> Option<Self> {
        Some(CanonicalKey {
            input_index: job.input.index,
            job_path: job.path()?.clone(),
            entry: entry.clone(),
            location: None,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct CollectedFile {
    pub key: CanonicalKey,
    pub location: Location,
    pub outcome: Outcome,
    pub regions: Vec<Region>,
    pub notes: Vec<Note>,
}

impl CollectedFile {
    pub fn has_unreadable_region(&self) -> bool {
        !self.regions.is_empty()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameError {
    Truncated,
    TooLong { len: u64, max: u64 },
    Encode,
    Decode,
    Io(io::ErrorKind),
}

impl fmt::Display for FrameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FrameError::Truncated => f.write_str("frame truncated"),
            FrameError::TooLong { len, max } => {
                write!(f, "frame length {len} is over the limit {max}")
            }
            FrameError::Encode => f.write_str("frame payload could not be encoded"),
            FrameError::Decode => f.write_str("frame payload could not be decoded"),
            FrameError::Io(kind) => write!(f, "frame read failed: {kind}"),
        }
    }
}

impl std::error::Error for FrameError {}

pub const FRAME_PREFIX_LEN: usize = 4;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decoded<T> {
    pub msg: T,
    pub payload_len: u64,
}

pub fn encode_frame<T: Serialize>(msg: &T, max_len: u64) -> Result<Vec<u8>, FrameError> {
    let payload = serde_json::to_vec(msg).map_err(|_| FrameError::Encode)?;
    let len = payload.len() as u64;
    let max = max_len.min(u64::from(u32::MAX));
    if len > max {
        return Err(FrameError::TooLong { len, max });
    }
    let mut frame = Vec::with_capacity(FRAME_PREFIX_LEN + payload.len());
    frame.extend_from_slice(&(len as u32).to_le_bytes());
    frame.extend_from_slice(&payload);
    Ok(frame)
}

pub fn decode_frame<T: DeserializeOwned, R: Read>(
    reader: &mut R,
    max_len: u64,
) -> Result<Option<Decoded<T>>, FrameError> {
    let mut prefix = [0u8; FRAME_PREFIX_LEN];
    let mut got = 0;
    while got < FRAME_PREFIX_LEN {
        match reader.read(&mut prefix[got..]) {
            Ok(0) => break,
            Ok(n) => got += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(FrameError::Io(e.kind())),
        }
    }
    if got == 0 {
        return Ok(None);
    }
    if got < FRAME_PREFIX_LEN {
        return Err(FrameError::Truncated);
    }
    let len = u64::from(u32::from_le_bytes(prefix));
    if len > max_len {
        return Err(FrameError::TooLong { len, max: max_len });
    }
    let mut payload = Vec::new();
    reader
        .take(len)
        .read_to_end(&mut payload)
        .map_err(|e| FrameError::Io(e.kind()))?;
    if (payload.len() as u64) < len {
        return Err(FrameError::Truncated);
    }
    let msg = serde_json::from_slice(&payload).map_err(|_| FrameError::Decode)?;
    Ok(Some(Decoded {
        msg,
        payload_len: len,
    }))
}

pub fn encode_compile_units(
    units: &[CompileUnit],
    max_len: u64,
) -> Result<Vec<Vec<u8>>, FrameError> {
    let empty = WorkerMsg::CompileUnits {
        entries: Vec::new(),
    };
    let overhead = serde_json::to_vec(&empty)
        .map_err(|_| FrameError::Encode)?
        .len() as u64;
    let mut frames = Vec::new();
    let mut chunk: Vec<CompileUnit> = Vec::new();
    let mut chunk_len = overhead;
    for unit in units {
        let unit_len = serde_json::to_vec(unit)
            .map_err(|_| FrameError::Encode)?
            .len() as u64;
        let added = unit_len + u64::from(!chunk.is_empty());
        if !chunk.is_empty() && chunk_len + added > max_len {
            let entries = std::mem::take(&mut chunk);
            frames.push(encode_frame(&WorkerMsg::CompileUnits { entries }, max_len)?);
            chunk_len = overhead;
        }
        chunk_len += unit_len + u64::from(!chunk.is_empty());
        chunk.push(unit.clone());
    }
    if !chunk.is_empty() || frames.is_empty() {
        frames.push(encode_frame(
            &WorkerMsg::CompileUnits { entries: chunk },
            max_len,
        )?);
    }
    Ok(frames)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EntryBudget {
    max_facts: u64,
    max_bytes: u64,
    facts: u64,
    bytes: u64,
}

impl EntryBudget {
    pub fn new(limits: &Limits) -> Self {
        EntryBudget {
            max_facts: limits.get(LimitName::EntryFacts),
            max_bytes: limits.get(LimitName::EntryMessageBytes),
            facts: 0,
            bytes: 0,
        }
    }

    pub fn charge_frame(&mut self, msg: &WorkerMsg, payload_len: u64) -> Result<(), LimitName> {
        match msg {
            WorkerMsg::Facts { facts, .. } => {
                self.facts = self.facts.saturating_add(facts.len() as u64);
                self.bytes = self.bytes.saturating_add(payload_len);
            }
            WorkerMsg::Regions { .. } | WorkerMsg::Notes { .. } => {
                self.bytes = self.bytes.saturating_add(payload_len);
            }
            WorkerMsg::Opened { .. }
            | WorkerMsg::EntryDone { .. }
            | WorkerMsg::CompileUnits { .. }
            | WorkerMsg::JobDone { .. } => {}
        }
        self.exceeded().map_or(Ok(()), Err)
    }

    pub fn exceeded(&self) -> Option<LimitName> {
        if self.facts > self.max_facts {
            Some(LimitName::EntryFacts)
        } else if self.bytes > self.max_bytes {
            Some(LimitName::EntryMessageBytes)
        } else {
            None
        }
    }

    pub fn exceeded_outcome(&self) -> Option<FileOutcome> {
        self.exceeded().map(|limit| {
            let (value, max) = match limit {
                LimitName::EntryFacts => (self.facts, self.max_facts),
                _ => (self.bytes, self.max_bytes),
            };
            FileOutcome::NotChecked {
                reason: NotCheckedReason::LimitExceeded(limit),
                details: Details::Measured { value, limit: max },
            }
        })
    }
}

pub fn faulting_entry(committed: &[EntryPath], last_seen: Option<&EntryPath>) -> EntryPath {
    let Some(max_committed) = committed.iter().max() else {
        return EntryPath::job();
    };
    let last = match last_seen {
        Some(seen) if seen > max_committed => seen,
        _ => max_committed,
    };
    let Some((_, level)) = last.as_slice().split_last() else {
        return EntryPath::job();
    };
    let depth = level.len();
    let next = committed
        .iter()
        .map(EntryPath::as_slice)
        .filter(|e| e.len() > depth && &e[..depth] == level)
        .map(|e| e[depth])
        .max()
        .map_or(0, |i| i.saturating_add(1));
    let mut out = level.to_vec();
    out.push(next);
    EntryPath(out)
}

pub fn resume_skips(entry: &EntryPath, after: &EntryPath) -> bool {
    entry <= after || after.is_prefix_of(entry)
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum FaultScope {
    Job,
    Entry(EntryPath),
    Container(EntryPath),
    RestOfContainer { at: EntryPath, container: EntryPath },
}

impl FaultScope {
    pub fn outcome_entry(&self) -> EntryPath {
        match self {
            FaultScope::Job => EntryPath::job(),
            FaultScope::Entry(entry) => entry.clone(),
            FaultScope::Container(container) => container.child(0),
            FaultScope::RestOfContainer { at, .. } => at.clone(),
        }
    }

    pub fn resume_after(&self) -> Option<EntryPath> {
        match self {
            FaultScope::Job => None,
            FaultScope::Entry(entry) => Some(entry.clone()),
            FaultScope::Container(container) => Some(container.clone()),
            FaultScope::RestOfContainer { container, .. } => {
                (!container.is_job()).then(|| container.clone())
            }
        }
    }

    pub fn outcome(&self, reason: NotCheckedReason) -> FileOutcome {
        match self {
            FaultScope::RestOfContainer { .. } => FileOutcome::Aggregate {
                reason,
                count: None,
                at_least: 1,
            },
            _ => FileOutcome::NotChecked {
                reason,
                details: Details::None,
            },
        }
    }
}

pub fn fault_scope(attributed: &EntryPath, opened: bool) -> FaultScope {
    match attributed.as_slice().split_last() {
        None | Some((0, [])) => FaultScope::Job,
        Some((0, container)) => FaultScope::Container(EntryPath::from(container)),
        Some((_, container)) if !opened => FaultScope::RestOfContainer {
            at: attributed.clone(),
            container: EntryPath::from(container),
        },
        Some(_) => FaultScope::Entry(attributed.clone()),
    }
}

pub const WORKER_MEMORY_CAP_EXIT: i32 = 113;
pub const PANIC_EXIT: i32 = 101;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Unix,
    Windows,
}

impl Platform {
    pub const HOST: Platform = if cfg!(windows) {
        Platform::Windows
    } else {
        Platform::Unix
    };
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RawExit {
    Code(i32),
    Signal(i32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkerExit {
    pub raw: RawExit,
    pub killed_by_parent: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitClass {
    Fault(FaultKind),
    WorkerMemoryCap,
    KilledByParent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParentKill {
    Watchdog,
    Cancel,
}

impl ParentKill {
    pub fn reason(self) -> Option<NotCheckedReason> {
        match self {
            ParentKill::Watchdog => Some(NotCheckedReason::LimitExceeded(LimitName::Watchdog)),
            ParentKill::Cancel => None,
        }
    }

    pub fn host_dependent(self) -> bool {
        matches!(self, ParentKill::Watchdog)
    }
}

impl ExitClass {
    pub fn reason(self) -> Option<NotCheckedReason> {
        match self {
            ExitClass::Fault(kind) => Some(NotCheckedReason::Fault(kind)),
            ExitClass::WorkerMemoryCap => {
                Some(NotCheckedReason::LimitExceeded(LimitName::WorkerMemoryCap))
            }
            ExitClass::KilledByParent => None,
        }
    }

    pub fn host_dependent(self) -> bool {
        matches!(self, ExitClass::WorkerMemoryCap)
    }

    pub fn is_fault(self) -> bool {
        matches!(self, ExitClass::Fault(_))
    }
}

const NTSTATUS_ERROR: u32 = 0xC000_0000;

pub fn classify_exit(platform: Platform, exit: WorkerExit) -> ExitClass {
    if exit.killed_by_parent {
        return ExitClass::KilledByParent;
    }
    match (platform, exit.raw) {
        (_, RawExit::Signal(_)) => ExitClass::Fault(FaultKind::Crash),
        (_, RawExit::Code(WORKER_MEMORY_CAP_EXIT)) => ExitClass::WorkerMemoryCap,
        (_, RawExit::Code(PANIC_EXIT)) => ExitClass::Fault(FaultKind::Panic),
        (Platform::Windows, RawExit::Code(code)) if code as u32 >= NTSTATUS_ERROR => {
            ExitClass::Fault(FaultKind::Crash)
        }
        (_, RawExit::Code(_)) => ExitClass::Fault(FaultKind::Exited),
    }
}
