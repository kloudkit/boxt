#![allow(dead_code)]

use boxt_core::Text;
use boxt_core::compile_db::CompileUnit;
use boxt_core::fact::{Branch, Directive, Fact, FactKind, FileKind, GuardFrame, RegionId};
use boxt_core::limits::{LimitName, Limits};
use boxt_core::location::{Frame, FrameKind, InputRef, Location, RelPath, Scope};
use boxt_core::message::{
    CollectorMsg, EntryPath, Job, JobKind, Note, Region, RegionReason, WorkerMsg,
};
use boxt_core::outcome::{
    Details, FaultKind, FileOutcome, LinkTarget, NotCheckedReason, OutOfScopeCause, Outcome,
    OwnFileRole, SpecialKind,
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

pub fn directives() -> Vec<Directive> {
    Directive::ALL.to_vec()
}

pub fn branches() -> Vec<Branch> {
    Branch::ALL.to_vec()
}

pub fn redirect_ops() -> Vec<RedirectOp> {
    RedirectOp::ALL.to_vec()
}

pub fn scope_variant(scope: &Scope) -> &'static str {
    match scope {
        Scope::File => "file",
        Scope::Function { .. } => "function",
        Scope::Symbol(_) => "symbol",
        Scope::UnitSection(_) => "unit_section",
        Scope::ConfigKey(_) => "config_key",
        Scope::Script(_) => "script",
    }
}

pub fn scopes() -> Vec<Scope> {
    vec![
        Scope::File,
        function_scope(),
        Scope::Symbol(Text::from("setuid")),
        Scope::UnitSection(Text::from("Service")),
        Scope::ConfigKey(Text::from("server.port")),
        Scope::Script(Text::from("%post")),
    ]
}

pub fn location_with(path: &str, line: Option<u32>, scope: Scope) -> Location {
    Location {
        input: input(),
        frames: frame_kinds()
            .into_iter()
            .map(|kind| Frame {
                kind,
                name: Text::from("app"),
            })
            .collect(),
        path: RelPath::try_from(path).unwrap(),
        line,
        scope,
    }
}

pub fn location(path: &str, line: u32, scope: Scope) -> Location {
    location_with(path, Some(line), scope)
}

pub fn guard() -> GuardFrame {
    GuardFrame {
        directive: Directive::Ifdef,
        branch: Branch::Else,
        text: Text::from("#ifdef _WIN32"),
        identifiers: vec![Text::from("_WIN32")].into(),
        has_alternative: true,
    }
}

pub fn guards() -> Vec<GuardFrame> {
    let mut out = Vec::new();
    for directive in directives() {
        for branch in branches() {
            out.push(GuardFrame {
                directive,
                branch,
                ..guard()
            });
        }
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
    let at =
        |path: &str, line: u32, scope: Scope| location_with(path, filled.then_some(line), scope);
    let src = |line| at("src/main.c", line, function_scope());
    let region = |i| filled.then_some(RegionId(i));
    match kind {
        FactKind::Call => Fact::Call {
            file_kind: FileKind::Source,
            location: src(10),
            callee: Text::from("setuid"),
            args: vec![Text::from("0")],
            region: region(1),
            guards: guards(),
        },
        FactKind::Include => Fact::Include {
            file_kind: FileKind::Source,
            location: src(1),
            target: Text::from("windows.h"),
            system: true,
            region: region(0),
            guards: guards(),
        },
        FactKind::StringLiteral => Fact::StringLiteral {
            file_kind: FileKind::Source,
            location: src(12),
            value: Text::from("/var/run/app.pid"),
            region: region(1),
            guards: guards(),
        },
        FactKind::ConfigKey => Fact::ConfigKey {
            file_kind: FileKind::Config,
            location: at(
                "etc/app.ini",
                3,
                Scope::ConfigKey(Text::from("server.port")),
            ),
            key: Text::from("server.port"),
            value: some("80"),
            region: region(0),
            guards: guards(),
        },
        FactKind::GuardRegion => Fact::GuardRegion {
            file_kind: FileKind::Source,
            location: src(9),
            region: RegionId(1),
            guards: guards(),
            end_line: 14,
        },
        FactKind::Import => Fact::Import {
            file_kind: FileKind::Binary,
            location: at("usr/bin/app", 0, Scope::Symbol(Text::from("setuid"))),
            symbol: Text::from("setuid"),
        },
        FactKind::Export => Fact::Export {
            file_kind: FileKind::Binary,
            location: at(
                "usr/lib/libapp.so",
                0,
                Scope::Symbol(Text::from("app_init")),
            ),
            symbol: Text::from("app_init"),
        },
        FactKind::NeededLibrary => Fact::NeededLibrary {
            file_kind: FileKind::Binary,
            location: at("usr/bin/app", 0, Scope::File),
            name: Text::from("libssl.so.3"),
        },
        FactKind::PackageFile => Fact::PackageFile {
            file_kind: FileKind::Package,
            location: at("app.rpm", 0, Scope::File),
            path: Text::from("/opt/app/bin/app"),
            mode: filled.then_some(0o4755),
            user: some("root"),
            group: some("root"),
        },
        FactKind::PackageDependency => Fact::PackageDependency {
            file_kind: FileKind::Package,
            location: at("app.rpm", 0, Scope::File),
            name: Text::from("glibc"),
            constraint: some(">= 2.17"),
        },
        FactKind::PackageScript => Fact::PackageScript {
            file_kind: FileKind::Package,
            location: at("app.rpm", 0, Scope::Script(Text::from("%post"))),
            name: Text::from("%post"),
            interpreter: some("/bin/sh"),
        },
        FactKind::ServiceSetting => Fact::ServiceSetting {
            file_kind: FileKind::Service,
            location: at(
                "usr/lib/systemd/system/app.service",
                7,
                Scope::UnitSection(Text::from("Service")),
            ),
            section: Text::from("Service"),
            key: Text::from("Type"),
            value: Text::from("forking"),
        },
        FactKind::ShellCommand => Fact::ShellCommand {
            file_kind: FileKind::Package,
            location: at("app.rpm", 2, Scope::Script(Text::from("%post"))),
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
    let reasons = vec![
        NotCheckedReason::NoReader,
        NotCheckedReason::EncodingUndeclared,
        NotCheckedReason::InvalidForDeclaredEncoding,
        NotCheckedReason::Unparseable,
        NotCheckedReason::UnsupportedInterpreter,
        NotCheckedReason::NoDecoder,
        NotCheckedReason::LimitExceeded(LimitName::EntryFacts),
        NotCheckedReason::LimitExceeded(LimitName::Watchdog),
        NotCheckedReason::LimitExceeded(LimitName::Memory),
        NotCheckedReason::Fault(FaultKind::StackOverflow),
        NotCheckedReason::RepeatedFault,
        NotCheckedReason::SpecialFile(SpecialKind::Fifo),
        NotCheckedReason::Symlink(LinkTarget::Special(SpecialKind::Socket)),
        NotCheckedReason::NotInCompileDatabase,
    ];
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
            path: RelPath::try_from("pkg/app-1.2.rpm").unwrap(),
            resume_after: Some(EntryPath::from([2, 1])),
        }),
        CollectorMsg::Job(Job {
            kind: JobKind::CompileDatabase,
            input: input(),
            path: RelPath::try_from("build/compile_commands.json").unwrap(),
            resume_after: None,
        }),
    ]
}

pub fn all_worker_msgs() -> Vec<WorkerMsg> {
    let mut msgs = vec![
        WorkerMsg::Facts {
            entry: EntryPath::from([0]),
            facts: all_facts(),
        },
        WorkerMsg::Facts {
            entry: EntryPath::job(),
            facts: vec![Fact::StringLiteral {
                file_kind: FileKind::Binary,
                location: location("usr/bin/app", 0, Scope::File),
                value: Text::from_bytes(vec![0xff, 0x00, b'x']),
                region: None,
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
        WorkerMsg::CompileUnits {
            entries: vec![CompileUnit {
                directory: Text::from("/build"),
                file: Text::from("../src/main.c"),
            }],
        },
        WorkerMsg::JobDone { outcome: None },
    ];
    for (i, outcome) in all_outcomes().into_iter().enumerate() {
        msgs.push(WorkerMsg::EntryDone {
            entry: EntryPath::from([1, i as u32]),
            outcome: outcome.clone(),
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
        CollectorMsg::Job(_) => "job",
    }
}

pub fn worker_variant(msg: &WorkerMsg) -> &'static str {
    match msg {
        WorkerMsg::Facts { .. } => "facts",
        WorkerMsg::Regions { .. } => "regions",
        WorkerMsg::Notes { .. } => "notes",
        WorkerMsg::EntryDone { .. } => "entry_done",
        WorkerMsg::CompileUnits { .. } => "compile_units",
        WorkerMsg::JobDone { .. } => "job_done",
    }
}
