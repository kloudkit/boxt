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
    BehaviourId, Confidence, Evidence, EvidenceId, Finding, NotCheckedEntry, ReportedFinding,
    Result, RuleId, TargetId, TargetVerdict, Tier, TierRange,
};
use boxt_render::boxt_core::version::Versions;
use boxt_render::boxt_core::{SortedVec, Text};
use boxt_render::terminal;

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
            NotCheckedEntry::Region {
                location: loc("partly.c", None, Scope::File),
                first_line: 10,
                last_line: 20,
                reason: RegionReason::Unparseable,
            },
            NotCheckedEntry::DynamicLoad {
                location: packaged,
                symbol: Text::from("dlopen"),
            },
        ]
        .into_iter()
        .collect(),
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
        "Source only: results describe the code",
        labels::HOST_DEPENDENT_TEXT,
        "docker-default  needs Windows  (1 found, 0 suspected)  5 files read, 2 not checked",
        "kubernetes-restricted  needs code change … blocked  (1 found, 0 suspected)",
        "\nFindings\n",
        "  9c1e00000000-1  identity/setuid  setuid  found  (id-change-call)",
        "      guard: #ifdef _WIN32",
        "      fix: Start as the user.",
        "\nNot checked\n",
        "  kubernetes-restricted: unmapped behaviour user/foo",
        "  broken.c: unparseable",
        "  partly.c lines 10–20: unparseable region",
        "  app!bin/x: loads code at run time through dlopen",
        "  Source lines not checked: 0.8% (3 of 412); files: 2 of 5",
    ];
    let mut last = 0;
    for needle in order {
        let at = position(&report, needle);
        assert!(at >= last, "{needle:?} out of order in\n{report}");
        last = at;
    }
    assert!(report.starts_with(&sample(true).input_scope_text));
}

#[test]
fn the_host_note_shows_only_when_host_dependent() {
    assert!(!terminal(&sample(false), false).contains(labels::HOST_DEPENDENT_TEXT));
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
            report.contains(&format!("docker-default  {}  (", labels::tier(tier))),
            "{report}"
        );
    }
}

#[test]
fn hostile_names_are_escaped() {
    let report = terminal(&sample(false), false);
    assert!(
        report.contains(
            "ev\\u{202e}il\\x09\\u{200b}.c:7  in drop\\u{202e}\\u{200b}_to(const char *, uid_t)"
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
    result.not_checked = SortedVec::new();
    let mut targets = result.targets.into_vec();
    for t in &mut targets {
        t.unmapped = SortedVec::new();
    }
    result.targets = targets.into_iter().collect();
    let report = terminal(&result, false);
    assert!(report.contains("\nFindings\n  none\n"), "{report}");
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
