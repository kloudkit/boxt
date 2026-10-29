use std::io::{Read, Seek};

use crate::fact::{Fact, FileKind};
use crate::limits::{LimitName, Limits};
use crate::location::{FrameKind, Location, Scope, ScriptName};
use crate::message::{Note, Region};
use crate::outcome::{Details, FileOutcome, NotCheckedReason, SpecialKind};
use crate::settings::WorkerSettings;
use crate::text::Text;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Exceeded {
    pub limit: LimitName,
    pub value: u64,
    pub max: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NotRead {
    pub reason: NotCheckedReason,
    pub details: Details,
}

impl NotRead {
    pub fn new(reason: NotCheckedReason) -> Self {
        NotRead {
            reason,
            details: Details::None,
        }
    }

    pub fn outcome(self) -> FileOutcome {
        FileOutcome::NotChecked {
            reason: self.reason,
            details: self.details,
        }
    }
}

impl From<Exceeded> for NotRead {
    fn from(e: Exceeded) -> Self {
        NotRead {
            reason: NotCheckedReason::LimitExceeded(e.limit),
            details: Details::Measured {
                value: e.value,
                limit: e.max,
            },
        }
    }
}

pub type ReadResult<T = ()> = Result<T, NotRead>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkBudget {
    max_work: u64,
    max_bytes: u64,
    max_depth: u64,
    max_file_size: u64,
    max_ratio: u64,
    max_entries: u64,
    work: u64,
    bytes: u64,
    depth: u64,
    entries: u64,
}

pub const RATIO_FLOOR: u64 = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decompression {
    max_ratio: u64,
    compressed: u64,
    produced: u64,
}

impl Decompression {
    pub fn consumed(&mut self, compressed: u64) {
        self.compressed = self.compressed.saturating_add(compressed);
    }

    pub fn produced(&mut self, bytes: u64) -> Result<(), Exceeded> {
        self.produced = self.produced.saturating_add(bytes);
        if self.produced < RATIO_FLOOR {
            return Ok(());
        }
        if self.produced > self.compressed.saturating_mul(self.max_ratio) {
            return Err(Exceeded {
                limit: LimitName::DecompressionRatio,
                value: self.produced / self.compressed.max(1),
                max: self.max_ratio,
            });
        }
        Ok(())
    }

    pub fn ratio(&self) -> u64 {
        self.produced / self.compressed.max(1)
    }
}

fn charge(limit: LimitName, total: &mut u64, add: u64, max: u64) -> Result<(), Exceeded> {
    *total = total.saturating_add(add);
    if *total > max {
        return Err(Exceeded {
            limit,
            value: *total,
            max,
        });
    }
    Ok(())
}

impl WorkBudget {
    pub fn new(limits: &Limits) -> Self {
        WorkBudget {
            max_work: limits.get(LimitName::Work),
            max_bytes: limits.get(LimitName::Memory),
            max_depth: limits.get(LimitName::ArchiveDepth),
            max_file_size: limits.get(LimitName::FileSize),
            max_ratio: limits.get(LimitName::DecompressionRatio),
            max_entries: limits.get(LimitName::TotalEntries),
            work: 0,
            bytes: 0,
            depth: 0,
            entries: 0,
        }
    }

    pub fn file_size(&self, size: u64) -> Result<(), Exceeded> {
        if size > self.max_file_size {
            return Err(Exceeded {
                limit: LimitName::FileSize,
                value: size,
                max: self.max_file_size,
            });
        }
        Ok(())
    }

    pub fn decompression(&self) -> Decompression {
        Decompression {
            max_ratio: self.max_ratio,
            compressed: 0,
            produced: 0,
        }
    }

    pub fn entry(&mut self) -> Result<(), Exceeded> {
        charge(
            LimitName::TotalEntries,
            &mut self.entries,
            1,
            self.max_entries,
        )
    }

    pub fn absorb_entries(&mut self, child: &WorkBudget) {
        self.entries = self.entries.max(child.entries);
    }

    pub fn entries(&self) -> u64 {
        self.entries
    }

    pub fn work(&mut self, steps: u64) -> Result<(), Exceeded> {
        charge(LimitName::Work, &mut self.work, steps, self.max_work)
    }

    pub fn buffer(&mut self, bytes: u64) -> Result<(), Exceeded> {
        charge(LimitName::Memory, &mut self.bytes, bytes, self.max_bytes)
    }

    pub fn release(&mut self, bytes: u64) {
        self.bytes = self.bytes.saturating_sub(bytes);
    }

    pub fn nested(&self) -> Result<WorkBudget, Exceeded> {
        let mut depth = self.depth;
        charge(LimitName::ArchiveDepth, &mut depth, 1, self.max_depth)?;
        Ok(WorkBudget {
            work: 0,
            bytes: 0,
            depth,
            ..*self
        })
    }

    pub fn depth(&self) -> u64 {
        self.depth
    }

    pub fn work_done(&self) -> u64 {
        self.work
    }

    pub fn buffered(&self) -> u64 {
        self.bytes
    }
}

pub struct ReadCtx<'a> {
    pub location: Location,
    pub settings: &'a WorkerSettings,
    budget: WorkBudget,
}

impl<'a> ReadCtx<'a> {
    pub fn new(location: Location, settings: &'a WorkerSettings) -> Self {
        ReadCtx {
            location,
            settings,
            budget: WorkBudget::new(&settings.limits),
        }
    }

    pub fn with_budget(
        location: Location,
        settings: &'a WorkerSettings,
        budget: WorkBudget,
    ) -> Self {
        ReadCtx {
            location,
            settings,
            budget,
        }
    }

    pub fn budget(&self) -> &WorkBudget {
        &self.budget
    }

    pub fn into_budget(self) -> WorkBudget {
        self.budget
    }

    pub fn work(&mut self, steps: u64) -> Result<(), Exceeded> {
        self.budget.work(steps)
    }

    pub fn buffer(&mut self, bytes: u64) -> Result<(), Exceeded> {
        self.budget.buffer(bytes)
    }

    pub fn release(&mut self, bytes: u64) {
        self.budget.release(bytes);
    }

    pub fn file_size(&self, size: u64) -> Result<(), Exceeded> {
        self.budget.file_size(size)
    }

    pub fn decompression(&self) -> Decompression {
        self.budget.decompression()
    }

    pub fn entry(&mut self) -> Result<(), Exceeded> {
        self.budget.entry()
    }

    pub fn at(&self, line: Option<u32>, scope: Scope) -> Location {
        self.location.at(line, scope)
    }
}

pub trait FactSink {
    fn fact(&mut self, fact: Fact) -> Result<(), Exceeded>;
    fn region(&mut self, region: Region) -> Result<(), Exceeded>;
    fn note(&mut self, note: Note) -> Result<(), Exceeded>;
}

pub trait Reader {
    fn file_kind(&self) -> FileKind;
    fn read(&self, bytes: &[u8], ctx: &mut ReadCtx<'_>, sink: &mut dyn FactSink) -> ReadResult;
}

pub trait ReadSeek: Read + Seek {}

impl<T: Read + Seek + ?Sized> ReadSeek for T {}

pub enum Input<'a> {
    Stream(&'a mut dyn Read),
    Seekable(&'a mut dyn ReadSeek),
}

impl Input<'_> {
    pub fn reader(&mut self) -> &mut dyn Read {
        match self {
            Input::Stream(r) => *r,
            Input::Seekable(r) => *r,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum EntryName {
    Path(Text),
    Script {
        name: ScriptName,
        interpreter: Option<Text>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum EntryKind {
    File,
    Directory,
    Link,
    Special(SpecialKind),
}

pub struct Entry<'e> {
    pub name: EntryName,
    pub kind: EntryKind,
    pub size: Option<u64>,
    pub data: Input<'e>,
}

pub trait ContainerReader {
    fn frame_kind(&self) -> FrameKind;
    fn open<'a>(
        &self,
        input: Input<'a>,
        ctx: &mut ReadCtx<'_>,
    ) -> ReadResult<Box<dyn Container + 'a>>;
}

pub trait Container {
    fn name(&self) -> Option<Text>;
    fn own_facts(&mut self, ctx: &mut ReadCtx<'_>, sink: &mut dyn FactSink) -> ReadResult;
    fn next_entry(&mut self, ctx: &mut ReadCtx<'_>) -> ReadResult<Option<Entry<'_>>>;
}
