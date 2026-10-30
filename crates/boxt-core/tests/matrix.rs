mod common;

use boxt_core::Text;
use boxt_core::compile_db::{CxxClass, classify};
use boxt_core::fact::FileKind;
use boxt_core::limits::LimitName;
use boxt_core::location::RelPath;
use boxt_core::location::frame_name;
use boxt_core::message::{
    CanonicalKey, EntryPath, Job, JobFile, JobKind, MacroDef, MacroDefError, MacroTable, Note,
    RegionError, WorkerMsg, encode_definitions,
};
use boxt_core::outcome::{
    Count, Details, FaultKind, FileOutcome, FileShare, LineShare, NotCheckedReason,
    OutOfScopeCause, Outcome, Sm3, line_count, sm3_share, verdict_counts,
};
use boxt_core::settings::PathPattern;
use boxt_core::shell::{RedirectOp, Redirection, lex};
use common::{collected, collected_at, macro_def};

#[test]
fn out_of_scope_adds_nothing() {
    let excluded = FileOutcome::OutOfScope {
        cause: OutOfScopeCause::Pattern(PathPattern {
            base: Text::from("/repo"),
            pattern: Text::from("vendor/**"),
        }),
    };
    let outcomes: Vec<FileOutcome> = std::iter::repeat_n(excluded, 5000)
        .chain(std::iter::repeat_n(FileOutcome::Read, 3))
        .collect();
    let counts = verdict_counts(&outcomes);
    assert_eq!(counts.read, 3);
    assert_eq!(counts.not_checked, Count::Exact(0));
}

#[test]
fn aggregate_of_unknown_count() {
    let counts = verdict_counts(&[FileOutcome::Aggregate {
        reason: NotCheckedReason::RepeatedFault,
        count: None,
        at_least: 0,
    }]);
    assert_eq!(counts.not_checked.to_string(), "at least 1");
}

#[test]
fn sm3_zero_denominator() {
    let config = Outcome::new(FileOutcome::Read, Some(FileKind::Config));
    let left_out = Outcome::new(
        FileOutcome::Aggregate {
            reason: NotCheckedReason::NotInCompileDatabase,
            count: Some(12),
            at_least: 12,
        },
        Some(FileKind::Source),
    );
    let single = Outcome::new(
        FileOutcome::NotChecked {
            reason: NotCheckedReason::NotInCompileDatabase,
            details: Details::None,
        },
        Some(FileKind::Source),
    );
    let none = Sm3::default();
    assert_eq!(sm3_share(std::iter::empty()), none);
    let files = [
        collected(config.with_lines(5), &[]),
        collected(left_out, &[]),
        collected(single.with_lines(5), &[(1, 2)]),
    ];
    for file in &files {
        assert_eq!(sm3_share([file]), none);
    }
    assert_eq!(sm3_share(&files), none);
}

#[test]
fn raw_text() {
    let t = Text::from_bytes(vec![0xff, 0x00]);
    assert_eq!(
        serde_json::to_string(&t).unwrap(),
        r#"{"bytes_base64":"/wA="}"#
    );
    assert!(Text::from_bytes(vec![0xfe, 0xff]) < t);
    assert!(Text::from("~") < t);
}

#[test]
fn frame_names() {
    for (input, expected) in [
        ("app-1.2.tar.gz", "app"),
        ("app-v2.1.tar.gz", "app"),
        ("app_1.2.orig.tar.gz", "app"),
        ("app.1.2.tar.gz", "app.1.2"),
        ("app-1.2-x86_64.tgz", "app"),
        ("app-x86_64.zip", "app-x86"),
    ] {
        assert_eq!(
            frame_name(&Text::from(input)),
            Some(Text::from(expected)),
            "{input}"
        );
    }
}

#[test]
fn shell() {
    let script = "useradd -u 501 app\n\
                  echo 'single quoted' \"double $X\" \\\n  continued\n\
                  a; b && c | d\n\
                  echo $$ > /var/run/app.pid\n";
    let cmds = lex(&Text::from(script)).unwrap();
    let argv: Vec<Vec<&str>> = cmds
        .iter()
        .map(|c| c.argv.iter().map(|t| t.as_str().unwrap()).collect())
        .collect();
    assert_eq!(
        argv,
        vec![
            vec!["useradd", "-u", "501", "app"],
            vec!["echo", "single quoted", "double $X", "continued"],
            vec!["a"],
            vec!["b"],
            vec!["c"],
            vec!["d"],
            vec!["echo", "$$"],
        ]
    );
    assert_eq!(
        cmds[6].redirections,
        vec![Redirection {
            fd: None,
            op: RedirectOp::Write,
            target: Text::from("/var/run/app.pid"),
        }]
    );
    assert_eq!(
        cmds.iter().map(|c| c.line).collect::<Vec<_>>(),
        vec![1, 2, 4, 4, 4, 4, 5]
    );
}

#[test]
fn header_class() {
    assert_eq!(classify(b"include/string"), CxxClass::Header);
    assert_eq!(classify(b"src/impl.ipp"), CxxClass::Header);
    assert_eq!(classify(b"src/main.cc"), CxxClass::Source);
}

#[test]
fn not_checked_reasons_have_no_free_text() {
    for reason in common::all_reasons() {
        let json = serde_json::to_value(FileOutcome::NotChecked {
            reason,
            details: Details::None,
        })
        .unwrap();
        let reason = &json["not_checked"]["reason"];
        let leaf = match reason {
            serde_json::Value::String(s) => s.clone(),
            serde_json::Value::Object(o) => o.keys().next().unwrap().clone(),
            other => panic!("unexpected reason shape {other}"),
        };
        assert!(
            leaf.bytes().all(|c| c.is_ascii_lowercase() || c == b'_'),
            "{leaf}"
        );
    }
}

fn source(file: FileOutcome) -> Outcome {
    Outcome::new(file, Some(FileKind::Source))
}

fn unparseable() -> FileOutcome {
    FileOutcome::NotChecked {
        reason: NotCheckedReason::Unparseable,
        details: Details::None,
    }
}

#[test]
fn sm3_read_file_with_two_overlapping_regions() {
    let file = collected(source(FileOutcome::Read).with_lines(100), &[(3, 5), (4, 9)]);
    let share = sm3_share([&file]);
    assert_eq!(
        share.lines,
        Some(LineShare {
            numerator: 7,
            denominator: 100,
            uncounted_files: 0,
            incomplete_files: 0,
        })
    );
    assert_eq!(
        share.files,
        Some(FileShare {
            numerator: 1,
            denominator: 1
        })
    );
}

#[test]
fn sm3_not_checked_file_counts_all_its_lines() {
    let file = collected(source(unparseable()).with_lines(40), &[]);
    let lines = sm3_share([&file]).lines.unwrap();
    assert_eq!((lines.numerator, lines.denominator), (40, 40));
}

#[test]
fn sm3_out_of_scope_adds_to_no_share() {
    let excluded = collected(
        source(FileOutcome::OutOfScope {
            cause: OutOfScopeCause::Pattern(PathPattern {
                base: Text::from("/repo"),
                pattern: Text::from("vendor/**"),
            }),
        })
        .with_lines(500),
        &[(1, 2)],
    );
    let read = collected(source(FileOutcome::Read).with_lines(10), &[]);
    let share = sm3_share([&excluded, &read]);
    assert_eq!(
        share.lines.map(|l| (l.numerator, l.denominator)),
        Some((0, 10))
    );
    assert_eq!(
        share.files.map(|f| (f.numerator, f.denominator)),
        Some((0, 1))
    );
}

#[test]
fn sm3_region_past_the_end_is_clamped_and_refused() {
    let file = collected(source(FileOutcome::Read).with_lines(10), &[(8, 12)]);
    assert_eq!(sm3_share([&file]).lines.unwrap().numerator, 3);
    assert_eq!(file.validate_regions(), Err(RegionError::PastEnd));
    let inside = collected(source(FileOutcome::Read).with_lines(10), &[(8, 10)]);
    assert_eq!(inside.validate_regions(), Ok(()));
    let inverted = collected(source(FileOutcome::Read).with_lines(10), &[(9, 8)]);
    assert_eq!(inverted.validate_regions(), Err(RegionError::Inverted));
}

#[test]
fn line_count_without_a_final_newline_and_of_an_empty_file() {
    assert_eq!(line_count(b"a\nb", None), 2);
    assert_eq!(line_count(b"", None), 0);
    let empty = collected(source(FileOutcome::Read).with_lines(0), &[]);
    let share = sm3_share([&empty]);
    assert_eq!(share.lines, None);
    assert_eq!(
        share.files.map(|f| (f.numerator, f.denominator)),
        Some((0, 1))
    );
}

#[test]
fn sm3_line_sums_saturate() {
    let big = collected(source(unparseable()).with_lines(u64::MAX - 1), &[]);
    let lines = sm3_share([&big, &big]).lines.unwrap();
    assert_eq!((lines.numerator, lines.denominator), (u64::MAX, u64::MAX));
}

#[test]
fn sm3_uncounted_files_make_the_line_share_a_lower_bound() {
    let cut = collected(
        source(FileOutcome::NotChecked {
            reason: NotCheckedReason::LimitExceeded(LimitName::FileSize),
            details: Details::Measured {
                value: 70_000_000,
                limit: 67_108_864,
            },
        }),
        &[],
    );
    let fault = collected(
        source(FileOutcome::NotChecked {
            reason: NotCheckedReason::Fault(FaultKind::Crash),
            details: Details::None,
        }),
        &[],
    );
    let aggregate = collected(
        source(FileOutcome::Aggregate {
            reason: NotCheckedReason::LimitExceeded(LimitName::TotalEntries),
            count: None,
            at_least: 1,
        }),
        &[],
    );
    let read = collected(source(FileOutcome::Read).with_lines(50), &[]);
    let share = sm3_share([&cut, &fault, &aggregate, &read]);
    let lines = share.lines.unwrap();
    assert_eq!(
        lines,
        LineShare {
            numerator: 0,
            denominator: 50,
            uncounted_files: 3,
            incomplete_files: 0,
        }
    );
    assert!(lines.is_lower_bound());
    assert_eq!(
        share.files.map(|f| (f.numerator, f.denominator)),
        Some((3, 4))
    );
}

#[test]
fn two_definitions_of_one_name_are_both_kept_in_order() {
    let table = MacroTable::from(vec![
        macro_def("FOO", "src/config.h", 9, None),
        macro_def("FOO", "src/config.h", 3, None),
    ]);
    let lines: Vec<Option<u32>> = table.iter().map(|d| d.location.line).collect();
    assert_eq!(lines, vec![Some(3), Some(9)]);
}

#[test]
fn an_oversized_definition_is_dropped_and_its_file_keeps_the_rest() {
    let cap = 2048;
    let big = MacroDef {
        body: Text::from("x".repeat(4096)),
        ..macro_def("BIG", "src/big.h", 1, None)
    };
    let small = macro_def("SMALL", "src/big.h", 2, None);
    let job = Job {
        kind: JobKind::Definitions,
        input: common::input(),
        file: JobFile::InInput(RelPath::try_from("src/big.h").unwrap()),
        resume_after: None,
    };
    assert!(matches!(
        big.validate(&job, cap),
        Err(MacroDefError::TooLong { .. })
    ));
    let read: Vec<MacroDef> = [small.clone(), big]
        .into_iter()
        .filter(|def| def.validate(&job, cap).is_ok())
        .collect();
    assert_eq!(read, [small]);
    assert!(encode_definitions(&read, cap).is_ok());
    let note = WorkerMsg::Notes {
        entry: EntryPath::job(),
        notes: vec![Note::DroppedDefinitions(1)],
    };
    assert!(note.allowed_in(JobKind::Definitions));
    assert_eq!(CanonicalKey::for_outcome(&job, &EntryPath::job()), None);
}

#[test]
fn sm3_worked_example_from_the_contract() {
    let cut = |file: FileOutcome| source(file);
    let files = [
        collected_at(
            "a.c",
            source(FileOutcome::Read).with_lines(100),
            &[(3, 5), (4, 9)],
        ),
        collected_at("b.c", source(unparseable()).with_lines(40), &[]),
        collected_at("c.c", source(FileOutcome::Read).with_lines(0), &[]),
        collected_at(
            "d.c",
            cut(FileOutcome::NotChecked {
                reason: NotCheckedReason::LimitExceeded(LimitName::FileSize),
                details: Details::None,
            }),
            &[],
        ),
        collected_at(
            "e.c",
            cut(FileOutcome::NotChecked {
                reason: NotCheckedReason::LimitExceeded(LimitName::FileSize),
                details: Details::None,
            })
            .with_partial_lines(400),
            &[],
        ),
        collected_at(
            "f.c",
            source(FileOutcome::OutOfScope {
                cause: OutOfScopeCause::Pattern(PathPattern {
                    base: Text::from("/repo"),
                    pattern: Text::from("vendor/**"),
                }),
            }),
            &[],
        ),
    ];
    let share = sm3_share(&files);
    assert_eq!(
        share,
        Sm3 {
            lines: Some(LineShare {
                numerator: 447,
                denominator: 540,
                uncounted_files: 1,
                incomplete_files: 1,
            }),
            files: Some(FileShare {
                numerator: 4,
                denominator: 5
            }),
            uncounted: vec![files[3].key.clone()].into(),
            incomplete: vec![files[4].key.clone()].into(),
        }
    );
    assert!(share.lines.unwrap().is_lower_bound());
}

#[test]
fn sm3_partial_count_joins_both_counts_as_a_lower_bound() {
    let cut = collected_at(
        "big.c",
        source(FileOutcome::NotChecked {
            reason: NotCheckedReason::LimitExceeded(LimitName::FileSize),
            details: Details::Measured {
                value: 2_000,
                limit: 1_000,
            },
        })
        .with_partial_lines(400),
        &[],
    );
    let share = sm3_share([&cut]);
    let lines = share.lines.unwrap();
    assert_eq!(
        lines,
        LineShare {
            numerator: 400,
            denominator: 400,
            uncounted_files: 0,
            incomplete_files: 1,
        }
    );
    assert!(lines.is_lower_bound());
    assert_eq!(share.incomplete.as_slice(), std::slice::from_ref(&cut.key));
    assert!(share.uncounted.is_empty());
    let whole = collected_at("big.c", source(unparseable()).with_lines(400), &[]);
    assert!(!sm3_share([&whole]).lines.unwrap().is_lower_bound());
}

#[test]
fn sm3_only_uncounted_files_keep_a_line_share_of_zero_of_zero() {
    let fault = |path| {
        collected_at(
            path,
            source(FileOutcome::NotChecked {
                reason: NotCheckedReason::Fault(FaultKind::Crash),
                details: Details::None,
            }),
            &[],
        )
    };
    let files = [fault("b.c"), fault("a.c")];
    let share = sm3_share(&files);
    let lines = share.lines.unwrap();
    assert_eq!(
        lines,
        LineShare {
            numerator: 0,
            denominator: 0,
            uncounted_files: 2,
            incomplete_files: 0,
        }
    );
    assert!(lines.is_lower_bound());
    assert_eq!(
        share.files,
        Some(FileShare {
            numerator: 2,
            denominator: 2
        })
    );
    assert_eq!(
        share.uncounted.as_slice(),
        &[files[1].key.clone(), files[0].key.clone()]
    );
}
