use std::io;
use std::num::NonZeroUsize;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;

use boxt_core::compile_db::extension_class;
use boxt_core::fact::Fact;
use boxt_core::limits::Limits;
use boxt_core::location::Location;
use boxt_core::message::{Job, JobKind, Note, Region};
use boxt_core::outcome::line_count;
use boxt_core::read::{Exceeded, FactSink, ReadCtx, Reader};
use boxt_read_cpp::{CppReader, Grammar};

use crate::path::{job_path, text_from_path};
use crate::walk::{ItemKind, walk};

pub struct Walked {
    pub jobs: Vec<Job>,
    pub skipped: usize,
}

pub fn cxx_jobs(root: &Path) -> io::Result<Walked> {
    let walked = walk(&[text_from_path(root)]).map_err(io::Error::other)?;
    let mut jobs = Vec::new();
    let mut skipped = 0;
    for input in walked {
        for item in input.items {
            match item.kind {
                ItemKind::File { .. } if extension_class(item.path().file_name()).is_some() => {
                    jobs.push(Job {
                        kind: JobKind::Route,
                        input: input.input.clone(),
                        file: item.file,
                        resume_after: None,
                    });
                }
                ItemKind::File { .. } => {}
                ItemKind::NotChecked(_) => skipped += 1,
            }
        }
    }
    Ok(Walked { jobs, skipped })
}

pub fn parse_in_process(jobs: &[Job], threads: NonZeroUsize) -> io::Result<usize> {
    let next = AtomicUsize::new(0);
    let read = AtomicUsize::new(0);
    thread::scope(|scope| {
        let workers: Vec<_> = (0..threads.get())
            .map(|_| {
                scope.spawn(|| -> io::Result<()> {
                    while let Some(job) = jobs.get(next.fetch_add(1, Ordering::Relaxed)) {
                        if parse_one(job)? {
                            read.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                    Ok(())
                })
            })
            .collect();
        workers.into_iter().try_for_each(|w| {
            w.join()
                .unwrap_or_else(|_| Err(io::Error::other("a parse thread panicked")))
        })
    })?;
    Ok(read.into_inner())
}

struct Discard;

impl FactSink for Discard {
    fn fact(&mut self, fact: Fact) -> Result<(), Exceeded> {
        std::hint::black_box(fact);
        Ok(())
    }

    fn region(&mut self, region: Region) -> Result<(), Exceeded> {
        std::hint::black_box(region);
        Ok(())
    }

    fn note(&mut self, _: Note) -> Result<(), Exceeded> {
        Ok(())
    }
}

fn parse_one(job: &Job) -> io::Result<bool> {
    let Some(path) = job_path(job) else {
        return Ok(false);
    };
    let Some(rel) = job
        .path()
        .filter(|p| Grammar::for_file_name(p.file_name()).is_some())
    else {
        return Ok(false);
    };
    let bytes = std::fs::read(path)?;
    std::hint::black_box(line_count(&bytes, None));
    let location = Location::file(job.input.clone(), Vec::new(), rel.clone());
    let mut ctx = ReadCtx::new(location, &Limits::default(), None);
    Ok(CppReader.read(&bytes, &mut ctx, &mut Discard).is_ok())
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ReadPeaks {
    pub candidates: u64,
    pub files: u64,
    pub not_cxx: u64,
    pub not_fetched: u64,
    pub not_read: u64,
    pub largest: u64,
    pub work: u64,
    pub memory: u64,
    pub memory_per_kib: u64,
    pub syntax_depth: u64,
    pub facts: u64,
    pub over_file_size: u64,
    pub over_work: u64,
    pub over_memory: u64,
    pub over_syntax_depth: u64,
    pub over_entry_facts: u64,
}

impl ReadPeaks {
    fn merge(&mut self, other: ReadPeaks) {
        self.candidates += other.candidates;
        self.files += other.files;
        self.not_cxx += other.not_cxx;
        self.not_fetched += other.not_fetched;
        self.not_read += other.not_read;
        self.largest = self.largest.max(other.largest);
        self.work = self.work.max(other.work);
        self.memory = self.memory.max(other.memory);
        self.memory_per_kib = self.memory_per_kib.max(other.memory_per_kib);
        self.syntax_depth = self.syntax_depth.max(other.syntax_depth);
        self.facts = self.facts.max(other.facts);
        self.over_file_size += other.over_file_size;
        self.over_work += other.over_work;
        self.over_memory += other.over_memory;
        self.over_syntax_depth += other.over_syntax_depth;
        self.over_entry_facts += other.over_entry_facts;
    }
}

struct Count(u64);

impl FactSink for Count {
    fn fact(&mut self, _: Fact) -> Result<(), Exceeded> {
        self.0 += 1;
        Ok(())
    }

    fn region(&mut self, _: Region) -> Result<(), Exceeded> {
        Ok(())
    }

    fn note(&mut self, _: Note) -> Result<(), Exceeded> {
        Ok(())
    }
}

const RATIO_FLOOR: u64 = 64 * 1024;

/// Reads every routed C/C++ candidate of `inputs` in this process exactly as the worker
/// charges it (the content check and the read share one `WorkBudget`, with the tree's merged
/// macro table attached), with `limits` raised so nothing is cut, and counts the files each
/// `shipped` default would cut. Every candidate is counted: files the content check rejects,
/// files that cannot be fetched and files the reader does not read are reported, not
/// dropped. `memory_per_kib` is the largest peak counted memory per KiB of file, over files
/// of at least 64 KiB.
pub fn measure_reads(
    inputs: &[boxt_core::Text],
    limits: &Limits,
    shipped: &Limits,
    threads: NonZeroUsize,
) -> io::Result<ReadPeaks> {
    use boxt_core::limits::LimitName;
    use boxt_core::message::{MacroTable, merge_macro_tables};

    use crate::router::{ByName, Fetched, Routed, by_name, fetch, route_content};

    let walked = walk(inputs).map_err(io::Error::other)?;
    let mut files = Vec::new();
    for input in walked {
        for item in input.items {
            if matches!(item.kind, ItemKind::File { .. })
                && by_name(item.path().file_name()) != ByName::Other
            {
                files.push(Job {
                    kind: JobKind::Route,
                    input: input.input.clone(),
                    file: item.file,
                    resume_after: None,
                });
            }
        }
    }
    let max_size = limits.get(LimitName::FileSize);
    let fetch_job = |job: &Job| -> Option<(Vec<u8>, Location)> {
        let path = job_path(job)?;
        let Fetched::Bytes(bytes) = fetch(&path, max_size, None) else {
            return None;
        };
        let rel = job.path()?.clone();
        Some((bytes, Location::file(job.input.clone(), Vec::new(), rel)))
    };
    let parallel = |work: &(dyn Fn(&Job, &mut ReadPeaks) + Sync)| -> ReadPeaks {
        let next = AtomicUsize::new(0);
        let total = std::sync::Mutex::new(ReadPeaks::default());
        thread::scope(|scope| {
            for _ in 0..threads.get() {
                scope.spawn(|| {
                    let mut mine = ReadPeaks::default();
                    while let Some(job) = files.get(next.fetch_add(1, Ordering::Relaxed)) {
                        work(job, &mut mine);
                    }
                    total
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .merge(mine);
                });
            }
        });
        total
            .into_inner()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    };
    let defs = std::sync::Mutex::new(Vec::new());
    parallel(&|job, _| {
        let Some((bytes, location)) = fetch_job(job) else {
            return;
        };
        let mut ctx = ReadCtx::new(location.clone(), limits, None);
        let name = by_name(location.path.as_ref().map_or(&[][..], |p| p.file_name()));
        if !matches!(route_content(&bytes, &mut ctx, name), Routed::Cxx) {
            return;
        }
        if let Ok(found) = boxt_read_cpp::definitions(&bytes, None, &location) {
            defs.lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .extend(found);
        }
    });
    let defs = defs
        .into_inner()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let merged = merge_macro_tables(defs, limits.get(LimitName::MacroTableBytes));
    let tables: Vec<(u32, MacroTable)> = merged.tables.clone();
    Ok(parallel(&|job, mine| {
        mine.candidates += 1;
        let Some((bytes, location)) = fetch_job(job) else {
            mine.not_fetched += 1;
            return;
        };
        let size = bytes.len() as u64;
        let name = by_name(location.path.as_ref().map_or(&[][..], |p| p.file_name()));
        let mut ctx = ReadCtx::new(location, limits, None);
        if !matches!(route_content(&bytes, &mut ctx, name), Routed::Cxx) {
            mine.not_cxx += 1;
            return;
        }
        let empty = MacroTable::new();
        let table = tables
            .binary_search_by_key(&job.input.index, |(t, _)| *t)
            .map_or(&empty, |at| &tables[at].1);
        let mut ctx = ctx.with_macros(table);
        let mut count = Count(0);
        if CppReader.read(&bytes, &mut ctx, &mut count).is_err() {
            mine.not_read += 1;
        }
        let budget = ctx.budget();
        mine.files += 1;
        mine.largest = mine.largest.max(size);
        mine.work = mine.work.max(budget.work_done());
        mine.memory = mine.memory.max(budget.peak_buffered());
        if size >= RATIO_FLOOR {
            mine.memory_per_kib = mine
                .memory_per_kib
                .max(budget.peak_buffered() * 1024 / size);
        }
        mine.syntax_depth = mine.syntax_depth.max(budget.deepest_syntax());
        mine.facts = mine.facts.max(count.0);
        mine.over_file_size += u64::from(size > shipped.get(LimitName::FileSize));
        mine.over_work += u64::from(budget.work_done() > shipped.get(LimitName::Work));
        mine.over_memory += u64::from(budget.peak_buffered() > shipped.get(LimitName::Memory));
        mine.over_syntax_depth +=
            u64::from(budget.deepest_syntax() > shipped.get(LimitName::SyntaxDepth));
        mine.over_entry_facts += u64::from(count.0 > shipped.get(LimitName::EntryFacts));
    }))
}
