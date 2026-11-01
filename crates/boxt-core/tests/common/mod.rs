#![allow(dead_code)]

pub mod compact;

use boxt_core::Text;
use boxt_core::compile_db::CompileUnit;
use boxt_core::fact::{Directive, Fact, FactKind, FileKind, GuardChain, GuardFrame, RegionId};
use boxt_core::limits::{LimitName, Limits};
use boxt_core::location::{
    Frame, FrameKind, InputRef, Location, PathText, RelPath, Scope, ScopeKind, ScriptName,
};
use boxt_core::message::{
    CanonicalKey, CollectedFile, CollectorMsg, EntryPath, Job, JobFile, JobKind, MacroDef,
    MacroParams, MacroTable, Note, Region, RegionReason, Variadic, WorkerMsg,
};
use boxt_core::outcome::{
    Details, FaultKind, FileOutcome, IoKind, LineCount, LinkTarget, NotCheckedReason,
    OutOfScopeCause, Outcome, OwnFileRole, SpecialKind,
};
use boxt_core::settings::{Encoding, EncodingDecl, PathPattern, WorkerSettings};
use boxt_core::shell::{Assignment, RedirectOp, Redirection};

pub fn input() -> InputRef {
    InputRef {
        index: 0,
        given: Text::from("/home/dev/app"),
    }
}

pub fn file_kinds() -> Vec<FileKind> {
    FileKind::ALL.to_vec()
}

pub fn frame_kinds() -> Vec<FrameKind> {
    FrameKind::ALL.to_vec()
}

pub fn redirect_ops() -> Vec<RedirectOp> {
    RedirectOp::ALL.to_vec()
}

pub fn scope_of(kind: ScopeKind) -> Scope {
    match kind {
        ScopeKind::File => Scope::File,
        ScopeKind::Function => function_scope(),
        ScopeKind::Symbol => Scope::Symbol(Text::from("setuid")),
        ScopeKind::UnitSection => Scope::UnitSection {
            section: Text::from("Service"),
            key: Text::from("Type"),
        },
        ScopeKind::ConfigKey => Scope::ConfigKey(Text::from("server.port")),
        ScopeKind::Script => Scope::Script(ScriptName::PostInstall),
    }
}

pub fn scopes() -> Vec<Scope> {
    let mut out: Vec<Scope> = ScopeKind::ALL.into_iter().map(scope_of).collect();
    out.extend(ScriptName::ALL.into_iter().map(Scope::Script));
    out
}

pub fn frames() -> Vec<Frame> {
    frame_kinds()
        .into_iter()
        .map(|kind| Frame {
            kind,
            name: Text::from("app"),
        })
        .collect()
}

pub fn location_at(path: Option<&str>, line: Option<u32>, scope: Scope) -> Location {
    Location {
        input: input(),
        frames: frames(),
        path: path.map(|p| RelPath::try_from(p).unwrap()),
        line,
        scope,
    }
}

pub fn location(path: &str, line: u32, scope: Scope) -> Location {
    location_at(Some(path), Some(line), scope)
}

pub fn frame(directive: Directive, holds_code: bool) -> GuardFrame {
    GuardFrame {
        directive,
        text: Text::from("#ifdef _WIN32"),
        identifiers: vec![Text::from("_WIN32")].into(),
        holds_code,
    }
}

pub fn guards() -> Vec<GuardChain> {
    let (openers, rest): (Vec<Directive>, Vec<Directive>) =
        Directive::ALL.into_iter().partition(|d| d.opens_chain());
    let mut out = Vec::new();
    for (i, opener) in openers.into_iter().enumerate() {
        let mut frames = vec![frame(opener, false)];
        if i == 0 {
            frames.extend(rest.iter().map(|&d| frame(d, false)));
        }
        frames.last_mut().unwrap().holds_code = true;
        out.push(GuardChain {
            frames,
            has_alternative: i == 0,
            region: RegionId(i as u32),
        });
    }
    out
}

pub fn function_scope() -> Scope {
    Scope::Function {
        qualified_name: Text::from("ns::serve"),
        signature: Text::from("int (int)"),
    }
}

pub fn sample_fact(kind: FactKind) -> Fact {
    sample_fact_with(kind, true)
}

pub fn sample_fact_with(kind: FactKind, filled: bool) -> Fact {
    let some = |t: &str| filled.then(|| Text::from(t));
    let src = |line| location("src/main.c", line, function_scope());
    let script = Scope::Script(ScriptName::PostInstall);
    let package = |scope: Scope| Location {
        frames: vec![Frame {
            kind: FrameKind::Rpm,
            name: Text::from("app"),
        }],
        ..location_at(None, None, scope)
    };
    match kind {
        FactKind::Call => Fact::Call {
            file_kind: FileKind::Source,
            location: src(10),
            callee: Text::from("setuid"),
            args: vec![Text::from("0")],
            guards: guards(),
        },
        FactKind::Include => Fact::Include {
            file_kind: FileKind::Source,
            location: src(1),
            target: Text::from("windows.h"),
            system: true,
            guards: guards(),
        },
        FactKind::StringLiteral => Fact::StringLiteral {
            file_kind: FileKind::Source,
            location: src(12),
            value: Text::from("/var/run/app.pid"),
            guards: guards(),
        },
        FactKind::ConfigKey => Fact::ConfigKey {
            file_kind: FileKind::Config,
            location: location(
                "etc/app.ini",
                3,
                Scope::ConfigKey(Text::from("server.port")),
            ),
            key: Text::from("server.port"),
            value: some("80"),
        },
        FactKind::GuardRegion => Fact::GuardRegion {
            file_kind: FileKind::Source,
            location: src(9),
            region: guards().last().unwrap().region,
            guards: guards(),
            end_line: 14,
        },
        FactKind::Import => Fact::Import {
            file_kind: FileKind::Binary,
            location: location_at(
                Some("usr/bin/app"),
                None,
                Scope::Symbol(Text::from("setuid")),
            ),
            symbol: Text::from("setuid"),
            version: some("GLIBC_2.2.5"),
        },
        FactKind::Export => Fact::Export {
            file_kind: FileKind::Binary,
            location: location_at(
                Some("usr/lib/libapp.so"),
                None,
                Scope::Symbol(Text::from("app_init")),
            ),
            symbol: Text::from("app_init"),
            version: some("APP_1.0"),
        },
        FactKind::NeededLibrary => Fact::NeededLibrary {
            file_kind: FileKind::Binary,
            location: location_at(Some("usr/bin/app"), None, Scope::File),
            name: Text::from("libssl.so.3"),
        },
        FactKind::PackageFile => Fact::PackageFile {
            file_kind: FileKind::Package,
            location: package(Scope::File),
            path: PathText::from("/opt/app/bin/app"),
            mode: filled.then_some(0o104755),
            user: some("root"),
            group: some("wheel"),
        },
        FactKind::PackageDependency => Fact::PackageDependency {
            file_kind: FileKind::Package,
            location: package(Scope::File),
            name: Text::from("glibc"),
            constraint: some(">= 2.17"),
        },
        FactKind::PackageScript => Fact::PackageScript {
            file_kind: FileKind::Package,
            location: package(script.clone()),
            name: ScriptName::PostInstall,
            interpreter: some("/bin/sh"),
        },
        FactKind::ServiceSetting => Fact::ServiceSetting {
            file_kind: FileKind::Service,
            location: location(
                "usr/lib/systemd/system/app.service",
                7,
                Scope::UnitSection {
                    section: Text::from("Service"),
                    key: Text::from("Type"),
                },
            ),
            section: Text::from("Service"),
            key: Text::from("Type"),
            value: Text::from("forking"),
        },
        FactKind::ShellCommand => Fact::ShellCommand {
            file_kind: FileKind::Package,
            location: Location {
                line: Some(2),
                ..package(script)
            },
            assignments: vec![Assignment {
                name: Text::from("HOME"),
                append: false,
                value: Text::from("/root"),
            }],
            argv: vec![
                Text::from("useradd"),
                Text::from("-u"),
                Text::from("501"),
                Text::from("app"),
            ],
            redirections: redirect_ops()
                .into_iter()
                .map(|op| Redirection {
                    fd: filled.then_some(2),
                    op,
                    target: Text::from("/dev/null"),
                })
                .collect(),
        },
    }
}

pub fn all_facts() -> Vec<Fact> {
    FactKind::ALL.into_iter().map(sample_fact).collect()
}

pub fn not_checked(reason: NotCheckedReason, details: Details) -> FileOutcome {
    FileOutcome::NotChecked { reason, details }
}

pub fn all_reasons() -> Vec<NotCheckedReason> {
    let mut reasons = vec![
        NotCheckedReason::NoReader,
        NotCheckedReason::EncodingUndeclared,
        NotCheckedReason::InvalidForDeclaredEncoding,
        NotCheckedReason::Unparseable,
        NotCheckedReason::UnsupportedInterpreter,
        NotCheckedReason::NoDecoder,
        NotCheckedReason::LimitExceeded(LimitName::EntryFacts),
        NotCheckedReason::LimitExceeded(LimitName::Watchdog),
        NotCheckedReason::LimitExceeded(LimitName::WorkerMemoryCap),
        NotCheckedReason::Fault(FaultKind::Crash),
        NotCheckedReason::RepeatedFault,
        NotCheckedReason::SpecialFile(SpecialKind::Fifo),
        NotCheckedReason::Symlink(LinkTarget::Special(SpecialKind::Socket)),
        NotCheckedReason::Symlink(LinkTarget::Unresolved),
        NotCheckedReason::Unreadable(IoKind::PermissionDenied),
        NotCheckedReason::UnsafeEntryName,
        NotCheckedReason::NotInCompileDatabase,
    ];
    reasons.extend(
        [
            FaultKind::Crash,
            FaultKind::Panic,
            FaultKind::BadFrame,
            FaultKind::Exited,
        ]
        .map(NotCheckedReason::Fault),
    );
    for reason in &reasons {
        match reason {
            NotCheckedReason::NoReader
            | NotCheckedReason::EncodingUndeclared
            | NotCheckedReason::InvalidForDeclaredEncoding
            | NotCheckedReason::Unparseable
            | NotCheckedReason::UnsupportedInterpreter
            | NotCheckedReason::NoDecoder
            | NotCheckedReason::LimitExceeded(_)
            | NotCheckedReason::Fault(_)
            | NotCheckedReason::RepeatedFault
            | NotCheckedReason::SpecialFile(_)
            | NotCheckedReason::Symlink(_)
            | NotCheckedReason::Unreadable(_)
            | NotCheckedReason::UnsafeEntryName
            | NotCheckedReason::NotInCompileDatabase => {}
        }
    }
    reasons
}

pub fn all_outcomes() -> Vec<Outcome> {
    let mut files = vec![
        FileOutcome::Read,
        not_checked(
            NotCheckedReason::LimitExceeded(LimitName::FileSize),
            Details::Measured {
                value: 70_000_000,
                limit: 67_108_864,
            },
        ),
        not_checked(
            NotCheckedReason::InvalidForDeclaredEncoding,
            Details::Encoding {
                declared: Encoding::WindowsCodePage(1252),
            },
        ),
        not_checked(
            NotCheckedReason::UnsupportedInterpreter,
            Details::Interpreter {
                interpreter: Text::from_bytes(b"<lua>\xff".to_vec()),
            },
        ),
        not_checked(
            NotCheckedReason::UnsafeEntryName,
            Details::EntryName {
                name: Text::from("../../etc/passwd"),
            },
        ),
        FileOutcome::Aggregate {
            reason: NotCheckedReason::RepeatedFault,
            count: None,
            at_least: 3,
        },
        FileOutcome::Aggregate {
            reason: NotCheckedReason::NotInCompileDatabase,
            count: Some(50),
            at_least: 50,
        },
        FileOutcome::OutOfScope {
            cause: OutOfScopeCause::Pattern(PathPattern {
                base: Text::from("/home/dev/app"),
                pattern: Text::from("third_party/**"),
            }),
        },
        FileOutcome::OutOfScope {
            cause: OutOfScopeCause::OwnFile(OwnFileRole::ProjectFile),
        },
    ];
    files.extend(
        all_reasons()
            .into_iter()
            .map(|r| not_checked(r, Details::None)),
    );
    files
        .into_iter()
        .enumerate()
        .map(|(i, file)| Outcome {
            lines: matches!(file, FileOutcome::Read | FileOutcome::NotChecked { .. }).then_some(
                LineCount {
                    lines: i as u64 * 7,
                    complete: i % 4 != 1,
                },
            ),
            file,
            file_kind: if i % 2 == 0 {
                Some(FileKind::Source)
            } else {
                None
            },
            host_dependent: i % 3 == 0,
        })
        .collect()
}

pub fn worker_settings() -> WorkerSettings {
    WorkerSettings {
        limits: Limits::default(),
        encodings: vec![EncodingDecl {
            pattern: PathPattern {
                base: Text::from("/home/dev/app"),
                pattern: Text::from("legacy/**"),
            },
            encoding: Encoding::WindowsCodePage(1252),
        }],
    }
}

pub fn all_collector_msgs() -> Vec<CollectorMsg> {
    vec![
        CollectorMsg::Settings(worker_settings()),
        CollectorMsg::Job(Job {
            kind: JobKind::Route,
            input: input(),
            file: JobFile::InInput(RelPath::try_from("pkg/app-1.2.rpm").unwrap()),
            resume_after: Some(EntryPath::from([2, 1])),
        }),
        CollectorMsg::Job(Job {
            kind: JobKind::Route,
            input: input(),
            file: JobFile::WholeInput(RelPath::try_from("app-1.2.rpm").unwrap()),
            resume_after: None,
        }),
        CollectorMsg::Job(Job {
            kind: JobKind::CompileDatabase,
            input: input(),
            file: JobFile::Setting(Text::from("/build/compile_commands.json")),
            resume_after: None,
        }),
        CollectorMsg::Job(Job {
            kind: JobKind::Definitions,
            input: input(),
            file: JobFile::InInput(RelPath::try_from("src/config.h").unwrap()),
            resume_after: None,
        }),
        CollectorMsg::Macros {
            tree: 0,
            definitions: macro_table(),
            end: true,
        },
        CollectorMsg::Macros {
            tree: 0,
            definitions: MacroTable::new(),
            end: false,
        },
    ]
}

pub fn tree_location(path: &str, line: u32) -> Location {
    Location {
        input: input(),
        frames: vec![],
        path: Some(RelPath::try_from(path).unwrap()),
        line: Some(line),
        scope: Scope::File,
    }
}

pub fn macro_def(name: &str, path: &str, line: u32, params: Option<MacroParams>) -> MacroDef {
    MacroDef {
        name: Text::from(name),
        location: tree_location(path, line),
        params,
        body: Text::from("do {   x; /* kept */ } while (0)"),
    }
}

pub fn macro_defs() -> Vec<MacroDef> {
    vec![
        macro_def("FOO", "src/config.h", 9, None),
        macro_def("FOO", "src/config.h", 3, None),
        macro_def(
            "LOG",
            "src/config.h",
            12,
            Some(MacroParams {
                names: vec![Text::from("level"), Text::from("fmt")],
                variadic: Some(Variadic::Ellipsis),
            }),
        ),
        macro_def(
            "TRACE",
            "src/config.h",
            14,
            Some(MacroParams {
                names: vec![],
                variadic: Some(Variadic::Named(Text::from("args"))),
            }),
        ),
        macro_def(
            "EMPTY",
            "src/config.h",
            15,
            Some(MacroParams {
                names: vec![],
                variadic: None,
            }),
        ),
        MacroDef {
            body: Text::from_bytes(vec![0xff, b'1']),
            ..macro_def("RAW", "src/config.h", 16, None)
        },
    ]
}

pub fn macro_table() -> MacroTable {
    let mut defs = macro_defs();
    defs.push(macro_def("FOO", "src/a.c", 2, None));
    MacroTable::from(defs)
}

pub fn collected(outcome: Outcome, regions: &[(u32, u32)]) -> CollectedFile {
    collected_at("src/main.c", outcome, regions)
}

pub fn collected_at(path: &str, outcome: Outcome, regions: &[(u32, u32)]) -> CollectedFile {
    CollectedFile {
        key: CanonicalKey::at(0, RelPath::try_from(path).unwrap(), EntryPath::job()),
        location: Location {
            line: None,
            ..tree_location(path, 1)
        },
        outcome,
        regions: regions
            .iter()
            .map(|&(first_line, last_line)| Region {
                first_line,
                last_line,
                reason: RegionReason::Unparseable,
            })
            .collect(),
        notes: vec![],
    }
}

pub fn opened(entry: EntryPath) -> WorkerMsg {
    WorkerMsg::Opened {
        entry,
        frames: frames(),
        path: Some(RelPath::try_from("usr/bin/app").unwrap()),
        scope: Scope::File,
        file_kind: Some(FileKind::Binary),
    }
}

pub fn all_worker_msgs() -> Vec<WorkerMsg> {
    let mut msgs = vec![
        opened(EntryPath::job()),
        WorkerMsg::Opened {
            entry: EntryPath::from([0]),
            frames: vec![Frame {
                kind: FrameKind::Rpm,
                name: Text::from("app"),
            }],
            path: None,
            scope: Scope::File,
            file_kind: Some(FileKind::Package),
        },
        WorkerMsg::Opened {
            entry: EntryPath::from([4]),
            frames: frames(),
            path: None,
            scope: Scope::Script(ScriptName::PostInstall),
            file_kind: Some(FileKind::Package),
        },
        WorkerMsg::Opened {
            entry: EntryPath::from([5]),
            frames: vec![],
            path: Some(RelPath::try_from("docs/README").unwrap()),
            scope: Scope::File,
            file_kind: None,
        },
        WorkerMsg::Facts {
            entry: EntryPath::from([0]),
            facts: all_facts(),
        },
        WorkerMsg::Facts {
            entry: EntryPath::job(),
            facts: vec![Fact::StringLiteral {
                file_kind: FileKind::Binary,
                location: location_at(Some("usr/bin/app"), None, Scope::File),
                value: Text::from_bytes(vec![0xff, 0x00, b'x']),
                guards: vec![],
            }],
        },
        WorkerMsg::Regions {
            entry: EntryPath::from([2, 0]),
            regions: vec![Region {
                first_line: 40,
                last_line: 52,
                reason: RegionReason::Unparseable,
            }],
        },
        WorkerMsg::Notes {
            entry: EntryPath::from([3]),
            notes: vec![Note::MissingSymbols],
        },
        WorkerMsg::Notes {
            entry: EntryPath::job(),
            notes: vec![Note::DroppedDefinitions(2)],
        },
        WorkerMsg::CompileUnits {
            entries: vec![CompileUnit {
                directory: Text::from("/build"),
                file: Text::from("../src/main.c"),
            }],
        },
        WorkerMsg::Definitions {
            definitions: macro_defs(),
        },
        WorkerMsg::JobDone { outcome: None },
    ];
    for (i, outcome) in all_outcomes().into_iter().enumerate() {
        msgs.push(WorkerMsg::EntryDone {
            entry: EntryPath::from([1, i as u32]),
            outcome: outcome.clone(),
            bytes: i as u64 * 1000,
        });
        msgs.push(WorkerMsg::JobDone {
            outcome: Some(outcome),
        });
    }
    msgs
}

pub fn collector_variant(msg: &CollectorMsg) -> &'static str {
    match msg {
        CollectorMsg::Settings(_) => "settings",
        CollectorMsg::Macros { .. } => "macros",
        CollectorMsg::Job(_) => "job",
    }
}

pub fn worker_variant(msg: &WorkerMsg) -> &'static str {
    match msg {
        WorkerMsg::Opened { .. } => "opened",
        WorkerMsg::Facts { .. } => "facts",
        WorkerMsg::Regions { .. } => "regions",
        WorkerMsg::Notes { .. } => "notes",
        WorkerMsg::EntryDone { .. } => "entry_done",
        WorkerMsg::CompileUnits { .. } => "compile_units",
        WorkerMsg::Definitions { .. } => "definitions",
        WorkerMsg::JobDone { .. } => "job_done",
    }
}
