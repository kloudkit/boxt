use boxt_render::boxt_core::fact::{Directive, GuardChain, GuardFrame, RegionId};
use boxt_render::boxt_core::identity::FindingId;
use boxt_render::boxt_core::input_scope::{self, InputScope};
use boxt_render::boxt_core::labels;
use boxt_render::boxt_core::limits::LimitName;
use boxt_render::boxt_core::location::{Frame, FrameKind, InputRef, Location, RelPath, Scope};
use boxt_render::boxt_core::message::{CanonicalKey, EntryPath, RegionReason};
use boxt_render::boxt_core::outcome::{
    Count, Details, FileOutcome, FileShare, LineShare, NotCheckedReason, Outcome, Sm3,
    VerdictCounts,
};
use boxt_render::boxt_core::verdict::{
    BehaviourId, Confidence, Evidence, EvidenceId, Finding, Group, GroupKey, LineRange,
    NotCheckedEntry, ReportedFinding, Result, RuleId, RuleSection, TargetId, TargetVerdict, Tier,
    TierRange,
};
use boxt_render::boxt_core::version::Versions;
use boxt_render::boxt_core::{SortedVec, Text};
use boxt_render::{TerminalOptions, terminal as render};
use unicode_width::UnicodeWidthStr;

fn terminal(result: &Result, styled: bool) -> String {
    render(
        result,
        &TerminalOptions {
            styled,
            width: 80,
            all: false,
        },
    )
}

fn at_width(result: &Result, width: usize, all: bool) -> String {
    render(
        result,
        &TerminalOptions {
            styled: false,
            width,
            all,
        },
    )
}

fn widest(report: &str) -> usize {
    report
        .lines()
        .map(UnicodeWidthStr::width)
        .max()
        .unwrap_or(0)
}

fn loc(path: &str, line: Option<u32>, scope: Scope) -> Location {
    Location {
        input: InputRef {
            index: 0,
            given: Text::from("src"),
        },
        frames: Vec::new(),
        path: Some(RelPath::try_from(path).unwrap()),
        line,
        scope,
    }
}

fn hostile_function() -> Scope {
    Scope::Function {
        qualified_name: Text::from("drop\u{202e}\u{200b}_to"),
        signature: Text::from("(const char *, uid_t)"),
    }
}

fn sample(host_dependent: bool) -> Result {
    let finding = Finding {
        location: loc("ev\u{202e}il\t\u{200b}.c", Some(7), hostile_function()),
        rule: RuleId::new("identity/setuid").unwrap(),
        behaviour: BehaviourId::new("setuid").unwrap(),
        confidence: Confidence::Found,
        evidence: Evidence {
            definition: EvidenceId::new("id-change-call").unwrap(),
        },
        guards: vec![GuardChain {
            frames: vec![GuardFrame {
                directive: Directive::Ifdef,
                text: Text::from("#ifdef _WIN32"),
                identifiers: [Text::from("_WIN32")].into_iter().collect(),
                holds_code: true,
            }],
            has_alternative: false,
            region: RegionId(0),
        }],
        fix: "Start as the user.".to_owned(),
    };
    let mut packaged = loc("bin/x", None, Scope::File);
    packaged.frames.push(Frame {
        kind: FrameKind::Archive,
        name: Text::from("app"),
    });
    Result {
        versions: Versions::CURRENT,
        input_scope: InputScope::SourceOnly,
        input_scope_text: input_scope::text(InputScope::SourceOnly, false),
        counts: VerdictCounts {
            read: 5,
            not_checked: Count::Exact(2),
        },
        sm3: Sm3 {
            lines: Some(LineShare {
                numerator: 3,
                denominator: 412,
                uncounted_files: 0,
                incomplete_files: 0,
            }),
            files: Some(FileShare {
                numerator: 2,
                denominator: 5,
            }),
            uncounted: SortedVec::new(),
            incomplete: SortedVec::new(),
        },
        macro_tables_cut: SortedVec::new(),
        host_dependent,
        targets: [
            TargetVerdict {
                target: TargetId::new("docker-default").unwrap(),
                range: TierRange {
                    found: Tier::NeedsWindows,
                    with_suspected: Tier::NeedsWindows,
                },
                found: 1,
                suspected: 0,
                allowed: 0,
                unmapped: SortedVec::new(),
            },
            TargetVerdict {
                target: TargetId::new("kubernetes-restricted").unwrap(),
                range: TierRange {
                    found: Tier::NeedsCodeChange,
                    with_suspected: Tier::Blocked,
                },
                found: 1,
                suspected: 0,
                allowed: 0,
                unmapped: [BehaviourId::new("user/foo").unwrap()]
                    .into_iter()
                    .collect(),
            },
        ]
        .into_iter()
        .collect(),
        findings: [ReportedFinding {
            finding,
            id: FindingId::parse("9c1e00000000-1").unwrap(),
        }]
        .into_iter()
        .collect(),
        groups: vec![Group {
            key: GroupKey::Behaviour(BehaviourId::new("setuid").unwrap()),
            found: 1,
            suspected: 0,
            rules: vec![RuleSection {
                rule: RuleId::new("identity/setuid").unwrap(),
                fix: "Start as the user.".to_owned(),
                findings: vec![FindingId::parse("9c1e00000000-1").unwrap()],
            }],
        }],
        not_checked: [
            NotCheckedEntry::File {
                location: loc("broken.c", None, Scope::File),
                outcome: Outcome::new(
                    FileOutcome::NotChecked {
                        reason: NotCheckedReason::Unparseable,
                        details: Details::None,
                    },
                    None,
                ),
            },
            NotCheckedEntry::Regions {
                location: loc("partly.c", None, Scope::File),
                reason: RegionReason::Unparseable,
                ranges: vec![LineRange {
                    first: 10,
                    last: 20,
                }],
            },
            NotCheckedEntry::DynamicLoad {
                location: packaged,
                symbol: Text::from("dlopen"),
            },
        ]
        .into_iter()
        .collect(),
        no_reader: 0,
        observations: SortedVec::new(),
    }
}

fn position(report: &str, needle: &str) -> usize {
    report
        .find(needle)
        .unwrap_or_else(|| panic!("{needle:?} not in\n{report}"))
}

#[test]
fn sections_come_in_order() {
    let report = terminal(&sample(true), false);
    let order = [
        "│ Source only: results describe the code",
        "│ Some files were ended by a time or memory limit of this host;",
        "\n  docker-default  [needs Windows]  1 found\n",
        "\n  kubernetes-restricted  [needs code change … blocked]  1 found · 1 unmapped\n",
        "\n\n  5 files read · 2 not checked · 0.8% of source lines not checked\n\n",
        "\n Findings   ● found   ○ suspected\n\n",
        "\n ▌setuid  1 found",
        " identity/setuid\n   fix  Start as the user.\n\n",
        "   ● ev\\u{202e}il\\x09\\u{200b}.c:7   drop",
        "uid_t)\n     id-change-call · 9c1e00000000-1\n",
        "     guard  #ifdef _WIN32\n\n",
        "\nNot checked\n",
        "  kubernetes-restricted: unmapped behaviour user/foo",
        "  broken.c: unparseable",
        "  partly.c: unparseable region at lines 10–20",
        "  app!bin/x: loads code at run time through dlopen",
        "  Source lines not checked: 0.8% (3 of 412); files: 2 of 5",
    ];
    let mut last = 0;
    for needle in order {
        let at = position(&report, needle);
        assert!(at >= last, "{needle:?} out of order in\n{report}");
        last = at;
    }
    assert!(report.starts_with("┌──"), "{report}");
}

#[test]
fn the_header_box_comes_first_and_holds_the_warning() {
    let report = terminal(&sample(false), false);
    let lines = lines_of(&report);
    assert_eq!(lines[0], format!("┌{}┐", "─".repeat(78)));
    assert!(
        lines[1].starts_with("│ Source only: results describe the code, not the built program."),
        "{report}"
    );
    assert_eq!(lines[1].width(), 80);
    assert_eq!(lines[3], format!("└{}┘", "─".repeat(78)));
}

#[test]
fn the_host_note_shows_only_when_host_dependent() {
    let marker = "Some files were ended by a time or memory limit";
    assert!(terminal(&sample(true), false).contains(marker));
    assert!(!terminal(&sample(false), false).contains(marker));
}

#[test]
fn every_tier_prints_its_label() {
    for tier in Tier::WORST_TO_BEST {
        let mut result = sample(false);
        let mut targets = result.targets.into_vec();
        targets[0].range = TierRange {
            found: tier,
            with_suspected: tier,
        };
        result.targets = targets.into_iter().collect();
        let report = terminal(&result, false);
        assert!(
            report.contains(&format!(
                "\n  docker-default  [{}]  1 found\n",
                labels::tier(tier)
            )),
            "{report}"
        );
        let styled = terminal(&result, true);
        assert!(
            styled.contains(&format!(" {} \x1b[0m", labels::tier(tier).to_uppercase())),
            "{styled}"
        );
    }
}

#[test]
fn a_range_prints_both_tiers_in_one_badge() {
    let report = terminal(&sample(false), false);
    assert!(
        report.contains("\n  kubernetes-restricted  [needs code change … blocked]  "),
        "{report}"
    );
    let mut result = sample(false);
    let mut targets = result.targets.into_vec();
    targets[0].range = TierRange {
        found: Tier::Blocked,
        with_suspected: Tier::Blocked,
    };
    result.targets = targets.into_iter().collect();
    assert!(terminal(&result, false).contains("\n  docker-default  [blocked]  1 found\n"));
}

#[test]
fn hostile_names_are_escaped() {
    let report = terminal(&sample(false), false);
    assert!(
        report.contains(
            "   ● ev\\u{202e}il\\x09\\u{200b}.c:7   drop\\u{202e}\\u{200b}_to(const char *, uid_t)\n"
        ),
        "{report}"
    );
    assert!(!report.contains('\u{202e}'));
    assert!(!report.contains('\u{200b}'));
}

#[test]
fn plain_output_has_no_escape_byte() {
    let report = terminal(&sample(true), false);
    assert!(!report.bytes().any(|b| b == 0x1b));
    let styled = terminal(&sample(true), true);
    assert!(styled.bytes().any(|b| b == 0x1b));
}

#[test]
fn a_hostile_symbol_is_escaped_even_when_styled() {
    let mut result = sample(false);
    result.not_checked = [NotCheckedEntry::DynamicLoad {
        location: loc("a.so", None, Scope::File),
        symbol: Text::from("\x1b]8;;http://x\x07dlopen\x1b]8;;\x07\x1b[31m"),
    }]
    .into_iter()
    .collect();
    let plain = terminal(&result, false);
    assert!(
        plain.contains("\\x1b]8;;http://x\\x07dlopen\\x1b]8;;\\x07\\x1b[31m"),
        "{plain}"
    );
    assert!(!plain.bytes().any(|b| b == 0x1b || b == 0x07));
    let styled = terminal(&result, true);
    assert!(!styled.contains("\x1b]8"));
    assert!(styled.contains("\\x1b]8;;http://x\\x07dlopen\\x1b]8;;\\x07\\x1b[31m"));
    assert!(!styled.contains("\x07"));
}

#[test]
fn a_lower_bound_share_says_at_least() {
    let mut result = sample(false);
    result.sm3.lines = Some(LineShare {
        numerator: 0,
        denominator: 0,
        uncounted_files: 2,
        incomplete_files: 0,
    });
    result.sm3.files = None;
    let report = terminal(&result, false);
    assert!(
        report.contains("Source lines not checked: at least 0 of 0, 2 files uncounted; files: n/a"),
        "{report}"
    );
    assert!(
        report.contains(
            "\n  5 files read · 2 not checked · at least 0 of 0 source lines not checked\n"
        ),
        "{report}"
    );
}

#[test]
fn a_missing_line_share_says_n_a() {
    let mut result = sample(false);
    result.sm3.lines = None;
    result.sm3.files = None;
    let report = terminal(&result, false);
    assert!(
        report.contains("\n  5 files read · 2 not checked · source lines not checked: n/a\n"),
        "{report}"
    );
    assert!(
        report.contains("  Source lines not checked: n/a; files: n/a\n"),
        "{report}"
    );
}

fn not_checked(location: Location, file: FileOutcome, host_dependent: bool) -> NotCheckedEntry {
    let mut outcome = Outcome::new(file, None);
    outcome.host_dependent = host_dependent;
    NotCheckedEntry::File { location, outcome }
}

fn lines_of(report: &str) -> Vec<&str> {
    report.lines().collect()
}

#[test]
fn details_aggregates_and_host_marks_print_their_lines() {
    let mut result = sample(false);
    result.not_checked = [
        not_checked(
            loc("big.c", None, Scope::File),
            FileOutcome::NotChecked {
                reason: NotCheckedReason::LimitExceeded(LimitName::FileSize),
                details: Details::Measured {
                    value: 5000,
                    limit: 4096,
                },
            },
            false,
        ),
        not_checked(
            loc("slow.c", None, Scope::File),
            FileOutcome::NotChecked {
                reason: NotCheckedReason::LimitExceeded(LimitName::Watchdog),
                details: Details::None,
            },
            true,
        ),
        not_checked(
            loc("rest.c", None, Scope::File),
            FileOutcome::Aggregate {
                reason: NotCheckedReason::LimitExceeded(LimitName::TotalEntries),
                count: None,
                at_least: 3,
            },
            false,
        ),
        not_checked(
            loc("pkg.c", None, Scope::File),
            FileOutcome::NotChecked {
                reason: NotCheckedReason::UnsafeEntryName,
                details: Details::EntryName {
                    name: Text::from("../\u{202e}x\x01"),
                },
            },
            false,
        ),
    ]
    .into_iter()
    .collect();
    let report = terminal(&result, false);
    let lines = lines_of(&report);
    for line in [
        "  big.c: over the file-size limit (5000 > 4096)",
        "  slow.c: over the watchdog limit; may differ on another host",
        "  rest.c: over the total-entries limit, at least 3 files from here on",
        "  pkg.c: unsafe entry name (../\\u{202e}x\\x01)",
    ] {
        assert!(lines.contains(&line), "{line:?} not in\n{report}");
    }
}

#[test]
fn empty_sections_print_none() {
    let mut result = sample(false);
    result.findings = SortedVec::new();
    result.groups = Vec::new();
    result.not_checked = SortedVec::new();
    let mut targets = result.targets.into_vec();
    for t in &mut targets {
        t.unmapped = SortedVec::new();
    }
    result.targets = targets.into_iter().collect();
    let report = terminal(&result, false);
    assert!(
        report.contains("\n Findings   none\n\nNot checked\n"),
        "{report}"
    );
    assert!(
        report.contains("\nNot checked\n  none\n  Source lines not checked: "),
        "{report}"
    );
}

#[test]
fn a_share_just_below_whole_never_prints_100_percent() {
    let mut result = sample(false);
    result.sm3.lines = Some(LineShare {
        numerator: 9999,
        denominator: 10000,
        uncounted_files: 0,
        incomplete_files: 0,
    });
    let report = terminal(&result, false);
    assert!(
        report.contains("not checked: 99.9% (9999 of 10000)"),
        "{report}"
    );
    result.sm3.lines = Some(LineShare {
        numerator: 5,
        denominator: 5,
        uncounted_files: 0,
        incomplete_files: 0,
    });
    assert!(terminal(&result, false).contains("not checked: 100.0% (5 of 5)"));
}

#[test]
fn two_inputs_prefix_each_location_with_the_input() {
    let mut result = sample(false);
    let mut other = loc("broken.c", None, Scope::File);
    other.input = InputRef {
        index: 1,
        given: Text::from("other\u{202e}"),
    };
    let mut entries = result.not_checked.into_vec();
    entries.push(not_checked(
        other,
        FileOutcome::NotChecked {
            reason: NotCheckedReason::Unparseable,
            details: Details::None,
        },
        false,
    ));
    result.not_checked = entries.into_iter().collect();
    let report = terminal(&result, false);
    let lines = lines_of(&report);
    assert!(lines.contains(&"  src: broken.c: unparseable"), "{report}");
    assert!(
        lines.contains(&"  other\\u{202e}: broken.c: unparseable"),
        "{report}"
    );
    assert!(!terminal(&sample(false), false).contains("src: "));
}

#[test]
fn a_frame_without_a_path_has_no_trailing_bang() {
    let mut result = sample(false);
    let mut frame_only = loc("x", None, Scope::File);
    frame_only.path = None;
    frame_only.frames.push(Frame {
        kind: FrameKind::Rpm,
        name: Text::from("app"),
    });
    result.not_checked = [NotCheckedEntry::DynamicLoad {
        location: frame_only,
        symbol: Text::from("dlopen"),
    }]
    .into_iter()
    .collect();
    let report = terminal(&result, false);
    assert!(
        lines_of(&report).contains(&"  app: loads code at run time through dlopen"),
        "{report}"
    );
}

#[test]
fn cut_macro_tables_and_uncounted_files_are_listed() {
    let mut result = sample(false);
    result.macro_tables_cut = [0].into_iter().collect();
    result.sm3.uncounted = [CanonicalKey::at(
        0,
        RelPath::try_from("huge\u{200b}.c").unwrap(),
        EntryPath::job(),
    )]
    .into_iter()
    .collect();
    result.sm3.incomplete = [CanonicalKey::at(
        0,
        RelPath::try_from("cut.c").unwrap(),
        EntryPath::job(),
    )]
    .into_iter()
    .collect();
    let report = terminal(&result, false);
    let lines = lines_of(&report);
    for line in [
        "  src: macro definitions past the macro-table-bytes limit were not expanded",
        "    uncounted: huge\\u{200b}.c",
        "    counted in part: cut.c",
    ] {
        assert!(lines.contains(&line), "{line:?} not in\n{report}");
    }
    let sm3 = report.find("Source lines not checked").unwrap();
    assert!(report.find("    uncounted: ").unwrap() > sm3);
}

struct Seeded {
    path: &'static str,
    line: u32,
    function: &'static str,
    signature: &'static str,
    rule: &'static str,
    confidence: Confidence,
    evidence: &'static str,
    guarded: bool,
    id: &'static str,
}

const NEEDS_ROOT_FIX: &str =
    "Run as a non-root user; drop the switch to user or group 0 and the calls that need root.";
const SETUID_FIX: &str =
    "Start as the user and groups the program needs, and drop the id-changing calls.";
const LOW_PORT_FIX: &str =
    "Listen on a port at or above 1024 and map it to the low port in the container runtime.";
const REGISTRY_FIX: &str =
    "Read the setting from a configuration file or the environment instead of the registry.";

fn seeded(s: &Seeded) -> ReportedFinding {
    let rule = RuleId::new(s.rule).unwrap();
    let behaviour = s.rule.split('/').nth(1).unwrap();
    let fix = match behaviour {
        "needs-root" => NEEDS_ROOT_FIX,
        "setuid" => SETUID_FIX,
        "low-port" => LOW_PORT_FIX,
        _ => REGISTRY_FIX,
    };
    let guards = if s.guarded {
        vec![GuardChain {
            frames: vec![GuardFrame {
                directive: Directive::Ifdef,
                text: Text::from("#ifdef _WIN32"),
                identifiers: [Text::from("_WIN32")].into_iter().collect(),
                holds_code: true,
            }],
            has_alternative: false,
            region: RegionId(0),
        }]
    } else {
        Vec::new()
    };
    ReportedFinding {
        finding: Finding {
            location: loc(
                s.path,
                Some(s.line),
                Scope::Function {
                    qualified_name: Text::from(s.function),
                    signature: Text::from(s.signature),
                },
            ),
            rule,
            behaviour: BehaviourId::new(behaviour).unwrap(),
            confidence: s.confidence,
            evidence: Evidence {
                definition: EvidenceId::new(s.evidence).unwrap(),
            },
            guards,
            fix: fix.to_owned(),
        },
        id: FindingId::parse(s.id).unwrap(),
    }
}

fn group(behaviour: &str, found: u64, suspected: u64, sections: &[(&str, &[&str])]) -> Group {
    Group {
        key: GroupKey::Behaviour(BehaviourId::new(behaviour).unwrap()),
        found,
        suspected,
        rules: sections
            .iter()
            .map(|(rule, ids)| RuleSection {
                rule: RuleId::new(*rule).unwrap(),
                fix: match behaviour {
                    "needs-root" => NEEDS_ROOT_FIX,
                    "setuid" => SETUID_FIX,
                    "low-port" => LOW_PORT_FIX,
                    _ => REGISTRY_FIX,
                }
                .to_owned(),
                findings: ids.iter().map(|id| FindingId::parse(id).unwrap()).collect(),
            })
            .collect(),
    }
}

fn seed_shaped() -> Result {
    let found = Confidence::Found;
    let rows = [
        Seeded {
            path: "low-port.c",
            line: 11,
            function: "listen_http",
            signature: "(void)",
            rule: "network/low-port",
            confidence: found,
            evidence: "htons-low-literal",
            guarded: false,
            id: "8cd6bfbb1cd6-1",
        },
        Seeded {
            path: "needs-root.c",
            line: 6,
            function: "become_root",
            signature: "(void)",
            rule: "identity/needs-root",
            confidence: found,
            evidence: "switch-to-root",
            guarded: false,
            id: "22f82d5b8473-1",
        },
        Seeded {
            path: "needs-root.c",
            line: 6,
            function: "become_root",
            signature: "(void)",
            rule: "identity/setuid",
            confidence: found,
            evidence: "id-change-call",
            guarded: false,
            id: "600ca31d27d3-1",
        },
        Seeded {
            path: "needs-root.c",
            line: 8,
            function: "become_root",
            signature: "(void)",
            rule: "identity/needs-root",
            confidence: Confidence::Suspected,
            evidence: "root-only-call",
            guarded: false,
            id: "22f82d5b8473-2",
        },
        Seeded {
            path: "needs-root.c",
            line: 13,
            function: "become_root_fully",
            signature: "(void)",
            rule: "identity/needs-root",
            confidence: found,
            evidence: "switch-to-root",
            guarded: false,
            id: "67658c4225d3-1",
        },
        Seeded {
            path: "registry.c",
            line: 8,
            function: "read_setting",
            signature: "(DWORD *)",
            rule: "os-coupling/registry",
            confidence: found,
            evidence: "registry-call",
            guarded: true,
            id: "a932e98472ec-1",
        },
        Seeded {
            path: "registry.c",
            line: 10,
            function: "read_setting",
            signature: "(DWORD *)",
            rule: "os-coupling/registry",
            confidence: found,
            evidence: "registry-call",
            guarded: true,
            id: "a932e98472ec-2",
        },
        Seeded {
            path: "setuid.c",
            line: 7,
            function: "drop_to",
            signature: "(const char *, uid_t, gid_t)",
            rule: "identity/setuid",
            confidence: found,
            evidence: "id-change-call",
            guarded: false,
            id: "bdb745a285af-1",
        },
    ];
    let mut result = sample(false);
    result.findings = rows.iter().map(seeded).collect();
    result.groups = vec![
        group(
            "low-port",
            1,
            0,
            &[("network/low-port", &["8cd6bfbb1cd6-1"])],
        ),
        group(
            "needs-root",
            2,
            1,
            &[(
                "identity/needs-root",
                &["22f82d5b8473-1", "22f82d5b8473-2", "67658c4225d3-1"],
            )],
        ),
        group(
            "registry",
            2,
            0,
            &[(
                "os-coupling/registry",
                &["a932e98472ec-1", "a932e98472ec-2"],
            )],
        ),
        group(
            "setuid",
            2,
            0,
            &[("identity/setuid", &["600ca31d27d3-1", "bdb745a285af-1"])],
        ),
    ];
    result.targets = [TargetVerdict {
        target: TargetId::new("kubernetes-restricted").unwrap(),
        range: TierRange {
            found: Tier::Blocked,
            with_suspected: Tier::Blocked,
        },
        found: 7,
        suspected: 1,
        allowed: 0,
        unmapped: SortedVec::new(),
    }]
    .into_iter()
    .collect();
    result.counts = VerdictCounts {
        read: 5,
        not_checked: Count::Exact(0),
    };
    result.sm3 = Sm3 {
        lines: Some(LineShare {
            numerator: 0,
            denominator: 72,
            uncounted_files: 0,
            incomplete_files: 0,
        }),
        files: Some(FileShare {
            numerator: 0,
            denominator: 5,
        }),
        uncounted: SortedVec::new(),
        incomplete: SortedVec::new(),
    };
    result.not_checked = SortedVec::new();
    result
}

const SEED_GOLDEN: &str = r#"
┌──────────────────────────────────────────────────────────────────────────────┐
│ Source only: results describe the code, not the built program. Add binaries  │
│ or packages for a richer result.                                             │
└──────────────────────────────────────────────────────────────────────────────┘

  target                  verdict     found   suspected   allowed
  kubernetes-restricted   [blocked]       7           1         –

  5 files read · 0 not checked · 0.0% of source lines not checked

────────────────────────────────────────────────────────────────────────────────
 Findings   ● found   ○ suspected

 ▌low-port  1 found                                             network/low-port
   fix  Listen on a port at or above 1024 and map it to the low port in the
        container runtime.

   ● low-port.c:11   listen_http(void)
     htons-low-literal · 8cd6bfbb1cd6-1

 ▌needs-root  2 found · 1 suspected                          identity/needs-root
   fix  Run as a non-root user; drop the switch to user or group 0 and the calls
        that need root.

   ● needs-root.c:6   become_root(void)
     switch-to-root · 22f82d5b8473-1

   ○ needs-root.c:8   become_root(void)
     root-only-call · 22f82d5b8473-2

   ● needs-root.c:13   become_root_fully(void)
     switch-to-root · 67658c4225d3-1

 ▌registry  2 found                                         os-coupling/registry
   fix  Read the setting from a configuration file or the environment instead of
        the registry.

   ● registry.c:8   read_setting(DWORD *)
     registry-call · a932e98472ec-1
     guard  #ifdef _WIN32

   ● registry.c:10   read_setting(DWORD *)
     registry-call · a932e98472ec-2
     guard  #ifdef _WIN32

 ▌setuid  2 found                                                identity/setuid
   fix  Start as the user and groups the program needs, and drop the id-changing
        calls.

   ● needs-root.c:6   become_root(void)
     id-change-call · 600ca31d27d3-1

   ● setuid.c:7   drop_to(const char *, uid_t, gid_t)
     id-change-call · bdb745a285af-1

Not checked
  none
  Source lines not checked: 0.0% (0 of 72); files: 0 of 5
"#;

#[test]
fn the_seed_shaped_result_prints_the_golden_layout_at_80_columns() {
    let report = terminal(&seed_shaped(), false);
    assert_eq!(report, SEED_GOLDEN.strip_prefix('\n').unwrap(), "{report}");
    assert_eq!(widest(&report), 80);
    for fix in [NEEDS_ROOT_FIX, SETUID_FIX, LOW_PORT_FIX, REGISTRY_FIX] {
        let start = &fix[..20];
        assert_eq!(report.matches(start).count(), 1, "{start}");
    }
}

#[test]
fn two_rules_of_one_behaviour_print_each_fix_once() {
    let mut result = seed_shaped();
    let mut user = seeded(&Seeded {
        path: "setuid.c",
        line: 9,
        function: "drop_to",
        signature: "(const char *, uid_t, gid_t)",
        rule: "identity/setuid",
        confidence: Confidence::Suspected,
        evidence: "id-change-call",
        guarded: false,
        id: "bdb745a285af-2",
    });
    user.finding.rule = RuleId::new("user/setuid-x").unwrap();
    "Ask the site team.".clone_into(&mut user.finding.fix);
    result.findings.insert(user);
    let mut setuid = group(
        "setuid",
        2,
        1,
        &[("identity/setuid", &["600ca31d27d3-1", "bdb745a285af-1"])],
    );
    setuid.rules.push(RuleSection {
        rule: RuleId::new("user/setuid-x").unwrap(),
        fix: "Ask the site team.".to_owned(),
        findings: vec![FindingId::parse("bdb745a285af-2").unwrap()],
    });
    result.groups[3] = setuid;
    let report = terminal(&result, false);
    let tail = &report[position(&report, "\n ▌setuid  2 found · 1 suspected\n")..];
    let expected = "
 ▌setuid  2 found · 1 suspected
   identity/setuid
     fix  Start as the user and groups the program needs, and drop the
          id-changing calls.

     ● needs-root.c:6   become_root(void)
       id-change-call · 600ca31d27d3-1

     ● setuid.c:7   drop_to(const char *, uid_t, gid_t)
       id-change-call · bdb745a285af-1

   user/setuid-x
     fix  Ask the site team.

     ○ setuid.c:9   drop_to(const char *, uid_t, gid_t)
       id-change-call · bdb745a285af-2

Not checked
";
    assert!(tail.starts_with(expected), "{report}");
    assert_eq!(report.matches("Ask the site team.").count(), 1);
    assert_eq!(report.matches("Start as the user and groups").count(), 1);
}

#[test]
fn a_long_signature_wraps_at_top_level_commas_under_its_parenthesis() {
    let report = terminal(&with_signature("subprocess", SUBPROCESS), false);
    assert!(widest(&report) <= 80, "{report}");
    let expected = "
   ● misc.c:7   subprocess(const char *, const char *, int, char **, FILE **,
                           u_int, struct passwd *, privdrop_fn *,
                           privrestore_fn *)
     id-change-call · 9c1e00000000-1
     guard  #ifdef _WIN32
";
    assert!(report.contains(expected), "{report}");
}

#[test]
fn a_long_guard_and_fix_wrap_under_their_label() {
    let mut result = sample(false);
    let mut findings = result.findings.into_vec();
    findings[0].finding.guards[0].frames[0].text = Text::from(
        "#if defined(HAVE_SETRESUID) && !defined(BROKEN_SETRESUID) && defined(HAVE_SETRESGID) && !defined(BROKEN_SETRESGID)",
    );
    result.findings = findings.into_iter().collect();
    result.groups[0].rules[0].fix = "word ".repeat(30);
    let report = terminal(&result, false);
    assert!(widest(&report) <= 80, "{report}");
    let lines = lines_of(&report);
    let guard = lines
        .iter()
        .position(|l| l.starts_with("     guard  #if defined(HAVE_SETRESUID)"))
        .unwrap();
    assert!(
        lines[guard + 1].starts_with("            defined("),
        "{report}"
    );
    let fix = lines
        .iter()
        .position(|l| l.starts_with("   fix  word"))
        .unwrap();
    assert!(lines[fix + 1].starts_with("        word"), "{report}");
}

#[test]
fn wide_characters_are_measured_by_display_width() {
    let mut result = sample(false);
    let wide = "源代码/很长的目录名称/另一个很长的目录名称/文件名称很长的源代码文件e\u{301}.c";
    let mut findings = result.findings.into_vec();
    findings[0].finding.location.path = Some(RelPath::try_from(wide).unwrap());
    result.findings = findings.into_iter().collect();
    result.not_checked = [not_checked(
        loc(wide, None, Scope::File),
        FileOutcome::NotChecked {
            reason: NotCheckedReason::Unparseable,
            details: Details::None,
        },
        false,
    )]
    .into_iter()
    .collect();
    for width in [60, 80] {
        let report = at_width(&result, width, false);
        for line in report.lines() {
            assert!(line.width() <= width, "{line:?} over {width} in\n{report}");
            if line.starts_with(['┌', '│', '└']) {
                assert_eq!(line.width(), width, "{line:?}");
            }
        }
        assert!(report.contains("文件e\u{301}.c"), "{report}");
    }
    let report = terminal(&result, false);
    assert!(
        report.contains(&format!("\n   ● {wide}:7\n     drop")),
        "{report}"
    );
    assert_eq!(format!("   ● {wide}:7   drop").width(), 84);
}

#[test]
fn hostile_text_is_escaped_before_it_is_measured_and_wrapped() {
    let mut result = sample(false);
    let hostile = format!("{}\x1b[31m\u{202e}.c", "x".repeat(70));
    let mut findings = result.findings.into_vec();
    findings[0].finding.location.path = Some(RelPath::try_from(hostile.as_str()).unwrap());
    result.findings = findings.into_iter().collect();
    for styled in [false, true] {
        let report = render(
            &result,
            &TerminalOptions {
                styled,
                width: 60,
                all: false,
            },
        );
        assert!(!report.contains('\u{202e}'));
        assert!(!report.contains("\x1b[31m"));
        let plain = String::from_utf8(strip_sgr(report.as_bytes())).unwrap();
        assert!(widest(&plain) <= 60, "{plain}");
        let joined: String = plain.lines().map(str::trim).collect();
        assert!(
            joined.contains(&format!("{}\\x1b[31m\\u{{202e}}.c:7", "x".repeat(70))),
            "{plain}"
        );
    }
}

fn no_reader_run(no_reader: usize) -> Result {
    let mut result = sample(false);
    let mut entries: Vec<NotCheckedEntry> = (0..no_reader)
        .map(|i| {
            not_checked(
                loc(&format!("doc/file-{i:04}.txt"), None, Scope::File),
                FileOutcome::NotChecked {
                    reason: NotCheckedReason::NoReader,
                    details: Details::None,
                },
                false,
            )
        })
        .collect();
    for path in ["a.c", "m.c", "z.c"] {
        entries.push(not_checked(
            loc(path, None, Scope::File),
            FileOutcome::NotChecked {
                reason: NotCheckedReason::Unparseable,
                details: Details::None,
            },
            false,
        ));
    }
    result.not_checked = entries.into_iter().collect();
    result.no_reader = no_reader as u64;
    result
}

#[test]
fn files_no_reader_understands_print_as_one_count() {
    let result = no_reader_run(2000);
    let report = terminal(&result, false);
    let lines = lines_of(&report);
    let at = lines.iter().position(|l| *l == "Not checked").unwrap();
    assert_eq!(
        &lines[at + 1..at + 5],
        [
            "  kubernetes-restricted: unmapped behaviour user/foo",
            "  no reader: 2000 files (--all to list)",
            "  a.c: unparseable",
            "  m.c: unparseable",
        ]
    );
    assert!(lines.contains(&"  z.c: unparseable"));
    assert!(!report.contains("file-0000.txt"));
    assert_eq!(report.matches(": no reader").count(), 0);
}

#[test]
fn all_lists_every_file_no_reader_understands() {
    let result = no_reader_run(2000);
    let report = at_width(&result, 80, true);
    assert!(!report.contains("(--all to list)"));
    assert_eq!(report.matches(": no reader\n").count(), 2000);
    assert_eq!(report.matches(": unparseable\n").count(), 3);
    assert!(report.contains("\n  doc/file-1999.txt: no reader\n"));
}

#[test]
fn with_no_count_the_entries_are_listed_as_before() {
    let mut result = no_reader_run(2);
    result.no_reader = 0;
    let report = terminal(&result, false);
    assert!(!report.contains("(--all to list)"));
    assert_eq!(report.matches(": no reader\n").count(), 2);
}

#[test]
fn the_width_is_clamped_to_60_and_200() {
    for (asked, laid_out) in [(40, 60), (0, 60), (60, 60), (123, 123), (500, 200)] {
        let report = at_width(&sample(true), asked, false);
        assert_eq!(report.lines().next().unwrap().width(), laid_out, "{asked}");
        assert!(widest(&report) <= laid_out, "{asked}\n{report}");
    }
}

#[test]
fn every_line_of_the_seed_fits_at_the_narrowest_width() {
    let report = at_width(&seed_shaped(), 60, false);
    for line in report.lines() {
        assert!(line.width() <= 60, "{line:?}");
    }
    assert!(
        report.contains("\n  kubernetes-restricted  [blocked]  7 found · 1 suspected\n"),
        "{report}"
    );
}

fn strip_sgr(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == 0x1b && bytes.get(i + 1) == Some(&b'[') {
            i += 2;
            while i < bytes.len() && !bytes[i].is_ascii_alphabetic() {
                i += 1;
            }
            i += 1;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    out
}

#[test]
fn a_style_never_spans_a_line_break() {
    let mut result = seed_shaped();
    let mut targets = result.targets.into_vec();
    targets[0].range = TierRange {
        found: Tier::NeedsCodeChange,
        with_suspected: Tier::Blocked,
    };
    result.targets = targets.into_iter().collect();
    for width in [60, 80] {
        let styled = render(
            &result,
            &TerminalOptions {
                styled: true,
                width,
                all: false,
            },
        );
        let plain = at_width(&result, width, false);
        assert!(styled.contains("\x1b[2m22f82d5b8473-1\x1b[0m"), "{styled}");
        assert!(
            styled.contains("\x1b[1m\x1b[35m▌needs-root\x1b[0m"),
            "{styled}"
        );
        assert!(styled.contains(" NEEDS CODE CHANGE \x1b[0m…"), "{styled}");
        let stripped = String::from_utf8(strip_sgr(styled.as_bytes())).unwrap();
        assert_eq!(stripped.lines().count(), plain.lines().count());
        for (s, p) in stripped.lines().zip(plain.lines()) {
            assert_eq!(s.width(), p.width(), "{s:?} / {p:?}");
        }
        for line in styled.lines() {
            let opens = line.matches("\x1b[").count();
            let resets = line.matches("\x1b[0m").count();
            if opens > 0 {
                assert!(
                    line.rfind("\x1b[").unwrap() == line.rfind("\x1b[0m").unwrap(),
                    "{line:?}"
                );
                assert!(resets > 0, "{line:?}");
            }
        }
    }
}

fn regions_at(ranges: &[(u32, u32)]) -> String {
    let mut result = sample(false);
    result.not_checked = [NotCheckedEntry::Regions {
        location: loc("archival/bbunzip.c", None, Scope::File),
        reason: RegionReason::Unparseable,
        ranges: ranges
            .iter()
            .map(|&(first, last)| LineRange { first, last })
            .collect(),
    }]
    .into_iter()
    .collect();
    terminal(&result, false)
}

#[test]
fn the_regions_of_a_file_print_on_one_line() {
    for (ranges, line) in [
        (
            &[(41, 41), (47, 47), (250, 251)][..],
            "  archival/bbunzip.c: unparseable regions at lines 41, 47, 250–251",
        ),
        (
            &[(41, 41)][..],
            "  archival/bbunzip.c: unparseable region at line 41",
        ),
        (
            &[(250, 251)][..],
            "  archival/bbunzip.c: unparseable region at lines 250–251",
        ),
    ] {
        let report = regions_at(ranges);
        assert!(
            lines_of(&report).contains(&line),
            "{line:?} not in\n{report}"
        );
    }
    let many: Vec<(u32, u32)> = (1..=60).map(|n| (n * 10, n * 10)).collect();
    let report = regions_at(&many);
    assert!(widest(&report) <= 80, "{report}");
    let joined: String = report.lines().map(str::trim).collect::<Vec<_>>().join(" ");
    let list: Vec<String> = many.iter().map(|(n, _)| n.to_string()).collect();
    let expected = format!(
        "archival/bbunzip.c: unparseable regions at lines {}",
        list.join(", ")
    );
    assert!(joined.contains(&expected), "{report}");
    assert!(report.contains("\n    "), "{report}");
}

#[test]
fn a_target_line_counts_found_suspected_and_allowed() {
    for ((found, suspected, allowed), text) in [
        (
            (3, 0, 10),
            "\n  docker-default  [needs Windows]  3 found · 10 allowed\n",
        ),
        (
            (12, 1, 26),
            "\n  docker-default  [needs Windows]  12 found · 1 suspected · 26 allowed\n",
        ),
        (
            (0, 2, 0),
            "\n  docker-default  [needs Windows]  2 suspected\n",
        ),
        (
            (0, 0, 0),
            "\n  docker-default  [needs Windows]  no findings\n",
        ),
    ] {
        let mut result = sample(false);
        let mut targets = result.targets.into_vec();
        targets[0].found = found;
        targets[0].suspected = suspected;
        targets[0].allowed = allowed;
        result.targets = targets.into_iter().collect();
        let report = terminal(&result, false);
        assert!(report.contains(text), "{text:?} not in\n{report}");
    }
}

#[test]
fn no_line_overflows_even_where_optimal_fit_would() {
    let url = format!("https://example.invalid/{}", "a".repeat(46));
    assert_eq!(url.len(), 70);
    let mut result = sample(false);
    result.groups[0].rules[0].fix = format!("See {url}");
    result.input_scope_text = format!("See {url}");
    for width in [60, 80] {
        let report = at_width(&result, width, false);
        assert!(widest(&report) <= width, "{report}");
        for line in report.lines().filter(|l| l.starts_with(['┌', '│', '└'])) {
            assert_eq!(line.width(), width, "{line:?}");
        }
        assert!(report.contains(&url[..40]), "{report}");
    }
    let report = terminal(&result, false);
    assert!(
        report.contains(&format!("   fix  See\n        {url}\n")),
        "{report}"
    );
    assert!(report.contains(&format!("│ See {url}   │\n")), "{report}");
}

#[test]
fn spaces_inside_a_value_are_kept_even_at_a_break() {
    let mut result = sample(false);
    let inner = format!("{} {}.c", "x".repeat(60), "y".repeat(20));
    result.not_checked = [" a.c", inner.as_str()]
        .into_iter()
        .map(|path| {
            not_checked(
                loc(path, None, Scope::File),
                FileOutcome::NotChecked {
                    reason: NotCheckedReason::Unparseable,
                    details: Details::None,
                },
                false,
            )
        })
        .collect();
    let mut findings = result.findings.into_vec();
    findings[0].finding.location.path = Some(RelPath::try_from(" a.c").unwrap());
    result.findings = findings.into_iter().collect();
    let report = terminal(&result, false);
    assert!(report.contains("\n   ●  a.c:7   drop"), "{report}");
    assert!(report.contains("\n   a.c: unparseable\n"), "{report}");
    assert!(
        report.contains(&format!(
            "\n  {} \n    {}.c: unparseable\n",
            "x".repeat(60),
            "y".repeat(20)
        )),
        "{report}"
    );
}

#[test]
fn one_file_no_reader_understands_is_singular() {
    let report = terminal(&no_reader_run(1), false);
    assert!(
        report.contains("\n  no reader: 1 file (--all to list)\n"),
        "{report}"
    );
}

#[test]
fn a_target_line_counts_unmapped_behaviours() {
    let report = terminal(&sample(false), false);
    assert!(
        report.contains(
            "\n  kubernetes-restricted  [needs code change … blocked]  1 found · 1 unmapped\n"
        ),
        "{report}"
    );
    let mut result = sample(false);
    let mut targets = result.targets.into_vec();
    targets[1].found = 0;
    result.targets = targets.into_iter().collect();
    let report = terminal(&result, false);
    assert!(
        report.contains("\n  kubernetes-restricted  [needs code change … blocked]  1 unmapped\n"),
        "{report}"
    );
    assert!(!report.contains("no findings"), "{report}");
}

#[test]
fn a_suspected_only_group_names_only_its_suspected_count() {
    let mut result = seed_shaped();
    let mut findings = result.findings.into_vec();
    findings[0].finding.confidence = Confidence::Suspected;
    result.findings = findings.into_iter().collect();
    result.groups[0].found = 0;
    result.groups[0].suspected = 1;
    let report = terminal(&result, false);
    assert!(report.contains("\n ▌low-port  1 suspected  "), "{report}");
    assert!(
        report.contains("\n   ○ low-port.c:11   listen_http(void)\n"),
        "{report}"
    );
}

fn verdict(target: &str, tier: Tier, counts: (u64, u64, u64), unmapped: &[&str]) -> TargetVerdict {
    TargetVerdict {
        target: TargetId::new(target).unwrap(),
        range: TierRange {
            found: tier,
            with_suspected: tier,
        },
        found: counts.0,
        suspected: counts.1,
        allowed: counts.2,
        unmapped: unmapped
            .iter()
            .map(|b| BehaviourId::new(*b).unwrap())
            .collect(),
    }
}

fn four_targets(unmapped: &[&str]) -> Result {
    let mut result = seed_shaped();
    result.targets = [
        verdict("docker-default", Tier::NeedsWindows, (3, 0, 10), &[]),
        verdict("kubernetes-restricted", Tier::Blocked, (7, 1, 0), unmapped),
        verdict("openshift-restricted-v2", Tier::Blocked, (7, 1, 0), &[]),
        verdict("rootless-podman", Tier::NeedsWindows, (7, 1, 0), &[]),
    ]
    .into_iter()
    .collect();
    result
}

fn styled_at(result: &Result, width: usize) -> String {
    render(
        result,
        &TerminalOptions {
            styled: true,
            width,
            all: false,
        },
    )
}

#[test]
fn targets_print_as_one_aligned_table() {
    let report = terminal(&four_targets(&[]), false);
    let expected = format!(
        "
  target                    verdict           found   suspected   allowed
  docker-default            [needs Windows]       3           –        10
  kubernetes-restricted     [blocked]             7           1         –
  openshift-restricted-v2   [blocked]             7           1         –
  rootless-podman           [needs Windows]       7           1         –

  5 files read · 0 not checked · 0.0% of source lines not checked

{}
 Findings   ● found   ○ suspected

",
        "─".repeat(80)
    );
    assert!(report.contains(&expected), "{report}");
    assert!(!report.contains("unmapped"), "{report}");
}

#[test]
fn the_unmapped_column_appears_only_when_a_target_has_some() {
    let report = at_width(&four_targets(&["user/foo", "user/bar"]), 90, false);
    let lines = lines_of(&report);
    let header = lines
        .iter()
        .position(|l| l.starts_with("  target "))
        .unwrap();
    assert!(lines[header].ends_with("   allowed   unmapped"), "{report}");
    assert!(lines[header + 1].ends_with("10          –"), "{report}");
    assert!(lines[header + 2].ends_with("–          2"), "{report}");
    assert!(widest(&report) <= 90, "{report}");
    let narrow = terminal(&four_targets(&["user/foo", "user/bar"]), false);
    assert!(
        narrow
            .contains("\n  kubernetes-restricted  [blocked]  7 found · 1 suspected · 2 unmapped\n"),
        "{narrow}"
    );
}

#[test]
fn table_rows_are_tinted_by_tier_without_banding() {
    let result = four_targets(&[]);
    let styled = styled_at(&result, 80);
    let lines = lines_of(&styled);
    let header = lines
        .iter()
        .position(|l| l.contains("target ") && l.contains("verdict"))
        .unwrap();
    assert!(
        lines[header].starts_with("  \x1b[2m\x1b[4mtarget "),
        "{:?}",
        lines[header]
    );
    for (row, (target, tint, badge)) in [
        ("docker-default", "\x1b[33m", " NEEDS WINDOWS "),
        ("kubernetes-restricted", "\x1b[31m", " BLOCKED "),
        ("openshift-restricted-v2", "\x1b[31m", " BLOCKED "),
        ("rootless-podman", "\x1b[33m", " NEEDS WINDOWS "),
    ]
    .iter()
    .enumerate()
    {
        let line = lines[header + 1 + row];
        assert!(line.starts_with(&format!("  {target} ")), "{line:?}");
        assert!(!line.contains("\x1b[2m"), "{line:?}");
        assert!(line.contains(&format!("{badge}\x1b[0m")), "{line:?}");
        assert!(
            line.contains(&format!("{tint}7\x1b[0m")) || row == 0,
            "{line:?}"
        );
        assert!(line.contains(&format!("{tint}–\x1b[0m")), "{line:?}");
    }
    assert!(lines[header + 1].contains("\x1b[33m10\x1b[0m"));
    let plain = terminal(&result, false);
    assert!(!plain.contains('\x1b'));
    assert!(!plain.contains("[2m"));
}

#[test]
fn below_72_columns_each_target_gets_one_line() {
    let result = four_targets(&[]);
    let narrow = at_width(&result, 71, false);
    assert!(!narrow.contains("  target "), "{narrow}");
    for line in [
        "  docker-default  [needs Windows]  3 found · 10 allowed",
        "  kubernetes-restricted  [blocked]  7 found · 1 suspected",
        "  openshift-restricted-v2  [blocked]  7 found · 1 suspected",
        "  rootless-podman  [needs Windows]  7 found · 1 suspected",
    ] {
        assert!(
            lines_of(&narrow).contains(&line),
            "{line:?} not in\n{narrow}"
        );
    }
    assert!(
        styled_at(&result, 71)
            .contains("  kubernetes-restricted  \x1b[1m\x1b[97m\x1b[41m BLOCKED \x1b[0m  7 found"),
    );
    assert!(at_width(&result, 72, false).contains("\n  docker-default  [needs"));
    assert!(at_width(&result, 73, false).contains("\n  target "));
}

#[test]
fn one_rule_sits_on_its_group_heading_and_two_rules_get_sub_lines() {
    let report = terminal(&seed_shaped(), false);
    let lines = lines_of(&report);
    let heading = lines
        .iter()
        .find(|l| l.starts_with(" ▌needs-root  "))
        .unwrap();
    assert!(
        heading.starts_with(" ▌needs-root  2 found · 1 suspected  "),
        "{heading:?}"
    );
    assert!(heading.ends_with("  identity/needs-root"), "{heading:?}");
    assert_eq!(heading.width(), 80);
    assert!(!lines.contains(&"   identity/needs-root"), "{report}");
    assert!(
        report.contains("\n   fix  Run as a non-root user;"),
        "{report}"
    );
    let narrow = at_width(&seed_shaped(), 60, false);
    let heading = lines_of(&narrow)
        .into_iter()
        .find(|l| l.starts_with(" ▌needs-root  "))
        .unwrap();
    assert_eq!(heading.width(), 60, "{narrow}");
}

const SUBPROCESS: &str = "(const char *, const char *, int, char **, FILE **, u_int, struct passwd *, privdrop_fn *, privrestore_fn *)";

fn with_signature(name: &str, signature: &str) -> Result {
    let mut result = sample(false);
    let mut findings = result.findings.into_vec();
    findings[0].finding.location.path = Some(RelPath::try_from("misc.c").unwrap());
    findings[0].finding.location.scope = Scope::Function {
        qualified_name: Text::from(name),
        signature: Text::from(signature),
    };
    result.findings = findings.into_iter().collect();
    result
}

fn signature_block(report: &str, name: &str) -> Vec<String> {
    let start = report.find(&format!("   ● misc.c:7   {name}(")).unwrap();
    let column = format!("   ● misc.c:7   {name}(").width();
    let mut lines = report[start..].lines();
    let first = lines.next().unwrap();
    let mut out = vec![first[first.find('(').unwrap() + 1..].to_owned()];
    for line in lines {
        if !line.starts_with(&" ".repeat(column)) {
            break;
        }
        assert!(!line[column..].starts_with(' '), "{line:?}");
        out.push(line[column..].to_owned());
    }
    out
}

#[test]
fn nested_parentheses_never_split_a_parameter() {
    let signature = "(struct a_rather_long_structure_name *, void (*)(int, int), struct another_long_structure_name *, unsigned long long)";
    let report = terminal(&with_signature("register_callback", signature), false);
    assert!(widest(&report) <= 80, "{report}");
    let block = signature_block(&report, "register_callback");
    assert!(block.len() > 1, "{report}");
    assert!(
        block.iter().any(|l| l.contains("void (*)(int, int),")),
        "{report}"
    );
    for line in &block[..block.len() - 1] {
        assert!(line.ends_with(','), "{line:?} in\n{report}");
    }
    assert_eq!(block.join(" "), &signature[1..]);
}

#[test]
fn a_parameter_wider_than_the_line_breaks_inside_itself() {
    let long = format!("struct {} *", "x".repeat(80));
    let signature = format!("(int, {long}, char)");
    let report = terminal(&with_signature("f", &signature), false);
    assert!(widest(&report) <= 80, "{report}");
    let block = signature_block(&report, "f");
    assert_eq!(block[0], "int,", "{report}");
    assert_eq!(block.last().unwrap(), "char)", "{report}");
    let middle: String = block[1..block.len() - 1].concat();
    assert_eq!(middle, format!("{long},"), "{report}");
    let short = terminal(&with_signature("f", "(int, char)"), false);
    assert!(short.contains("   ● misc.c:7   f(int, char)\n"), "{short}");
}

#[test]
fn fixes_and_findings_are_separated_by_one_blank_line() {
    let report = terminal(&seed_shaped(), false);
    assert!(!report.contains("\n\n\n"), "{report}");
    for gap in [
        "        container runtime.\n\n   ● low-port.c:11",
        "     htons-low-literal · 8cd6bfbb1cd6-1\n\n ▌needs-root",
        "     switch-to-root · 22f82d5b8473-1\n\n   ○ needs-root.c:8",
        "     guard  #ifdef _WIN32\n\n   ● registry.c:10",
        "     id-change-call · bdb745a285af-1\n\nNot checked\n",
        "─\n Findings   ● found   ○ suspected\n\n ▌low-port",
        "\n\n  5 files read · 0 not checked · 0.0% of source lines not checked\n\n─",
    ] {
        assert!(report.contains(gap), "{gap:?} not in\n{report}");
    }
}

fn unbadged(styled: &str) -> String {
    let mut text = String::from_utf8(strip_sgr(styled.as_bytes())).unwrap();
    for a in Tier::WORST_TO_BEST {
        for b in Tier::WORST_TO_BEST {
            text = text.replace(
                &format!(
                    " {} … {} ",
                    labels::tier(a).to_uppercase(),
                    labels::tier(b).to_uppercase()
                ),
                &format!("[{} … {}]", labels::tier(a), labels::tier(b)),
            );
        }
    }
    for t in Tier::WORST_TO_BEST {
        text = text.replace(
            &format!(" {} ", labels::tier(t).to_uppercase()),
            &format!("[{}]", labels::tier(t)),
        );
    }
    text
}

#[test]
fn styled_and_plain_hold_the_same_text_once_escapes_are_stripped() {
    let mut ranged = four_targets(&["user/foo"]);
    let mut targets = ranged.targets.into_vec();
    targets[1].range = TierRange {
        found: Tier::NeedsCodeChange,
        with_suspected: Tier::Blocked,
    };
    ranged.targets = targets.into_iter().collect();
    for result in [seed_shaped(), four_targets(&[]), ranged, sample(true)] {
        for width in [60, 71, 80, 120] {
            let styled = styled_at(&result, width);
            assert!(styled.contains('\x1b'));
            assert_eq!(
                unbadged(&styled),
                at_width(&result, width, false),
                "{width}"
            );
        }
    }
}

#[test]
fn colours_follow_the_operator_palette() {
    let styled = styled_at(&seed_shaped(), 80);
    for piece in [
        "\x1b[33m┌",
        "\x1b[33m│ \x1b[0m\x1b[33mSource\x1b[0m",
        "\x1b[2m\x1b[4mtarget ",
        "\x1b[2m5\x1b[0m \x1b[2mfiles\x1b[0m",
        "\x1b[2m────",
        "\x1b[1mFindings\x1b[0m   \x1b[1m●\x1b[0m found   \x1b[2m○\x1b[0m suspected",
        "\x1b[1m\x1b[35m▌low-port\x1b[0m  1 found",
        "\x1b[2mnetwork/low-port\x1b[0m\n",
        "\x1b[1m\x1b[32mfix\x1b[0m  Listen",
        "● \x1b[36mlow-port.c\x1b[0m\x1b[2m:11\x1b[0m   \x1b[1mlisten_http\x1b[0m\x1b[2m(\x1b[0m\x1b[2mvoid)\x1b[0m",
        "\x1b[2mhtons-low-literal\x1b[0m \x1b[2m·\x1b[0m \x1b[2m8cd6bfbb1cd6-1\x1b[0m",
        "\x1b[35mguard\x1b[0m  #ifdef",
    ] {
        assert!(styled.contains(piece), "{piece:?} not in\n{styled}");
    }
    for code in styled.split('\x1b').skip(1) {
        let sgr = &code[1..code.find('m').unwrap()];
        for n in sgr.split(';') {
            let n: u32 = n.parse().unwrap();
            assert!(
                matches!(n, 0..=9 | 30..=37 | 40..=47 | 90..=97 | 100..=107),
                "{sgr}"
            );
        }
    }
}
