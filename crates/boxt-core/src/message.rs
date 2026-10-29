use std::fmt;
use std::io::{self, Read};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::compile_db::CompileUnit;
use crate::fact::Fact;
use crate::limits::{LimitName, Limits};
use crate::location::{InputRef, Location, RelPath};
use crate::outcome::{Details, FileOutcome, NotCheckedReason, Outcome};
use crate::settings::WorkerSettings;

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
pub struct Job {
    pub kind: JobKind,
    pub input: InputRef,
    pub path: RelPath,
    pub resume_after: Option<EntryPath>,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Note {
    MissingSymbols,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerMsg {
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
    },
    CompileUnits {
        entries: Vec<CompileUnit>,
    },
    JobDone {
        outcome: Option<Outcome>,
    },
}

impl WorkerMsg {
    pub fn opened(entry: EntryPath) -> Self {
        WorkerMsg::Facts {
            entry,
            facts: Vec::new(),
        }
    }

    pub fn entry(&self) -> Option<&EntryPath> {
        match self {
            WorkerMsg::Facts { entry, .. }
            | WorkerMsg::Regions { entry, .. }
            | WorkerMsg::Notes { entry, .. }
            | WorkerMsg::EntryDone { entry, .. } => Some(entry),
            WorkerMsg::CompileUnits { .. } | WorkerMsg::JobDone { .. } => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CanonicalKey {
    pub input_index: u32,
    pub job_path: RelPath,
    pub entry: EntryPath,
    pub location: Option<Location>,
}

impl CanonicalKey {
    pub fn for_fact(job: &Job, entry: &EntryPath, fact: &Fact) -> Self {
        CanonicalKey {
            input_index: job.input.index,
            job_path: job.path.clone(),
            entry: entry.clone(),
            location: Some(fact.location().clone()),
        }
    }

    pub fn for_outcome(job: &Job, entry: &EntryPath) -> Self {
        CanonicalKey {
            input_index: job.input.index,
            job_path: job.path.clone(),
            entry: entry.clone(),
            location: None,
        }
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
            WorkerMsg::EntryDone { .. }
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
