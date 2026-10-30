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

use boxt_core::fact::FileKind;
use boxt_core::limits::{LimitError, LimitName, Limits, MEMORY_CAP_FACTOR};
use boxt_core::message::{
    CollectorMsg, Decoded, EntryBudget, EntryPath, ExitClass, FrameError, Job, JobKind, ParentKill,
    Platform, RawExit, Region, WorkerExit, WorkerMsg, classify_exit, decode_frame, encode_frame,
    fault_scope, faulting_entry,
};
use boxt_core::outcome::{FaultKind, FileOutcome, NotCheckedReason, Outcome};
use boxt_core::settings::WorkerSettings;

use crate::os;
use crate::worker::{PARENT_ENV, STARTUP_CAP_EXIT, WORKER_SUBCOMMAND, file_kind_by_name};

pub const MAX_FAILED_SPAWNS: u32 = 3;

const SPAWN_BACKOFF: Duration = Duration::from_millis(50);

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
}

impl PoolConfig {
    pub fn new(settings: WorkerSettings, jobs: Option<NonZeroU32>) -> io::Result<Self> {
        let jobs = match jobs {
            Some(n) => NonZeroUsize::new(n.get() as usize).unwrap_or(NonZeroUsize::MIN),
            None => thread::available_parallelism().unwrap_or(NonZeroUsize::MIN),
        };
        Ok(PoolConfig {
            program: std::env::current_exe()?,
            args: vec![OsString::from(WORKER_SUBCOMMAND)],
            env: Vec::new(),
            jobs,
            settings,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobReport {
    pub job: Job,
    pub outcome: Outcome,
    pub attempts: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunReport {
    pub jobs: Vec<JobReport>,
    pub spawns: u32,
}

impl RunReport {
    pub fn read_count(&self) -> usize {
        self.jobs
            .iter()
            .filter(|r| r.outcome.file == FileOutcome::Read)
            .count()
    }
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
    os::suppress_error_dialogs();
    os::kill_workers_on_exit().map_err(|e| PoolError::KillOnExit(e.kind()))?;
    let shared = Shared {
        config,
        queue: Mutex::new(jobs.iter().cloned().enumerate().collect()),
        results: Mutex::new(vec![None; jobs.len()]),
        failed_spawns: AtomicU32::new(0),
        spawns: AtomicU32::new(0),
        stop: AtomicBool::new(false),
        error: Mutex::new(None),
    };
    let threads = config.jobs.get().min(jobs.len().max(1));
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
    Ok(RunReport {
        jobs: results.into_iter().flatten().collect(),
        spawns: shared.spawns.load(Ordering::SeqCst),
    })
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

struct Shared<'a> {
    config: &'a PoolConfig,
    queue: Mutex<VecDeque<(usize, Job)>>,
    results: Mutex<Vec<Option<JobReport>>>,
    failed_spawns: AtomicU32,
    spawns: AtomicU32,
    stop: AtomicBool,
    error: Mutex<Option<PoolError>>,
}

enum Ended {
    Done(Outcome),
    Fault(NotCheckedReason),
    Retryable(NotCheckedReason),
}

impl Shared<'_> {
    fn drive(&self) {
        let mut worker: Option<Worker> = None;
        while !self.stop.load(Ordering::SeqCst) {
            let Some((index, job)) = lock(&self.queue).pop_front() else {
                break;
            };
            match self.run_with_retry(&mut worker, &job) {
                Ok((outcome, attempts)) => {
                    lock(&self.results)[index] = Some(JobReport {
                        job,
                        outcome,
                        attempts,
                    });
                }
                Err(error) => {
                    self.stop.store(true, Ordering::SeqCst);
                    lock(&self.error).get_or_insert(error);
                    break;
                }
            }
        }
    }

    fn run_with_retry(
        &self,
        worker: &mut Option<Worker>,
        job: &Job,
    ) -> Result<(Outcome, u32), PoolError> {
        let first = self.attempt(worker, job)?;
        let Ended::Retryable(first_reason) = first else {
            return Ok((finish(first, job), 1));
        };
        let second = self.attempt(worker, job)?;
        let mut outcome = match second {
            Ended::Retryable(reason) => fault_outcome(job, reason),
            other => finish(other, job),
        };
        debug_assert!(matches!(
            first_reason,
            NotCheckedReason::LimitExceeded(LimitName::WorkerMemoryCap | LimitName::Watchdog)
        ));
        outcome.host_dependent = true;
        Ok((outcome, 2))
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

    fn spawn(&self) -> Result<Worker, PoolError> {
        loop {
            match Worker::spawn(self.config) {
                Ok(worker) => {
                    self.spawns.fetch_add(1, Ordering::SeqCst);
                    return Ok(worker);
                }
                Err(e) => self.failed_start(e.to_string())?,
            }
        }
    }

    fn attempt(&self, slot: &mut Option<Worker>, job: &Job) -> Result<Ended, PoolError> {
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
    ) -> Result<Option<Ended>, PoolError> {
        let worker = match slot {
            Some(worker) => worker,
            None => slot.insert(self.spawn()?),
        };
        let result = worker.run_job(job, &self.config.settings.limits);
        let answered = worker.answered;
        if answered {
            self.failed_spawns.store(0, Ordering::SeqCst);
        }
        let death = match result {
            Ok(outcome) => return Ok(Some(Ended::Done(outcome))),
            Err(death) => death,
        };
        let mut dead = slot.take().expect("the worker that ran the job");
        if !answered && matches!(death, Death::Died) {
            let raw = raw_exit(dead.wait());
            if raw == RawExit::Code(STARTUP_CAP_EXIT) {
                return Err(PoolError::WorkerMemoryCap);
            }
            self.failed_start(format!("the worker ended during startup ({raw:?})"))?;
            return Ok(None);
        }
        let ended = match death {
            Death::BadFrame => {
                dead.kill();
                Ended::Fault(NotCheckedReason::Fault(FaultKind::BadFrame))
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
                let raw = raw_exit(dead.wait());
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
        Ok(Some(ended))
    }
}

fn finish(ended: Ended, job: &Job) -> Outcome {
    match ended {
        Ended::Done(outcome) => outcome,
        Ended::Fault(reason) | Ended::Retryable(reason) => fault_outcome(job, reason),
    }
}

fn fault_outcome(job: &Job, reason: NotCheckedReason) -> Outcome {
    let scope = fault_scope(&faulting_entry(&[], None), false);
    Outcome::new(scope.outcome(reason), file_kind_by_name(job))
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
    Died,
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
    fn spawn(config: &PoolConfig) -> io::Result<Worker> {
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

    fn run_job(&mut self, job: &Job, limits: &Limits) -> Result<Outcome, Death> {
        let watchdog = Duration::from_secs(limits.get(LimitName::Watchdog));
        let max_len = limits.get(LimitName::EntryMessageBytes);
        let Ok(frame) = encode_frame(&CollectorMsg::Job(job.clone()), max_len) else {
            return Err(Death::BadFrame);
        };
        if self.write(&frame).is_err() {
            return Err(Death::Died);
        }
        let deadline = Instant::now().checked_add(watchdog);
        let mut opened: Option<Option<FileKind>> = None;
        let mut check = EntryCheck::new(limits);
        loop {
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
                Err(RecvTimeoutError::Timeout) => return Err(Death::Watchdog),
                Err(RecvTimeoutError::Disconnected) => return Err(Death::Died),
            };
            let (msg, payload_len) = match frame {
                Ok(Some(decoded)) => (decoded.msg, decoded.payload_len),
                Ok(None) => return Err(Death::Died),
                Err(_) => return Err(Death::BadFrame),
            };
            self.answered = true;
            if !msg.allowed_in(job.kind) {
                return Err(Death::BadFrame);
            }
            match msg {
                WorkerMsg::Opened {
                    entry, file_kind, ..
                } if entry == EntryPath::job() && opened.is_none() => {
                    opened = Some(file_kind);
                }
                WorkerMsg::Facts { ref entry, .. } | WorkerMsg::Regions { ref entry, .. }
                    if *entry == EntryPath::job() && opened.is_some() =>
                {
                    check.frame(&msg, payload_len)?;
                }
                WorkerMsg::JobDone {
                    outcome: Some(outcome),
                } => {
                    let valid = outcome.validate_from_worker(&EntryPath::job()).is_ok()
                        && outcome
                            .validate_lines(limits.get(LimitName::FileSize))
                            .is_ok()
                        && (job.kind != JobKind::Route
                            || opened.is_some_and(|kind| kind == outcome.file_kind));
                    if !valid {
                        return Err(Death::BadFrame);
                    }
                    return check.done(outcome);
                }
                _ => return Err(Death::BadFrame),
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
