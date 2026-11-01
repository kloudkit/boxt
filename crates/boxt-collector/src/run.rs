use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fmt;
use std::io;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use boxt_core::SortedVec;
use boxt_core::compile_db::CompileDatabaseError;
use boxt_core::fact::{Fact, FileKind};
use boxt_core::limits::LimitName;
use boxt_core::location::{InputRef, Location, RelPath};
use boxt_core::message::{
    CanonicalKey, CollectedFile, EntryPath, Job, JobFile, JobKind, Note, encode_frame,
    encode_macro_table, faulting_entry, merge_macro_tables,
};
use boxt_core::outcome::{Details, FileOutcome, NotCheckedReason, Outcome};
use boxt_core::settings::Settings;

use crate::pool::{self, Phase, PoolConfig, PoolError, RunReport, default_jobs};
use crate::router::{ByName, by_name, wants_definitions};
use crate::walk::{ItemKind, WalkError, walk};
use crate::worker::WORKER_SUBCOMMAND;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileFacts {
    pub key: CanonicalKey,
    pub facts: Vec<Fact>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Collected {
    pub files: SortedVec<CollectedFile>,
    pub facts: Vec<FileFacts>,
    pub macro_tables_cut: SortedVec<u32>,
    pub host_dependent_trees: SortedVec<u32>,
}

impl Collected {
    pub fn digest(&self) -> u64 {
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        let mut feed = |frame: Result<Vec<u8>, boxt_core::message::FrameError>| {
            let bytes = frame.unwrap_or_else(|_| b"\xffunencodable".to_vec());
            for b in bytes {
                hash ^= u64::from(b);
                hash = hash.wrapping_mul(0x0100_0000_01b3);
            }
        };
        let max = u64::from(u32::MAX);
        feed(encode_frame(&self.macro_tables_cut, max));
        feed(encode_frame(&self.host_dependent_trees, max));
        for file in self.files.iter() {
            feed(encode_frame(file, max));
        }
        for facts in &self.facts {
            feed(encode_frame(&facts.key, max));
            for fact in &facts.facts {
                feed(encode_frame(fact, max));
            }
        }
        hash
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunStats {
    pub walked: u64,
    pub largest_file: u64,
    pub longest_job: Duration,
    pub elapsed: Duration,
    pub worker_peak_bytes: u64,
    pub macro_table_bytes: u64,
    pub most_entry_facts: u64,
    pub run_watchdog: bool,
}

#[derive(Debug, Clone)]
pub struct RunOptions {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    pub env: Vec<(OsString, OsString)>,
    pub measure: bool,
}

impl RunOptions {
    pub fn current() -> io::Result<Self> {
        Ok(RunOptions {
            program: std::env::current_exe()?,
            args: vec![OsString::from(WORKER_SUBCOMMAND)],
            env: Vec::new(),
            measure: false,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunError {
    NoInputs,
    Walk(WalkError),
    CompileDatabase(CompileDatabaseError),
    Pool(PoolError),
    Executable(io::ErrorKind),
    MacroTable,
    NothingRead,
}

impl fmt::Display for RunError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RunError::NoInputs => f.write_str("no input was given"),
            RunError::Walk(e) => e.fmt(f),
            RunError::CompileDatabase(e) => e.fmt(f),
            RunError::Pool(e) => e.fmt(f),
            RunError::Executable(kind) => write!(f, "cannot find its own executable: {kind}"),
            RunError::MacroTable => f.write_str("a macro table could not be encoded"),
            RunError::NothingRead => {
                f.write_str("nothing was read: no input held a file boxt reads")
            }
        }
    }
}

impl std::error::Error for RunError {}

impl From<WalkError> for RunError {
    fn from(e: WalkError) -> Self {
        RunError::Walk(e)
    }
}

impl From<PoolError> for RunError {
    fn from(e: PoolError) -> Self {
        RunError::Pool(e)
    }
}

impl From<CompileDatabaseError> for RunError {
    fn from(e: CompileDatabaseError) -> Self {
        RunError::CompileDatabase(e)
    }
}

pub fn run(settings: &Settings) -> Result<Collected, RunError> {
    let options = RunOptions::current().map_err(|e| RunError::Executable(e.kind()))?;
    run_with(settings, &options).map(|(collected, _)| collected)
}

struct Pending {
    input: InputRef,
    path: RelPath,
    entry: EntryPath,
    file_kind: Option<FileKind>,
}

fn kind_by_path(path: &RelPath) -> Option<FileKind> {
    (by_name(path.file_name()) == ByName::Listed).then_some(FileKind::Source)
}

fn aggregates(
    pending: &[Pending],
    reason: NotCheckedReason,
    host_dependent: bool,
) -> Vec<CollectedFile> {
    let mut groups: BTreeMap<(u32, Option<FileKind>), (usize, u64)> = BTreeMap::new();
    for (i, p) in pending.iter().enumerate() {
        let group = groups.entry((p.input.index, p.file_kind)).or_insert((i, 0));
        let earlier = &pending[group.0];
        if (&p.path, &p.entry) < (&earlier.path, &earlier.entry) {
            group.0 = i;
        }
        group.1 += 1;
    }
    groups
        .into_iter()
        .map(|((_, file_kind), (first, count))| {
            let p = &pending[first];
            let mut outcome = Outcome::new(
                FileOutcome::Aggregate {
                    reason,
                    count: Some(count),
                    at_least: count,
                },
                file_kind,
            );
            outcome.host_dependent = host_dependent;
            CollectedFile {
                key: CanonicalKey::at(p.input.index, p.path.clone(), p.entry.clone()),
                location: Location::file(p.input.clone(), Vec::new(), p.path.clone()),
                outcome,
                regions: Vec::new(),
                notes: Vec::new(),
            }
        })
        .collect()
}

fn pending_of(job: &Job, entry: EntryPath) -> Option<Pending> {
    let path = job.path()?.clone();
    Some(Pending {
        input: job.input.clone(),
        file_kind: kind_by_path(&path),
        path,
        entry,
    })
}

fn past(deadline: Option<Instant>) -> bool {
    deadline.is_some_and(|d| Instant::now() >= d)
}

fn note_stats(stats: &mut RunStats, report: &RunReport) {
    stats.longest_job = stats.longest_job.max(report.stats.longest_job);
    stats.worker_peak_bytes = stats.worker_peak_bytes.max(report.stats.worker_peak_bytes);
    stats.run_watchdog |= report.run_watchdog;
}

pub fn run_with(
    settings: &Settings,
    options: &RunOptions,
) -> Result<(Collected, RunStats), RunError> {
    let start = Instant::now();
    if settings.inputs.is_empty() {
        return Err(RunError::NoInputs);
    }
    let limits = &settings.limits;
    let deadline = start.checked_add(Duration::from_secs(limits.get(LimitName::RunWatchdog)));
    let max_len = limits.get(LimitName::EntryMessageBytes);
    let walked = walk(&settings.inputs)?;
    let config = PoolConfig {
        program: options.program.clone(),
        args: options.args.clone(),
        env: options.env.clone(),
        jobs: default_jobs(settings.jobs),
        settings: settings.worker(),
        measure: options.measure,
    };
    let mut stats = RunStats::default();

    if let Some(job) = settings.compile_database_job()? {
        let phase = Phase {
            tables: &[],
            deadline,
        };
        let report = pool::run_phase(&config, vec![job], phase)?;
        note_stats(&mut stats, &report);
        if !report.run_watchdog {
            CompileDatabaseError::check(report.jobs.first().map(|r| &r.outcome))?;
        }
    }
    let mut stopped = stats.run_watchdog || past(deadline);

    let mut planned = Vec::new();
    for input in walked {
        for item in input.items {
            planned.push((input.input.clone(), item));
        }
    }
    stats.walked = planned.len() as u64;
    stats.largest_file = planned
        .iter()
        .filter_map(|(_, item)| match item.kind {
            ItemKind::File { size } => Some(size),
            ItemKind::NotChecked(_) => None,
        })
        .max()
        .unwrap_or(0);

    let max_entries = usize::try_from(limits.get(LimitName::TotalEntries)).unwrap_or(usize::MAX);
    let cut = if planned.len() > max_entries {
        planned.split_off(max_entries)
    } else {
        Vec::new()
    };
    let cut: Vec<Pending> = cut
        .into_iter()
        .map(|(input, item)| Pending {
            file_kind: match item.kind {
                ItemKind::File { .. } => kind_by_path(item.path()),
                ItemKind::NotChecked(_) => None,
            },
            input,
            path: item.path().clone(),
            entry: EntryPath::job(),
        })
        .collect();
    let mut files = aggregates(
        &cut,
        NotCheckedReason::LimitExceeded(LimitName::TotalEntries),
        false,
    );

    let mut route_jobs = Vec::new();
    let mut definition_jobs = Vec::new();
    for (input, item) in planned {
        let path = item.path().clone();
        match item.kind {
            ItemKind::NotChecked(reason) => files.push(CollectedFile {
                key: CanonicalKey::at(input.index, path.clone(), EntryPath::job()),
                location: Location::file(input, Vec::new(), path),
                outcome: Outcome::new(
                    FileOutcome::NotChecked {
                        reason,
                        details: Details::None,
                    },
                    None,
                ),
                regions: Vec::new(),
                notes: Vec::new(),
            }),
            ItemKind::File { .. } => {
                let job = Job {
                    kind: JobKind::Route,
                    input,
                    file: item.file,
                    resume_after: None,
                };
                if wants_definitions(path.file_name()) {
                    definition_jobs.push(Job {
                        kind: JobKind::Definitions,
                        ..job.clone()
                    });
                }
                route_jobs.push(job);
            }
        }
    }

    let phase = Phase {
        tables: &[],
        deadline,
    };
    let defined = if stopped {
        RunReport::default()
    } else {
        pool::run_phase(&config, definition_jobs, phase)?
    };
    note_stats(&mut stats, &defined);
    stopped |= defined.run_watchdog || past(deadline);
    let mut host_trees = BTreeSet::new();
    let mut dropped: BTreeMap<(u32, JobFile), Vec<Note>> = BTreeMap::new();
    let mut definitions = Vec::new();
    for report in defined.jobs {
        if !report.finished {
            continue;
        }
        if report.ended_on_host {
            host_trees.insert(report.job.input.index);
        }
        if report.outcome.file != FileOutcome::Read {
            continue;
        }
        definitions.extend(report.definitions);
        if !report.notes.is_empty() {
            dropped.insert((report.job.input.index, report.job.file), report.notes);
        }
    }
    let merged = merge_macro_tables(definitions, limits.get(LimitName::MacroTableBytes));
    stats.macro_table_bytes = merged
        .tables
        .iter()
        .flat_map(|(_, table)| table.iter())
        .fold(0u64, |sum, def| sum.saturating_add(def.table_bytes()));
    let mut frames = Vec::new();
    for (tree, table) in &merged.tables {
        if table.is_empty() {
            continue;
        }
        frames.extend(encode_macro_table(*tree, table, max_len).map_err(|_| RunError::MacroTable)?);
    }

    let (reports, mut unfinished) = if stopped {
        stats.run_watchdog = true;
        let pending = route_jobs
            .iter()
            .filter_map(|job| pending_of(job, EntryPath::job()))
            .collect();
        (Vec::new(), pending)
    } else {
        let phase = Phase {
            tables: &frames,
            deadline,
        };
        let routed = pool::run_phase(&config, route_jobs.clone(), phase)?;
        note_stats(&mut stats, &routed);
        let reported: BTreeSet<usize> = routed.jobs.iter().map(|r| r.index).collect();
        let unfinished: Vec<Pending> = routed
            .unfinished
            .iter()
            .filter(|i| !reported.contains(i))
            .filter_map(|&i| pending_of(&route_jobs[i], EntryPath::job()))
            .collect();
        (routed.jobs, unfinished)
    };

    let mut facts = Vec::new();
    for report in reports {
        if !report.finished {
            let seen: Vec<EntryPath> = report.entries.iter().map(|c| c.entry.clone()).collect();
            let entry = faulting_entry(&seen, report.last_seen.as_ref());
            unfinished.extend(pending_of(&report.job, entry));
        }
        let host = host_trees.contains(&report.job.input.index);
        for committed in report.entries {
            let Some(key) = CanonicalKey::for_outcome(&report.job, &committed.entry) else {
                continue;
            };
            let mut outcome = committed.outcome;
            outcome.host_dependent |= host;
            let mut notes = committed.notes;
            if committed.entry.is_job()
                && let Some(more) =
                    dropped.remove(&(report.job.input.index, report.job.file.clone()))
            {
                notes.extend(more);
            }
            stats.most_entry_facts = stats.most_entry_facts.max(committed.facts.len() as u64);
            if !committed.facts.is_empty() {
                let mut held = committed.facts;
                held.sort_by(|a, b| a.location().cmp(b.location()));
                facts.push(FileFacts {
                    key: key.clone(),
                    facts: held,
                });
            }
            files.push(CollectedFile {
                key,
                location: committed.location,
                outcome,
                regions: committed.regions,
                notes,
            });
        }
    }
    files.extend(aggregates(
        &unfinished,
        NotCheckedReason::LimitExceeded(LimitName::RunWatchdog),
        true,
    ));

    if !files.iter().any(|f| f.outcome.file == FileOutcome::Read) {
        return Err(RunError::NothingRead);
    }
    facts.sort_by(|a, b| a.key.cmp(&b.key));
    stats.elapsed = start.elapsed();
    let collected = Collected {
        files: SortedVec::from(files),
        facts,
        macro_tables_cut: merged.cut,
        host_dependent_trees: host_trees.into_iter().collect(),
    };
    Ok((collected, stats))
}
