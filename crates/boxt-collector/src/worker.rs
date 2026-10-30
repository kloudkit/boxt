use std::io::{self, BufWriter, Read, Write};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::process::ExitCode;

use boxt_core::fact::FileKind;
use boxt_core::limits::{LimitName, Limits};
use boxt_core::location::Scope;
use boxt_core::message::{
    CollectorMsg, EntryPath, Job, JobKind, WorkerMsg, decode_frame, encode_frame,
};
use boxt_core::outcome::{Details, FaultKind, FileOutcome, NotCheckedReason, Outcome, line_count};
use boxt_core::settings::WorkerSettings;
use boxt_read_cpp::Grammar;

use crate::path::job_path;
use crate::{alloc, os};

pub const WORKER_SUBCOMMAND: &str = "worker";

pub const STARTUP_CAP_EXIT: i32 = 114;
pub const STARTUP_PROTOCOL_EXIT: i32 = 115;

pub const PARENT_ENV: &str = "BOXT_WORKER_PARENT";

#[cfg(feature = "fault-fixtures")]
pub const FAIL_CAP_ENV: &str = "BOXT_FIXTURE_FAIL_CAP";

#[cfg(feature = "fault-fixtures")]
pub const PID_DIR_ENV: &str = "BOXT_FIXTURE_PID_DIR";

pub fn run() -> ExitCode {
    os::suppress_error_dialogs();
    alloc::enter_worker();
    let parent = std::env::var(PARENT_ENV)
        .ok()
        .and_then(|pid| pid.parse::<u32>().ok());
    if let Err(e) = os::die_with_parent(parent) {
        eprintln!("boxt worker: cannot tie its life to its parent: {e}");
        return ExitCode::from(STARTUP_PROTOCOL_EXIT as u8);
    }
    alloc::install_parser_allocator();

    let mut input = io::stdin().lock();
    let mut output = BufWriter::new(io::stdout().lock());

    let first_max = Limits::default().get(LimitName::EntryMessageBytes);
    let settings = match decode_frame::<CollectorMsg, _>(&mut input, first_max) {
        Ok(Some(decoded)) => match decoded.msg {
            CollectorMsg::Settings(settings) => settings,
            _ => {
                eprintln!("boxt worker: the first message was not the settings");
                return ExitCode::from(STARTUP_PROTOCOL_EXIT as u8);
            }
        },
        Ok(None) => return ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("boxt worker: cannot read the settings: {e}");
            return ExitCode::from(STARTUP_PROTOCOL_EXIT as u8);
        }
    };

    if let Err(e) = set_cap(&settings) {
        eprintln!("boxt worker: cannot set its memory cap: {e}");
        return ExitCode::from(STARTUP_CAP_EXIT as u8);
    }

    let max_len = settings.limits.get(LimitName::EntryMessageBytes);
    loop {
        let msg = match decode_frame::<CollectorMsg, _>(&mut input, max_len) {
            Ok(Some(decoded)) => decoded.msg,
            Ok(None) => return ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("boxt worker: bad frame from the collector: {e}");
                return ExitCode::from(STARTUP_PROTOCOL_EXIT as u8);
            }
        };
        match msg {
            CollectorMsg::Job(job) => {
                if run_job(&job, &settings, &mut output).is_err() {
                    return ExitCode::SUCCESS;
                }
            }
            CollectorMsg::Settings(_) => {
                eprintln!("boxt worker: a second settings message");
                return ExitCode::from(STARTUP_PROTOCOL_EXIT as u8);
            }
            CollectorMsg::Macros { .. } => {}
        }
    }
}

fn set_cap(settings: &WorkerSettings) -> io::Result<()> {
    #[cfg(feature = "fault-fixtures")]
    if std::env::var_os(FAIL_CAP_ENV).is_some() {
        return Err(io::Error::other("fixture forced the cap to fail"));
    }
    os::set_memory_cap(settings.limits.get(LimitName::WorkerMemoryCap))
}

fn send(out: &mut impl Write, msg: &WorkerMsg, max_len: u64) -> io::Result<()> {
    let frame = encode_frame(msg, max_len).map_err(io::Error::other)?;
    out.write_all(&frame)
}

fn run_job(job: &Job, settings: &WorkerSettings, out: &mut impl Write) -> io::Result<()> {
    let max_len = settings.limits.get(LimitName::EntryMessageBytes);
    let outcome = if job.kind == JobKind::Route {
        let routed = catch_unwind(AssertUnwindSafe(|| route(job, settings, out)));
        match routed {
            Ok(result) => result?,
            Err(_) => Outcome::new(
                FileOutcome::NotChecked {
                    reason: NotCheckedReason::Fault(FaultKind::Panic),
                    details: Details::None,
                },
                file_kind_by_name(job),
            ),
        }
    } else {
        not_checked(NotCheckedReason::NoReader, None)
    };
    send(
        out,
        &WorkerMsg::JobDone {
            outcome: Some(outcome),
        },
        max_len,
    )?;
    out.flush()
}

pub fn file_kind_by_name(job: &Job) -> Option<FileKind> {
    let path = job.path()?;
    Grammar::for_file_name(path.file_name()).map(|_| FileKind::Source)
}

fn not_checked(reason: NotCheckedReason, file_kind: Option<FileKind>) -> Outcome {
    Outcome::new(
        FileOutcome::NotChecked {
            reason,
            details: Details::None,
        },
        file_kind,
    )
}

fn route(job: &Job, settings: &WorkerSettings, out: &mut impl Write) -> io::Result<Outcome> {
    let max_len = settings.limits.get(LimitName::EntryMessageBytes);
    let path = job.path().cloned();
    let grammar = path
        .as_ref()
        .and_then(|p| Grammar::for_file_name(p.file_name()));
    let file_kind = grammar.map(|_| FileKind::Source);
    send(
        out,
        &WorkerMsg::Opened {
            entry: EntryPath::job(),
            frames: Vec::new(),
            path: path.clone(),
            scope: Scope::File,
            file_kind,
        },
        max_len,
    )?;

    #[cfg(feature = "fault-fixtures")]
    if let Some(name) = path.as_ref().map(|p| p.file_name()) {
        out.flush()?;
        fixtures::trigger(name, out, max_len);
    }

    let Some(grammar) = grammar else {
        return Ok(not_checked(NotCheckedReason::NoReader, None));
    };
    let Some(on_disk) = job_path(job) else {
        return Ok(not_checked(NotCheckedReason::NoReader, file_kind));
    };
    let max_size = settings.limits.get(LimitName::FileSize);
    let bytes = match read_bounded(&on_disk, max_size) {
        Ok(Ok(bytes)) => bytes,
        Ok(Err(size)) => {
            return Ok(Outcome::new(
                FileOutcome::NotChecked {
                    reason: NotCheckedReason::LimitExceeded(LimitName::FileSize),
                    details: Details::Measured {
                        value: size,
                        limit: max_size,
                    },
                },
                file_kind,
            ));
        }
        Err(_) => return Ok(not_checked(NotCheckedReason::NoReader, file_kind)),
    };
    let lines = line_count(&bytes, None);
    let outcome = match boxt_read_cpp::parse(&bytes, grammar) {
        Ok(_) => Outcome::new(FileOutcome::Read, file_kind),
        Err(_) => not_checked(NotCheckedReason::Unparseable, file_kind),
    };
    Ok(outcome.with_lines(lines))
}

fn read_bounded(path: &std::path::Path, max: u64) -> io::Result<Result<Vec<u8>, u64>> {
    let file = std::fs::File::open(path)?;
    let size = file.metadata()?.len();
    if size > max {
        return Ok(Err(size));
    }
    let mut bytes = Vec::new();
    file.take(max.saturating_add(1)).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > max {
        return Ok(Err(size.max(bytes.len() as u64)));
    }
    Ok(Ok(bytes))
}

#[cfg(feature = "fault-fixtures")]
mod fixtures {
    #[cfg(not(debug_assertions))]
    compile_error!("the fault-fixtures feature is for test builds only");

    use std::hint::black_box;
    use std::io::Write;

    use boxt_core::message::{WorkerMsg, encode_frame};

    pub const PREFIX: &[u8] = b"boxt-fixture-";

    pub fn trigger(file_name: &[u8], out: &mut impl Write, max_len: u64) {
        let Some(rest) = file_name.strip_prefix(PREFIX) else {
            return;
        };
        let stem = rest.split(|&b| b == b'.').next().unwrap_or(rest);
        match stem {
            b"segfault" => segfault(),
            b"abort" => std::process::abort(),
            b"stack-overflow" => {
                black_box(recurse(black_box(1)));
            }
            b"alloc" => allocate_forever(),
            b"spin" => {
                record_pid();
                loop {
                    std::hint::spin_loop();
                }
            }
            b"panic" => panic!("fixture panic"),
            b"half-frame" => half_frame(out, max_len),
            _ => {}
        }
    }

    fn record_pid() {
        if let Some(dir) = std::env::var_os(super::PID_DIR_ENV) {
            let pid = std::process::id().to_string();
            let _ = std::fs::write(std::path::Path::new(&dir).join(pid), b"");
        }
    }

    fn segfault() {
        let at = std::ptr::without_provenance::<u8>(black_box(8));
        black_box(unsafe { std::ptr::read_volatile(at) });
    }

    #[allow(unconditional_recursion)]
    fn recurse(n: u64) -> u64 {
        let frame = black_box([n; 128]);
        recurse(frame[0].wrapping_add(1)).wrapping_add(frame[127])
    }

    fn allocate_forever() {
        const CHUNK: usize = 16 * 1024 * 1024;
        let mut held: Vec<Vec<u8>> = Vec::new();
        loop {
            let mut chunk = vec![0u8; CHUNK];
            for page in chunk.chunks_mut(4096) {
                page[0] = 1;
            }
            held.push(black_box(chunk));
        }
    }

    fn half_frame(out: &mut impl Write, max_len: u64) {
        let frame = encode_frame(&WorkerMsg::JobDone { outcome: None }, max_len)
            .expect("a small frame encodes");
        let _ = out.write_all(&frame[..frame.len() / 2]);
        let _ = out.flush();
        std::process::exit(0);
    }
}
