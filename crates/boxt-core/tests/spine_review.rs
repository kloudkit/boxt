mod common;

use std::io::Cursor;

use boxt_core::Text;
use boxt_core::compile_db::CompileDatabaseError;
use boxt_core::fact::{
    Directive, Fact, FactError, FactKind, FileKind, GuardChain, GuardError, GuardRegionError,
    LineRule, PathRule, RegionId,
};
use boxt_core::limits::{LimitError, LimitName, LimitScope, Limits, MEMORY_CAP_FACTOR};
use boxt_core::location::{
    EntryNameError, Frame, FrameKind, Location, PathText, RelPath, SCRIPT_NAMES, Scope, ScopeKind,
    ScriptName, entry_path, frame_name, normalize_value_path,
};
use boxt_core::message::{
    CanonicalKey, CollectedFile, CollectorMsg, EntryBudget, EntryPath, ExitClass, FRAME_PREFIX_LEN,
    FaultScope, FrameError, Job, JobFile, JobKind, MacroDef, MacroDefError, MacroParams,
    MacroTable, MacroTables, MacroTablesError, MergedTables, Note, PANIC_EXIT, ParentKill,
    Platform, RawExit, Region, RegionError, RegionReason, Variadic, WORKER_MEMORY_CAP_EXIT,
    WorkerExit, WorkerMsg, classify_exit, decode_frame, encode_definitions, encode_frame,
    encode_macro_table, fault_scope, faulting_entry, merge_macro_tables, resume_skips,
};
use boxt_core::outcome::{
    Count, Details, FaultKind, FileOutcome, LineCount, NotCheckedReason, Outcome, OutcomeError,
    line_count, sm3_share, verdict_counts,
};
use boxt_core::read::{
    Container, ContainerReader, Entry, EntryKind, EntryName, Exceeded, FactSink, Input, NotRead,
    RATIO_FLOOR, ReadCtx, ReadResult, Reader, WorkBudget,
};
use boxt_core::settings::{CompileDatabaseSetting, Encoding, Settings, WorkerSettings};
use boxt_core::shell::{self, is_shell_interpreter};
use boxt_core::verdict::{CoverageError, RuleCoverage, RuleId};
use boxt_core::version::{RULE_SCHEMA_VERSION, SchemaVersion, Versions};

use common::{compact, frame, input, opened, sample_fact, worker_settings};

struct LineReader;

impl Reader for LineReader {
    fn file_kind(&self) -> FileKind {
        FileKind::Binary
    }

    fn read(&self, bytes: &[u8], ctx: &mut ReadCtx, sink: &mut dyn FactSink) -> ReadResult {
        for line in bytes.split(|&c| c == b'\n').filter(|l| !l.is_empty()) {
            ctx.work(1)?;
            sink.fact(Fact::StringLiteral {
                file_kind: self.file_kind(),
                location: ctx.at(None, Scope::File),
                value: Text::from(line),
                guards: vec![],
            })?;
        }
        Ok(())
    }
}

struct PackedReader;

struct Packed<'a> {
    data: &'a mut dyn std::io::Read,
    current: Cursor<Vec<u8>>,
}

impl ContainerReader for PackedReader {
    fn frame_kind(&self) -> FrameKind {
        FrameKind::Archive
    }

    fn open<'a>(
        &self,
        mut input: Input<'a>,
        ctx: &mut ReadCtx,
    ) -> ReadResult<Box<dyn Container + 'a>> {
        let mut magic = [0u8; 4];
        input
            .reader()
            .read_exact(&mut magic)
            .map_err(|_| NotRead::new(NotCheckedReason::Unparseable))?;
        if &magic != b"PACK" {
            return Err(NotRead::new(NotCheckedReason::Unparseable));
        }
        ctx.work(1)?;
        let data = match input {
            Input::Stream(r) => r,
            Input::Seekable(r) => r as &mut dyn std::io::Read,
        };
        Ok(Box::new(Packed {
            data,
            current: Cursor::new(Vec::new()),
        }))
    }
}

impl Container for Packed<'_> {
    fn name(&self) -> Option<Text> {
        None
    }

    fn own_facts(&mut self, _: &mut ReadCtx, _: &mut dyn FactSink) -> ReadResult {
        Ok(())
    }

    fn next_entry(&mut self, ctx: &mut ReadCtx) -> ReadResult<Option<Entry<'_>>> {
        let mut header = [0u8; 2];
        if self.data.read_exact(&mut header).is_err() {
            return Ok(None);
        }
        let mut name = vec![0u8; usize::from(header[0])];
        let mut body = vec![0u8; usize::from(header[1])];
        let bad = |_| NotRead::new(NotCheckedReason::Unparseable);
        self.data.read_exact(&mut name).map_err(bad)?;
        self.data.read_exact(&mut body).map_err(bad)?;
        ctx.buffer(body.len() as u64)?;
        let size = Some(body.len() as u64);
        self.current = Cursor::new(body);
        Ok(Some(Entry {
            name: EntryName::Path(Text::from_bytes(name)),
            kind: EntryKind::File,
            size,
            data: Input::Seekable(&mut self.current),
        }))
    }
}

fn packed(entries: &[(&str, &str)]) -> Vec<u8> {
    let mut out = b"PACK".to_vec();
    for (name, body) in entries {
        out.push(name.len() as u8);
        out.push(body.len() as u8);
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(body.as_bytes());
    }
    out
}

struct Collected {
    budget: EntryBudget,
    facts: Vec<Fact>,
    regions: Vec<Region>,
    notes: Vec<Note>,
}

impl Collected {
    fn new(limits: &Limits) -> Self {
        Collected {
            budget: EntryBudget::new(limits),
            facts: vec![],
            regions: vec![],
            notes: vec![],
        }
    }

    fn charge(&mut self, msg: WorkerMsg) -> Result<(), Exceeded> {
        let len = serde_json::to_vec(&msg).unwrap().len() as u64;
        self.budget
            .charge_frame(&msg, len)
            .map_err(|limit| Exceeded {
                limit,
                value: 0,
                max: 0,
            })
    }
}

impl FactSink for Collected {
    fn fact(&mut self, fact: Fact) -> Result<(), Exceeded> {
        self.charge(WorkerMsg::Facts {
            entry: EntryPath::job(),
            facts: vec![fact.clone()],
        })?;
        self.facts.push(fact);
        Ok(())
    }

    fn region(&mut self, region: Region) -> Result<(), Exceeded> {
        self.regions.push(region);
        Ok(())
    }

    fn note(&mut self, note: Note) -> Result<(), Exceeded> {
        self.notes.push(note);
        Ok(())
    }
}

type Routed = Vec<(EntryPath, Option<RelPath>, Result<Vec<Fact>, NotRead>)>;

fn route(
    file: &RelPath,
    bytes: &[u8],
    containers: &dyn ContainerReader,
    reader: &dyn Reader,
    settings: &WorkerSettings,
) -> Result<Routed, NotRead> {
    let mut ctx = ReadCtx::new(
        Location::file(input(), vec![], file.clone()),
        &settings.limits,
        None,
    );
    let mut stream = bytes;
    let mut container = containers.open(Input::Stream(&mut stream), &mut ctx)?;
    let name = container
        .name()
        .or_else(|| frame_name(&Text::from(file.file_name())))
        .unwrap();
    let frames = vec![Frame {
        kind: containers.frame_kind(),
        name,
    }];
    let mut base = ReadCtx::with_budget(
        Location::frame(input(), frames.clone()),
        None,
        ctx.budget().nested()?,
    );
    let mut out = Vec::new();
    let mut index = 1;
    while let Some(mut entry) = container.next_entry(&mut base)? {
        let at = EntryPath::from([index]);
        index += 1;
        let EntryName::Path(raw) = &entry.name else {
            unreachable!()
        };
        let path = match entry_path(raw.as_bytes()) {
            Ok(path) => path,
            Err(_) => {
                out.push((
                    at,
                    None,
                    Err(NotRead {
                        reason: NotCheckedReason::UnsafeEntryName,
                        details: Details::EntryName { name: raw.clone() },
                    }),
                ));
                continue;
            }
        };
        let mut body = Vec::new();
        entry.data.reader().read_to_end(&mut body).unwrap();
        let mut entry_ctx = ReadCtx::new(
            Location::file(input(), frames.clone(), path.clone()),
            &settings.limits,
            settings.resolve_encoding(|p| path.as_bytes().starts_with(p.pattern.as_bytes())),
        );
        let mut sink = Collected::new(&settings.limits);
        let result = reader
            .read(&body, &mut entry_ctx, &mut sink)
            .map(|()| sink.facts);
        out.push((at, Some(path), result));
    }
    Ok(out)
}

#[test]
fn f1_readers_get_bytes_a_base_location_and_one_work_budget() {
    let readers: Vec<Box<dyn Reader>> = vec![Box::new(LineReader)];
    let containers: Vec<Box<dyn ContainerReader>> = vec![Box::new(PackedReader)];
    let settings = worker_settings();
    let file = RelPath::try_from("dist/app-1.2.zip").unwrap();
    let bytes = packed(&[
        ("./bin/app", "a\nb\n"),
        ("../evil", "x"),
        ("etc/app.conf", "c"),
    ]);
    let routed = route(&file, &bytes, &*containers[0], &*readers[0], &settings).unwrap();
    assert_eq!(routed.len(), 3);

    let (entry, path, facts) = &routed[0];
    assert_eq!(entry, &EntryPath::from([1]));
    assert_eq!(path.as_ref().unwrap().as_bytes(), b"bin/app");
    let facts = facts.as_ref().unwrap();
    assert_eq!(facts.len(), 2);
    for fact in facts {
        assert_eq!(fact.validate(), Ok(()));
        let location = fact.location();
        assert_eq!(
            location.frames,
            vec![Frame {
                kind: FrameKind::Archive,
                name: Text::from("app"),
            }]
        );
        assert_eq!(location.path, path.clone());
    }

    assert_eq!(
        routed[1].2,
        Err(NotRead {
            reason: NotCheckedReason::UnsafeEntryName,
            details: Details::EntryName {
                name: Text::from("../evil"),
            },
        })
    );

    let tight = WorkerSettings {
        limits: Limits::default().with(LimitName::Work, 1).unwrap(),
        encodings: vec![],
    };
    let routed = route(&file, &bytes, &PackedReader, &LineReader, &tight).unwrap();
    assert_eq!(
        routed[0].2,
        Err(NotRead {
            reason: NotCheckedReason::LimitExceeded(LimitName::Work),
            details: Details::Measured { value: 2, limit: 1 },
        })
    );
    assert!(routed[2].2.is_ok());

    let few_facts = WorkerSettings {
        limits: Limits::default().with(LimitName::EntryFacts, 1).unwrap(),
        encodings: vec![],
    };
    let routed = route(&file, &bytes, &PackedReader, &LineReader, &few_facts).unwrap();
    assert_eq!(
        routed[0].2.as_ref().unwrap_err().reason,
        NotCheckedReason::LimitExceeded(LimitName::EntryFacts)
    );

    let shallow = WorkerSettings {
        limits: Limits::default().with(LimitName::ArchiveDepth, 1).unwrap(),
        encodings: vec![],
    };
    assert!(route(&file, &bytes, &PackedReader, &LineReader, &shallow).is_ok());
    let budget = WorkBudget::new(&shallow.limits).nested().unwrap();
    assert_eq!(budget.depth(), 1);
    assert_eq!(
        budget.nested(),
        Err(Exceeded {
            limit: LimitName::ArchiveDepth,
            value: 2,
            max: 1,
        })
    );

    let small_buffer = WorkerSettings {
        limits: Limits::default().with(LimitName::Memory, 3).unwrap(),
        encodings: vec![],
    };
    assert_eq!(
        route(&file, &bytes, &PackedReader, &LineReader, &small_buffer)
            .unwrap_err()
            .reason,
        NotCheckedReason::LimitExceeded(LimitName::Memory)
    );
    assert_eq!(
        route(&file, b"ZIP!", &PackedReader, &LineReader, &settings).unwrap_err(),
        NotRead::new(NotCheckedReason::Unparseable)
    );

    let mut budget = WorkBudget::new(&small_buffer.limits);
    assert_eq!(budget.buffer(3), Ok(()));
    budget.release(2);
    assert_eq!(budget.buffered(), 1);
    assert_eq!(budget.buffer(2), Ok(()));
    assert!(budget.buffer(1).is_err());
}

#[test]
fn f2_opened_names_the_entry_and_entry_done_counts_its_bytes() {
    let msg = WorkerMsg::Opened {
        entry: EntryPath::from([2, 0]),
        frames: vec![Frame {
            kind: FrameKind::Rpm,
            name: Text::from("app"),
        }],
        path: None,
        scope: Scope::File,
        file_kind: Some(FileKind::Package),
    };
    let location = msg.opened_location(&input()).unwrap();
    assert_eq!(location.path, None);
    assert_eq!(location.line, None);
    assert_eq!(location.frames.len(), 1);
    assert_eq!(msg.entry(), Some(&EntryPath::from([2, 0])));
    assert_eq!(
        WorkerMsg::JobDone { outcome: None }.opened_location(&input()),
        None
    );

    let done = WorkerMsg::EntryDone {
        entry: EntryPath::from([2, 0]),
        outcome: Outcome::new(FileOutcome::Read, Some(FileKind::Package)),
        bytes: 4096,
    };
    let json = serde_json::to_value(&done).unwrap();
    assert_eq!(json["entry_done"]["bytes"], 4096);

    let mut budget = EntryBudget::new(&Limits::default());
    assert_eq!(budget.charge_frame(&msg, u64::MAX), Ok(()));

    let job = Job {
        kind: JobKind::Route,
        input: input(),
        file: JobFile::InInput(RelPath::try_from("app.rpm").unwrap()),
        resume_after: None,
    };
    let file = |entry: EntryPath, region: bool| CollectedFile {
        key: CanonicalKey::for_outcome(&job, &entry).unwrap(),
        location: location.clone(),
        outcome: Outcome::new(FileOutcome::Read, Some(FileKind::Source)).with_lines(10),
        regions: if region {
            vec![Region {
                first_line: 1,
                last_line: 3,
                reason: RegionReason::Unparseable,
            }]
        } else {
            vec![]
        },
        notes: vec![],
    };
    let files: boxt_core::SortedVec<CollectedFile> = vec![
        file(EntryPath::from([3]), false),
        file(EntryPath::from([0]), true),
        file(EntryPath::from([2, 1]), false),
    ]
    .into();
    let order: Vec<&EntryPath> = files.iter().map(|f| &f.key.entry).collect();
    assert_eq!(
        order,
        vec![
            &EntryPath::from([0]),
            &EntryPath::from([2, 1]),
            &EntryPath::from([3])
        ]
    );
    let share = sm3_share(&files);
    assert_eq!(
        share.files.map(|s| (s.numerator, s.denominator)),
        Some((1, 3))
    );
    assert_eq!(
        share
            .lines
            .map(|s| (s.numerator, s.denominator, s.uncounted_files)),
        Some((3, 30, 0))
    );
    assert_eq!(
        verdict_counts(files.iter().map(|f| &f.outcome.file)).read,
        3
    );
}

#[test]
fn f3_install_scripts_are_entries_of_their_package_frame() {
    for name in ScriptName::ALL {
        assert!(
            SCRIPT_NAMES.iter().any(|s| s.name == name),
            "{name:?} has no native name"
        );
    }
    for spec in SCRIPT_NAMES {
        assert_eq!(
            ScriptName::from_native(spec.format, spec.native.as_bytes()),
            Some(spec.name)
        );
    }
    assert_eq!(
        ScriptName::from_native(FrameKind::Rpm, b"%post"),
        Some(ScriptName::PostInstall)
    );
    assert_eq!(
        ScriptName::from_native(FrameKind::Deb, b"postinst"),
        Some(ScriptName::PostInstall)
    );
    assert_eq!(ScriptName::from_native(FrameKind::Deb, b"%post"), None);
    assert_eq!(
        ScriptName::from_native(FrameKind::Archive, b"postinst"),
        None
    );
    assert_eq!(
        ScriptName::PostRemove.native(FrameKind::Deb),
        Some("postrm")
    );
    assert_eq!(ScriptName::Config.native(FrameKind::Rpm), None);

    let frames = vec![Frame {
        kind: FrameKind::Rpm,
        name: Text::from("app"),
    }];
    let at = |path: Option<&str>, scope: Scope| Location {
        path: path.map(|p| RelPath::try_from(p).unwrap()),
        line: None,
        ..Location::frame(input(), frames.clone()).at(None, scope)
    };
    let ldconfig = Fact::PackageScript {
        file_kind: FileKind::Package,
        location: at(None, Scope::Script(ScriptName::PostInstall)),
        name: ScriptName::PostInstall,
        interpreter: Some(Text::from("/sbin/ldconfig")),
    };
    assert_eq!(ldconfig.validate(), Ok(()));
    let mismatched = Fact::PackageScript {
        file_kind: FileKind::Package,
        location: at(None, Scope::Script(ScriptName::PreInstall)),
        name: ScriptName::PostInstall,
        interpreter: None,
    };
    assert_eq!(mismatched.validate(), Err(FactError::ScriptNameMismatch));

    let script = Text::from("useradd -u 501 app\n");
    let commands = shell::lex(&script).unwrap();
    let command = |path: Option<&str>, scope: Scope, file_kind| Fact::ShellCommand {
        file_kind,
        location: Location {
            line: Some(commands[0].line),
            ..at(path, scope)
        },
        assignments: commands[0].assignments.clone(),
        argv: commands[0].argv.clone(),
        redirections: commands[0].redirections.clone(),
    };
    let in_script = Scope::Script(ScriptName::PostInstall);
    assert_eq!(
        command(None, in_script.clone(), FileKind::Package).validate(),
        Ok(())
    );
    assert_eq!(
        command(Some("etc/init.d/app"), Scope::File, FileKind::Service).validate(),
        Ok(())
    );
    assert_eq!(
        command(Some("etc/init.d/app"), in_script.clone(), FileKind::Package).validate(),
        Err(FactError::PathNotAllowed {
            kind: FactKind::ShellCommand,
            rule: PathRule::Frame,
        })
    );
    assert_eq!(
        command(None, Scope::File, FileKind::Service).validate(),
        Err(FactError::PathNotAllowed {
            kind: FactKind::ShellCommand,
            rule: PathRule::File,
        })
    );
    assert_eq!(
        command(Some("etc/init.d/app"), Scope::File, FileKind::Package).validate(),
        Err(FactError::ScopeNotAllowed {
            kind: FactKind::ShellCommand,
            scope: ScopeKind::File,
        })
    );
    assert_eq!(
        command(None, in_script, FileKind::Service).validate(),
        Err(FactError::ScopeNotAllowed {
            kind: FactKind::ShellCommand,
            scope: ScopeKind::Script,
        })
    );

    for (interpreter, shell) in [
        (None, true),
        (Some("/bin/sh"), true),
        (Some("/bin/bash -e"), true),
        (Some("/usr/bin/env bash"), true),
        (Some("/usr/bin/env -S dash -e"), true),
        (Some("dash"), true),
        (Some("<lua>"), false),
        (Some("/usr/bin/perl"), false),
        (Some("/sbin/ldconfig"), false),
        (Some("/usr/bin/env"), false),
        (Some(""), true),
        (Some("  \t "), true),
        (Some("/bin/sh\r"), true),
        (Some("/bin/bash -e\r"), true),
        (Some("/usr/bin/env -u X bash"), true),
        (Some("/usr/bin/env -u X"), false),
        (Some("/usr/bin/env LC_ALL=C sh"), true),
        (Some("/usr/bin/env -C /tmp LC_ALL=C dash"), true),
        (Some("/usr/bin/env -S bash -e"), true),
        (Some("/usr/bin/env -Sbash"), true),
        (Some("/usr/bin/env -S"), false),
        (Some("/usr/bin/env LC_ALL=C perl"), false),
        (Some("/usr/bin/env -u bash perl"), false),
        (Some("#!/bin/sh"), true),
        (Some("#! /bin/sh"), true),
        (Some("  #!\t/bin/bash -e\r"), true),
        (Some("#!/usr/bin/perl"), false),
        (Some("#!"), true),
    ] {
        let interpreter = interpreter.map(Text::from);
        assert_eq!(
            is_shell_interpreter(interpreter.as_ref()),
            shell,
            "{interpreter:?}"
        );
    }
}

fn paths(list: &[&[u32]]) -> Vec<EntryPath> {
    list.iter().map(|p| EntryPath::from(*p)).collect()
}

#[test]
fn f4_a_fault_before_open_or_on_entry_0_costs_the_rest_of_its_container() {
    assert_eq!(fault_scope(&EntryPath::job(), true), FaultScope::Job);
    assert_eq!(fault_scope(&EntryPath::from([0]), false), FaultScope::Job);
    assert_eq!(FaultScope::Job.resume_after(), None);
    assert_eq!(FaultScope::Job.outcome_entry(), EntryPath::job());

    let committed = paths(&[&[0], &[1], &[2, 0], &[2, 1]]);
    let attributed = faulting_entry(&committed, None);
    assert_eq!(attributed, EntryPath::from([2, 2]));

    let header = fault_scope(&attributed, false);
    assert_eq!(
        header,
        FaultScope::RestOfContainer {
            at: EntryPath::from([2, 2]),
            container: EntryPath::from([2]),
        }
    );
    assert_eq!(header.resume_after(), Some(EntryPath::from([2])));
    assert_eq!(header.outcome_entry(), EntryPath::from([2, 2]));
    let crash = NotCheckedReason::Fault(FaultKind::Crash);
    assert_eq!(
        header.outcome(crash),
        FileOutcome::Aggregate {
            reason: crash,
            count: None,
            at_least: 1,
        }
    );

    let inside = fault_scope(&attributed, true);
    assert_eq!(inside, FaultScope::Entry(EntryPath::from([2, 2])));
    assert_eq!(inside.resume_after(), Some(EntryPath::from([2, 2])));
    assert_eq!(inside.outcome_entry(), EntryPath::from([2, 2]));
    assert_eq!(
        inside.outcome(crash),
        FileOutcome::NotChecked {
            reason: crash,
            details: Details::None,
        }
    );

    let probe = faulting_entry(&paths(&[&[0], &[1]]), Some(&EntryPath::from([2, 0])));
    assert_eq!(probe, EntryPath::from([2, 0]));
    for opened in [false, true] {
        let scope = fault_scope(&probe, opened);
        assert_eq!(scope, FaultScope::Container(EntryPath::from([2])));
        assert_eq!(scope.outcome_entry(), EntryPath::from([2, 0]));
        let after = scope.resume_after().unwrap();
        assert_eq!(after, EntryPath::from([2]));
        for skipped in paths(&[&[2, 0], &[2, 1], &[2, 7, 0]]) {
            assert!(resume_skips(&skipped, &after), "{skipped:?}");
        }
        assert!(!resume_skips(&EntryPath::from([3]), &after));
    }

    let top = fault_scope(&EntryPath::from([3]), false);
    assert_eq!(
        top,
        FaultScope::RestOfContainer {
            at: EntryPath::from([3]),
            container: EntryPath::job(),
        }
    );
    assert_eq!(top.resume_after(), None);
}

#[test]
fn f5_exits_are_classified_per_os_and_the_memory_cap_has_its_own_code() {
    let crash = ExitClass::Fault(FaultKind::Crash);
    let raw = |raw| WorkerExit {
        raw,
        killed_by_parent: false,
    };
    for (platform, exit, class) in [
        (Platform::Unix, RawExit::Signal(11), crash),
        (Platform::Unix, RawExit::Signal(6), crash),
        (Platform::Unix, RawExit::Signal(9), crash),
        (
            Platform::Unix,
            RawExit::Code(0),
            ExitClass::Fault(FaultKind::Exited),
        ),
        (
            Platform::Unix,
            RawExit::Code(134),
            ExitClass::Fault(FaultKind::Exited),
        ),
        (
            Platform::Unix,
            RawExit::Code(PANIC_EXIT),
            ExitClass::Fault(FaultKind::Panic),
        ),
        (
            Platform::Unix,
            RawExit::Code(WORKER_MEMORY_CAP_EXIT),
            ExitClass::WorkerMemoryCap,
        ),
        (
            Platform::Windows,
            RawExit::Code(0xC000_0005_u32 as i32),
            crash,
        ),
        (
            Platform::Windows,
            RawExit::Code(0xC000_00FD_u32 as i32),
            crash,
        ),
        (
            Platform::Windows,
            RawExit::Code(0xC000_0409_u32 as i32),
            crash,
        ),
        (
            Platform::Windows,
            RawExit::Code(1),
            ExitClass::Fault(FaultKind::Exited),
        ),
        (
            Platform::Windows,
            RawExit::Code(WORKER_MEMORY_CAP_EXIT),
            ExitClass::WorkerMemoryCap,
        ),
    ] {
        assert_eq!(
            classify_exit(platform, raw(exit)),
            class,
            "{platform:?} {exit:?}"
        );
    }
    for platform in [Platform::Unix, Platform::Windows] {
        for exit in [
            RawExit::Signal(9),
            RawExit::Code(1),
            RawExit::Code(WORKER_MEMORY_CAP_EXIT),
        ] {
            let killed = WorkerExit {
                raw: exit,
                killed_by_parent: true,
            };
            let class = classify_exit(platform, killed);
            assert_eq!(class, ExitClass::KilledByParent);
            assert!(!class.is_fault());
            assert_eq!(class.reason(), None);
        }
    }
    assert_eq!(
        ParentKill::Watchdog.reason(),
        Some(NotCheckedReason::LimitExceeded(LimitName::Watchdog))
    );
    assert!(ParentKill::Watchdog.host_dependent());
    assert_eq!(ParentKill::Cancel.reason(), None);
    assert!(!ParentKill::Cancel.host_dependent());
    assert_eq!(
        ExitClass::WorkerMemoryCap.reason(),
        Some(NotCheckedReason::LimitExceeded(LimitName::WorkerMemoryCap))
    );
    assert!(ExitClass::WorkerMemoryCap.host_dependent());
    assert!(!crash.host_dependent());
    assert_eq!(
        crash.reason(),
        Some(NotCheckedReason::Fault(FaultKind::Crash))
    );
    let names: Vec<serde_json::Value> = [
        FaultKind::Crash,
        FaultKind::Panic,
        FaultKind::BadFrame,
        FaultKind::Exited,
    ]
    .iter()
    .map(|k| serde_json::to_value(k).unwrap())
    .collect();
    assert_eq!(names, ["crash", "panic", "bad_frame", "exited"]);
    assert!(serde_json::from_str::<FaultKind>(r#""abort""#).is_err());
    assert!(serde_json::from_str::<FaultKind>(r#""stack_overflow""#).is_err());
}

#[test]
fn f6_the_worker_cap_is_its_own_limit_and_at_least_four_times_memory() {
    assert_eq!(
        LimitName::from_flag("worker-memory-cap"),
        Some(LimitName::WorkerMemoryCap)
    );
    assert_eq!(
        LimitName::from_flag("run-watchdog"),
        Some(LimitName::RunWatchdog)
    );
    let defaults = Limits::default();
    let memory = defaults.get(LimitName::Memory);
    let cap = defaults.get(LimitName::WorkerMemoryCap);
    assert!(cap >= MEMORY_CAP_FACTOR * memory);

    let mut limits = defaults.clone();
    let big = cap / MEMORY_CAP_FACTOR + 1;
    assert_eq!(limits.set(LimitName::Memory, big), Ok(()));
    assert_eq!(
        limits.validate(),
        Err(LimitError::CapBelowMemory { cap, memory: big })
    );
    assert_eq!(
        limits.set(LimitName::WorkerMemoryCap, big * MEMORY_CAP_FACTOR),
        Ok(())
    );
    assert_eq!(limits.validate(), Ok(()));

    let mut cap_first = defaults.clone();
    cap_first
        .set(LimitName::WorkerMemoryCap, big * MEMORY_CAP_FACTOR)
        .unwrap();
    cap_first.set(LimitName::Memory, big).unwrap();
    assert_eq!(cap_first, limits);
    assert_eq!(defaults.validate(), Ok(()));
    assert!(
        serde_json::from_value::<Limits>(
            serde_json::to_value(defaults.clone().with(LimitName::Memory, big).unwrap()).unwrap()
        )
        .is_err()
    );

    let mut json = serde_json::to_value(&defaults).unwrap();
    json["worker_memory_cap"] = serde_json::Value::from(memory * MEMORY_CAP_FACTOR - 1);
    assert!(serde_json::from_value::<Limits>(json).is_err());
}

#[test]
fn f7_guards_are_chains_from_the_opening_directive_to_the_branch_holding_the_code() {
    let named = |directive, text: &str, identifier: Option<&str>, holds_code| {
        let mut f = frame(directive, holds_code);
        f.text = Text::from(text);
        f.identifiers = identifier.into_iter().map(Text::from).collect();
        f
    };
    let guards = vec![
        GuardChain {
            frames: vec![
                named(Directive::If, "#if A", Some("A"), false),
                named(Directive::Else, "#else", None, true),
            ],
            has_alternative: true,
            region: RegionId(1),
        },
        GuardChain {
            frames: vec![named(Directive::Ifdef, "#ifdef B", Some("B"), true)],
            has_alternative: false,
            region: RegionId(2),
        },
    ];
    let call = Fact::Call {
        file_kind: FileKind::Source,
        location: common::location("src/a.c", 7, common::function_scope()),
        callee: Text::from("f"),
        args: vec![],
        guards: guards.clone(),
    };
    assert_eq!(call.validate(), Ok(()));
    assert_eq!(call.guards(), guards.as_slice());

    let chain = |frames: &[(Directive, bool)]| GuardChain {
        frames: frames.iter().map(|&(d, h)| frame(d, h)).collect(),
        has_alternative: false,
        region: RegionId(0),
    };
    for (frames, expected) in [
        (&[][..], Err(GuardError::Empty)),
        (
            &[(Directive::Elif, true)][..],
            Err(GuardError::NotOpenedByIf),
        ),
        (
            &[(Directive::If, false), (Directive::Ifdef, true)][..],
            Err(GuardError::OpenerInsideChain),
        ),
        (
            &[
                (Directive::If, false),
                (Directive::Else, false),
                (Directive::Elif, true),
            ][..],
            Err(GuardError::ElseNotLast),
        ),
        (
            &[(Directive::If, true), (Directive::Elif, true)][..],
            Err(GuardError::HoldsCodeNotLast),
        ),
        (
            &[(Directive::If, false)][..],
            Err(GuardError::HoldsCodeNotLast),
        ),
        (
            &[
                (Directive::Ifndef, false),
                (Directive::Elifdef, false),
                (Directive::Elifndef, false),
                (Directive::Elif, true),
            ][..],
            Ok(()),
        ),
    ] {
        assert_eq!(chain(frames).validate(), expected, "{frames:?}");
    }

    let bad = Fact::GuardRegion {
        file_kind: FileKind::Source,
        location: common::location("src/a.c", 7, Scope::File),
        region: RegionId(0),
        guards: vec![chain(&[(Directive::Else, true)])],
        end_line: 9,
    };
    assert_eq!(
        bad.validate(),
        Err(FactError::Guard {
            kind: FactKind::GuardRegion,
            error: GuardError::NotOpenedByIf,
        })
    );
}

#[test]
fn f8_each_fact_kind_has_its_scope_path_and_value_forms() {
    for kind in FactKind::ALL {
        assert_eq!(sample_fact(kind).validate(), Ok(()), "{}", kind.name());
    }
    let config_key_in_source = Fact::ConfigKey {
        file_kind: FileKind::Source,
        location: common::location("src/a.c", 3, Scope::ConfigKey(Text::from("port"))),
        key: Text::from("port"),
        value: None,
    };
    assert_eq!(
        config_key_in_source.validate(),
        Err(FactError::FileKindNotAllowed {
            kind: FactKind::ConfigKey,
            file_kind: FileKind::Source,
        })
    );
    let import = |scope: Scope, line| Fact::Import {
        file_kind: FileKind::Binary,
        location: common::location_at(Some("usr/bin/app"), line, scope),
        symbol: Text::from("dlopen"),
        version: Some(Text::from("GLIBC_2.34")),
    };
    assert_eq!(
        import(Scope::File, None).validate(),
        Err(FactError::ScopeNotAllowed {
            kind: FactKind::Import,
            scope: boxt_core::location::ScopeKind::File,
        })
    );
    assert_eq!(
        import(Scope::Symbol(Text::from("dlopen")), Some(0)).validate(),
        Err(FactError::LineZero {
            kind: FactKind::Import
        })
    );
    let Fact::PackageFile { location, .. } = sample_fact(FactKind::PackageFile) else {
        unreachable!()
    };
    assert_eq!(location.path, None);
    let mut in_file = sample_fact(FactKind::PackageFile);
    if let Fact::PackageFile { location, .. } = &mut in_file {
        location.path = Some(RelPath::try_from("app.rpm").unwrap());
    }
    assert_eq!(
        in_file.validate(),
        Err(FactError::PathNotAllowed {
            kind: FactKind::PackageFile,
            rule: PathRule::Frame,
        })
    );
    let mut zero_end = sample_fact(FactKind::GuardRegion);
    if let Fact::GuardRegion { end_line, .. } = &mut zero_end {
        *end_line = 0;
    }
    assert!(matches!(
        zero_end.validate(),
        Err(FactError::LineZero { .. })
    ));

    for (raw, normal) in [
        ("/opt/app/bin/app", "/opt/app/bin/app"),
        ("/opt//app/./bin/", "/opt/app/bin"),
        ("/opt/app/../lib", "/opt/lib"),
        ("/../etc", "/etc"),
        ("/", "/"),
        ("//", "/"),
        ("var/run/../log", "var/log"),
        ("../x", "../x"),
        ("a/../../x", "../x"),
        ("./", "."),
        ("a/..", "."),
        ("", ""),
        ("C:\\Program Files\\app", "C:\\Program Files\\app"),
    ] {
        assert_eq!(
            normalize_value_path(&Text::from(raw)).as_bytes(),
            normal.as_bytes(),
            "{raw}"
        );
    }
    let raw = Text::from_bytes(b"/opt/\xff//x".to_vec());
    assert_eq!(normalize_value_path(&raw).as_bytes(), b"/opt/\xff/x");
    let decoded: PathText = serde_json::from_str(r#""/usr//bin/./app""#).unwrap();
    assert_eq!(decoded.as_bytes(), b"/usr/bin/app");
    assert_eq!(
        serde_json::to_string(&decoded).unwrap(),
        r#""/usr/bin/app""#
    );

    let unit = Scope::UnitSection {
        section: Text::from("Service"),
        key: Text::from("User"),
    };
    assert_eq!(
        serde_json::to_value(&unit).unwrap(),
        serde_json::json!({"unit_section": {"section": "Service", "key": "User"}})
    );
}

#[test]
fn f9_coverage_lists_alternatives_each_needing_all_its_file_kinds() {
    let rule = || RuleId::new("identity/needs-root").unwrap();
    let alts = |list: Vec<Vec<FileKind>>| -> boxt_core::SortedVec<boxt_core::SortedVec<FileKind>> {
        list.into_iter().map(Into::into).collect()
    };
    let coverage = RuleCoverage::new(
        rule(),
        alts(vec![
            vec![FileKind::Source, FileKind::Package],
            vec![FileKind::Binary],
        ]),
    )
    .unwrap();
    let alternatives: Vec<Vec<FileKind>> =
        coverage.file_kinds().iter().map(|a| a.to_vec()).collect();
    assert_eq!(
        alternatives,
        vec![
            vec![FileKind::Source, FileKind::Package],
            vec![FileKind::Binary]
        ]
    );
    assert!(coverage.can_fire(&[FileKind::Binary]));
    assert!(coverage.can_fire(&[FileKind::Package, FileKind::Source]));
    assert!(!coverage.can_fire(&[FileKind::Package]));
    assert!(!coverage.can_fire(&[FileKind::Source, FileKind::Config]));
    assert!(!coverage.can_fire(&[]));
    let json = serde_json::to_value(&coverage).unwrap();
    assert_eq!(
        json["file_kinds"],
        serde_json::json!([["source", "package"], ["binary"]])
    );
    assert_eq!(
        serde_json::from_value::<RuleCoverage>(json).unwrap(),
        coverage
    );

    for (list, error) in [
        (vec![vec![]], CoverageError::EmptyAlternative),
        (
            vec![vec![FileKind::Binary], vec![]],
            CoverageError::EmptyAlternative,
        ),
        (
            vec![vec![FileKind::Binary], vec![FileKind::Binary]],
            CoverageError::DuplicateAlternative,
        ),
        (
            vec![vec![FileKind::Binary, FileKind::Binary]],
            CoverageError::DuplicateFileKind,
        ),
    ] {
        assert_eq!(
            RuleCoverage::new(rule(), alts(list.clone())),
            Err(error),
            "{list:?}"
        );
        let json = serde_json::json!({"rule": "identity/needs-root", "file_kinds": list});
        assert!(serde_json::from_value::<RuleCoverage>(json).is_err());
    }
}

#[test]
fn f10_the_compile_database_is_a_setting_job_outside_the_run() {
    let settings = Settings {
        inputs: vec![Text::from("/src/a"), Text::from("/src/b")],
        targets: Default::default(),
        limits: Limits::default(),
        encodings: vec![],
        excludes: vec![],
        ignored_macros: Default::default(),
        jobs: None,
        compile_database: Some(CompileDatabaseSetting {
            file: Text::from("/build/b/compile_commands.json"),
            tree: 1,
        }),
    };
    let job = settings.compile_database_job().unwrap().unwrap();
    assert_eq!(job.kind, JobKind::CompileDatabase);
    assert_eq!(job.input.index, 1);
    assert_eq!(job.input.given, Text::from("/src/b"));
    assert_eq!(
        job.file,
        JobFile::Setting(Text::from("/build/b/compile_commands.json"))
    );
    assert!(job.is_well_formed());
    assert_eq!(job.path(), None);
    assert_eq!(CanonicalKey::for_outcome(&job, &EntryPath::job()), None);

    let mut out_of_range = settings.clone();
    out_of_range.compile_database.as_mut().unwrap().tree = 2;
    assert_eq!(
        out_of_range.compile_database_job(),
        Err(CompileDatabaseError::TreeOutOfRange { tree: 2, inputs: 2 })
    );
    let mut none = settings.clone();
    none.compile_database = None;
    assert_eq!(none.compile_database_job(), Ok(None));
    assert_eq!(
        CompileDatabaseError::NotRead(NotCheckedReason::Unparseable).to_string(),
        "the compile database could not be read (unparseable)"
    );
    assert_eq!(
        CompileDatabaseError::NotRead(NotCheckedReason::LimitExceeded(
            LimitName::EntryMessageBytes
        ))
        .to_string(),
        "the compile database could not be read (limit_exceeded(entry_message_bytes))"
    );

    let route_with_setting = Job {
        kind: JobKind::Route,
        ..job.clone()
    };
    assert!(!route_with_setting.is_well_formed());
    let resumed = Job {
        resume_after: Some(EntryPath::from([1])),
        ..job.clone()
    };
    assert!(!resumed.is_well_formed());

    let read = Outcome::new(FileOutcome::Read, Some(FileKind::Config));
    assert_eq!(CompileDatabaseError::check(Some(&read)), Ok(()));
    let unparseable = Outcome::new(
        FileOutcome::NotChecked {
            reason: NotCheckedReason::Unparseable,
            details: Details::None,
        },
        None,
    );
    assert_eq!(
        CompileDatabaseError::check(Some(&unparseable)),
        Err(CompileDatabaseError::NotRead(NotCheckedReason::Unparseable))
    );
    assert_eq!(
        CompileDatabaseError::check(None),
        Err(CompileDatabaseError::NotRead(NotCheckedReason::Fault(
            FaultKind::BadFrame
        )))
    );
}

#[test]
fn f11_a_container_stopped_by_a_limit_ends_with_an_aggregate_entry() {
    let stop = |entry: EntryPath, file| WorkerMsg::EntryDone {
        entry,
        outcome: Outcome::new(file, None),
        bytes: 0,
    };
    let cut = FileOutcome::Aggregate {
        reason: NotCheckedReason::LimitExceeded(LimitName::TotalEntries),
        count: None,
        at_least: 1,
    };
    assert_eq!(
        stop(EntryPath::from([2, 3]), cut.clone()).ends_container(),
        Some(EntryPath::from([2]))
    );
    assert_eq!(
        stop(EntryPath::from([5]), cut.clone()).ends_container(),
        Some(EntryPath::job())
    );
    assert_eq!(
        stop(EntryPath::from([2, 0]), cut.clone()).ends_container(),
        None
    );
    assert_eq!(
        stop(EntryPath::from([2, 3]), FileOutcome::Read).ends_container(),
        None
    );
    assert_eq!(opened(EntryPath::from([2, 3])).ends_container(), None);

    let stream = [
        stop(EntryPath::from([0]), FileOutcome::Read),
        stop(EntryPath::from([1]), FileOutcome::Read),
        stop(EntryPath::from([2]), cut),
    ];
    let counts = verdict_counts(stream.iter().filter_map(|m| match m {
        WorkerMsg::EntryDone { outcome, .. } => Some(&outcome.file),
        _ => None,
    }));
    assert_eq!(counts.read, 2);
    assert_eq!(counts.not_checked.to_string(), "at least 1");
}

#[test]
fn f12_archive_entry_names_are_normalized_in_one_place() {
    for (raw, path) in [
        ("usr/bin/app", "usr/bin/app"),
        ("./usr/bin/app", "usr/bin/app"),
        ("/usr/bin/app", "usr/bin/app"),
        ("usr//bin/./app/", "usr/bin/app"),
        ("dir\\file.c", "dir\\file.c"),
        ("..a/b..", "..a/b.."),
    ] {
        let got = entry_path(raw.as_bytes()).unwrap();
        assert_eq!(got.as_bytes(), path.as_bytes(), "{raw}");
        assert_eq!(RelPath::new(got.as_text().clone()), Ok(got));
    }
    for (raw, error) in [
        ("../etc/passwd", EntryNameError::DotDotSegment),
        ("a/../b", EntryNameError::DotDotSegment),
        ("a/..", EntryNameError::DotDotSegment),
        ("", EntryNameError::Empty),
        ("./", EntryNameError::Empty),
        ("/", EntryNameError::Empty),
        ("a\0b", EntryNameError::Nul),
    ] {
        assert_eq!(entry_path(raw.as_bytes()), Err(error), "{raw:?}");
    }
    let raw = entry_path(b"./\xff/app").unwrap();
    assert_eq!(raw.as_bytes(), b"\xff/app");
    assert!(!raw.as_text().is_utf8());
    let reason = serde_json::to_value(NotCheckedReason::UnsafeEntryName).unwrap();
    assert_eq!(reason, "unsafe_entry_name");
}

#[test]
fn wire_types_round_trip_through_a_format_that_is_not_self_describing() {
    for msg in common::all_worker_msgs() {
        let bytes = compact::to_bytes(&msg).unwrap();
        assert_eq!(compact::from_bytes::<WorkerMsg>(&bytes).unwrap(), msg);
    }
    for msg in common::all_collector_msgs() {
        let bytes = compact::to_bytes(&msg).unwrap();
        assert_eq!(compact::from_bytes::<CollectorMsg>(&bytes).unwrap(), msg);
    }
    for value in [
        Text::from("héllo"),
        Text::from_bytes(vec![0xff, 0x00]),
        Text::default(),
    ] {
        let bytes = compact::to_bytes(&value).unwrap();
        let mut expected = (value.as_bytes().len() as u64).to_le_bytes().to_vec();
        expected.extend_from_slice(value.as_bytes());
        assert_eq!(bytes, expected);
        assert_eq!(compact::from_bytes::<Text>(&bytes).unwrap(), value);
    }
    let key = CanonicalKey::for_fact(
        &Job {
            kind: JobKind::Route,
            input: input(),
            file: JobFile::InInput(RelPath::try_from("a.rpm").unwrap()),
            resume_after: None,
        },
        &EntryPath::from([1, 2]),
        &sample_fact(FactKind::Call),
    )
    .unwrap();
    let bytes = compact::to_bytes(&key).unwrap();
    assert_eq!(compact::from_bytes::<CanonicalKey>(&bytes).unwrap(), key);
    let versions = compact::to_bytes(&Versions::CURRENT).unwrap();
    assert_eq!(
        compact::from_bytes::<Versions>(&versions).unwrap(),
        Versions::CURRENT
    );

    assert!(compact::from_bytes::<serde_json::Value>(&bytes).is_err());
}

#[test]
fn facts_use_external_tagging_and_frames_carry_json_for_now() {
    let fact = sample_fact(FactKind::Import);
    let json = serde_json::to_value(&fact).unwrap();
    let outer = json.as_object().unwrap();
    assert_eq!(outer.len(), 1);
    assert!(outer.contains_key("import"));
    assert!(outer["import"].get("kind").is_none());

    let msg = WorkerMsg::Facts {
        entry: EntryPath::job(),
        facts: vec![fact],
    };
    let frame = encode_frame(&msg, u64::from(u32::MAX)).unwrap();
    let mut reader = frame.as_slice();
    let decoded = decode_frame::<WorkerMsg, _>(&mut reader, u64::from(u32::MAX))
        .unwrap()
        .unwrap();
    assert_eq!(decoded.msg, msg);
}

#[test]
fn schema_versions_are_major_dot_minor() {
    assert_eq!(RULE_SCHEMA_VERSION, SchemaVersion::new(1, 0));
    let json = serde_json::to_value(Versions::CURRENT).unwrap();
    for (name, version) in json.as_object().unwrap() {
        assert_eq!(version, &serde_json::json!("1.0"), "{name}");
    }
    assert!(RULE_SCHEMA_VERSION.accepts("1.0".parse().unwrap()));
    assert!(!RULE_SCHEMA_VERSION.accepts("1.4".parse().unwrap()));
    assert!(!RULE_SCHEMA_VERSION.accepts("2.0".parse().unwrap()));
}

#[test]
fn f8_validate_uses_the_row_for_the_facts_file_kind() {
    let binary_string = |scope: Scope, line| Fact::StringLiteral {
        file_kind: FileKind::Binary,
        location: common::location_at(Some("usr/bin/app"), line, scope),
        value: Text::from("x"),
        guards: vec![],
    };
    assert_eq!(binary_string(Scope::File, None).validate(), Ok(()));
    assert_eq!(
        binary_string(common::function_scope(), Some(5)).validate(),
        Err(FactError::ScopeNotAllowed {
            kind: FactKind::StringLiteral,
            scope: ScopeKind::Function,
        })
    );
    assert_eq!(
        binary_string(Scope::File, Some(5)).validate(),
        Err(FactError::LineNotAllowed {
            kind: FactKind::StringLiteral,
            rule: LineRule::Absent,
        })
    );
    let mut no_line = sample_fact(FactKind::Call);
    if let Fact::Call { location, .. } = &mut no_line {
        location.line = None;
    }
    assert_eq!(
        no_line.validate(),
        Err(FactError::LineNotAllowed {
            kind: FactKind::Call,
            rule: LineRule::Required,
        })
    );
}

#[test]
fn f8_scope_text_script_format_and_guard_regions_match_the_fact() {
    let mismatched_symbol = Fact::Import {
        file_kind: FileKind::Binary,
        location: common::location_at(
            Some("usr/bin/app"),
            None,
            Scope::Symbol(Text::from("dlsym")),
        ),
        symbol: Text::from("dlopen"),
        version: None,
    };
    let mismatched_key = Fact::ConfigKey {
        file_kind: FileKind::Config,
        location: common::location("etc/a.ini", 1, Scope::ConfigKey(Text::from("a.b"))),
        key: Text::from("a.c"),
        value: None,
    };
    let mismatched_unit = Fact::ServiceSetting {
        file_kind: FileKind::Service,
        location: common::location(
            "app.service",
            2,
            Scope::UnitSection {
                section: Text::from("Service"),
                key: Text::from("User"),
            },
        ),
        section: Text::from("Service"),
        key: Text::from("Group"),
        value: Text::from("app"),
    };
    for (fact, kind) in [
        (mismatched_symbol, FactKind::Import),
        (mismatched_key, FactKind::ConfigKey),
        (mismatched_unit, FactKind::ServiceSetting),
    ] {
        assert_eq!(
            fact.validate(),
            Err(FactError::ScopeDiffersFromFields { kind })
        );
    }

    let script = |format: FrameKind, name: ScriptName| Fact::PackageScript {
        file_kind: FileKind::Package,
        location: Location::frame(
            input(),
            vec![Frame {
                kind: format,
                name: Text::from("app"),
            }],
        )
        .at(None, Scope::Script(name)),
        name,
        interpreter: None,
    };
    assert_eq!(
        script(FrameKind::Rpm, ScriptName::Verify).validate(),
        Ok(())
    );
    assert_eq!(
        script(FrameKind::Deb, ScriptName::Config).validate(),
        Ok(())
    );
    for (format, name) in [
        (FrameKind::Rpm, ScriptName::Config),
        (FrameKind::Deb, ScriptName::Verify),
        (FrameKind::Archive, ScriptName::PostInstall),
    ] {
        assert_eq!(
            script(format, name).validate(),
            Err(FactError::ScriptNameNotForFormat {
                kind: FactKind::PackageScript
            }),
            "{format:?} {name:?}"
        );
    }

    let mut frameless = sample_fact(FactKind::PackageFile);
    if let Fact::PackageFile { location, .. } = &mut frameless {
        location.frames.clear();
    }
    assert_eq!(
        frameless.validate(),
        Err(FactError::NoFrame {
            kind: FactKind::PackageFile
        })
    );

    let region = |guards: Vec<GuardChain>, region, line, end_line| Fact::GuardRegion {
        file_kind: FileKind::Source,
        location: common::location("src/a.c", line, Scope::File),
        region,
        guards,
        end_line,
    };
    let chains = common::guards();
    let last = chains.last().unwrap().region;
    assert_eq!(region(chains.clone(), last, 3, 3).validate(), Ok(()));
    assert_eq!(
        region(vec![], last, 3, 4).validate(),
        Err(FactError::GuardRegion(GuardRegionError::NoGuards))
    );
    assert_eq!(
        region(chains.clone(), last, 5, 4).validate(),
        Err(FactError::GuardRegion(GuardRegionError::EndBeforeLine))
    );
    assert_eq!(
        region(chains, RegionId(last.0 + 1), 3, 4).validate(),
        Err(FactError::GuardRegion(GuardRegionError::RegionMismatch))
    );
}

#[test]
fn worker_outcomes_and_regions_are_checked_by_the_collector() {
    let at = |file: FileOutcome| Outcome::new(file, Some(FileKind::Source));
    let aggregate = |count, at_least| {
        at(FileOutcome::Aggregate {
            reason: NotCheckedReason::LimitExceeded(LimitName::TotalEntries),
            count,
            at_least,
        })
    };
    let child = EntryPath::from([2, 3]);
    assert_eq!(
        at(FileOutcome::Read)
            .with_lines(12)
            .validate_from_worker(&child),
        Ok(())
    );
    assert_eq!(aggregate(Some(5), 5).validate_from_worker(&child), Ok(()));
    assert_eq!(aggregate(None, 0).validate_from_worker(&child), Ok(()));
    let marked = Outcome {
        host_dependent: true,
        ..at(FileOutcome::Read).with_lines(12)
    };
    for (outcome, entry, error) in [
        (
            at(FileOutcome::OutOfScope {
                cause: boxt_core::outcome::OutOfScopeCause::OwnFile(
                    boxt_core::outcome::OwnFileRole::Output,
                ),
            }),
            child.clone(),
            OutcomeError::OutOfScope,
        ),
        (
            at(FileOutcome::NotChecked {
                reason: NotCheckedReason::NotInCompileDatabase,
                details: Details::None,
            }),
            child.clone(),
            OutcomeError::NotInCompileDatabase,
        ),
        (marked, child.clone(), OutcomeError::HostDependent),
        (
            aggregate(Some(0), 0),
            child.clone(),
            OutcomeError::EmptyCount,
        ),
        (
            aggregate(Some(2), 3),
            child.clone(),
            OutcomeError::CountBelowAtLeast,
        ),
        (
            aggregate(None, 1),
            EntryPath::from([2, 0]),
            OutcomeError::AggregateOnEntryZero,
        ),
    ] {
        assert_eq!(
            outcome.validate_from_worker(&entry),
            Err(error),
            "{error:?}"
        );
    }

    let region = |first_line, last_line| Region {
        first_line,
        last_line,
        reason: RegionReason::Unparseable,
    };
    assert_eq!(region(1, 1).validate(), Ok(()));
    assert_eq!(region(3, 9).validate(), Ok(()));
    assert_eq!(region(0, 2).validate(), Err(RegionError::LineZero));
    assert_eq!(region(5, 4).validate(), Err(RegionError::Inverted));
}

#[test]
fn counts_saturate_instead_of_overflowing() {
    let huge = |reason| FileOutcome::Aggregate {
        reason,
        count: Some(u64::MAX),
        at_least: u64::MAX,
    };
    let outcomes = [
        huge(NotCheckedReason::RepeatedFault),
        huge(NotCheckedReason::RepeatedFault),
        FileOutcome::NotChecked {
            reason: NotCheckedReason::NoReader,
            details: Details::None,
        },
    ];
    assert_eq!(
        verdict_counts(&outcomes).not_checked,
        Count::Exact(u64::MAX)
    );
    let source: Vec<CollectedFile> = outcomes
        .iter()
        .map(|f| {
            let outcome = Outcome::new(f.clone(), Some(FileKind::Source));
            let outcome = match f {
                FileOutcome::NotChecked { .. } => outcome.with_lines(u64::MAX - 1),
                _ => outcome,
            };
            common::collected(outcome, &[])
        })
        .chain([
            common::collected(
                Outcome::new(FileOutcome::Read, Some(FileKind::Source)).with_lines(u64::MAX),
                &[(1, u32::MAX)],
            ),
            common::collected(
                Outcome::new(FileOutcome::Read, Some(FileKind::Source)).with_lines(u64::MAX),
                &[(1, u32::MAX)],
            ),
        ])
        .collect();
    let share = sm3_share(&source);
    let files = share.files.unwrap();
    assert_eq!((files.numerator, files.denominator), (u64::MAX, u64::MAX));
    let lines = share.lines.unwrap();
    assert_eq!(
        (lines.numerator, lines.denominator, lines.uncounted_files),
        (u64::MAX, u64::MAX, u64::MAX)
    );
}

#[test]
fn nested_budgets_start_with_fresh_work_and_bytes() {
    let limits = Limits::default()
        .with(LimitName::Work, 10)
        .unwrap()
        .with(LimitName::Memory, 100)
        .unwrap();
    let mut parent = WorkBudget::new(&limits);
    parent.work(7).unwrap();
    parent.buffer(60).unwrap();
    let mut child = parent.nested().unwrap();
    assert_eq!(child.work_done(), 0);
    assert_eq!(child.buffered(), 0);
    assert_eq!(child.work(10), Ok(()));
    assert!(child.work(1).is_err());
    assert_eq!(child.buffer(100), Ok(()));
    assert_eq!(parent.work_done(), 7);
}

#[test]
fn budgets_count_file_size_ratio_and_entries_per_job() {
    let limits = Limits::default()
        .with(LimitName::FileSize, 1000)
        .unwrap()
        .with(LimitName::DecompressionRatio, 10)
        .unwrap()
        .with(LimitName::TotalEntries, 3)
        .unwrap()
        .with(LimitName::TotalDecompressedBytes, 40 * RATIO_FLOOR)
        .unwrap();
    let mut budget = WorkBudget::new(&limits);
    assert_eq!(budget.file_size(1000), Ok(()));
    assert_eq!(
        budget.file_size(1001),
        Err(Exceeded {
            limit: LimitName::FileSize,
            value: 1001,
            max: 1000,
        })
    );

    let mut stream = budget.decompression();
    stream.consumed(1000);
    assert_eq!(stream.produced(RATIO_FLOOR - 1), Ok(()));
    assert_eq!(
        stream.produced(1),
        Err(Exceeded {
            limit: LimitName::DecompressionRatio,
            value: RATIO_FLOOR / 1000,
            max: 10,
        })
    );
    let mut fine = budget.decompression();
    fine.consumed(RATIO_FLOOR);
    assert_eq!(fine.produced(10 * RATIO_FLOOR), Ok(()));
    assert_eq!(fine.ratio(), 10);
    assert!(fine.produced(1).is_err());
    let mut small = budget.decompression();
    assert_eq!(small.produced(RATIO_FLOOR - 1), Ok(()));

    assert_eq!(budget.entry(), Ok(()));
    let mut nested = budget.nested().unwrap();
    assert_eq!(nested.entries(), 1);
    assert_eq!(nested.entry(), Ok(()));
    assert_eq!(nested.entry(), Ok(()));
    let mut tail = nested.decompression();
    tail.consumed(RATIO_FLOOR * 4);
    assert_eq!(tail.produced(3 * RATIO_FLOOR), Ok(()));
    assert_eq!(budget.entries(), 3);
    let used = budget.decompressed();
    assert_eq!(used, 15 * RATIO_FLOOR);
    let mut last = budget.decompression();
    last.consumed(u64::MAX);
    let left = 40 * RATIO_FLOOR - used;
    assert_eq!(last.produced(left), Ok(()));
    assert_eq!(
        last.produced(1),
        Err(Exceeded {
            limit: LimitName::TotalDecompressedBytes,
            value: 40 * RATIO_FLOOR + 1,
            max: 40 * RATIO_FLOOR,
        })
    );
    assert_eq!(
        budget.entry(),
        Err(Exceeded {
            limit: LimitName::TotalEntries,
            value: 4,
            max: 3,
        })
    );
}

#[test]
fn decode_frame_survives_short_and_interrupted_reads() {
    struct Trickle {
        bytes: Vec<u8>,
        at: usize,
        interrupted: bool,
    }
    impl std::io::Read for Trickle {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            if !self.interrupted {
                self.interrupted = true;
                return Err(std::io::ErrorKind::Interrupted.into());
            }
            if self.at == self.bytes.len() || buf.is_empty() {
                return Ok(0);
            }
            buf[0] = self.bytes[self.at];
            self.at += 1;
            Ok(1)
        }
    }
    let msgs = common::all_worker_msgs();
    let mut bytes = Vec::new();
    for msg in &msgs {
        bytes.extend(encode_frame(msg, u64::from(u32::MAX)).unwrap());
    }
    let mut reader = Trickle {
        bytes,
        at: 0,
        interrupted: false,
    };
    for msg in &msgs {
        let decoded = decode_frame::<WorkerMsg, _>(&mut reader, u64::from(u32::MAX))
            .unwrap()
            .unwrap();
        assert_eq!(&decoded.msg, msg);
    }
    assert_eq!(
        decode_frame::<WorkerMsg, _>(&mut reader, u64::from(u32::MAX)),
        Ok(None)
    );
}

#[test]
fn only_function_scope_text_refuses_a_carriage_return() {
    let setting = |value: &str| Fact::ServiceSetting {
        file_kind: FileKind::Service,
        location: common::location(
            "app.service",
            2,
            Scope::UnitSection {
                section: Text::from("Service"),
                key: Text::from("Type"),
            },
        ),
        section: Text::from("Service"),
        key: Text::from("Type"),
        value: Text::from(value),
    };
    assert_eq!(setting("forking").validate(), Ok(()));
    assert_eq!(setting("forking\r").validate(), Ok(()));
    let key = |value: Option<&str>| Fact::ConfigKey {
        file_kind: FileKind::Config,
        location: common::location("etc/a.ini", 1, Scope::ConfigKey(Text::from("a.b"))),
        key: Text::from("a.b"),
        value: value.map(Text::from),
    };
    assert_eq!(key(Some("1")).validate(), Ok(()));
    assert_eq!(key(Some("1\r")).validate(), Ok(()));
    let symbol = Fact::Import {
        file_kind: FileKind::Binary,
        location: common::location_at(
            Some("usr/bin/app"),
            None,
            Scope::Symbol(Text::from("odd\rname")),
        ),
        symbol: Text::from("odd\rname"),
        version: None,
    };
    assert_eq!(symbol.validate(), Ok(()));
    let call = |signature: &str| Fact::Call {
        file_kind: FileKind::Source,
        location: common::location(
            "src/a.cc",
            3,
            Scope::Function {
                qualified_name: Text::from("ns::f"),
                signature: Text::from(signature),
            },
        ),
        callee: Text::from("g"),
        args: vec![],
        guards: vec![],
    };
    assert_eq!(call("int, char *").validate(), Ok(()));
    assert_eq!(
        call("int,\r\n char *").validate(),
        Err(FactError::CarriageReturn {
            kind: FactKind::Call
        })
    );
}

#[test]
fn the_router_resolves_the_encoding_and_readers_see_only_the_result() {
    use boxt_core::settings::{Encoding, EncodingDecl, PathPattern};
    let decl = |pattern: &str, encoding| EncodingDecl {
        pattern: PathPattern {
            base: Text::from("/repo"),
            pattern: Text::from(pattern),
        },
        encoding,
    };
    let settings = WorkerSettings {
        limits: Limits::default(),
        encodings: vec![
            decl("legacy/", Encoding::Latin1),
            decl("legacy/win/", Encoding::WindowsCodePage(1252)),
        ],
    };
    let prefix = |path: &'static str| {
        move |p: &PathPattern| path.as_bytes().starts_with(p.pattern.as_bytes())
    };
    assert_eq!(
        settings.resolve_encoding(prefix("legacy/win/a.c")),
        Some(Encoding::Latin1)
    );
    assert_eq!(settings.resolve_encoding(prefix("src/a.c")), None);
    let ctx = ReadCtx::new(
        Location::file(input(), vec![], RelPath::try_from("legacy/a.c").unwrap()),
        &settings.limits,
        settings.resolve_encoding(prefix("legacy/a.c")),
    );
    assert_eq!(ctx.encoding, Some(Encoding::Latin1));
}

#[test]
fn collector_made_outcomes_take_the_key_of_the_first_file_they_cover() {
    let first = RelPath::try_from("vendor/a.c").unwrap();
    let key = CanonicalKey::at(0, first.clone(), EntryPath::job());
    assert_eq!(key.location, None);
    let job = Job {
        kind: JobKind::Route,
        input: input(),
        file: JobFile::InInput(first),
        resume_after: None,
    };
    assert_eq!(
        Some(key.clone()),
        CanonicalKey::for_outcome(&job, &EntryPath::job())
    );
    let later = CanonicalKey::at(
        0,
        RelPath::try_from("vendor/b.c").unwrap(),
        EntryPath::job(),
    );
    assert!(key < later);

    let whole = Job {
        file: JobFile::WholeInput(RelPath::try_from("app.rpm").unwrap()),
        ..job
    };
    assert!(whole.is_well_formed());
    assert_eq!(whole.path().unwrap().as_bytes(), b"app.rpm");
}

struct Expand<R> {
    inner: R,
    byte: u8,
    left: usize,
}

impl<R: std::io::Read> std::io::Read for Expand<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.left == 0 {
            let mut one = [0u8; 1];
            if self.inner.read(&mut one)? == 0 {
                return Ok(0);
            }
            self.byte = one[0];
            self.left = 64 * 1024;
        }
        let n = buf.len().min(self.left);
        buf[..n].fill(self.byte);
        self.left -= n;
        Ok(n)
    }
}

type Lazy = boxt_core::read::ChargedRead<Expand<boxt_core::read::CountedRead<Cursor<Vec<u8>>>>>;

struct Compressed {
    members: std::vec::IntoIter<Vec<u8>>,
    current: Option<Lazy>,
}

impl Container for Compressed {
    fn name(&self) -> Option<Text> {
        None
    }

    fn own_facts(&mut self, _: &mut ReadCtx, _: &mut dyn FactSink) -> ReadResult {
        Ok(())
    }

    fn next_entry(&mut self, ctx: &mut ReadCtx) -> ReadResult<Option<Entry<'_>>> {
        let Some(member) = self.members.next() else {
            return Ok(None);
        };
        let counted = boxt_core::read::CountedRead::new(Cursor::new(member));
        let count = counted.count();
        let expand = Expand {
            inner: counted,
            byte: 0,
            left: 0,
        };
        self.current = Some(boxt_core::read::ChargedRead::new(
            expand,
            ctx.decompression(),
            count,
        ));
        Ok(Some(Entry {
            name: EntryName::Path(Text::from("member")),
            kind: EntryKind::File,
            size: None,
            data: Input::Stream(self.current.as_mut().unwrap()),
        }))
    }
}

fn read_lazily(limits: &Limits, members: Vec<Vec<u8>>) -> Vec<Result<u64, Exceeded>> {
    let mut ctx = ReadCtx::new(
        Location::file(input(), vec![], RelPath::try_from("a.tgz").unwrap()),
        limits,
        None,
    );
    let mut container = Compressed {
        members: members.into_iter(),
        current: None,
    };
    let mut out = Vec::new();
    while let Some(mut entry) = container.next_entry(&mut ctx).unwrap() {
        let mut body = Vec::new();
        out.push(
            entry
                .data
                .reader()
                .read_to_end(&mut body)
                .map(|n| n as u64)
                .map_err(|e| Exceeded::from_io(&e).expect("a limit")),
        );
    }
    out
}

#[test]
fn a_lazily_read_entry_charges_the_ratio_and_the_job_cap() {
    let bomb = Limits::default()
        .with(LimitName::DecompressionRatio, 100)
        .unwrap();
    let result = read_lazily(&bomb, vec![vec![b'a'; 40]]);
    let Err(e) = result[0] else {
        panic!("the ratio must trip: {result:?}");
    };
    assert_eq!(e.limit, LimitName::DecompressionRatio);
    assert_eq!(e.max, 100);

    let capped = Limits::default()
        .with(LimitName::DecompressionRatio, 1_000_000)
        .unwrap()
        .with(LimitName::TotalDecompressedBytes, 3 * 64 * 1024)
        .unwrap();
    let result = read_lazily(&capped, vec![vec![b'a'; 2], vec![b'b'; 2]]);
    assert_eq!(result[0], Ok(2 * 64 * 1024));
    let Err(e) = result[1] else {
        panic!("the job cap must trip on the second entry: {result:?}");
    };
    assert_eq!(e.limit, LimitName::TotalDecompressedBytes);
    assert_eq!(e.max, 3 * 64 * 1024);
}

#[test]
fn config_key_paths_are_joined_in_one_place() {
    use boxt_core::fact::{KeySegment, config_key_path};
    let name = |s: &str| KeySegment::Name(Text::from(s));
    let attr = |s: &str| KeySegment::Attribute(Text::from(s));
    for (segments, joined) in [
        (vec![name("server"), name("port")], "server.port"),
        (vec![name("a"), KeySegment::Index(0), name("b")], "a[0].b"),
        (vec![name("a"), name("x.y"), name("c")], "a[\"x.y\"].c"),
        (
            vec![name("a"), KeySegment::Index(2), KeySegment::Index(1)],
            "a[2][1]",
        ),
        (vec![KeySegment::Index(0), name("b")], "[0].b"),
        (vec![name("x[1]")], "[\"x[1]\"]"),
        (vec![name("line\r\nend")], "[\"line\\r\\nend\"]"),
        (vec![name("q\"b\\")], "q\"b\\"),
        (vec![name("q\"b.\\")], "[\"q\\\"b.\\\\\"]"),
        (vec![name("bell\u{7}")], "[\"bell\\u0007\"]"),
        (vec![name("server"), attr("port")], "server.@port"),
        (vec![name("server"), attr("a.b")], "server[\"@a.b\"]"),
        (vec![name("@lit")], "[\"@lit\"]"),
        (vec![name("")], "[\"\"]"),
        (vec![name("é")], "é"),
        (vec![], ""),
    ] {
        assert_eq!(
            config_key_path(&segments),
            Text::from(joined),
            "{segments:?}"
        );
    }
}

#[test]
fn a_cut_inside_a_container_sorts_after_the_entries_before_it() {
    let path = RelPath::try_from("dist/app.tar").unwrap();
    let job = Job {
        kind: JobKind::Route,
        input: input(),
        file: JobFile::InInput(path.clone()),
        resume_after: None,
    };
    let mut keys = [
        CanonicalKey::at(0, path.clone(), EntryPath::from([2, 1])),
        CanonicalKey::for_outcome(&job, &EntryPath::from([3])).unwrap(),
        CanonicalKey::for_outcome(&job, &EntryPath::from([2, 0])).unwrap(),
        CanonicalKey::for_outcome(&job, &EntryPath::from([1])).unwrap(),
    ];
    keys.sort();
    let entries: Vec<&EntryPath> = keys.iter().map(|k| &k.entry).collect();
    assert_eq!(
        entries,
        vec![
            &EntryPath::from([1]),
            &EntryPath::from([2, 0]),
            &EntryPath::from([2, 1]),
            &EntryPath::from([3])
        ]
    );
}

fn source_outcome(file: FileOutcome) -> Outcome {
    Outcome::new(file, Some(FileKind::Source))
}

fn file_size_cut() -> FileOutcome {
    FileOutcome::NotChecked {
        reason: NotCheckedReason::LimitExceeded(LimitName::FileSize),
        details: Details::None,
    }
}

#[test]
fn s1_a_partial_count_is_marked_and_never_rides_on_a_read_outcome() {
    let child = EntryPath::from([1]);
    let partial = source_outcome(file_size_cut()).with_partial_lines(400);
    assert_eq!(
        partial.lines,
        Some(LineCount {
            lines: 400,
            complete: false
        })
    );
    assert_eq!(partial.validate_from_worker(&child), Ok(()));
    assert_eq!(
        source_outcome(FileOutcome::Read)
            .with_partial_lines(4)
            .validate_from_worker(&child),
        Err(OutcomeError::PartialCountOnRead)
    );
}

#[test]
fn s2_s5_uncounted_and_incomplete_files_are_listed_in_canonical_order() {
    let files = [
        common::collected_at("z.c", source_outcome(file_size_cut()), &[]),
        common::collected_at(
            "m.c",
            source_outcome(file_size_cut()).with_partial_lines(3),
            &[],
        ),
        common::collected_at("a.c", source_outcome(file_size_cut()), &[]),
    ];
    let share = sm3_share(&files);
    let lines = share.lines.unwrap();
    assert_eq!(
        (lines.numerator, lines.denominator),
        (3, 3),
        "the partial count joins both counts"
    );
    assert_eq!((lines.uncounted_files, lines.incomplete_files), (2, 1));
    let keys = |paths: &[&str]| -> Vec<CanonicalKey> {
        paths
            .iter()
            .map(|p| CanonicalKey::at(0, RelPath::try_from(*p).unwrap(), EntryPath::job()))
            .collect()
    };
    assert_eq!(share.uncounted.as_slice(), keys(&["a.c", "z.c"]).as_slice());
    assert_eq!(share.incomplete.as_slice(), keys(&["m.c"]).as_slice());

    let empty = common::collected_at("e.c", source_outcome(FileOutcome::Read).with_lines(0), &[]);
    assert_eq!(sm3_share([&empty]).lines, None);
    let json = serde_json::to_value(sm3_share(&files)).unwrap();
    let mut fields: Vec<&str> = json
        .as_object()
        .unwrap()
        .keys()
        .map(|k| k.as_str())
        .collect();
    fields.sort_unstable();
    assert_eq!(fields, ["files", "incomplete", "lines", "uncounted"]);
}

fn macros_job(path: &str) -> Job {
    Job {
        kind: JobKind::Definitions,
        input: input(),
        file: JobFile::InInput(RelPath::try_from(path).unwrap()),
        resume_after: None,
    }
}

#[test]
fn s6_a_worker_keeps_only_complete_tables_and_replaces_them() {
    let def = |name: &str, line| common::macro_def(name, "src/config.h", line, None);
    let mut tables = MacroTables::new();
    assert_eq!(tables.for_tree(0), None);

    tables.receive(0, vec![def("B", 2)].into(), false).unwrap();
    assert_eq!(
        tables.for_tree(0),
        None,
        "a table without its end mark is not used"
    );
    tables.receive(0, vec![def("A", 1)].into(), true).unwrap();
    let first = tables.for_tree(0).unwrap().clone();
    assert_eq!(first.len(), 2);
    assert_eq!(first[0].name, Text::from("A"));

    tables.receive(0, vec![def("C", 3)].into(), false).unwrap();
    assert_eq!(
        tables.for_tree(0),
        None,
        "a replacement drops the old table"
    );
    tables.receive(0, MacroTable::new(), true).unwrap();
    let second = tables.for_tree(0).unwrap();
    assert_eq!(second.len(), 1);
    assert_eq!(second[0].name, Text::from("C"));

    tables.receive(4, MacroTable::new(), true).unwrap();
    assert_eq!(tables.for_tree(4), Some(&MacroTable::new()));
    assert_eq!(tables.for_tree(3), None);

    assert_eq!(
        tables.receive(5, vec![def("D", 4)].into(), true),
        Err(MacroTablesError::ForeignDefinition { tree: 5 })
    );
    assert_eq!(tables.for_tree(5), None);
}

#[test]
fn s7_a_definition_must_name_an_identifier_with_distinct_parameters() {
    let job = macros_job("src/config.h");
    let with = |name: &str, params: Option<MacroParams>| MacroDef {
        name: Text::from(name),
        ..common::macro_def("X", "src/config.h", 3, params)
    };
    let params = |names: &[&str], variadic: Option<Variadic>| {
        Some(MacroParams {
            names: names.iter().map(|n| Text::from(*n)).collect(),
            variadic,
        })
    };
    let cap = 4096;
    for ok in ["FOO", "_x1", "$sys", "na\u{ef}ve", "a"] {
        assert_eq!(with(ok, None).validate(&job, cap), Ok(()), "{ok}");
    }
    assert_eq!(
        with("", None).validate(&job, cap),
        Err(MacroDefError::EmptyName)
    );
    for bad in ["1A", "A-B", "A B", "A(", "A.b"] {
        assert_eq!(
            with(bad, None).validate(&job, cap),
            Err(MacroDefError::NotIdentifier),
            "{bad}"
        );
    }
    let raw = MacroDef {
        name: Text::from_bytes(vec![b'A', 0xFF]),
        ..with("A", None)
    };
    assert_eq!(raw.validate(&job, cap), Err(MacroDefError::NotIdentifier));
    assert_eq!(
        with("F", params(&["a", "b"], Some(Variadic::Ellipsis))).validate(&job, cap),
        Ok(())
    );
    assert_eq!(
        with("F", params(&["a", "a"], None)).validate(&job, cap),
        Err(MacroDefError::DuplicateParameter)
    );
    assert_eq!(
        with("F", params(&["a"], Some(Variadic::Named(Text::from("a"))))).validate(&job, cap),
        Err(MacroDefError::DuplicateParameter)
    );
    assert_eq!(
        with(
            "F",
            params(&["a"], Some(Variadic::Named(Text::from("rest"))))
        )
        .validate(&job, cap),
        Ok(())
    );
    assert_eq!(
        with("F", params(&["1"], None)).validate(&job, cap),
        Err(MacroDefError::BadParameter)
    );
    assert_eq!(
        with("F", params(&[], Some(Variadic::Named(Text::from(""))))).validate(&job, cap),
        Err(MacroDefError::BadParameter)
    );
}

#[test]
fn s8_a_line_count_is_never_above_the_bytes_read() {
    let outcome = source_outcome(FileOutcome::Read).with_lines(10);
    assert_eq!(outcome.validate_lines(10), Ok(()));
    assert_eq!(
        outcome.validate_lines(9),
        Err(OutcomeError::LinesAboveBytes {
            lines: 10,
            bytes: 9
        })
    );
    let partial = source_outcome(file_size_cut()).with_partial_lines(5);
    assert!(partial.validate_lines(4).is_err());
    assert_eq!(source_outcome(file_size_cut()).validate_lines(0), Ok(()));
}

#[test]
fn s4_the_merged_table_stops_at_its_byte_cap_in_canonical_order() {
    let defs = vec![
        common::macro_def("A", "src/z.h", 1, None),
        common::macro_def("Z", "src/a.h", 9, None),
        common::macro_def("M", "src/a.h", 2, None),
    ];
    let whole = merge_macro_tables(defs.clone(), u64::MAX);
    assert!(whole.cut.is_empty());
    assert_eq!(whole.table(0), MacroTable::from(defs.clone()));

    let two = defs[1].table_bytes() + defs[2].table_bytes();
    let cut = merge_macro_tables(defs.clone(), two);
    assert_eq!(cut.cut.as_slice(), &[0]);
    let names: Vec<Text> = cut.table(0).iter().map(|d| d.name.clone()).collect();
    assert_eq!(names, [Text::from("M"), Text::from("Z")]);
    let reversed: Vec<MacroDef> = defs.iter().rev().cloned().collect();
    assert_eq!(merge_macro_tables(reversed, two), cut);
    assert_eq!(merge_macro_tables(defs, two - 1).table(0).len(), 0);
    assert_eq!(LimitName::MacroTableBytes.flag(), "macro-table-bytes");
    assert_eq!(LimitName::MacroTableBytes.spec().scope, LimitScope::RunWide);
}

#[test]
fn s12_a_cap_above_the_frame_prefix_splits_like_the_prefix_maximum() {
    let defs: Vec<MacroDef> = (1..=3)
        .map(|i| common::macro_def(&format!("M{i}"), "src/config.h", i, None))
        .collect();
    assert_eq!(
        encode_definitions(&defs, u64::MAX).unwrap(),
        encode_definitions(&defs, u64::from(u32::MAX)).unwrap()
    );
    let table = MacroTable::from(defs);
    assert_eq!(
        encode_macro_table(0, &table, u64::MAX).unwrap(),
        encode_macro_table(0, &table, u64::from(u32::MAX)).unwrap()
    );
}

#[test]
fn s14_a_definition_at_the_envelope_boundary_fits_both_directions() {
    let job = macros_job("src/config.h");
    let def = common::macro_def("EDGE", "src/config.h", 7, None);
    let table = MacroTable::from(vec![def.clone()]);
    let worker = payload_len(&WorkerMsg::Definitions {
        definitions: vec![def.clone()],
    });
    let collector = payload_len(&CollectorMsg::Macros {
        tree: u32::MAX,
        definitions: table.clone(),
        end: false,
    });
    let edge = worker.max(collector);
    assert_eq!(def.validate(&job, edge), Ok(()));
    assert!(matches!(
        def.validate(&job, edge - 1),
        Err(MacroDefError::TooLong { .. })
    ));
    assert!(encode_definitions(std::slice::from_ref(&def), edge).is_ok());
    assert!(encode_macro_table(u32::MAX, &table, edge).is_ok());
    assert!(matches!(
        encode_definitions(std::slice::from_ref(&def), edge - 1),
        Err(FrameError::TooLong { .. })
    ));
}

fn payload_len<T: serde::Serialize>(msg: &T) -> u64 {
    let frame = encode_frame(msg, u64::from(u32::MAX)).unwrap();
    (frame.len() - FRAME_PREFIX_LEN) as u64
}

#[test]
fn s15_only_an_undeclared_file_is_sniffed_for_a_utf16_mark() {
    let bytes = [0xFF, 0xFE, b'\n', 0x00];
    assert_eq!(line_count(&bytes, None), 1);
    assert_eq!(line_count(&bytes, Some(Encoding::Utf16Le)), 1);
    for declared in [
        Encoding::Utf8,
        Encoding::Latin1,
        Encoding::WindowsCodePage(1252),
    ] {
        assert_eq!(line_count(&bytes, Some(declared)), 2, "{declared:?}");
    }
    let be = [0xFE, 0xFF, 0x00, b'\n'];
    assert_eq!(line_count(&be, None), 1);
    assert_eq!(line_count(&be, Some(Encoding::Utf8)), 1);
    assert_eq!(
        line_count(&[0xFE, 0xFF, b'a', b'\n', b'b'], Some(Encoding::Utf8)),
        2
    );
}

#[test]
fn s16_regions_of_a_file_with_no_count_are_not_checked_against_one() {
    let file = common::collected_at(
        "src/main.c",
        source_outcome(file_size_cut()),
        &[(5, 1_000_000)],
    );
    assert_eq!(file.outcome.lines, None);
    assert_eq!(file.validate_regions(), Ok(()));
    let bad = common::collected_at("src/main.c", source_outcome(file_size_cut()), &[(0, 3)]);
    assert_eq!(bad.validate_regions(), Err(RegionError::LineZero));
}

#[test]
fn s4_the_merge_keeps_whole_files_and_a_strict_prefix() {
    let def = |name: &str, path: &str, body: usize| MacroDef {
        body: Text::from("x".repeat(body)),
        ..common::macro_def(name, path, 1, None)
    };
    let defs = vec![
        def("A", "src/x.h", 1),
        def("B", "src/y.h", 400),
        def("C", "src/z.h", 1),
        def("D", "src/x.h", 1),
    ];
    let small = defs[0].table_bytes() + defs[3].table_bytes();
    let cap = small + defs[2].table_bytes() + 10;
    assert!(cap < small + defs[1].table_bytes());
    let merged = merge_macro_tables(defs.clone(), cap);
    assert_eq!(merged.cut.as_slice(), &[0]);
    let names: Vec<Text> = merged.table(0).iter().map(|d| d.name.clone()).collect();
    assert_eq!(names, [Text::from("A"), Text::from("D")]);
    assert_eq!(merge_macro_tables(defs, small - 1).table(0).len(), 0);
}

fn in_input(def: MacroDef, index: u32, given: &str) -> MacroDef {
    MacroDef {
        location: Location {
            input: boxt_core::location::InputRef {
                index,
                given: Text::from(given),
            },
            ..def.location.clone()
        },
        ..def
    }
}

#[test]
fn s4_the_cap_is_run_wide_in_input_order_and_ignores_the_given_path() {
    let def = |name: &str, path: &str| common::macro_def(name, path, 1, None);
    let defs = |given: &str| {
        vec![
            in_input(def("B", "b.h"), 1, given),
            in_input(def("A", "a.h"), 0, "/repo"),
            in_input(def("C", "c.h"), 1, given),
            in_input(def("D", "d.h"), 2, "/third"),
        ]
    };
    let cap = 3 * def("A", "a.h").table_bytes();
    let short = merge_macro_tables(defs("/src"), cap);
    let long = merge_macro_tables(defs("/a/much/longer/way/to/the/same/src"), cap);
    assert_eq!(short.cut, long.cut);
    assert_eq!(short.cut.as_slice(), &[2]);
    let names = |m: &MergedTables, tree| -> Vec<Text> {
        m.table(tree).iter().map(|d| d.name.clone()).collect()
    };
    for merged in [&short, &long] {
        assert_eq!(names(merged, 0), [Text::from("A")]);
        assert_eq!(names(merged, 1), [Text::from("B"), Text::from("C")]);
        assert!(names(merged, 2).is_empty());
    }
    let tight = merge_macro_tables(defs("/src"), cap - 1);
    assert_eq!(tight.cut.as_slice(), &[1, 2]);
    assert_eq!(names(&tight, 1), [Text::from("B")]);
}

#[test]
fn s7_a_parameter_named_va_args_and_non_ascii_blanks_are_refused() {
    let job = macros_job("src/config.h");
    let with = |name: &str, names: &[&str]| MacroDef {
        name: Text::from(name),
        ..common::macro_def(
            "X",
            "src/config.h",
            3,
            Some(MacroParams {
                names: names.iter().map(|n| Text::from(*n)).collect(),
                variadic: None,
            }),
        )
    };
    assert_eq!(
        with("F", &["__VA_ARGS__"]).validate(&job, 4096),
        Err(MacroDefError::BadParameter)
    );
    for bad in ["A\u{a0}B", "A\u{2028}", "\u{85}A", "A\u{3000}"] {
        assert_eq!(
            with(bad, &[]).validate(&job, 4096),
            Err(MacroDefError::NotIdentifier),
            "{bad:?}"
        );
        assert_eq!(
            with("F", &[bad]).validate(&job, 4096),
            Err(MacroDefError::BadParameter),
            "{bad:?}"
        );
    }
}

#[test]
fn s10_a_region_past_the_end_is_clamped_to_the_last_line_and_still_counts() {
    let region = Region {
        first_line: 11,
        last_line: 11,
        reason: RegionReason::Unparseable,
    };
    let fitted = region.fit_within(10).unwrap();
    assert_eq!((fitted.first_line, fitted.last_line), (10, 10));
    assert_eq!(region.fit_within(0), None);
    let wide = Region {
        first_line: 8,
        last_line: 12,
        ..region
    };
    let fitted_wide = wide.fit_within(10).unwrap();
    assert_eq!((fitted_wide.first_line, fitted_wide.last_line), (8, 10));

    let file = common::collected_at(
        "src/main.c",
        source_outcome(FileOutcome::Read).with_lines(10),
        &[(fitted.first_line, fitted.last_line)],
    );
    assert_eq!(file.validate_regions(), Ok(()));
    assert!(file.has_unreadable_region());
    let share = sm3_share([&file]);
    let lines = share.lines.unwrap();
    assert_eq!((lines.numerator, lines.denominator), (1, 10));
    let files = share.files.unwrap();
    assert_eq!((files.numerator, files.denominator), (1, 1));
}

#[test]
fn s5_an_uncounted_aggregate_lists_one_key_and_adds_its_count() {
    let aggregate = common::collected_at(
        "b.c",
        source_outcome(FileOutcome::Aggregate {
            reason: NotCheckedReason::LimitExceeded(LimitName::TotalEntries),
            count: Some(3),
            at_least: 3,
        }),
        &[],
    );
    let single = common::collected_at("a.c", source_outcome(file_size_cut()), &[]);
    let share = sm3_share([&aggregate, &single]);
    assert_eq!(share.lines.unwrap().uncounted_files, 4);
    assert_eq!(
        share.uncounted.as_slice(),
        &[single.key.clone(), aggregate.key.clone()]
    );
}

#[test]
fn s16_regions_are_checked_against_an_incomplete_count() {
    let outcome = source_outcome(file_size_cut()).with_partial_lines(5);
    let inside = common::collected_at("src/main.c", outcome.clone(), &[(2, 5)]);
    assert_eq!(inside.validate_regions(), Ok(()));
    let past = common::collected_at("src/main.c", outcome, &[(4, 6)]);
    assert_eq!(past.validate_regions(), Err(RegionError::PastEnd));
}
