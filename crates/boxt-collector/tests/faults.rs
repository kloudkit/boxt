use std::ffi::OsString;
use std::num::NonZeroUsize;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use boxt_collector::CapAllocator;
use boxt_collector::path::text_from_path;
use boxt_collector::pool::{self, JobReport, MAX_FAILED_SPAWNS, PoolConfig, PoolError, RunReport};
use boxt_collector::worker::{FAIL_CAP_ENV, PID_DIR_ENV, STARTUP_PROTOCOL_EXIT, WORKER_SUBCOMMAND};
use boxt_core::Text;
use boxt_core::fact::FileKind;
use boxt_core::limits::{LimitName, Limits};
use boxt_core::location::{InputRef, RelPath};
use boxt_core::message::{Job, JobFile, JobKind};
use boxt_core::outcome::{Details, FaultKind, FileOutcome, NotCheckedReason, Outcome};
use boxt_core::settings::WorkerSettings;

#[global_allocator]
static ALLOCATOR: CapAllocator = CapAllocator;

const ORDINARY: usize = 20;
const MIB: u64 = 1024 * 1024;
const HOLD_POOL: &str = "hold-pool";
const DIE_AT_STARTUP: &str = "die-at-startup";

fn main() -> ExitCode {
    boxt_collector::os::suppress_error_dialogs();
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some(WORKER_SUBCOMMAND) => return boxt_collector::worker::run(),
        Some(DIE_AT_STARTUP) => return ExitCode::from(STARTUP_PROTOCOL_EXIT as u8),
        Some(HOLD_POOL) => {
            return match args.get(1) {
                Some(root) => hold_pool(Path::new(root)),
                None => ExitCode::FAILURE,
            };
        }
        _ => {}
    }
    let filter: Vec<&String> = args.iter().filter(|a| !a.starts_with('-')).collect();
    let tests: &[(&str, fn())] = &[
        ("ordinary_files_all_read", ordinary_files_all_read),
        (
            "a_source_with_facts_and_a_region_is_read",
            a_source_with_facts_and_a_region_is_read,
        ),
        ("segfault_ends_only_its_file", segfault_ends_only_its_file),
        (
            "segfault_before_opened_ends_only_its_file",
            segfault_before_opened_ends_only_its_file,
        ),
        ("abort_ends_only_its_file", abort_ends_only_its_file),
        (
            "stack_overflow_ends_only_its_file",
            stack_overflow_ends_only_its_file,
        ),
        (
            "memory_cap_is_retried_once_and_host_dependent",
            memory_cap_is_retried_once_and_host_dependent,
        ),
        (
            "parser_out_of_memory_hits_the_cap",
            parser_out_of_memory_hits_the_cap,
        ),
        (
            "panic_is_caught_without_a_respawn",
            panic_is_caught_without_a_respawn,
        ),
        (
            "watchdog_is_retried_once_and_host_dependent",
            watchdog_is_retried_once_and_host_dependent,
        ),
        ("half_a_frame_is_a_bad_frame", half_a_frame_is_a_bad_frame),
        (
            "file_over_the_size_limit_is_measured",
            file_over_the_size_limit_is_measured,
        ),
        ("mixed_faults_on_four_workers", mixed_faults_on_four_workers),
        (
            "cap_failure_is_a_typed_startup_error",
            cap_failure_is_a_typed_startup_error,
        ),
        (
            "spawn_failures_stop_after_three",
            spawn_failures_stop_after_three,
        ),
        (
            "startup_deaths_stop_after_three",
            startup_deaths_stop_after_three,
        ),
        (
            "no_worker_outlives_a_killed_parent",
            no_worker_outlives_a_killed_parent,
        ),
    ];
    let mut failed = 0;
    let mut ran = 0;
    for (name, test) in tests {
        if !filter.is_empty() && !filter.iter().any(|f| name.contains(f.as_str())) {
            continue;
        }
        ran += 1;
        let start = Instant::now();
        let result = catch_unwind(AssertUnwindSafe(|| {
            test();
            assert_no_children();
        }));
        let status = if result.is_ok() { "ok" } else { "FAILED" };
        println!(
            "test {name} ... {status} ({:.2}s)",
            start.elapsed().as_secs_f64()
        );
        failed += usize::from(result.is_err());
    }
    println!("{ran} run, {failed} failed");
    if failed == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

struct Tree {
    root: PathBuf,
    jobs: Vec<Job>,
}

impl Drop for Tree {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn ordinary_body(i: usize) -> Vec<u8> {
    format!("int f{i}(int x) {{\n  return x + {i};\n}}\n").into_bytes()
}

fn tree(name: &str, special: &[(String, Vec<u8>)]) -> Tree {
    let root = std::env::temp_dir().join(format!("boxt-faults-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let mut files: Vec<(String, Vec<u8>)> = (0..ORDINARY)
        .map(|i| (format!("ok-{i:02}.c"), ordinary_body(i)))
        .collect();
    let step = ORDINARY / (special.len() + 1);
    for (k, file) in special.iter().enumerate() {
        files.insert(step * (k + 1) + k, file.clone());
    }
    for (file, body) in &files {
        std::fs::write(root.join(file), body).unwrap();
    }
    let input = InputRef {
        index: 0,
        given: text_from_path(&root),
    };
    let jobs = files
        .iter()
        .map(|(file, _)| Job {
            kind: JobKind::Route,
            input: input.clone(),
            file: JobFile::InInput(RelPath::new(Text::from(file.as_str())).unwrap()),
            resume_after: None,
        })
        .collect();
    Tree { root, jobs }
}

fn fixtures(names: &[&str]) -> Vec<(String, Vec<u8>)> {
    names
        .iter()
        .map(|n| (format!("boxt-fixture-{n}.c"), ordinary_body(0)))
        .collect()
}

fn config(limits: Limits, jobs: usize) -> PoolConfig {
    let mut config = PoolConfig::new(
        WorkerSettings {
            limits,
            encodings: Vec::new(),
        },
        None,
    )
    .unwrap();
    config.jobs = NonZeroUsize::new(jobs).unwrap();
    config
}

fn capped(cap: u64) -> Limits {
    let limits = Limits::default()
        .with(LimitName::Memory, cap / 8)
        .unwrap()
        .with(LimitName::MacroTableBytes, cap / 8)
        .unwrap()
        .with(LimitName::WorkerMemoryCap, cap)
        .unwrap();
    limits.validate().unwrap();
    limits
}

fn read_outcome() -> Outcome {
    Outcome::new(FileOutcome::Read, Some(FileKind::Source)).with_lines(3)
}

fn not_checked(reason: NotCheckedReason, host_dependent: bool) -> Outcome {
    let mut outcome = Outcome::new(
        FileOutcome::NotChecked {
            reason,
            details: Details::None,
        },
        Some(FileKind::Source),
    );
    outcome.host_dependent = host_dependent;
    outcome
}

fn report_for<'a>(report: &'a RunReport, file: &str) -> &'a JobReport {
    let reports: Vec<&JobReport> = report
        .jobs
        .iter()
        .filter(|r| {
            r.job
                .path()
                .is_some_and(|p| p.file_name() == file.as_bytes())
        })
        .collect();
    assert_eq!(reports.len(), 1, "{file}");
    reports[0]
}

fn is_ordinary(job: &Job) -> bool {
    job.path()
        .is_some_and(|p| p.file_name().starts_with(b"ok-"))
}

fn assert_others_read(tree: &Tree, report: &RunReport) {
    assert_eq!(report.jobs.len(), tree.jobs.len());
    for (sent, got) in tree.jobs.iter().zip(&report.jobs) {
        assert_eq!(&got.job, sent);
    }
    let others: Vec<&JobReport> = report.jobs.iter().filter(|r| is_ordinary(&r.job)).collect();
    assert_eq!(others.len(), ORDINARY);
    for other in others {
        assert_eq!(other.outcome, read_outcome(), "{:?}", other.job.file);
        assert_eq!(other.attempts, 1);
    }
}

fn run_tree(tree: &Tree, limits: Limits, jobs: usize) -> RunReport {
    let report = pool::run(&config(limits, jobs), tree.jobs.clone()).unwrap();
    assert_others_read(tree, &report);
    report
}

fn run_fixture(fixture: &str, limits: Limits, jobs: usize) -> RunReport {
    run_tree(&tree(fixture, &fixtures(&[fixture])), limits, jobs)
}

fn fixture_file(fixture: &str) -> String {
    format!("boxt-fixture-{fixture}.c")
}

fn a_source_with_facts_and_a_region_is_read() {
    let source = b"#include <unistd.h>\nint main(void) {\n#ifdef ROOT\n  setuid(0);\n#endif\n  return puts(\"hi\");\n}\n\nstatic char *\nf(a)\n\tchar *a;\n{\n  return a;\n}\n";
    let special = vec![("facts.c".to_string(), source.to_vec())];
    let report = run_tree(&tree("facts", &special), Limits::default(), 2);
    let got = report_for(&report, "facts.c");
    assert_eq!(
        got.outcome,
        Outcome::new(FileOutcome::Read, Some(FileKind::Source)).with_lines(14)
    );
    assert_eq!(got.attempts, 1);
}

fn ordinary_files_all_read() {
    let tree = tree("ordinary", &[]);
    let report = pool::run(&config(Limits::default(), 3), tree.jobs.clone()).unwrap();
    assert_eq!(report.read_count(), ORDINARY);
    assert_eq!(report.spawns, 3);
}

fn crash_fixture(fixture: &str) {
    let report = run_fixture(fixture, Limits::default(), 1);
    let got = report_for(&report, &fixture_file(fixture));
    assert_eq!(
        got.outcome,
        not_checked(NotCheckedReason::Fault(FaultKind::Crash), false)
    );
    assert_eq!(got.attempts, 1);
    assert_eq!(report.spawns, 2, "the dead worker is replaced once");
}

fn segfault_before_opened_ends_only_its_file() {
    crash_fixture("early-segfault");
}

fn segfault_ends_only_its_file() {
    crash_fixture("segfault");
}

fn abort_ends_only_its_file() {
    crash_fixture("abort");
}

fn stack_overflow_ends_only_its_file() {
    crash_fixture("stack-overflow");
}

fn memory_cap_is_retried_once_and_host_dependent() {
    let report = run_fixture("alloc", capped(256 * MIB), 1);
    let got = report_for(&report, &fixture_file("alloc"));
    assert_eq!(
        got.outcome,
        not_checked(
            NotCheckedReason::LimitExceeded(LimitName::WorkerMemoryCap),
            true
        )
    );
    assert_eq!(got.attempts, 2);
    assert_eq!(report.spawns, 3);
}

fn parser_out_of_memory_hits_the_cap() {
    let mut source = Vec::new();
    for i in 0..100_000 {
        source.extend_from_slice(format!("int a{i} = f({i}, g({i}));\n").as_bytes());
    }
    let big = vec![("big-parse.c".to_string(), source)];
    let report = run_tree(&tree("parser-oom", &big), capped(128 * MIB), 1);
    let got = report_for(&report, "big-parse.c");
    assert_eq!(
        got.outcome,
        not_checked(
            NotCheckedReason::LimitExceeded(LimitName::WorkerMemoryCap),
            true
        )
    );
    assert_eq!(got.attempts, 2);
}

fn panic_is_caught_without_a_respawn() {
    let report = run_fixture("panic", Limits::default(), 1);
    let got = report_for(&report, &fixture_file("panic"));
    assert_eq!(
        got.outcome,
        not_checked(NotCheckedReason::Fault(FaultKind::Panic), false).with_lines(3)
    );
    assert_eq!(got.attempts, 1);
    assert_eq!(report.spawns, 1, "a caught panic keeps its worker");
}

fn watchdog_is_retried_once_and_host_dependent() {
    let limits = Limits::default().with(LimitName::Watchdog, 3).unwrap();
    let start = Instant::now();
    let report = run_fixture("spin", limits, 1);
    assert!(start.elapsed() >= Duration::from_secs(6));
    let got = report_for(&report, &fixture_file("spin"));
    assert_eq!(
        got.outcome,
        not_checked(NotCheckedReason::LimitExceeded(LimitName::Watchdog), true)
    );
    assert_eq!(got.attempts, 2);
    assert_eq!(report.spawns, 3);
}

fn half_a_frame_is_a_bad_frame() {
    let report = run_fixture("half-frame", Limits::default(), 1);
    let got = report_for(&report, &fixture_file("half-frame"));
    assert_eq!(
        got.outcome,
        not_checked(NotCheckedReason::Fault(FaultKind::BadFrame), false)
    );
    assert_eq!(got.attempts, 1);
    assert_eq!(report.spawns, 2);
}

fn file_over_the_size_limit_is_measured() {
    let mut body = Vec::new();
    while body.len() < 1000 {
        body.extend_from_slice(b"int padding;\n");
    }
    let big = vec![("big-size.c".to_string(), body)];
    let tree = tree("file-size", &big);
    let size = std::fs::metadata(tree.root.join("big-size.c"))
        .unwrap()
        .len();
    let limits = Limits::default().with(LimitName::FileSize, 500).unwrap();
    let report = run_tree(&tree, limits, 2);
    let got = report_for(&report, "big-size.c");
    assert_eq!(
        got.outcome,
        Outcome::new(
            FileOutcome::NotChecked {
                reason: NotCheckedReason::LimitExceeded(LimitName::FileSize),
                details: Details::Measured {
                    value: size,
                    limit: 500
                },
            },
            Some(FileKind::Source),
        )
    );
    assert_eq!(got.attempts, 1);
}

fn mixed_faults_on_four_workers() {
    let names = ["segfault", "abort", "half-frame", "panic", "stack-overflow"];
    let tree = tree("mixed", &fixtures(&names));
    let report = run_tree(&tree, Limits::default(), 4);
    let expect = [
        ("segfault", FaultKind::Crash),
        ("abort", FaultKind::Crash),
        ("half-frame", FaultKind::BadFrame),
        ("panic", FaultKind::Panic),
        ("stack-overflow", FaultKind::Crash),
    ];
    for (fixture, kind) in expect {
        let got = report_for(&report, &fixture_file(fixture));
        let mut expect = not_checked(NotCheckedReason::Fault(kind), false);
        if kind == FaultKind::Panic {
            expect = expect.with_lines(3);
        }
        assert_eq!(got.outcome, expect, "{fixture}");
        assert_eq!(got.attempts, 1, "{fixture}");
    }
}

fn cap_failure_is_a_typed_startup_error() {
    let tree = tree("cap-fails", &[]);
    let mut config = config(Limits::default(), 2);
    config
        .env
        .push((OsString::from(FAIL_CAP_ENV), OsString::from("1")));
    assert_eq!(
        pool::run(&config, tree.jobs.clone()),
        Err(PoolError::WorkerMemoryCap)
    );
}

fn spawn_failures_stop_after_three() {
    let tree = tree("spawn-fails", &[]);
    let mut config = config(Limits::default(), 1);
    config.program = tree.root.join("no-such-worker");
    match pool::run(&config, tree.jobs.clone()) {
        Err(PoolError::SpawnFailed { attempts, .. }) => assert_eq!(attempts, MAX_FAILED_SPAWNS),
        other => panic!("expected a spawn failure, got {other:?}"),
    }
    assert_eq!(MAX_FAILED_SPAWNS, 3);
}

fn startup_deaths_stop_after_three() {
    let tree = tree("startup-deaths", &[]);
    let mut config = config(Limits::default(), 2);
    config.args = vec![OsString::from(DIE_AT_STARTUP)];
    match pool::run(&config, tree.jobs.clone()) {
        Err(PoolError::SpawnFailed { attempts, .. }) => assert_eq!(attempts, MAX_FAILED_SPAWNS),
        other => panic!("expected a spawn failure, got {other:?}"),
    }
}

fn hold_pool(root: &Path) -> ExitCode {
    let input = InputRef {
        index: 0,
        given: text_from_path(root),
    };
    let jobs = (0..2)
        .map(|_| Job {
            kind: JobKind::Route,
            input: input.clone(),
            file: JobFile::InInput(RelPath::new(Text::from("boxt-fixture-spin.c")).unwrap()),
            resume_after: None,
        })
        .collect();
    let _ = pool::run(&config(Limits::default(), 2), jobs);
    ExitCode::SUCCESS
}

fn no_worker_outlives_a_killed_parent() {
    let tree = tree("orphans", &fixtures(&["spin"]));
    let pids = tree.root.join("pids");
    std::fs::create_dir_all(&pids).unwrap();
    let mut parent = std::process::Command::new(std::env::current_exe().unwrap())
        .arg(HOLD_POOL)
        .arg(&tree.root)
        .env(PID_DIR_ENV, &pids)
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    let workers = loop {
        let workers: Vec<u32> = std::fs::read_dir(&pids)
            .unwrap()
            .flatten()
            .filter_map(|e| e.file_name().to_str().and_then(|s| s.parse().ok()))
            .collect();
        if workers.len() == 2 || Instant::now() > deadline {
            break workers;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    stop_parent(&mut parent);
    let _ = parent.wait();
    let deadline = Instant::now() + Duration::from_secs(10);
    while workers.iter().any(|&w| running(w)) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    let survivors: Vec<u32> = workers.iter().copied().filter(|&w| running(w)).collect();
    for &w in &survivors {
        kill(w);
    }
    assert_eq!(workers.len(), 2, "both workers started");
    assert!(survivors.is_empty(), "a worker outlived its parent");
}

#[cfg(unix)]
fn stop_parent(parent: &mut std::process::Child) {
    use rustix::process::{Pid, Signal, kill_process};
    let pid = Pid::from_raw(parent.id() as i32).unwrap();
    let _ = kill_process(pid, Signal::INT);
}

#[cfg(windows)]
fn stop_parent(parent: &mut std::process::Child) {
    let _ = parent.kill();
}

#[cfg(unix)]
fn kill(pid: u32) {
    use rustix::process::{Pid, Signal, kill_process};
    let _ = kill_process(Pid::from_raw(pid as i32).unwrap(), Signal::KILL);
}

#[cfg(windows)]
fn kill(pid: u32) {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_TERMINATE, TerminateProcess};
    unsafe {
        let handle = OpenProcess(PROCESS_TERMINATE, 0, pid);
        if !handle.is_null() {
            TerminateProcess(handle, 1);
            CloseHandle(handle);
        }
    }
}

#[cfg(target_os = "linux")]
fn running(pid: u32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/stat"))
        .ok()
        .and_then(|stat| stat.rfind(')').map(|i| stat[i + 2..i + 3].to_string()))
        .is_some_and(|state| state != "Z")
}

#[cfg(windows)]
fn running(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    const STILL_ACTIVE: u32 = 259;
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return false;
        }
        let mut code = 0u32;
        let ok = GetExitCodeProcess(handle, &mut code) != 0;
        CloseHandle(handle);
        ok && code == STILL_ACTIVE
    }
}

#[cfg(target_os = "linux")]
fn children_of(pid: u32) -> Vec<u32> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir("/proc").unwrap().flatten() {
        let Some(child) = entry
            .file_name()
            .to_str()
            .and_then(|s| s.parse::<u32>().ok())
        else {
            continue;
        };
        let Ok(stat) = std::fs::read_to_string(entry.path().join("stat")) else {
            continue;
        };
        let Some(after) = stat.rfind(')').map(|i| &stat[i + 2..]) else {
            continue;
        };
        let fields: Vec<&str> = after.split(' ').collect();
        if fields.get(1).and_then(|p| p.parse::<u32>().ok()) == Some(pid) && fields[0] != "Z" {
            out.push(child);
        }
    }
    out
}

#[cfg(target_os = "linux")]
fn assert_no_children() {
    assert!(
        children_of(std::process::id()).is_empty(),
        "a worker outlived its run"
    );
}

#[cfg(not(target_os = "linux"))]
fn assert_no_children() {}
