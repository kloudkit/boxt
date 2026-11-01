use std::cell::Cell;
use std::io::{self, Read, Seek};
use std::rc::Rc;

use crate::fact::{Fact, FileKind};
use crate::limits::{LimitName, Limits};
use crate::location::{FrameKind, Location, Scope, ScriptName};
use crate::message::{MacroTable, Note, Region};
use crate::outcome::{Details, FileOutcome, NotCheckedReason, SpecialKind};
use crate::settings::Encoding;
use crate::text::Text;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Exceeded {
    pub limit: LimitName,
    pub value: u64,
    pub max: u64,
}

impl std::fmt::Display for Exceeded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "limit {} exceeded: {} over {}",
            self.limit.flag(),
            self.value,
            self.max
        )
    }
}

impl std::error::Error for Exceeded {}

impl Exceeded {
    pub fn from_io(error: &io::Error) -> Option<Exceeded> {
        error.get_ref()?.downcast_ref::<Exceeded>().copied()
    }
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
    max_syntax_depth: u64,
    work: u64,
    bytes: u64,
    depth: u64,
    peak_bytes: u64,
    deepest_syntax: Cell<u64>,
    job: Rc<JobCounters>,
}

#[derive(Debug, PartialEq, Eq)]
struct JobCounters {
    max_entries: u64,
    max_decompressed: u64,
    entries: Cell<u64>,
    decompressed: Cell<u64>,
}

fn charge_cell(limit: LimitName, total: &Cell<u64>, add: u64, max: u64) -> Result<(), Exceeded> {
    let mut value = total.get();
    let result = charge(limit, &mut value, add, max);
    total.set(value);
    result
}

pub const RATIO_FLOOR: u64 = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decompression {
    max_ratio: u64,
    compressed: u64,
    produced: u64,
    job: Rc<JobCounters>,
}

impl Decompression {
    pub fn consumed(&mut self, compressed: u64) {
        self.compressed = self.compressed.saturating_add(compressed);
    }

    pub fn set_consumed(&mut self, compressed: u64) {
        self.compressed = compressed;
    }

    pub fn produced(&mut self, bytes: u64) -> Result<(), Exceeded> {
        charge_cell(
            LimitName::TotalDecompressedBytes,
            &self.job.decompressed,
            bytes,
            self.job.max_decompressed,
        )?;
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
            max_syntax_depth: limits.get(LimitName::SyntaxDepth),
            work: 0,
            bytes: 0,
            depth: 0,
            peak_bytes: 0,
            deepest_syntax: Cell::new(0),
            job: Rc::new(JobCounters {
                max_entries: limits.get(LimitName::TotalEntries),
                max_decompressed: limits.get(LimitName::TotalDecompressedBytes),
                entries: Cell::new(0),
                decompressed: Cell::new(0),
            }),
        }
    }

    pub fn decompressed(&self) -> u64 {
        self.job.decompressed.get()
    }

    pub fn sibling(&self) -> WorkBudget {
        WorkBudget {
            work: 0,
            bytes: 0,
            peak_bytes: 0,
            deepest_syntax: Cell::new(0),
            job: Rc::clone(&self.job),
            ..*self
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
            job: Rc::clone(&self.job),
        }
    }

    pub fn entry(&mut self) -> Result<(), Exceeded> {
        charge_cell(
            LimitName::TotalEntries,
            &self.job.entries,
            1,
            self.job.max_entries,
        )
    }

    pub fn entries(&self) -> u64 {
        self.job.entries.get()
    }

    pub fn work(&mut self, steps: u64) -> Result<(), Exceeded> {
        charge(LimitName::Work, &mut self.work, steps, self.max_work)
    }

    pub fn max_syntax_depth(&self) -> u64 {
        self.max_syntax_depth
    }

    pub fn syntax_depth(&self, depth: u64) -> Result<(), Exceeded> {
        self.deepest_syntax
            .set(self.deepest_syntax.get().max(depth));
        if depth > self.max_syntax_depth {
            return Err(Exceeded {
                limit: LimitName::SyntaxDepth,
                value: depth,
                max: self.max_syntax_depth,
            });
        }
        Ok(())
    }

    pub fn buffer(&mut self, bytes: u64) -> Result<(), Exceeded> {
        let charged = charge(LimitName::Memory, &mut self.bytes, bytes, self.max_bytes);
        self.peak_bytes = self.peak_bytes.max(self.bytes);
        charged
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
            peak_bytes: 0,
            deepest_syntax: Cell::new(0),
            job: Rc::clone(&self.job),
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

    pub fn peak_buffered(&self) -> u64 {
        self.peak_bytes
    }

    pub fn deepest_syntax(&self) -> u64 {
        self.deepest_syntax.get()
    }
}

pub struct ReadCtx<'m> {
    pub location: Location,
    pub encoding: Option<Encoding>,
    budget: WorkBudget,
    macros: Option<&'m MacroTable>,
}

impl ReadCtx<'static> {
    pub fn new(location: Location, limits: &Limits, encoding: Option<Encoding>) -> Self {
        ReadCtx {
            location,
            encoding,
            budget: WorkBudget::new(limits),
            macros: None,
        }
    }

    pub fn with_budget(location: Location, encoding: Option<Encoding>, budget: WorkBudget) -> Self {
        ReadCtx {
            location,
            encoding,
            budget,
            macros: None,
        }
    }
}

impl<'m> ReadCtx<'m> {
    pub fn with_macros<'t>(self, macros: &'t MacroTable) -> ReadCtx<'t> {
        ReadCtx {
            location: self.location,
            encoding: self.encoding,
            budget: self.budget,
            macros: Some(macros),
        }
    }

    pub fn macros(&self) -> Option<&'m MacroTable> {
        self.macros
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

    pub fn syntax_depth(&self, depth: u64) -> Result<(), Exceeded> {
        self.budget.syntax_depth(depth)
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

pub type ByteCount = Rc<Cell<u64>>;

pub struct CountedRead<R> {
    inner: R,
    count: ByteCount,
}

impl<R: Read> CountedRead<R> {
    pub fn new(inner: R) -> Self {
        CountedRead {
            inner,
            count: ByteCount::default(),
        }
    }

    pub fn count(&self) -> ByteCount {
        Rc::clone(&self.count)
    }
}

impl<R: Read> Read for CountedRead<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.count.set(self.count.get().saturating_add(n as u64));
        Ok(n)
    }
}

pub struct ChargedRead<R> {
    inner: R,
    stream: Decompression,
    compressed: ByteCount,
    exceeded: Option<Exceeded>,
}

impl<R: Read> ChargedRead<R> {
    pub fn new(inner: R, stream: Decompression, compressed: ByteCount) -> Self {
        ChargedRead {
            inner,
            stream,
            compressed,
            exceeded: None,
        }
    }

    pub fn exceeded(&self) -> Option<Exceeded> {
        self.exceeded
    }

    pub fn stream(&self) -> &Decompression {
        &self.stream
    }
}

impl<R: Read> Read for ChargedRead<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if let Some(e) = self.exceeded {
            return Err(io::Error::other(e));
        }
        let n = self.inner.read(buf)?;
        self.stream.set_consumed(self.compressed.get());
        if let Err(e) = self.stream.produced(n as u64) {
            self.exceeded = Some(e);
            return Err(io::Error::other(e));
        }
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::location::{InputRef, RelPath};

    fn ctx(limits: &Limits) -> ReadCtx<'static> {
        let location = Location::file(
            InputRef {
                index: 0,
                given: Text::from("/src"),
            },
            Vec::new(),
            RelPath::try_from("a.c").unwrap(),
        );
        ReadCtx::new(location, limits, None)
    }

    #[test]
    fn a_macro_table_is_borrowed_and_optional() {
        let limits = Limits::default();
        assert!(ctx(&limits).macros().is_none());
        let table = MacroTable::new();
        let with = ctx(&limits).with_macros(&table);
        assert!(std::ptr::eq(with.macros().unwrap(), &table));
    }

    #[test]
    fn syntax_depth_is_a_maximum_not_a_total() {
        let limits = Limits::default().with(LimitName::SyntaxDepth, 10).unwrap();
        let ctx = ctx(&limits);
        assert_eq!(ctx.budget().max_syntax_depth(), 10);
        assert_eq!(ctx.syntax_depth(10), Ok(()));
        assert_eq!(ctx.syntax_depth(10), Ok(()));
        let over = ctx.syntax_depth(11).unwrap_err();
        assert_eq!(
            over,
            Exceeded {
                limit: LimitName::SyntaxDepth,
                value: 11,
                max: 10
            }
        );
        assert_eq!(
            NotRead::from(over).outcome(),
            FileOutcome::NotChecked {
                reason: NotCheckedReason::LimitExceeded(LimitName::SyntaxDepth),
                details: Details::Measured {
                    value: 11,
                    limit: 10
                }
            }
        );
    }
}
