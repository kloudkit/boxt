use std::collections::VecDeque;
use std::ffi::OsString;
use std::fmt;
use std::io::{self, BufReader, Write};
use std::num::{NonZeroU32, NonZeroUsize};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::sync::{Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use boxt_core::fact::{Fact, FileKind};
use boxt_core::limits::{LimitError, LimitName, Limits, MEMORY_CAP_FACTOR};
use boxt_core::location::Location;
use boxt_core::message::{
    CollectorMsg, Decoded, EntryBudget, EntryPath, ExitClass, FrameError, Job, JobKind, MacroDef,
    Note, ParentKill, Platform, RawExit, Region, WorkerExit, WorkerMsg, classify_exit,
    decode_frame, encode_frame, fault_scope, faulting_entry, resume_skips,
};
use boxt_core::outcome::{FaultKind, FileOutcome, NotCheckedReason, Outcome};
use boxt_core::settings::WorkerSettings;

use crate::os;
use crate::router::file_kind_by_name;
use crate::worker::{PARENT_ENV, STARTUP_CAP_EXIT, STARTUP_PROTOCOL_EXIT, WORKER_SUBCOMMAND};

pub const MAX_FAILED_SPAWNS: u32 = 3;

const SPAWN_BACKOFF: Duration = Duration::from_millis(50);

const REAP_GRACE: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PoolError {
    SpawnFailed { attempts: u32, cause: String },
    WorkerMemoryCap,
    KillOnExit(io::ErrorKind),
}

impl fmt::Display for PoolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PoolError::SpawnFailed { attempts, cause } => {
                write!(
                    f,
                    "a worker failed to start {attempts} times in a row: {cause}"
                )
            }
            PoolError::WorkerMemoryCap => f.write_str("a worker could not set its memory cap"),
            PoolError::KillOnExit(kind) => {
                write!(f, "cannot make the workers die with this process: {kind}")
            }
        }
    }
}

impl std::error::Error for PoolError {}

#[derive(Debug, Clone)]
pub struct PoolConfig {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    pub env: Vec<(OsString, OsString)>,
    pub jobs: NonZeroUsize,
    pub settings: WorkerSettings,
    pub measure: bool,
}

pub fn default_jobs(jobs: Option<NonZeroU32>) -> NonZeroUsize {
    match jobs {
        Some(n) => NonZeroUsize::new(n.get() as usize).unwrap_or(NonZeroUsize::MIN),
        None => thread::available_parallelism().unwrap_or(NonZeroUsize::MIN),
    }
}

impl PoolConfig {
    pub fn new(settings: WorkerSettings, jobs: Option<NonZeroU32>) -> io::Result<Self> {
        Ok(PoolConfig {
            program: std::env::current_exe()?,
            args: vec![OsString::from(WORKER_SUBCOMMAND)],
            env: Vec::new(),
            jobs: default_jobs(jobs),
            settings,
            measure: false,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Committed {
    pub entry: EntryPath,
    pub location: Location,
    pub outcome: Outcome,
    pub facts: Vec<Fact>,
    pub regions: Vec<Region>,
    pub notes: Vec<Note>,
}

impl Committed {
    fn bare(entry: EntryPath, location: Location, outcome: Outcome) -> Self {
        Committed {
            entry,
            location,
            outcome,
            facts: Vec::new(),
            regions: Vec::new(),
            notes: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobReport {
    pub index: usize,
    pub job: Job,
    pub outcome: Outcome,
    pub entries: Vec<Committed>,
    pub definitions: Vec<MacroDef>,
    pub notes: Vec<Note>,
    pub attempts: u32,
    pub ended_on_host: bool,
    pub elapsed: Duration,
    pub finished: bool,
    pub last_seen: Option<EntryPath>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PoolStats {
    pub longest_job: Duration,
    pub worker_peak_bytes: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunReport {
    pub jobs: Vec<JobReport>,
    pub unfinished: Vec<usize>,
    pub spawns: u32,
    pub run_watchdog: bool,
    pub stats: PoolStats,
}

impl RunReport {
    pub fn read_count(&self) -> usize {
        self.jobs
            .iter()
            .filter(|r| r.outcome.file == FileOutcome::Read)
            .count()
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Phase<'a> {
    pub tables: &'a [Vec<u8>],
    pub deadline: Option<Instant>,
}

pub fn default_settings() -> WorkerSettings {
    WorkerSettings {
        limits: Limits::default(),
        encodings: Vec::new(),
    }
}

pub fn settings_with_memory_cap(cap: u64) -> Result<WorkerSettings, LimitError> {
    let memory = (cap / MEMORY_CAP_FACTOR).min(LimitName::Memory.default_value());
    let table = cap.min(LimitName::MacroTableBytes.default_value());
    let limits = Limits::default()
        .with(LimitName::Memory, memory)?
        .with(LimitName::MacroTableBytes, table)?
        .with(LimitName::WorkerMemoryCap, cap)?;
    limits.validate()?;
    Ok(WorkerSettings {
        limits,
        encodings: Vec::new(),
    })
}

pub fn run(config: &PoolConfig, jobs: Vec<Job>) -> Result<RunReport, PoolError> {
    run_phase(config, jobs, Phase::default())
}

pub fn run_phase(
    config: &PoolConfig,
    jobs: Vec<Job>,
    phase: Phase<'_>,
) -> Result<RunReport, PoolError> {
    os::suppress_error_dialogs();
    os::kill_workers_on_exit().map_err(|e| PoolError::KillOnExit(e.kind()))?;
    let count = jobs.len();
    let shared = Shared {
        config,
        phase,
        queue: Mutex::new(jobs.into_iter().enumerate().collect()),
        results: Mutex::new(vec![None; count]),
        failed_spawns: AtomicU32::new(0),
        spawns: AtomicU32::new(0),
        stop: AtomicBool::new(false),
        run_watchdog: AtomicBool::new(false),
        error: Mutex::new(None),
        stats: Mutex::new(PoolStats::default()),
    };
    let threads = config.jobs.get().min(count.max(1));
    thread::scope(|scope| {
        for _ in 0..threads {
            scope.spawn(|| shared.drive());
        }
    });
    if let Some(error) = lock(&shared.error).take() {
        return Err(error);
    }
    let results = shared
        .results
        .into_inner()
        .unwrap_or_else(PoisonError::into_inner);
    let unfinished = results
        .iter()
        .enumerate()
        .filter(|(_, r)| r.as_ref().is_none_or(|r| !r.finished))
        .map(|(i, _)| i)
        .collect();
    Ok(RunReport {
        jobs: results.into_iter().flatten().collect(),
        unfinished,
        spawns: shared.spawns.load(Ordering::SeqCst),
        run_watchdog: shared.run_watchdog.load(Ordering::SeqCst),
        stats: shared
            .stats
            .into_inner()
            .unwrap_or_else(PoisonError::into_inner),
    })
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

struct Shared<'a> {
    config: &'a PoolConfig,
    phase: Phase<'a>,
    queue: Mutex<VecDeque<(usize, Job)>>,
    results: Mutex<Vec<Option<JobReport>>>,
    failed_spawns: AtomicU32,
    spawns: AtomicU32,
    stop: AtomicBool,
    run_watchdog: AtomicBool,
    error: Mutex<Option<PoolError>>,
    stats: Mutex<PoolStats>,
}

enum Ended {
    Done,
    Fault(NotCheckedReason),
    Retryable(NotCheckedReason),
    RunWatchdog,
}

impl Shared<'_> {
    fn past_deadline(&self) -> bool {
        self.phase.deadline.is_some_and(|d| Instant::now() >= d)
    }

    fn drive(&self) {
        let mut worker: Option<Worker> = None;
        while !self.stop.load(Ordering::SeqCst) {
            if self.past_deadline() {
                self.run_watchdog.store(true, Ordering::SeqCst);
                self.stop.store(true, Ordering::SeqCst);
                break;
            }
            let Some((index, job)) = lock(&self.queue).pop_front() else {
                break;
            };
            match self.run_job(&mut worker, index, job) {
                Ok(report) => {
                    let finished = report.finished;
                    let mut stats = lock(&self.stats);
                    stats.longest_job = stats.longest_job.max(report.elapsed);
                    if self.config.measure
                        && let Some(worker) = worker.as_ref()
                    {
                        stats.worker_peak_bytes = stats.worker_peak_bytes.max(worker.peak_bytes());
                    }
                    drop(stats);
                    lock(&self.results)[index] = Some(report);
                    if !finished {
                        self.run_watchdog.store(true, Ordering::SeqCst);
                        self.stop.store(true, Ordering::SeqCst);
                        break;
                    }
                }
                Err(error) => {
                    self.stop.store(true, Ordering::SeqCst);
                    lock(&self.error).get_or_insert(error);
                    break;
                }
            }
        }
    }

    fn run_job(
        &self,
        slot: &mut Option<Worker>,
        index: usize,
        job: Job,
    ) -> Result<JobReport, PoolError> {
        let start = Instant::now();
        let mut finished = true;
        let mut last_seen = None;
        let mut current = job.clone();
        let mut attempts = 0;
        let mut retried = false;
        let mut faults = 0;
        let mut entries: Vec<Committed> = Vec::new();
        let mut definitions = Vec::new();
        let mut notes = Vec::new();
        loop {
            attempts += 1;
            let (ended, attempt) = self.attempt(slot, &current)?;
            last_seen.clone_from(&attempt.last_seen);
            let reason = match ended {
                Ended::Done => {
                    entries.extend(attempt.committed);
                    definitions = attempt.definitions;
                    notes = attempt.notes;
                    break;
                }
                Ended::RunWatchdog => {
                    entries.extend(attempt.committed);
                    finished = false;
                    break;
                }
                Ended::Retryable(_) if !retried => {
                    retried = true;
                    continue;
                }
                Ended::Retryable(reason) | Ended::Fault(reason) => reason,
            };
            let mut seen: Vec<EntryPath> = entries.iter().map(|c| c.entry.clone()).collect();
            seen.extend(attempt.committed.iter().map(|c| c.entry.clone()));
            let attributed = faulting_entry(&seen, attempt.last_seen.as_ref());
            let opened = attempt.opened(&attributed);
            let scope = fault_scope(&attributed, opened.is_some());
            entries.extend(attempt.committed.iter().cloned());
            faults += 1;
            if faults >= 2 {
                let location = attempt.location_for(&attributed, &job);
                let outcome = Outcome::new(
                    FileOutcome::Aggregate {
                        reason: NotCheckedReason::RepeatedFault,
                        count: None,
                        at_least: 1,
                    },
                    None,
                );
                entries.push(Committed::bare(attributed, location, outcome));
                break;
            }
            let at = scope.outcome_entry();
            let location = attempt.location_for(&at, &job);
            let file_kind = match attempt.opened(&at) {
                Some(open) => open.file_kind,
                None if at.is_job() && job.kind == JobKind::Route => file_kind_by_name(&job),
                None => None,
            };
            let outcome = Outcome::new(scope.outcome(reason), file_kind);
            entries.push(Committed::bare(at, location, outcome));
            match scope.resume_after() {
                Some(after) if job.kind == JobKind::Route => {
                    current = Job {
                        resume_after: Some(after),
                        ..job.clone()
                    };
                }
                _ => break,
            }
        }
        if retried {
            for committed in &mut entries {
                committed.outcome.host_dependent = true;
            }
        }
        let own = entries
            .iter()
            .find(|c| c.entry.is_job())
            .or_else(|| entries.iter().find(|c| c.entry == EntryPath::from([0])))
            .map(|c| c.outcome.clone())
            .unwrap_or_else(|| {
                Outcome::new(
                    FileOutcome::NotChecked {
                        reason: NotCheckedReason::Fault(FaultKind::BadFrame),
                        details: boxt_core::outcome::Details::None,
                    },
                    None,
                )
            });
        Ok(JobReport {
            index,
            job,
            outcome: own,
            entries,
            definitions,
            notes,
            attempts,
            ended_on_host: retried,
            elapsed: start.elapsed(),
            finished,
            last_seen,
        })
    }

    fn failed_start(&self, cause: String) -> Result<(), PoolError> {
        let failed = self.failed_spawns.fetch_add(1, Ordering::SeqCst) + 1;
        if failed >= MAX_FAILED_SPAWNS {
            return Err(PoolError::SpawnFailed {
                attempts: failed.min(MAX_FAILED_SPAWNS),
                cause,
            });
        }
        thread::sleep(SPAWN_BACKOFF * failed);
        Ok(())
    }

    fn spawn(&self) -> Result<Option<Worker>, PoolError> {
        loop {
            if self.past_deadline() {
                return Ok(None);
            }
            match Worker::spawn(self.config, self.phase.tables) {
                Ok(worker) => {
                    self.spawns.fetch_add(1, Ordering::SeqCst);
                    return Ok(Some(worker));
                }
                Err(e) => self.failed_start(e.to_string())?,
            }
        }
    }

    fn attempt(&self, slot: &mut Option<Worker>, job: &Job) -> Result<(Ended, Attempt), PoolError> {
        loop {
            if let Some(ended) = self.attempt_once(slot, job)? {
                return Ok(ended);
            }
        }
    }

    fn attempt_once(
        &self,
        slot: &mut Option<Worker>,
        job: &Job,
    ) -> Result<Option<(Ended, Attempt)>, PoolError> {
        let mut attempt = Attempt::new(job, &self.config.settings.limits);
        if self.past_deadline() {
            return Ok(Some((Ended::RunWatchdog, attempt)));
        }
        let worker = match slot {
            Some(worker) => worker,
            None => match self.spawn()? {
                Some(worker) => slot.insert(worker),
                None => return Ok(Some((Ended::RunWatchdog, attempt))),
            },
        };
        let result = worker.run_job(
            &mut attempt,
            &self.config.settings.limits,
            self.phase.deadline,
        );
        let answered = worker.answered;
        if answered {
            self.failed_spawns.store(0, Ordering::SeqCst);
        }
        let death = match result {
            Ok(()) if attempt.committed.iter().any(|c| c.entry.is_job()) => {
                return Ok(Some((Ended::Done, attempt)));
            }
            Ok(()) => Death::BadFrame,
            Err(death) => death,
        };
        let mut dead = slot.take().expect("the worker that ran the job");
        let mut died = None;
        if !answered && matches!(death, Death::Died) {
            let (status, killed) = dead.reap();
            let raw = raw_exit(status);
            if killed {
                return Ok(Some((
                    Ended::Fault(NotCheckedReason::Fault(FaultKind::Exited)),
                    attempt,
                )));
            }
            if raw == RawExit::Code(STARTUP_CAP_EXIT) {
                return Err(PoolError::WorkerMemoryCap);
            }
            if raw == RawExit::Code(STARTUP_PROTOCOL_EXIT) {
                self.failed_start(format!("the worker ended during startup ({raw:?})"))?;
                return Ok(None);
            }
            died = Some(raw);
        }
        let ended = match death {
            Death::BadFrame => {
                dead.kill();
                Ended::Fault(NotCheckedReason::Fault(FaultKind::BadFrame))
            }
            Death::RunWatchdog => {
                dead.kill();
                Ended::RunWatchdog
            }
            Death::Watchdog => {
                let status = dead.kill();
                let class = classify_exit(
                    Platform::HOST,
                    WorkerExit {
                        raw: raw_exit(status),
                        killed_by_parent: true,
                    },
                );
                debug_assert_eq!(class, ExitClass::KilledByParent);
                let reason = ParentKill::Watchdog
                    .reason()
                    .expect("the watchdog names a limit");
                Ended::Retryable(reason)
            }
            Death::Died => {
                let raw = match died {
                    Some(raw) => raw,
                    None => match dead.reap() {
                        (_, true) => {
                            return Ok(Some((
                                Ended::Fault(NotCheckedReason::Fault(FaultKind::Exited)),
                                attempt,
                            )));
                        }
                        (status, false) => raw_exit(status),
                    },
                };
                let class = classify_exit(
                    Platform::HOST,
                    WorkerExit {
                        raw,
                        killed_by_parent: false,
                    },
                );
                match class.reason() {
                    Some(reason) if class.host_dependent() => Ended::Retryable(reason),
                    Some(reason) => Ended::Fault(reason),
                    None => Ended::Fault(NotCheckedReason::Fault(FaultKind::Exited)),
                }
            }
        };
        Ok(Some((ended, attempt)))
    }
}

#[cfg(unix)]
fn raw_exit(status: Option<ExitStatus>) -> RawExit {
    use std::os::unix::process::ExitStatusExt;
    match status {
        Some(status) => match status.signal() {
            Some(signal) => RawExit::Signal(signal),
            None => RawExit::Code(status.code().unwrap_or(-1)),
        },
        None => RawExit::Code(-1),
    }
}

#[cfg(windows)]
fn raw_exit(status: Option<ExitStatus>) -> RawExit {
    RawExit::Code(status.and_then(|s| s.code()).unwrap_or(-1))
}

#[derive(Debug, PartialEq, Eq)]
enum Death {
    BadFrame,
    Watchdog,
    RunWatchdog,
    Died,
}

#[derive(Debug)]
struct OpenEntry {
    entry: EntryPath,
    location: Location,
    file_kind: Option<FileKind>,
    facts: Vec<Fact>,
    notes: Vec<Note>,
    check: EntryCheck,
    container: bool,
}

#[derive(Debug)]
struct Attempt {
    job: Job,
    max_size: u64,
    max_len: u64,
    open: Vec<OpenEntry>,
    opened: Vec<(EntryPath, Location, Option<FileKind>)>,
    committed: Vec<Committed>,
    last_seen: Option<EntryPath>,
    definitions: Vec<MacroDef>,
    notes: Vec<Note>,
    budget: EntryBudget,
    limits: Limits,
}

#[derive(Debug, Clone)]
struct OpenedEntry {
    file_kind: Option<FileKind>,
}

impl Attempt {
    fn new(job: &Job, limits: &Limits) -> Self {
        Attempt {
            job: job.clone(),
            max_size: limits.get(LimitName::FileSize),
            max_len: limits.get(LimitName::EntryMessageBytes),
            open: Vec::new(),
            opened: Vec::new(),
            committed: Vec::new(),
            last_seen: None,
            definitions: Vec::new(),
            notes: Vec::new(),
            budget: EntryBudget::new(limits),
            limits: limits.clone(),
        }
    }

    fn opened(&self, entry: &EntryPath) -> Option<OpenedEntry> {
        self.opened
            .iter()
            .find(|(e, ..)| e == entry)
            .map(|&(_, _, file_kind)| OpenedEntry { file_kind })
    }

    fn location_for(&self, entry: &EntryPath, job: &Job) -> Location {
        if let Some((_, location, _)) = self.opened.iter().find(|(e, ..)| e == entry) {
            return location.clone();
        }
        if let Some(parent) = entry.parent()
            && let Some((_, location, _)) = self.opened.iter().find(|(e, ..)| *e == parent.child(0))
        {
            return location.clone();
        }
        match job.path() {
            Some(path) => Location::file(job.input.clone(), Vec::new(), path.clone()),
            None => Location::frame(job.input.clone(), Vec::new()),
        }
    }

    fn see(&mut self, entry: &EntryPath) {
        if self.last_seen.as_ref().is_none_or(|l| entry > l) {
            self.last_seen = Some(entry.clone());
        }
    }

    fn commit(&mut self, entry: EntryPath, outcome: Outcome, bytes: u64) -> Result<(), Death> {
        if outcome.validate_from_worker(&entry).is_err() || outcome.validate_lines(bytes).is_err() {
            return Err(Death::BadFrame);
        }
        if let Some(at) = self.open.iter().position(|o| o.entry == entry) {
            if at + 1 != self.open.len() || self.open[at].container {
                return Err(Death::BadFrame);
            }
            let open = self.open.remove(at);
            if open.file_kind != outcome.file_kind {
                return Err(Death::BadFrame);
            }
            let outcome = open.check.done(outcome)?;
            let read = outcome.file == FileOutcome::Read;
            self.committed.push(Committed {
                entry,
                location: open.location,
                outcome,
                facts: if read { open.facts } else { Vec::new() },
                regions: if read { open.check.regions } else { Vec::new() },
                notes: if read { open.notes } else { Vec::new() },
            });
            return Ok(());
        }
        let ends = matches!(outcome.file, FileOutcome::Aggregate { .. })
            && entry.last().is_some_and(|i| i > 0);
        let skipped = self
            .job
            .resume_after
            .as_ref()
            .is_some_and(|after| resume_skips(&entry, after));
        if !ends || skipped || self.committed.iter().any(|c| c.entry == entry) {
            return Err(Death::BadFrame);
        }
        let location = self.location_for(&entry, &self.job.clone());
        self.committed
            .push(Committed::bare(entry, location, outcome));
        Ok(())
    }

    fn handle(&mut self, msg: WorkerMsg, payload_len: u64) -> Result<bool, Death> {
        if !msg.allowed_in(self.job.kind) {
            return Err(Death::BadFrame);
        }
        match self.job.kind {
            JobKind::Route => self.route(msg, payload_len),
            JobKind::Definitions => self.definitions(msg, payload_len),
            JobKind::CompileDatabase => match msg {
                WorkerMsg::CompileUnits { .. } => Ok(false),
                WorkerMsg::JobDone {
                    outcome: Some(outcome),
                } => self.finish(outcome),
                _ => Err(Death::BadFrame),
            },
        }
    }

    fn route(&mut self, msg: WorkerMsg, payload_len: u64) -> Result<bool, Death> {
        match msg {
            WorkerMsg::Opened {
                ref entry,
                file_kind,
                ..
            } => {
                let entry = entry.clone();
                if self.last_seen.as_ref().is_some_and(|l| entry <= *l)
                    || self
                        .job
                        .resume_after
                        .as_ref()
                        .is_some_and(|after| resume_skips(&entry, after))
                {
                    return Err(Death::BadFrame);
                }
                let location = msg
                    .opened_location(&self.job.input)
                    .ok_or(Death::BadFrame)?;
                while let Some(last) = self.open.last()
                    && !last.entry.is_prefix_of(&entry)
                {
                    if !last.container {
                        return Err(Death::BadFrame);
                    }
                    self.open.pop();
                }
                if let Some(parent) = self.open.last_mut() {
                    parent.container = true;
                }
                self.see(&entry);
                self.opened
                    .push((entry.clone(), location.clone(), file_kind));
                self.open.push(OpenEntry {
                    entry,
                    location,
                    file_kind,
                    facts: Vec::new(),
                    notes: Vec::new(),
                    check: EntryCheck::new(&self.limits),
                    container: false,
                });
                Ok(false)
            }
            WorkerMsg::Facts { ref entry, .. }
            | WorkerMsg::Regions { ref entry, .. }
            | WorkerMsg::Notes { ref entry, .. } => {
                let entry = entry.clone();
                self.see(&entry);
                let open = self
                    .open
                    .last_mut()
                    .filter(|o| o.entry == entry && !o.container)
                    .ok_or(Death::BadFrame)?;
                open.check.frame(&msg, payload_len)?;
                match msg {
                    WorkerMsg::Facts { facts, .. } => open.facts.extend(facts),
                    WorkerMsg::Notes { notes, .. } => open.notes.extend(notes),
                    _ => {}
                }
                Ok(false)
            }
            WorkerMsg::EntryDone {
                entry,
                outcome,
                bytes,
            } => {
                self.see(&entry);
                self.commit(entry, outcome, bytes)?;
                Ok(false)
            }
            WorkerMsg::JobDone {
                outcome: Some(outcome),
            } => {
                self.commit(EntryPath::job(), outcome, self.max_size)?;
                if !self.open.is_empty() {
                    return Err(Death::BadFrame);
                }
                Ok(true)
            }
            _ => Err(Death::BadFrame),
        }
    }

    fn definitions(&mut self, msg: WorkerMsg, payload_len: u64) -> Result<bool, Death> {
        match msg {
            WorkerMsg::Definitions { ref definitions } => {
                if definitions
                    .iter()
                    .any(|d| d.validate(&self.job, self.max_len).is_err())
                {
                    return Err(Death::BadFrame);
                }
                let _ = self.budget.charge_frame(&msg, payload_len);
                if let WorkerMsg::Definitions { definitions } = msg {
                    self.definitions.extend(definitions);
                }
                Ok(false)
            }
            WorkerMsg::Notes { ref notes, .. } => {
                let notes = notes.clone();
                let _ = self.budget.charge_frame(&msg, payload_len);
                self.notes.extend(notes);
                Ok(false)
            }
            WorkerMsg::JobDone {
                outcome: Some(outcome),
            } => self.finish(outcome),
            _ => Err(Death::BadFrame),
        }
    }

    fn finish(&mut self, mut outcome: Outcome) -> Result<bool, Death> {
        if outcome.validate_from_worker(&EntryPath::job()).is_err()
            || outcome.validate_lines(self.max_size).is_err()
        {
            return Err(Death::BadFrame);
        }
        if let Some(exceeded) = self.budget.exceeded_outcome() {
            if outcome.file == FileOutcome::Read {
                return Err(Death::BadFrame);
            }
            outcome.file = exceeded;
        }
        if outcome.file != FileOutcome::Read {
            self.definitions.clear();
            self.notes.clear();
        }
        let location = self.location_for(&EntryPath::job(), &self.job.clone());
        self.committed
            .push(Committed::bare(EntryPath::job(), location, outcome));
        Ok(true)
    }
}

type Frame = Result<Option<Decoded<WorkerMsg>>, FrameError>;

struct Worker {
    child: Child,
    stdin: Option<ChildStdin>,
    frames: Receiver<Frame>,
    reader: Option<JoinHandle<()>>,
    answered: bool,
    reaped: bool,
}

impl Worker {
    fn spawn(config: &PoolConfig, tables: &[Vec<u8>]) -> io::Result<Worker> {
        let mut child = Command::new(&config.program)
            .args(&config.args)
            .envs(config.env.iter().map(|(k, v)| (k, v)))
            .env(PARENT_ENV, std::process::id().to_string())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()?;
        let max_len = config.settings.limits.get(LimitName::EntryMessageBytes);
        let stdout = child.stdout.take().expect("piped stdout");
        let (tx, frames) = channel();
        let reader = thread::spawn(move || {
            let mut stdout = BufReader::new(stdout);
            loop {
                let frame = decode_frame::<WorkerMsg, _>(&mut stdout, max_len);
                let last = !matches!(frame, Ok(Some(_)));
                if tx.send(frame).is_err() || last {
                    break;
                }
            }
        });
        let mut worker = Worker {
            stdin: child.stdin.take(),
            child,
            frames,
            reader: Some(reader),
            answered: false,
            reaped: false,
        };
        let settings = CollectorMsg::Settings(config.settings.clone());
        let first_max = Limits::default().get(LimitName::EntryMessageBytes);
        let frame = encode_frame(&settings, first_max).map_err(io::Error::other)?;
        worker.write(&frame)?;
        for table in tables {
            worker.write(table)?;
        }
        Ok(worker)
    }

    fn write(&mut self, frame: &[u8]) -> io::Result<()> {
        let stdin = self
            .stdin
            .as_mut()
            .ok_or_else(|| io::Error::from(io::ErrorKind::BrokenPipe))?;
        stdin.write_all(frame)?;
        stdin.flush()
    }

    #[cfg(target_os = "linux")]
    fn peak_bytes(&self) -> u64 {
        let status = std::fs::read_to_string(format!("/proc/{}/status", self.child.id()));
        status
            .ok()
            .and_then(|s| {
                s.lines()
                    .find_map(|l| l.strip_prefix("VmHWM:"))
                    .and_then(|v| v.trim().trim_end_matches("kB").trim().parse::<u64>().ok())
            })
            .map_or(0, |kib| kib.saturating_mul(1024))
    }

    #[cfg(not(target_os = "linux"))]
    fn peak_bytes(&self) -> u64 {
        0
    }

    fn run_job(
        &mut self,
        attempt: &mut Attempt,
        limits: &Limits,
        run_deadline: Option<Instant>,
    ) -> Result<(), Death> {
        let watchdog = Duration::from_secs(limits.get(LimitName::Watchdog));
        let max_len = limits.get(LimitName::EntryMessageBytes);
        let Ok(frame) = encode_frame(&CollectorMsg::Job(attempt.job.clone()), max_len) else {
            return Err(Death::BadFrame);
        };
        if self.write(&frame).is_err() {
            return Err(Death::Died);
        }
        let job_deadline = Instant::now().checked_add(watchdog);
        loop {
            let (deadline, by_run) = match (job_deadline, run_deadline) {
                (Some(job), Some(run)) if run <= job => (Some(run), true),
                (None, Some(run)) => (Some(run), true),
                (job, _) => (job, false),
            };
            let received = match deadline {
                Some(deadline) => self
                    .frames
                    .recv_timeout(deadline.saturating_duration_since(Instant::now())),
                None => self
                    .frames
                    .recv()
                    .map_err(|_| RecvTimeoutError::Disconnected),
            };
            let frame = match received {
                Ok(frame) => frame,
                Err(RecvTimeoutError::Timeout) if by_run => return Err(Death::RunWatchdog),
                Err(RecvTimeoutError::Timeout) => return Err(Death::Watchdog),
                Err(RecvTimeoutError::Disconnected) => return Err(Death::Died),
            };
            let (msg, payload_len) = match frame {
                Ok(Some(decoded)) => (decoded.msg, decoded.payload_len),
                Ok(None) => return Err(Death::Died),
                Err(_) => return Err(Death::BadFrame),
            };
            self.answered = true;
            if attempt.handle(msg, payload_len)? {
                return Ok(());
            }
        }
    }

    fn reap(&mut self) -> (Option<ExitStatus>, bool) {
        let until = Instant::now() + REAP_GRACE;
        loop {
            match self.child.try_wait() {
                Ok(Some(_)) => return (self.wait(), false),
                Ok(None) if Instant::now() < until => thread::sleep(Duration::from_millis(5)),
                _ => return (self.kill(), true),
            }
        }
    }

    fn kill(&mut self) -> Option<ExitStatus> {
        let _ = self.child.kill();
        self.wait()
    }

    fn wait(&mut self) -> Option<ExitStatus> {
        self.stdin = None;
        let status = self.child.wait().ok();
        self.reaped = true;
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
        status
    }
}

#[derive(Debug)]
struct EntryCheck {
    budget: EntryBudget,
    regions: Vec<Region>,
}

impl EntryCheck {
    fn new(limits: &Limits) -> Self {
        EntryCheck {
            budget: EntryBudget::new(limits),
            regions: Vec::new(),
        }
    }

    fn frame(&mut self, msg: &WorkerMsg, payload_len: u64) -> Result<(), Death> {
        match msg {
            WorkerMsg::Facts { facts, .. } => {
                if !self.regions.is_empty() || facts.iter().any(|f| f.validate().is_err()) {
                    return Err(Death::BadFrame);
                }
            }
            WorkerMsg::Regions { regions, .. } => {
                let mut last = self.regions.last().map_or(0, |r| r.last_line);
                for region in regions {
                    if region.validate().is_err() || region.first_line <= last {
                        return Err(Death::BadFrame);
                    }
                    last = region.last_line;
                }
                self.regions.extend(regions.iter().copied());
            }
            WorkerMsg::Notes { .. } => {}
            _ => return Err(Death::BadFrame),
        }
        let _ = self.budget.charge_frame(msg, payload_len);
        Ok(())
    }

    fn done(&self, mut outcome: Outcome) -> Result<Outcome, Death> {
        if let Some(exceeded) = self.budget.exceeded_outcome() {
            if outcome.file == FileOutcome::Read {
                return Err(Death::BadFrame);
            }
            outcome.file = exceeded;
            return Ok(outcome);
        }
        if self.regions.is_empty() {
            return Ok(outcome);
        }
        let Some(count) = outcome.lines else {
            return Err(Death::BadFrame);
        };
        if self
            .regions
            .iter()
            .any(|region| region.validate_within(count.lines).is_err())
        {
            return Err(Death::BadFrame);
        }
        Ok(outcome)
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        if !self.reaped {
            self.kill();
        }
    }
}

#[cfg(test)]
mod tests {
    use boxt_core::Text;
    use boxt_core::fact::Fact;
    use boxt_core::location::{InputRef, Location, RelPath, Scope};
    use boxt_core::message::RegionReason;
    use boxt_core::outcome::Details;

    use super::*;

    fn fact() -> Fact {
        let location = Location::file(
            InputRef {
                index: 0,
                given: Text::from("/src"),
            },
            Vec::new(),
            RelPath::try_from("a.c").unwrap(),
        );
        Fact::Call {
            file_kind: FileKind::Source,
            location: location.at(Some(1), Scope::File),
            callee: Text::from("f"),
            args: Vec::new(),
            guards: Vec::new(),
        }
    }

    fn facts(n: usize) -> WorkerMsg {
        WorkerMsg::Facts {
            entry: EntryPath::job(),
            facts: vec![fact(); n],
        }
    }

    fn regions(lines: &[(u32, u32)]) -> WorkerMsg {
        WorkerMsg::Regions {
            entry: EntryPath::job(),
            regions: lines
                .iter()
                .map(|&(first_line, last_line)| Region {
                    first_line,
                    last_line,
                    reason: RegionReason::Unparseable,
                })
                .collect(),
        }
    }

    fn read(lines: Option<u64>) -> Outcome {
        let outcome = Outcome::new(FileOutcome::Read, Some(FileKind::Source));
        match lines {
            Some(n) => outcome.with_lines(n),
            None => outcome,
        }
    }

    fn route_job() -> Job {
        Job {
            kind: JobKind::Route,
            input: InputRef {
                index: 0,
                given: Text::from("/src"),
            },
            file: boxt_core::message::JobFile::InInput(RelPath::try_from("pkg.tar").unwrap()),
            resume_after: None,
        }
    }

    fn opened(entry: EntryPath, file_kind: Option<FileKind>) -> WorkerMsg {
        WorkerMsg::Opened {
            entry,
            frames: Vec::new(),
            path: Some(RelPath::try_from("a.c").unwrap()),
            scope: Scope::File,
            file_kind,
        }
    }

    #[test]
    fn container_entries_commit_at_entry_done_and_drop_what_was_not_read() {
        let mut attempt = Attempt::new(&route_job(), &Limits::default());
        let source = Some(FileKind::Source);
        let steps = [
            opened(EntryPath::job(), None),
            opened(EntryPath::from([0]), None),
            WorkerMsg::EntryDone {
                entry: EntryPath::from([0]),
                outcome: Outcome::new(FileOutcome::Read, None),
                bytes: 0,
            },
            opened(EntryPath::from([1]), source),
            WorkerMsg::Facts {
                entry: EntryPath::from([1]),
                facts: vec![fact()],
            },
            WorkerMsg::EntryDone {
                entry: EntryPath::from([1]),
                outcome: read(Some(3)),
                bytes: 10,
            },
            opened(EntryPath::from([2]), source),
            WorkerMsg::Facts {
                entry: EntryPath::from([2]),
                facts: vec![fact()],
            },
            WorkerMsg::EntryDone {
                entry: EntryPath::from([2]),
                outcome: Outcome::new(
                    FileOutcome::NotChecked {
                        reason: NotCheckedReason::Unparseable,
                        details: Details::None,
                    },
                    source,
                ),
                bytes: 10,
            },
        ];
        for msg in steps {
            assert_eq!(attempt.handle(msg, 10), Ok(false));
        }
        assert_eq!(
            attempt.handle(WorkerMsg::JobDone { outcome: None }, 10),
            Err(Death::BadFrame)
        );
        let committed: Vec<(EntryPath, usize)> = attempt
            .committed
            .iter()
            .map(|c| (c.entry.clone(), c.facts.len()))
            .collect();
        assert_eq!(
            committed,
            [
                (EntryPath::from([0]), 0),
                (EntryPath::from([1]), 1),
                (EntryPath::from([2]), 0)
            ]
        );
        assert_eq!(attempt.last_seen, Some(EntryPath::from([2])));
        assert_eq!(
            faulting_entry(
                &committed.iter().map(|(e, _)| e.clone()).collect::<Vec<_>>(),
                attempt.last_seen.as_ref()
            ),
            EntryPath::from([3])
        );
    }

    #[test]
    fn a_leaf_job_must_close_every_entry_and_name_its_own_outcome() {
        let mut attempt = Attempt::new(&route_job(), &Limits::default());
        assert_eq!(attempt.handle(opened(EntryPath::job(), None), 1), Ok(false));
        assert_eq!(
            attempt.handle(opened(EntryPath::from([0]), None), 1),
            Ok(false)
        );
        let done = WorkerMsg::JobDone {
            outcome: Some(Outcome::new(FileOutcome::Read, None)),
        };
        assert_eq!(attempt.handle(done, 1), Err(Death::BadFrame));

        let mut sibling = Attempt::new(&route_job(), &Limits::default());
        assert_eq!(
            sibling.handle(opened(EntryPath::from([0]), None), 1),
            Ok(false)
        );
        assert_eq!(
            sibling.handle(opened(EntryPath::from([1]), None), 1),
            Err(Death::BadFrame)
        );

        let mut outer = Attempt::new(&route_job(), &Limits::default());
        assert_eq!(outer.handle(opened(EntryPath::job(), None), 1), Ok(false));
        assert_eq!(
            outer.handle(opened(EntryPath::from([0]), None), 1),
            Ok(false)
        );
        let facts = WorkerMsg::Facts {
            entry: EntryPath::job(),
            facts: vec![fact()],
        };
        assert_eq!(outer.handle(facts, 1), Err(Death::BadFrame));
    }

    #[test]
    fn an_aggregate_for_a_committed_or_skipped_entry_is_a_bad_frame() {
        let aggregate = |entry: EntryPath| WorkerMsg::EntryDone {
            entry,
            outcome: Outcome::new(
                FileOutcome::Aggregate {
                    reason: NotCheckedReason::LimitExceeded(LimitName::TotalEntries),
                    count: None,
                    at_least: 1,
                },
                None,
            ),
            bytes: 0,
        };
        let resumed = Job {
            resume_after: Some(EntryPath::from([2])),
            ..route_job()
        };
        let mut attempt = Attempt::new(&resumed, &Limits::default());
        assert_eq!(
            attempt.handle(aggregate(EntryPath::from([1])), 1),
            Err(Death::BadFrame)
        );
        let mut attempt = Attempt::new(&route_job(), &Limits::default());
        assert_eq!(
            attempt.handle(aggregate(EntryPath::from([1])), 1),
            Ok(false)
        );
        assert_eq!(
            attempt.handle(aggregate(EntryPath::from([1])), 1),
            Err(Death::BadFrame)
        );
    }

    #[test]
    fn entries_out_of_order_or_skipped_by_resume_are_bad_frames() {
        let mut attempt = Attempt::new(&route_job(), &Limits::default());
        assert_eq!(
            attempt.handle(opened(EntryPath::from([1]), None), 1),
            Ok(false)
        );
        assert_eq!(
            attempt.handle(opened(EntryPath::from([0]), None), 1),
            Err(Death::BadFrame)
        );
        let resumed = Job {
            resume_after: Some(EntryPath::from([1])),
            ..route_job()
        };
        let mut attempt = Attempt::new(&resumed, &Limits::default());
        assert_eq!(
            attempt.handle(opened(EntryPath::from([1, 0]), None), 1),
            Err(Death::BadFrame)
        );
        assert_eq!(
            attempt.handle(opened(EntryPath::from([2]), None), 1),
            Ok(false)
        );
        let mut attempt = Attempt::new(&route_job(), &Limits::default());
        assert_eq!(
            attempt.handle(
                WorkerMsg::Facts {
                    entry: EntryPath::job(),
                    facts: vec![fact()]
                },
                1
            ),
            Err(Death::BadFrame)
        );
    }

    #[test]
    fn an_entry_over_its_fact_limit_is_not_checked_or_a_bad_frame() {
        let limits = Limits::default().with(LimitName::EntryFacts, 2).unwrap();
        let mut check = EntryCheck::new(&limits);
        check.frame(&facts(3), 10).unwrap();
        assert_eq!(check.done(read(Some(5))), Err(Death::BadFrame));
        let claimed = Outcome::new(
            FileOutcome::NotChecked {
                reason: NotCheckedReason::LimitExceeded(LimitName::EntryFacts),
                details: Details::None,
            },
            Some(FileKind::Source),
        )
        .with_lines(5);
        let got = check.done(claimed).unwrap();
        assert_eq!(
            got.file,
            FileOutcome::NotChecked {
                reason: NotCheckedReason::LimitExceeded(LimitName::EntryFacts),
                details: Details::Measured { value: 3, limit: 2 }
            }
        );
    }

    #[test]
    fn regions_need_a_line_count_and_fit_it() {
        let mut check = EntryCheck::new(&Limits::default());
        check.frame(&regions(&[(2, 3)]), 10).unwrap();
        assert_eq!(check.done(read(None)), Err(Death::BadFrame));
        assert_eq!(check.done(read(Some(2))), Err(Death::BadFrame));
        assert!(check.done(read(Some(3))).is_ok());
        assert!(EntryCheck::new(&Limits::default()).done(read(None)).is_ok());
    }

    #[test]
    fn frames_keep_their_order() {
        let mut check = EntryCheck::new(&Limits::default());
        check.frame(&facts(1), 10).unwrap();
        check.frame(&regions(&[(1, 1), (3, 4)]), 10).unwrap();
        assert_eq!(check.frame(&facts(1), 10), Err(Death::BadFrame));
        let mut again = EntryCheck::new(&Limits::default());
        again.frame(&regions(&[(1, 1), (3, 4)]), 10).unwrap();
        assert_eq!(
            again.frame(&regions(&[(1, 1), (3, 4)]), 10),
            Err(Death::BadFrame)
        );
        again.frame(&regions(&[(6, 6)]), 10).unwrap();
        let mut overlap = EntryCheck::new(&Limits::default());
        assert_eq!(
            overlap.frame(&regions(&[(3, 4), (4, 5)]), 10),
            Err(Death::BadFrame)
        );
    }
}
