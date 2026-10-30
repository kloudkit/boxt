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
use crate::sorted::SortedVec;
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
    Definitions,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobFile {
    InInput(RelPath),
    WholeInput(RelPath),
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
            JobFile::InInput(path) | JobFile::WholeInput(path) => Some(path),
            JobFile::Setting(_) => None,
        }
    }

    pub fn is_well_formed(&self) -> bool {
        matches!(
            (self.kind, &self.file),
            (
                JobKind::Route | JobKind::Definitions,
                JobFile::InInput(_) | JobFile::WholeInput(_)
            ) | (JobKind::CompileDatabase, JobFile::Setting(_))
        ) && (self.kind == JobKind::Route || self.resume_after.is_none())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CollectorMsg {
    Settings(WorkerSettings),
    Macros {
        tree: u32,
        definitions: MacroTable,
        end: bool,
    },
    Job(Job),
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Variadic {
    Ellipsis,
    Named(Text),
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct MacroParams {
    pub names: Vec<Text>,
    pub variadic: Option<Variadic>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct MacroDef {
    pub name: Text,
    pub location: Location,
    pub params: Option<MacroParams>,
    pub body: Text,
}

pub type MacroTable = SortedVec<MacroDef>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MacroDefError {
    EmptyName,
    NotIdentifier,
    BadParameter,
    DuplicateParameter,
    NoLine,
    NoPath,
    InContainer,
    NotFileScope,
    OtherFile,
    TooLong { len: u64, max: u64 },
    Encode,
}

impl fmt::Display for MacroDefError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MacroDefError::EmptyName => f.write_str("a macro definition has an empty name"),
            MacroDefError::NotIdentifier => {
                f.write_str("a macro definition's name is not a C identifier")
            }
            MacroDefError::BadParameter => {
                f.write_str("a macro parameter's name is not a C identifier")
            }
            MacroDefError::DuplicateParameter => {
                f.write_str("a macro definition names one parameter twice")
            }
            MacroDefError::Encode => f.write_str("a macro definition could not be encoded"),
            MacroDefError::NoLine => f.write_str("a macro definition has no line, or line 0"),
            MacroDefError::NoPath => f.write_str("a macro definition has no path"),
            MacroDefError::InContainer => f.write_str("a macro definition sits in a container"),
            MacroDefError::NotFileScope => f.write_str("a macro definition's scope is not File"),
            MacroDefError::OtherFile => {
                f.write_str("a macro definition names a file other than its job's")
            }
            MacroDefError::TooLong { len, max } => {
                write!(
                    f,
                    "a macro definition of {len} bytes cannot fit a frame of {max}"
                )
            }
        }
    }
}

impl std::error::Error for MacroDefError {}

fn is_identifier(text: &Text) -> bool {
    let Some(name) = text.as_str() else {
        return false;
    };
    let mut chars = name.chars();
    let start = |c: char| {
        c == '_'
            || c == '$'
            || c.is_ascii_alphabetic()
            || !(c.is_ascii() || c.is_whitespace() || c.is_control())
    };
    chars.next().is_some_and(start) && chars.all(|c| start(c) || c.is_ascii_digit())
}

impl MacroParams {
    fn validate(&self) -> Result<(), MacroDefError> {
        let mut names: Vec<&Text> = self.names.iter().collect();
        if let Some(Variadic::Named(name)) = &self.variadic {
            names.push(name);
        }
        if !names
            .iter()
            .all(|name| is_identifier(name) && name.as_bytes() != b"__VA_ARGS__")
        {
            return Err(MacroDefError::BadParameter);
        }
        names.sort();
        if names.windows(2).any(|pair| pair[0] == pair[1]) {
            return Err(MacroDefError::DuplicateParameter);
        }
        Ok(())
    }
}

impl MacroDef {
    /// `max_len` is the `EntryMessageBytes` limit, as for `encode_macro_table`.
    pub fn validate(&self, job: &Job, max_len: u64) -> Result<(), MacroDefError> {
        if self.name.is_empty() {
            return Err(MacroDefError::EmptyName);
        }
        if !is_identifier(&self.name) {
            return Err(MacroDefError::NotIdentifier);
        }
        if let Some(params) = &self.params {
            params.validate()?;
        }
        if self.location.line.is_none_or(|line| line == 0) {
            return Err(MacroDefError::NoLine);
        }
        let Some(path) = self.location.path.as_ref() else {
            return Err(MacroDefError::NoPath);
        };
        if !self.location.frames.is_empty() {
            return Err(MacroDefError::InContainer);
        }
        if self.location.scope != Scope::File {
            return Err(MacroDefError::NotFileScope);
        }
        if job.kind != JobKind::Definitions
            || self.location.input != job.input
            || job.path() != Some(path)
        {
            return Err(MacroDefError::OtherFile);
        }
        let max = max_len.min(u64::from(u32::MAX));
        let len = definitions_overhead()
            .and_then(|overhead| Ok(overhead.saturating_add(envelope_len(self)?)))
            .map_err(|_| MacroDefError::Encode)?;
        if len > max {
            return Err(MacroDefError::TooLong { len, max });
        }
        Ok(())
    }
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
    PastEnd,
}

impl fmt::Display for RegionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            RegionError::LineZero => "a region starts on line 0; lines are 1-based",
            RegionError::Inverted => "a region ends before it starts",
            RegionError::PastEnd => "a region ends past the file's last line",
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

    pub fn fit_within(&self, lines: u64) -> Option<Region> {
        let last = u32::try_from(lines).unwrap_or(u32::MAX);
        (last > 0).then_some(Region {
            first_line: self.first_line.min(last),
            last_line: self.last_line.min(last),
            reason: self.reason,
        })
    }

    pub fn validate_within(&self, lines: u64) -> Result<(), RegionError> {
        if u64::from(self.last_line) > lines {
            return Err(RegionError::PastEnd);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Note {
    MissingSymbols,
    DroppedDefinitions(u64),
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
    Definitions {
        definitions: Vec<MacroDef>,
    },
    JobDone {
        outcome: Option<Outcome>,
    },
}

impl WorkerMsg {
    pub fn allowed_in(&self, kind: JobKind) -> bool {
        match self {
            WorkerMsg::JobDone { outcome } => outcome.is_some() || kind != JobKind::Definitions,
            WorkerMsg::CompileUnits { .. } => kind == JobKind::CompileDatabase,
            WorkerMsg::Definitions { .. } => kind == JobKind::Definitions,
            WorkerMsg::Notes { entry, notes } => match kind {
                JobKind::Route => !notes
                    .iter()
                    .any(|n| matches!(n, Note::DroppedDefinitions(_))),
                JobKind::Definitions => {
                    entry.is_job()
                        && notes
                            .iter()
                            .all(|n| matches!(n, Note::DroppedDefinitions(_)))
                }
                JobKind::CompileDatabase => false,
            },
            WorkerMsg::Opened { .. }
            | WorkerMsg::Facts { .. }
            | WorkerMsg::Regions { .. }
            | WorkerMsg::EntryDone { .. } => kind == JobKind::Route,
        }
    }

    pub fn entry(&self) -> Option<&EntryPath> {
        match self {
            WorkerMsg::Opened { entry, .. }
            | WorkerMsg::Facts { entry, .. }
            | WorkerMsg::Regions { entry, .. }
            | WorkerMsg::Notes { entry, .. }
            | WorkerMsg::EntryDone { entry, .. } => Some(entry),
            WorkerMsg::CompileUnits { .. }
            | WorkerMsg::Definitions { .. }
            | WorkerMsg::JobDone { .. } => None,
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
    pub fn at(input_index: u32, job_path: RelPath, entry: EntryPath) -> Self {
        CanonicalKey {
            input_index,
            job_path,
            entry,
            location: None,
        }
    }

    pub fn for_fact(job: &Job, entry: &EntryPath, fact: &Fact) -> Option<Self> {
        if job.kind != JobKind::Route {
            return None;
        }
        Some(CanonicalKey {
            input_index: job.input.index,
            job_path: job.path()?.clone(),
            entry: entry.clone(),
            location: Some(fact.location().clone()),
        })
    }

    pub fn for_outcome(job: &Job, entry: &EntryPath) -> Option<Self> {
        if job.kind != JobKind::Route {
            return None;
        }
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

    pub fn validate_regions(&self) -> Result<(), RegionError> {
        for region in &self.regions {
            region.validate()?;
            if let Some(count) = self.outcome.lines {
                region.validate_within(count.lines)?;
            }
        }
        Ok(())
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

fn envelope_len<T: Serialize>(msg: &T) -> Result<u64, FrameError> {
    serde_json::to_vec(msg)
        .map(|v| v.len() as u64)
        .map_err(|_| FrameError::Encode)
}

fn definitions_overhead() -> Result<u64, FrameError> {
    let worker = envelope_len(&WorkerMsg::Definitions {
        definitions: Vec::new(),
    })?;
    let collector = envelope_len(&CollectorMsg::Macros {
        tree: u32::MAX,
        definitions: MacroTable::new(),
        end: false,
    })?;
    Ok(worker.max(collector))
}

fn split_frames<T, M>(
    items: &[T],
    overhead: u64,
    max_len: u64,
    wrap: impl Fn(Vec<T>, bool) -> M,
) -> Result<Vec<Vec<u8>>, FrameError>
where
    T: Serialize + Clone,
    M: Serialize,
{
    let max_len = max_len.min(u64::from(u32::MAX));
    let mut chunks = Vec::new();
    let mut chunk: Vec<T> = Vec::new();
    let mut chunk_len = overhead;
    for item in items {
        let item_len = envelope_len(item)?;
        let added = item_len + u64::from(!chunk.is_empty());
        if !chunk.is_empty() && chunk_len + added > max_len {
            chunks.push(std::mem::take(&mut chunk));
            chunk_len = overhead;
        }
        chunk_len += item_len + u64::from(!chunk.is_empty());
        chunk.push(item.clone());
    }
    if !chunk.is_empty() || chunks.is_empty() {
        chunks.push(chunk);
    }
    let last = chunks.len() - 1;
    chunks
        .into_iter()
        .enumerate()
        .map(|(i, chunk)| encode_frame(&wrap(chunk, i == last), max_len))
        .collect()
}

pub fn encode_compile_units(
    units: &[CompileUnit],
    max_len: u64,
) -> Result<Vec<Vec<u8>>, FrameError> {
    let overhead = envelope_len(&WorkerMsg::CompileUnits {
        entries: Vec::new(),
    })?;
    split_frames(units, overhead, max_len, |entries, _| {
        WorkerMsg::CompileUnits { entries }
    })
}

pub fn encode_definitions(
    definitions: &[MacroDef],
    max_len: u64,
) -> Result<Vec<Vec<u8>>, FrameError> {
    let overhead = definitions_overhead()?;
    let max = max_len.min(u64::from(u32::MAX));
    for def in definitions {
        let len = overhead + envelope_len(def)?;
        if len > max {
            return Err(FrameError::TooLong { len, max });
        }
    }
    split_frames(definitions, overhead, max_len, |definitions, _| {
        WorkerMsg::Definitions { definitions }
    })
}

/// `max_len` is the `EntryMessageBytes` limit, the same value the collector passed to
/// `MacroDef::validate` for every definition in `table`, so no frame is refused.
pub fn encode_macro_table(
    tree: u32,
    table: &MacroTable,
    max_len: u64,
) -> Result<Vec<Vec<u8>>, FrameError> {
    let overhead = definitions_overhead()?;
    split_frames(table.as_slice(), overhead, max_len, |definitions, end| {
        CollectorMsg::Macros {
            tree,
            definitions: MacroTable::from(definitions),
            end,
        }
    })
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MergedTables {
    pub tables: Vec<(u32, MacroTable)>,
    pub cut: SortedVec<u32>,
}

impl MergedTables {
    pub fn table(&self, tree: u32) -> MacroTable {
        self.tables
            .binary_search_by_key(&tree, |(t, _)| *t)
            .map(|at| self.tables[at].1.clone())
            .unwrap_or_default()
    }
}

impl MacroDef {
    pub fn table_bytes(&self) -> u64 {
        let params = self.params.iter().flat_map(|p| {
            let variadic = match &p.variadic {
                Some(Variadic::Named(name)) => Some(name),
                _ => None,
            };
            p.names.iter().chain(variadic)
        });
        std::iter::once(&self.name)
            .chain(params)
            .chain(std::iter::once(&self.body))
            .fold(0u64, |sum, text| {
                sum.saturating_add(text.as_bytes().len() as u64)
            })
    }
}

pub fn merge_macro_tables(
    definitions: impl IntoIterator<Item = MacroDef>,
    max_bytes: u64,
) -> MergedTables {
    let mut definitions: Vec<MacroDef> = definitions.into_iter().collect();
    definitions.sort_by(|a, b| a.location.cmp(&b.location).then_with(|| a.cmp(b)));
    let same_file = |a: &MacroDef, b: &MacroDef| {
        a.location.input.index == b.location.input.index && a.location.path == b.location.path
    };
    let mut total = 0u64;
    let mut kept: Vec<MacroDef> = Vec::with_capacity(definitions.len());
    let mut cut = Vec::new();
    for file in definitions.chunk_by(same_file) {
        let size = file
            .iter()
            .fold(0u64, |sum, def| sum.saturating_add(def.table_bytes()));
        if cut.is_empty() {
            total = total.saturating_add(size);
        }
        if !cut.is_empty() || total > max_bytes {
            cut.push(file[0].location.input.index);
            continue;
        }
        kept.extend_from_slice(file);
    }
    let mut tables: Vec<(u32, MacroTable)> = Vec::new();
    for tree in kept.chunk_by(|a, b| a.location.input.index == b.location.input.index) {
        tables.push((
            tree[0].location.input.index,
            MacroTable::from(tree.to_vec()),
        ));
    }
    MergedTables {
        tables,
        cut: SortedVec::from({
            cut.dedup();
            cut
        }),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MacroTablesError {
    ForeignDefinition { tree: u32 },
}

impl fmt::Display for MacroTablesError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MacroTablesError::ForeignDefinition { tree } => write!(
                f,
                "a macro table for tree {tree} holds a definition from another input"
            ),
        }
    }
}

impl std::error::Error for MacroTablesError {}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct TreeTable {
    tree: u32,
    complete: Option<MacroTable>,
    pending: Option<Vec<MacroDef>>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MacroTables {
    trees: Vec<TreeTable>,
}

impl MacroTables {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn receive(
        &mut self,
        tree: u32,
        definitions: MacroTable,
        end: bool,
    ) -> Result<(), MacroTablesError> {
        if definitions
            .iter()
            .any(|def| def.location.input.index != tree)
        {
            return Err(MacroTablesError::ForeignDefinition { tree });
        }
        let at = match self.trees.binary_search_by_key(&tree, |t| t.tree) {
            Ok(at) => at,
            Err(at) => {
                self.trees.insert(
                    at,
                    TreeTable {
                        tree,
                        ..TreeTable::default()
                    },
                );
                at
            }
        };
        let slot = &mut self.trees[at];
        if slot.pending.is_none() {
            slot.complete = None;
        }
        slot.pending
            .get_or_insert_with(Vec::new)
            .extend(definitions.into_vec());
        if end {
            slot.complete = slot.pending.take().map(MacroTable::from);
        }
        Ok(())
    }

    pub fn for_tree(&self, tree: u32) -> Option<&MacroTable> {
        self.trees
            .binary_search_by_key(&tree, |t| t.tree)
            .ok()
            .and_then(|at| self.trees[at].complete.as_ref())
    }
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
            WorkerMsg::Regions { .. } | WorkerMsg::Notes { .. } | WorkerMsg::Definitions { .. } => {
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
