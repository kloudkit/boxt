mod common;

use boxt_core::Text;
use boxt_core::compile_db::{CxxClass, classify};
use boxt_core::fact::FileKind;
use boxt_core::location::frame_name;
use boxt_core::outcome::{
    Count, Details, FileOutcome, NotCheckedReason, OutOfScopeCause, Outcome, sm3_share,
    verdict_counts,
};
use boxt_core::settings::PathPattern;
use boxt_core::shell::{RedirectOp, Redirection, lex};

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
    assert_eq!(sm3_share(std::iter::empty()), None);
    assert_eq!(sm3_share([(&config, false)]), None);
    assert_eq!(sm3_share([(&left_out, false)]), None);
    assert_eq!(sm3_share([(&single, true)]), None);
    assert_eq!(
        sm3_share([(&config, true), (&left_out, true), (&single, true)]),
        None
    );
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
