mod common;

use std::io::Cursor;

use boxt_core::Text;
use boxt_core::compile_db::CompileDatabaseError;
use boxt_core::fact::{
    Directive, Fact, FactError, FactKind, FileKind, GuardChain, GuardError, PathRule, RegionId,
};
use boxt_core::limits::{LimitError, LimitName, Limits, MEMORY_CAP_FACTOR};
use boxt_core::location::{
    EntryNameError, Frame, FrameKind, Location, PathText, RelPath, SCRIPT_NAMES, Scope, ScriptName,
    entry_path, frame_name, normalize_value_path,
};
use boxt_core::message::{
    CanonicalKey, CollectedFile, CollectorMsg, EntryBudget, EntryPath, ExitClass, FaultScope, Job,
    JobFile, JobKind, Note, PANIC_EXIT, Platform, RawExit, Region, RegionReason,
    WORKER_MEMORY_CAP_EXIT, WorkerMsg, classify_exit, decode_frame, encode_frame, fault_scope,
    faulting_entry, resume_skips,
};
use boxt_core::outcome::{
    Details, FaultKind, FileOutcome, NotCheckedReason, Outcome, sm3_share, verdict_counts,
};
use boxt_core::read::{
    Container, ContainerReader, Entry, EntryKind, EntryName, Exceeded, FactSink, Input, NotRead,
    ReadCtx, ReadResult, Reader, WorkBudget,
};
use boxt_core::settings::{CompileDatabaseSetting, Settings, WorkerSettings};
use boxt_core::shell::{self, is_shell_interpreter};
use boxt_core::verdict::{RuleCoverage, RuleId};
use boxt_core::version::{RULE_SCHEMA_VERSION, SchemaVersion, Versions};

use common::{compact, frame, input, opened, sample_fact, worker_settings};

struct LineReader;

impl Reader for LineReader {
    fn file_kind(&self) -> FileKind {
        FileKind::Binary
    }

    fn read(&self, bytes: &[u8], ctx: &mut ReadCtx<'_>, sink: &mut dyn FactSink) -> ReadResult {
        for line in bytes.split(|&c| c == b'\n').filter(|l| !l.is_empty()) {
            ctx.budget.work(1)?;
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
        ctx: &mut ReadCtx<'_>,
    ) -> ReadResult<Box<dyn Container + 'a>> {
        let mut magic = [0u8; 4];
        input
            .reader()
            .read_exact(&mut magic)
            .map_err(|_| NotRead::new(NotCheckedReason::Unparseable))?;
        if &magic != b"PACK" {
            return Err(NotRead::new(NotCheckedReason::Unparseable));
        }
        ctx.budget.work(1)?;
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

    fn own_facts(&mut self, _: &mut ReadCtx<'_>, _: &mut dyn FactSink) -> ReadResult {
        Ok(())
    }

    fn next_entry(&mut self, ctx: &mut ReadCtx<'_>) -> ReadResult<Option<Entry<'_>>> {
        let mut header = [0u8; 2];
        if self.data.read_exact(&mut header).is_err() {
            return Ok(None);
        }
        let mut name = vec![0u8; usize::from(header[0])];
        let mut body = vec![0u8; usize::from(header[1])];
        let bad = |_| NotRead::new(NotCheckedReason::Unparseable);
        self.data.read_exact(&mut name).map_err(bad)?;
        self.data.read_exact(&mut body).map_err(bad)?;
        ctx.budget.buffer(body.len() as u64)?;
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
    let mut ctx = ReadCtx::new(Location::file(input(), vec![], file.clone()), settings);
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
    let mut base = ReadCtx::new(Location::frame(input(), frames.clone()), settings);
    base.budget = ctx.budget.nested()?;
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
            settings,
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
        outcome: Outcome::new(FileOutcome::Read, Some(FileKind::Source)),
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
    let share = sm3_share(
        files
            .iter()
            .map(|f| (&f.outcome, f.has_unreadable_region())),
    );
    assert_eq!(share.map(|s| (s.numerator, s.denominator)), Some((1, 3)));
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
    for bad in [
        command(Some("etc/init.d/app"), in_script, FileKind::Package),
        command(None, Scope::File, FileKind::Service),
    ] {
        assert_eq!(
            bad.validate(),
            Err(FactError::PathNotAllowed {
                kind: FactKind::ShellCommand,
                rule: PathRule::FrameForScripts,
            })
        );
    }

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
        (Some(""), false),
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
            classify_exit(platform, exit),
            class,
            "{platform:?} {exit:?}"
        );
    }
    assert_eq!(
        ExitClass::WorkerMemoryCap.reason(),
        NotCheckedReason::LimitExceeded(LimitName::WorkerMemoryCap)
    );
    assert!(ExitClass::WorkerMemoryCap.host_dependent());
    assert!(!crash.host_dependent());
    assert_eq!(crash.reason(), NotCheckedReason::Fault(FaultKind::Crash));
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
    assert_eq!(
        limits.set(LimitName::Memory, cap / MEMORY_CAP_FACTOR + 1),
        Err(LimitError::CapBelowMemory {
            cap,
            memory: cap / MEMORY_CAP_FACTOR + 1
        })
    );
    assert_eq!(limits, defaults);
    assert_eq!(
        limits.set(LimitName::Memory, cap / MEMORY_CAP_FACTOR),
        Ok(())
    );
    assert!(limits.set(LimitName::WorkerMemoryCap, cap - 1).is_err());
    assert_eq!(limits.get(LimitName::WorkerMemoryCap), cap);
    assert_eq!(limits.set(LimitName::WorkerMemoryCap, 4 * cap), Ok(()));
    assert_eq!(limits.set(LimitName::Memory, cap), Ok(()));
    assert_eq!(
        limits.set(LimitName::Memory, u64::MAX),
        Err(LimitError::CapBelowMemory {
            cap: 4 * cap,
            memory: u64::MAX
        })
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
        region: RegionId(3),
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
    let coverage = RuleCoverage {
        rule: RuleId::new("identity/needs-root").unwrap(),
        file_kinds: vec![
            vec![FileKind::Source, FileKind::Package].into(),
            vec![FileKind::Binary].into(),
        ]
        .into(),
    };
    let alternatives: Vec<Vec<FileKind>> = coverage.file_kinds.iter().map(|a| a.to_vec()).collect();
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
    let job = settings.compile_database_job().unwrap();
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
    assert_eq!(out_of_range.compile_database_job(), None);

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
    for (_, version) in json.as_object().unwrap() {
        assert_eq!(version, &serde_json::json!({"major": 1, "minor": 0}));
    }
    assert!(RULE_SCHEMA_VERSION.accepts("1.4".parse().unwrap()));
    assert!(!RULE_SCHEMA_VERSION.accepts("2.0".parse().unwrap()));
}
