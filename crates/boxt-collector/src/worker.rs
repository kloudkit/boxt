use std::io::{self, BufWriter, Write};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::process::ExitCode;

use boxt_core::fact::{Fact, FileKind};
use boxt_core::limits::{LimitName, Limits};
use boxt_core::location::{Location, Scope};
use boxt_core::message::{
    CollectorMsg, EntryBudget, EntryPath, FRAME_PREFIX_LEN, FrameError, Job, JobFile, JobKind,
    MacroDef, MacroTables, Note, Region, WorkerMsg, decode_frame, encode_definitions, encode_frame,
};
use boxt_core::outcome::{
    Details, FaultKind, FileOutcome, IoKind, NotCheckedReason, Outcome, line_count,
};
use boxt_core::read::{Exceeded, FactSink, NotRead, ReadCtx, Reader, WorkBudget};
use boxt_core::settings::{Encoding, WorkerSettings};
use boxt_read_cpp::CppReader;

use crate::path::{declared_for, job_path, text_from_path};
pub use crate::router::file_kind_by_name;
use crate::router::{ByName, Fetched, Routed, by_name, fetch, route_content};
use crate::{alloc, os};

pub const WORKER_SUBCOMMAND: &str = "__worker";

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

    os::commit_stack();
    if let Err(e) = set_cap(&settings) {
        eprintln!("boxt worker: cannot set its memory cap: {e}");
        return ExitCode::from(STARTUP_CAP_EXIT as u8);
    }

    let max_len = settings.limits.get(LimitName::EntryMessageBytes);
    let mut tables = MacroTables::new();
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
                if run_job(&job, &settings, &tables, &mut output).is_err() {
                    return ExitCode::SUCCESS;
                }
            }
            CollectorMsg::Settings(_) => {
                eprintln!("boxt worker: a second settings message");
                return ExitCode::from(STARTUP_PROTOCOL_EXIT as u8);
            }
            CollectorMsg::Macros {
                tree,
                definitions,
                end,
            } => {
                if let Err(e) = tables.receive(tree, definitions, end) {
                    eprintln!("boxt worker: {e}");
                    return ExitCode::from(STARTUP_PROTOCOL_EXIT as u8);
                }
            }
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

pub fn run_job(
    job: &Job,
    settings: &WorkerSettings,
    tables: &MacroTables,
    out: &mut impl Write,
) -> io::Result<()> {
    let max_len = settings.limits.get(LimitName::EntryMessageBytes);
    let outcome = match job.kind {
        JobKind::Route => route(job, settings, tables, out)?,
        JobKind::Definitions => definitions(job, settings, out)?,
        JobKind::CompileDatabase => not_checked(NotCheckedReason::NoReader, None),
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

fn not_checked(reason: NotCheckedReason, file_kind: Option<FileKind>) -> Outcome {
    Outcome::new(
        FileOutcome::NotChecked {
            reason,
            details: Details::None,
        },
        file_kind,
    )
}

fn panicked(file_kind: Option<FileKind>, lines: Option<u64>) -> Outcome {
    let outcome = not_checked(NotCheckedReason::Fault(FaultKind::Panic), file_kind);
    match lines {
        Some(lines) => outcome.with_lines(lines),
        None => outcome,
    }
}

struct Loaded {
    name: ByName,
    bytes: Vec<u8>,
    encoding: Option<Encoding>,
    location: Location,
    lines: u64,
}

enum Prepared {
    Done(Outcome),
    Cxx(Box<(Loaded, WorkBudget)>),
}

fn job_file_on_disk(job: &Job) -> Option<std::path::PathBuf> {
    let path = job_path(job)?;
    match job.file {
        JobFile::WholeInput(_) => std::fs::canonicalize(&path).ok(),
        _ => Some(path),
    }
}

fn load(job: &Job, settings: &WorkerSettings) -> Result<Loaded, Outcome> {
    let Some(path) = job.path().cloned() else {
        return Err(not_checked(NotCheckedReason::NoReader, None));
    };
    let name = by_name(path.file_name());
    if name == ByName::Other {
        return Err(not_checked(NotCheckedReason::NoReader, None));
    }
    let kind_by_name = file_kind_by_name(job);
    let Some(on_disk) = job_file_on_disk(job) else {
        return Err(not_checked(
            NotCheckedReason::Unreadable(IoKind::NotFound),
            kind_by_name,
        ));
    };
    let max_size = settings.limits.get(LimitName::FileSize);
    let bytes = match fetch(&on_disk, max_size, kind_by_name) {
        Fetched::Bytes(bytes) => bytes,
        Fetched::NotChecked(file, kind) => return Err(Outcome::new(file, kind)),
    };
    let on_disk_text = text_from_path(&on_disk);
    let encoding = settings.resolve_encoding(|p| declared_for(p, &on_disk_text));
    let lines = line_count(&bytes, encoding);
    Ok(Loaded {
        name,
        bytes,
        encoding,
        location: Location::file(job.input.clone(), Vec::new(), path),
        lines,
    })
}

fn identify(loaded: Loaded, settings: &WorkerSettings) -> Prepared {
    let mut ctx = ReadCtx::new(loaded.location.clone(), &settings.limits, loaded.encoding);
    let routed = route_content(&loaded.bytes, &mut ctx, loaded.name);
    let lines = loaded.lines;
    match routed {
        Routed::Cxx => Prepared::Cxx(Box::new((loaded, ctx.into_budget()))),
        Routed::NoReader => {
            Prepared::Done(not_checked(NotCheckedReason::NoReader, None).with_lines(lines))
        }
        Routed::NotRead(not_read, kind) => {
            Prepared::Done(Outcome::new(not_read.outcome(), kind).with_lines(lines))
        }
    }
}

fn prepare(job: &Job, settings: &WorkerSettings) -> Prepared {
    let loaded = match load(job, settings) {
        Ok(loaded) => loaded,
        Err(outcome) => return Prepared::Done(outcome),
    };
    let lines = loaded.lines;
    let kind = if loaded.name == ByName::Listed {
        Some(FileKind::Source)
    } else {
        None
    };
    catch_unwind(AssertUnwindSafe(|| identify(loaded, settings)))
        .unwrap_or_else(|_| Prepared::Done(panicked(kind, Some(lines))))
}

fn route(
    job: &Job,
    settings: &WorkerSettings,
    tables: &MacroTables,
    out: &mut impl Write,
) -> io::Result<Outcome> {
    let max_len = settings.limits.get(LimitName::EntryMessageBytes);

    #[cfg(feature = "fault-fixtures")]
    if let Some(name) = job.path().map(|p| p.file_name()) {
        fixtures::trigger_early(name, job, out, max_len);
    }

    let prepared = prepare(job, settings);
    let file_kind = match &prepared {
        Prepared::Done(outcome) => outcome.file_kind,
        Prepared::Cxx(..) => Some(FileKind::Source),
    };
    send(
        out,
        &WorkerMsg::Opened {
            entry: EntryPath::job(),
            frames: Vec::new(),
            path: job.path().cloned(),
            scope: Scope::File,
            file_kind,
        },
        max_len,
    )?;

    #[cfg(feature = "fault-fixtures")]
    if let Some(name) = job.path().map(|p| p.file_name()) {
        out.flush()?;
        if catch_unwind(AssertUnwindSafe(|| fixtures::trigger(name, out, max_len))).is_err() {
            let lines = match &prepared {
                Prepared::Done(outcome) => outcome.lines.map(|l| l.lines),
                Prepared::Cxx(ready) => Some(ready.0.lines),
            };
            return Ok(panicked(file_kind, lines));
        }
    }

    let (loaded, budget) = match prepared {
        Prepared::Done(outcome) => return Ok(outcome),
        Prepared::Cxx(ready) => *ready,
    };
    let lines = loaded.lines;
    let read = catch_unwind(AssertUnwindSafe(|| {
        let ctx = ReadCtx::with_budget(loaded.location, loaded.encoding, budget);
        let mut ctx = match tables.for_tree(job.input.index) {
            Some(table) => ctx.with_macros(table),
            None => ctx,
        };
        let mut sink = FrameSink::new(&mut *out, &settings.limits);
        let read = CppReader.read(&loaded.bytes, &mut ctx, &mut sink);
        let read = read.and_then(|()| sink.flush().map_err(NotRead::from));
        match sink.failed.take() {
            Some(error) => Err(error),
            None => Ok(read),
        }
    }));
    let file = match read {
        Ok(Ok(Ok(()))) => FileOutcome::Read,
        Ok(Ok(Err(not_read))) => not_read.outcome(),
        Ok(Err(error)) => return Err(error),
        Err(_) => return Ok(panicked(file_kind, Some(lines))),
    };
    Ok(Outcome::new(file, file_kind).with_lines(lines))
}

fn definitions(job: &Job, settings: &WorkerSettings, out: &mut impl Write) -> io::Result<Outcome> {
    let max_len = settings.limits.get(LimitName::EntryMessageBytes);

    #[cfg(feature = "fault-fixtures")]
    if let Some(name) = job.path().map(|p| p.file_name()) {
        fixtures::trigger_definitions(name);
    }

    let loaded = match prepare(job, settings) {
        Prepared::Done(outcome) => return Ok(outcome),
        Prepared::Cxx(ready) => ready.0,
    };
    let lines = loaded.lines;
    let read = |file| Outcome::new(file, Some(FileKind::Source)).with_lines(lines);
    let found = catch_unwind(AssertUnwindSafe(|| {
        boxt_read_cpp::definitions(&loaded.bytes, loaded.encoding, &loaded.location)
    }));
    let defs = match found {
        Ok(Ok(defs)) => defs,
        Ok(Err(reason)) => {
            return Ok(read(FileOutcome::NotChecked {
                reason,
                details: Details::None,
            }));
        }
        Err(_) => return Ok(panicked(Some(FileKind::Source), Some(lines))),
    };
    let total = defs.len();
    let valid: Vec<MacroDef> = defs
        .into_iter()
        .filter(|def| def.validate(job, max_len).is_ok())
        .collect();
    let dropped = (total - valid.len()) as u64;
    let mut entry = EntryBudget::new(&settings.limits);
    let mut frames = if valid.is_empty() {
        Vec::new()
    } else {
        encode_definitions(&valid, max_len).map_err(io::Error::other)?
    };
    let charge = WorkerMsg::Definitions {
        definitions: Vec::new(),
    };
    let mut charges: Vec<WorkerMsg> = vec![charge; frames.len()];
    if dropped > 0 {
        let note = WorkerMsg::Notes {
            entry: EntryPath::job(),
            notes: vec![Note::DroppedDefinitions(dropped)],
        };
        frames.push(encode_frame(&note, max_len).map_err(io::Error::other)?);
        charges.push(note);
    }
    for (frame, msg) in frames.iter().zip(&charges) {
        let payload_len = frame.len().saturating_sub(FRAME_PREFIX_LEN) as u64;
        if entry.charge_frame(msg, payload_len).is_err() {
            break;
        }
        out.write_all(frame)?;
    }
    if let Some(exceeded) = entry.exceeded_outcome() {
        return Ok(read(exceeded));
    }
    Ok(read(FileOutcome::Read))
}

const FACTS_PER_FRAME: usize = 1024;
const REGIONS_PER_FRAME: usize = 1024;

struct FrameSink<'w, W: Write> {
    out: &'w mut W,
    budget: EntryBudget,
    max_len: u64,
    facts: Vec<Fact>,
    regions: Vec<Region>,
    failed: Option<io::Error>,
}

impl<'w, W: Write> FrameSink<'w, W> {
    fn new(out: &'w mut W, limits: &Limits) -> Self {
        FrameSink {
            out,
            budget: EntryBudget::new(limits),
            max_len: limits.get(LimitName::EntryMessageBytes),
            facts: Vec::new(),
            regions: Vec::new(),
            failed: None,
        }
    }

    fn over(&self) -> Exceeded {
        match self.budget.exceeded_outcome() {
            Some(FileOutcome::NotChecked {
                reason: NotCheckedReason::LimitExceeded(limit),
                details: Details::Measured { value, limit: max },
            }) => Exceeded { limit, value, max },
            _ => Exceeded {
                limit: LimitName::EntryMessageBytes,
                value: self.max_len.saturating_add(1),
                max: self.max_len,
            },
        }
    }

    fn send(&mut self, msg: &WorkerMsg) -> Result<(), Exceeded> {
        if self.failed.is_some() {
            return Err(self.over());
        }
        let frame = match encode_frame(msg, self.max_len) {
            Ok(frame) => frame,
            Err(FrameError::TooLong { len, .. }) => {
                let _ = self.budget.charge_frame(msg, len);
                return Err(self.over());
            }
            Err(e) => {
                self.failed = Some(io::Error::other(e));
                return Err(self.over());
            }
        };
        let payload_len = frame.len().saturating_sub(4) as u64;
        if self.budget.charge_frame(msg, payload_len).is_err() {
            return Err(self.over());
        }
        if let Err(e) = self.out.write_all(&frame) {
            self.failed = Some(e);
            return Err(self.over());
        }
        Ok(())
    }

    fn flush_facts(&mut self) -> Result<(), Exceeded> {
        if self.facts.is_empty() {
            return Ok(());
        }
        let msg = WorkerMsg::Facts {
            entry: EntryPath::job(),
            facts: std::mem::take(&mut self.facts),
        };
        self.send(&msg)
    }

    fn flush_regions(&mut self) -> Result<(), Exceeded> {
        if self.regions.is_empty() {
            return Ok(());
        }
        let msg = WorkerMsg::Regions {
            entry: EntryPath::job(),
            regions: std::mem::take(&mut self.regions),
        };
        self.send(&msg)
    }

    fn flush(&mut self) -> Result<(), Exceeded> {
        self.flush_facts()?;
        self.flush_regions()
    }
}

impl<W: Write> FactSink for FrameSink<'_, W> {
    fn fact(&mut self, fact: Fact) -> Result<(), Exceeded> {
        self.facts.push(fact);
        if self.facts.len() >= FACTS_PER_FRAME {
            self.flush_facts()?;
        }
        Ok(())
    }

    fn region(&mut self, region: Region) -> Result<(), Exceeded> {
        self.flush_facts()?;
        self.regions.push(region);
        if self.regions.len() >= REGIONS_PER_FRAME {
            self.flush_regions()?;
        }
        Ok(())
    }

    fn note(&mut self, _: Note) -> Result<(), Exceeded> {
        Ok(())
    }
}

#[cfg(feature = "fault-fixtures")]
mod fixtures {
    #[cfg(not(debug_assertions))]
    compile_error!("the fault-fixtures feature is for test builds only");

    use std::hint::black_box;
    use std::io::Write;

    use boxt_core::location::Scope;
    use boxt_core::message::{EntryPath, Job, WorkerMsg, encode_frame};
    use boxt_core::outcome::{FileOutcome, Outcome};

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

    pub fn trigger_early(file_name: &[u8], job: &Job, out: &mut impl Write, max_len: u64) {
        let Some(rest) = file_name.strip_prefix(PREFIX) else {
            return;
        };
        let stem = rest.split(|&b| b == b'.').next().unwrap_or(rest);
        match stem {
            b"early-segfault" => segfault(),
            b"container-crash" => container_crash(job, out, max_len),
            _ => {}
        }
    }

    pub fn trigger_definitions(file_name: &[u8]) {
        let Some(rest) = file_name.strip_prefix(PREFIX) else {
            return;
        };
        let stem = rest.split(|&b| b == b'.').next().unwrap_or(rest);
        if stem == b"defs-spin" {
            loop {
                std::hint::spin_loop();
            }
        }
    }

    fn container_crash(job: &Job, out: &mut impl Write, max_len: u64) {
        let opened = |entry: EntryPath| WorkerMsg::Opened {
            entry,
            frames: Vec::new(),
            path: job.path().cloned(),
            scope: Scope::File,
            file_kind: None,
        };
        let mut msgs = Vec::new();
        if job.resume_after.is_none() {
            msgs.push(opened(EntryPath::job()));
            msgs.push(opened(EntryPath::from([0])));
            msgs.push(WorkerMsg::EntryDone {
                entry: EntryPath::from([0]),
                outcome: Outcome::new(FileOutcome::Read, None),
                bytes: 0,
            });
            msgs.push(opened(EntryPath::from([1])));
        } else {
            msgs.push(opened(EntryPath::from([2])));
        }
        for msg in msgs {
            let frame = encode_frame(&msg, max_len).expect("a small frame encodes");
            let _ = out.write_all(&frame);
        }
        let _ = out.flush();
        segfault();
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

#[cfg(test)]
mod tests {
    use boxt_core::Text;
    use boxt_core::location::{InputRef, RelPath};
    use boxt_core::message::{Job, JobFile, JobKind, WorkerMsg, decode_frame};
    use boxt_core::settings::{Encoding, EncodingDecl, PathPattern};

    use super::*;
    use crate::path::text_from_path;

    struct Dir(std::path::PathBuf);

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn dir(name: &str, files: &[(&str, &[u8])]) -> Dir {
        let root = std::env::temp_dir().join(format!("boxt-worker-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        for (file, body) in files {
            std::fs::write(root.join(file), body).unwrap();
        }
        Dir(root)
    }

    fn job(root: &Dir, file: &str) -> Job {
        Job {
            kind: JobKind::Route,
            input: InputRef {
                index: 0,
                given: text_from_path(&root.0),
            },
            file: JobFile::InInput(RelPath::new(Text::from(file)).unwrap()),
            resume_after: None,
        }
    }

    fn settings(limits: Limits, encodings: Vec<EncodingDecl>) -> WorkerSettings {
        WorkerSettings { limits, encodings }
    }

    fn messages(bytes: &[u8]) -> Vec<WorkerMsg> {
        let mut input = bytes;
        let mut out = Vec::new();
        while let Some(decoded) = decode_frame::<WorkerMsg, _>(&mut input, u64::MAX).unwrap() {
            out.push(decoded.msg);
        }
        out
    }

    fn outcome(msgs: &[WorkerMsg]) -> Outcome {
        match msgs.last() {
            Some(WorkerMsg::JobDone {
                outcome: Some(outcome),
            }) => outcome.clone(),
            other => panic!("no JobDone: {other:?}"),
        }
    }

    const SOURCE: &[u8] = b"#include <unistd.h>\nint main(void) {\n#ifdef ROOT\n  setuid(0);\n#endif\n  return puts(\"hi\");\n}\n\nstatic char *\nf(a)\n\tchar *a;\n{\n  return a;\n}\n";

    #[test]
    fn a_source_job_streams_facts_and_regions_then_its_outcome() {
        let root = dir("stream", &[("a.c", SOURCE)]);
        let mut out = Vec::new();
        run_job(
            &job(&root, "a.c"),
            &settings(Limits::default(), Vec::new()),
            &MacroTables::new(),
            &mut out,
        )
        .unwrap();
        let msgs = messages(&out);
        assert!(matches!(msgs[0], WorkerMsg::Opened { .. }));
        let facts: Vec<&Fact> = msgs
            .iter()
            .filter_map(|m| match m {
                WorkerMsg::Facts { facts, .. } => Some(facts),
                _ => None,
            })
            .flatten()
            .collect();
        assert!(facts.iter().all(|f| f.validate().is_ok()));
        assert!(facts.iter().any(|f| matches!(f,
            Fact::Call { callee, .. } if callee == &Text::from("setuid"))));
        assert!(facts.iter().any(|f| matches!(f, Fact::Include { .. })));
        assert!(msgs.iter().any(|m| matches!(m,
            WorkerMsg::Regions { regions, .. } if !regions.is_empty())));
        let done = outcome(&msgs);
        assert_eq!(done.file, FileOutcome::Read);
        assert_eq!(done.file_kind, Some(FileKind::Source));
        assert_eq!(done.lines.map(|l| (l.lines, l.complete)), Some((14, true)));
    }

    #[test]
    fn the_declared_encoding_reaches_the_reader() {
        let latin = b"const char *s = \"caf\xe9\";\n";
        let root = dir("encoding", &[("a.c", latin)]);
        let mut out = Vec::new();
        run_job(
            &job(&root, "a.c"),
            &settings(Limits::default(), Vec::new()),
            &MacroTables::new(),
            &mut out,
        )
        .unwrap();
        let undeclared = outcome(&messages(&out));
        assert_eq!(
            undeclared.file,
            FileOutcome::NotChecked {
                reason: NotCheckedReason::EncodingUndeclared,
                details: Details::None
            }
        );
        assert_eq!(undeclared.lines.map(|l| l.lines), Some(1));

        let decl = EncodingDecl {
            pattern: PathPattern {
                base: text_from_path(&root.0),
                pattern: Text::from("**/*.c"),
            },
            encoding: Encoding::Latin1,
        };
        let mut out = Vec::new();
        run_job(
            &job(&root, "a.c"),
            &settings(Limits::default(), vec![decl]),
            &MacroTables::new(),
            &mut out,
        )
        .unwrap();
        let msgs = messages(&out);
        assert_eq!(outcome(&msgs).file, FileOutcome::Read);
        assert!(
            msgs.iter()
                .any(|m| matches!(m, WorkerMsg::Facts { facts, .. }
            if facts.iter().any(|f| matches!(f,
                Fact::StringLiteral { value, .. } if value == &Text::from("café")))))
        );
    }

    #[test]
    fn a_fact_count_over_the_entry_limit_is_not_checked() {
        let root = dir("facts", &[("a.c", SOURCE)]);
        let limits = Limits::default().with(LimitName::EntryFacts, 2).unwrap();
        let mut out = Vec::new();
        run_job(
            &job(&root, "a.c"),
            &settings(limits, Vec::new()),
            &MacroTables::new(),
            &mut out,
        )
        .unwrap();
        let done = outcome(&messages(&out));
        assert!(matches!(
            done.file,
            FileOutcome::NotChecked {
                reason: NotCheckedReason::LimitExceeded(LimitName::EntryFacts),
                ..
            }
        ));
        assert_eq!(done.lines.map(|l| l.lines), Some(14));
    }

    #[test]
    fn many_facts_go_out_in_bounded_frames() {
        let mut src = String::from("void f(void) {\n");
        for i in 0..1500 {
            src += &format!("  g({i});\n");
        }
        src += "}\n";
        let root = dir("many", &[("a.c", src.as_bytes())]);
        let mut out = Vec::new();
        run_job(
            &job(&root, "a.c"),
            &settings(Limits::default(), Vec::new()),
            &MacroTables::new(),
            &mut out,
        )
        .unwrap();
        let msgs = messages(&out);
        let sizes: Vec<usize> = msgs
            .iter()
            .filter_map(|m| match m {
                WorkerMsg::Facts { facts, .. } => Some(facts.len()),
                _ => None,
            })
            .collect();
        assert!(sizes.len() >= 2, "{sizes:?}");
        assert!(sizes.iter().all(|&n| n <= FACTS_PER_FRAME));
        let location = Location::file(
            job(&root, "a.c").input,
            Vec::new(),
            RelPath::new(Text::from("a.c")).unwrap(),
        );
        let mut ctx = ReadCtx::new(location, &Limits::default(), None);
        let mut direct = Vec::new();
        struct Collect<'a>(&'a mut Vec<Fact>);
        impl FactSink for Collect<'_> {
            fn fact(&mut self, fact: Fact) -> Result<(), Exceeded> {
                self.0.push(fact);
                Ok(())
            }
            fn region(&mut self, _: Region) -> Result<(), Exceeded> {
                Ok(())
            }
            fn note(&mut self, _: Note) -> Result<(), Exceeded> {
                Ok(())
            }
        }
        CppReader
            .read(src.as_bytes(), &mut ctx, &mut Collect(&mut direct))
            .unwrap();
        assert_eq!(sizes.iter().sum::<usize>(), direct.len());
        assert_eq!(outcome(&msgs).file, FileOutcome::Read);
    }

    fn big_fact() -> Fact {
        let location = Location::file(
            InputRef {
                index: 0,
                given: Text::from("/src"),
            },
            Vec::new(),
            RelPath::new(Text::from("a.c")).unwrap(),
        );
        Fact::StringLiteral {
            file_kind: FileKind::Source,
            location: location.at(Some(1), Scope::File),
            value: Text::from("x".repeat(4096)),
            guards: Vec::new(),
        }
    }

    #[test]
    fn a_frame_over_the_cap_is_never_sent() {
        let limits = Limits::default()
            .with(LimitName::EntryMessageBytes, 1024)
            .unwrap();
        let mut out = Vec::new();
        let mut sink = FrameSink::new(&mut out, &limits);
        sink.fact(big_fact()).unwrap();
        let over = sink.flush().unwrap_err();
        assert_eq!(over.limit, LimitName::EntryMessageBytes);
        assert_eq!(over.max, 1024);
        assert!(over.value > 1024);
        assert!(sink.failed.is_none());
        assert!(out.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn a_file_swapped_for_a_fifo_is_reported_from_the_handle() {
        let root = dir("swapped", &[]);
        rustix::fs::mkfifoat(rustix::fs::CWD, root.0.join("a.c"), rustix::fs::Mode::RUSR).unwrap();
        let mut out = Vec::new();
        run_job(
            &job(&root, "a.c"),
            &settings(Limits::default(), Vec::new()),
            &MacroTables::new(),
            &mut out,
        )
        .unwrap();
        let msgs = messages(&out);
        let done = outcome(&msgs);
        assert_eq!(
            done.file,
            FileOutcome::NotChecked {
                reason: NotCheckedReason::SpecialFile(boxt_core::outcome::SpecialKind::Fifo),
                details: Details::None
            }
        );
        assert_eq!(done.file_kind, None);
        assert!(matches!(
            &msgs[0],
            WorkerMsg::Opened {
                file_kind: None,
                ..
            }
        ));
    }

    #[cfg(unix)]
    #[test]
    fn a_link_swapped_in_is_never_followed() {
        let root = dir("swapped-link", &[("real.c", SOURCE)]);
        std::os::unix::fs::symlink("real.c", root.0.join("a.c")).unwrap();
        let mut out = Vec::new();
        run_job(
            &job(&root, "a.c"),
            &settings(Limits::default(), Vec::new()),
            &MacroTables::new(),
            &mut out,
        )
        .unwrap();
        let msgs = messages(&out);
        assert!(msgs.iter().all(|m| !matches!(m, WorkerMsg::Facts { .. })));
        assert_eq!(
            outcome(&msgs).file,
            FileOutcome::NotChecked {
                reason: NotCheckedReason::Symlink(boxt_core::outcome::LinkTarget::Unresolved),
                details: Details::None
            }
        );
    }

    #[test]
    fn a_vanished_file_is_unreadable() {
        let root = dir("vanished", &[]);
        let mut out = Vec::new();
        run_job(
            &job(&root, "gone.c"),
            &settings(Limits::default(), Vec::new()),
            &MacroTables::new(),
            &mut out,
        )
        .unwrap();
        let done = outcome(&messages(&out));
        assert_eq!(
            done.file,
            FileOutcome::NotChecked {
                reason: NotCheckedReason::Unreadable(IoKind::NotFound),
                details: Details::None
            }
        );
        assert_eq!(done.file_kind, Some(FileKind::Source));
    }

    #[test]
    fn a_definitions_job_sends_its_defines_then_its_outcome() {
        let header =
            b"#define A 1\n#define F(x, ...) g(x, __VA_ARGS__)\n#if 0\n#define A 2\n#endif\n";
        let root = dir("defs", &[("a.h", header), ("README", b"text\n")]);
        let defs_job = |file: &str| Job {
            kind: JobKind::Definitions,
            ..job(&root, file)
        };
        let mut out = Vec::new();
        run_job(
            &defs_job("a.h"),
            &settings(Limits::default(), Vec::new()),
            &MacroTables::new(),
            &mut out,
        )
        .unwrap();
        let msgs = messages(&out);
        assert!(msgs.iter().all(|m| m.allowed_in(JobKind::Definitions)));
        let defs: Vec<&MacroDef> = msgs
            .iter()
            .filter_map(|m| match m {
                WorkerMsg::Definitions { definitions } => Some(definitions),
                _ => None,
            })
            .flatten()
            .collect();
        assert_eq!(defs.len(), 3);
        assert!(
            defs.iter()
                .all(|d| d.validate(&defs_job("a.h"), u64::MAX).is_ok())
        );
        assert_eq!(outcome(&msgs).file, FileOutcome::Read);

        let mut out = Vec::new();
        run_job(
            &defs_job("README"),
            &settings(Limits::default(), Vec::new()),
            &MacroTables::new(),
            &mut out,
        )
        .unwrap();
        let msgs = messages(&out);
        assert_eq!(msgs.len(), 1);
        assert_eq!(
            outcome(&msgs).file,
            FileOutcome::NotChecked {
                reason: NotCheckedReason::NoReader,
                details: Details::None
            }
        );
    }

    struct Broken;

    impl Write for Broken {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::Error::from(io::ErrorKind::BrokenPipe))
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn a_failed_write_stops_the_reader_and_is_kept() {
        let mut out = Broken;
        let mut sink = FrameSink::new(&mut out, &Limits::default());
        sink.fact(big_fact()).unwrap();
        assert!(sink.flush().is_err());
        assert_eq!(
            sink.failed.as_ref().map(io::Error::kind),
            Some(io::ErrorKind::BrokenPipe)
        );
        assert!(sink.fact(big_fact()).is_ok());
        assert!(sink.flush().is_err());
    }
}
