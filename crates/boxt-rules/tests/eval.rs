mod common;

use boxt_core::fact::{Directive, Fact, FileKind, GuardChain, GuardFrame, RegionId};
use boxt_core::location::{
    Frame, FrameKind, InputRef, Location, PathText, RelPath, Scope, ScriptName,
};
use boxt_core::verdict::{Confidence, Finding, Observation};
use boxt_core::{RulesOutput, Text};
use boxt_rules::{Corpus, evaluate, load_dir};

use common::corpora;

fn corpus() -> Corpus {
    load_dir(&corpora().join("eval")).unwrap()
}

fn only(rule: &str) -> Corpus {
    let mut corpus = corpus();
    corpus
        .rules
        .retain(|r| r.id.as_str() == format!("eval/{rule}"));
    assert_eq!(corpus.rules.len(), 1, "{rule}");
    corpus
}

fn run(rule: &str, facts: &[Fact]) -> Vec<Finding> {
    evaluate(&only(rule), facts).findings.into_vec()
}

fn lines(findings: &[Finding]) -> Vec<Option<u32>> {
    findings.iter().map(|f| f.location.line).collect()
}

fn t(s: &str) -> Text {
    Text::from(s)
}

fn input() -> InputRef {
    InputRef {
        index: 0,
        given: t("/src/app"),
    }
}

fn function(name: &str) -> Scope {
    Scope::Function {
        qualified_name: t(name),
        signature: t("(int)"),
    }
}

fn at(path: &str, line: u32, scope: Scope) -> Location {
    Location {
        input: input(),
        frames: Vec::new(),
        path: Some(RelPath::try_from(path).unwrap()),
        line: Some(line),
        scope,
    }
}

fn src(line: u32) -> Location {
    at("src/main.c", line, Scope::File)
}

fn call_at(location: Location, callee: &str, args: &[&str]) -> Fact {
    Fact::Call {
        file_kind: FileKind::Source,
        location,
        callee: t(callee),
        args: args.iter().map(|a| t(a)).collect(),
        guards: Vec::new(),
    }
}

fn call(line: u32, callee: &str, args: &[&str]) -> Fact {
    call_at(src(line), callee, args)
}

fn literal_at(location: Location, file_kind: FileKind, value: &str) -> Fact {
    Fact::StringLiteral {
        file_kind,
        location,
        value: t(value),
        guards: Vec::new(),
    }
}

fn rpm() -> Vec<Frame> {
    vec![Frame {
        kind: FrameKind::Rpm,
        name: t("app"),
    }]
}

fn package_location(scope: Scope) -> Location {
    Location {
        input: input(),
        frames: rpm(),
        path: None,
        line: None,
        scope,
    }
}

fn package_file(path: &str, mode: Option<u32>) -> Fact {
    Fact::PackageFile {
        file_kind: FileKind::Package,
        location: package_location(Scope::File),
        path: PathText::from(path),
        mode,
        user: None,
        group: None,
    }
}

fn package_script() -> Fact {
    Fact::PackageScript {
        file_kind: FileKind::Package,
        location: package_location(Scope::Script(ScriptName::PostInstall)),
        name: ScriptName::PostInstall,
        interpreter: None,
    }
}

fn binary(path: &str, scope: Scope) -> Location {
    Location {
        line: None,
        ..at(path, 1, scope)
    }
}

fn import(symbol: &str) -> Fact {
    Fact::Import {
        file_kind: FileKind::Binary,
        location: binary("usr/bin/app", Scope::Symbol(t(symbol))),
        symbol: t(symbol),
        version: None,
    }
}

fn config_key(line: u32, key: &str, value: Option<&str>) -> Fact {
    Fact::ConfigKey {
        file_kind: FileKind::Config,
        location: at("etc/app.ini", line, Scope::ConfigKey(t(key))),
        key: t(key),
        value: value.map(t),
    }
}

fn include(line: u32, target: &str, system: bool) -> Fact {
    Fact::Include {
        file_kind: FileKind::Source,
        location: src(line),
        target: t(target),
        system,
        guards: Vec::new(),
    }
}

fn windows_else_chain() -> GuardChain {
    GuardChain {
        frames: vec![
            GuardFrame {
                directive: Directive::Ifdef,
                text: t("#ifdef _WIN32"),
                identifiers: vec![t("_WIN32")].into(),
                holds_code: false,
                has_token: true,
            },
            GuardFrame {
                directive: Directive::Else,
                text: t("#else"),
                identifiers: Vec::new().into(),
                holds_code: true,
                has_token: true,
            },
        ],
        later: Vec::new(),
        has_alternative: true,
        region: RegionId(1),
    }
}

#[test]
fn equals_compares_text_byte_exact() {
    let facts = [
        call(1, "setuid", &[]),
        call(2, "Setuid", &[]),
        call(3, "setuid ", &[]),
    ];
    assert_eq!(lines(&run("equals-text", &facts)), [Some(1)]);
}

#[test]
fn equals_on_a_path_field_normalizes_the_literal() {
    let facts = [
        package_file("/opt/app/bin", None),
        package_file("/opt/app", None),
    ];
    let found = run("equals-path", &facts);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].location, package_location(Scope::File));
}

#[test]
fn equals_compares_integers_by_value() {
    let facts = [call(3, "a", &[]), call(4, "a", &[])];
    assert_eq!(lines(&run("equals-integer", &facts)), [Some(4)]);
}

#[test]
fn equals_compares_bools_by_value() {
    let facts = [include(1, "a.h", false), include(2, "b.h", true)];
    assert_eq!(lines(&run("equals-bool", &facts)), [Some(2)]);
}

#[test]
fn one_of_holds_for_any_listed_value() {
    let facts = [
        call(1, "setuid", &[]),
        call(2, "setgid", &[]),
        call(3, "seteuid", &[]),
    ];
    assert_eq!(lines(&run("one-of-text", &facts)), [Some(1), Some(2)]);
    let facts = [call(3, "a", &[]), call(4, "a", &[]), call(5, "a", &[])];
    assert_eq!(lines(&run("one-of-integer", &facts)), [Some(3), Some(5)]);
}

#[test]
fn a_text_star_crosses_slashes_and_a_path_star_does_not() {
    let facts = [literal_at(
        at("a/b/c", 1, Scope::File),
        FileKind::Source,
        "a/b/c",
    )];
    assert_eq!(run("pattern-text", &facts).len(), 1);
    assert_eq!(run("pattern-path", &facts).len(), 0);
    let flat = [literal_at(at("abc", 1, Scope::File), FileKind::Source, "x")];
    assert_eq!(run("pattern-path", &flat).len(), 1);
}

#[test]
fn path_under_is_the_path_or_below_it() {
    for (path, want) in [
        ("/opt", 1),
        ("/opt/x", 1),
        ("/optx", 0),
        ("/opt/x/y", 1),
        ("/", 0),
    ] {
        let facts = [package_file(path, None)];
        assert_eq!(run("path-under", &facts).len(), want, "{path}");
    }
}

#[test]
fn an_unnormalized_fact_path_matches_after_normalizing() {
    let facts = [package_file("/opt//x/./y", None)];
    assert_eq!(run("path-under-deep", &facts).len(), 1);
    let facts = [package_file("/opt/y/../x/z", None)];
    assert_eq!(run("path-under-deep", &facts).len(), 1);
    let facts = [package_file("/opt/xy", None)];
    assert_eq!(run("path-under-deep", &facts).len(), 0);
}

#[test]
fn number_compares_integer_fields() {
    let facts = [
        package_file("/a", Some(0o4755)),
        package_file("/b", Some(0o755)),
        package_file("/c", None),
    ];
    let found = run("number-integer", &facts);
    assert_eq!(found.len(), 1);
}

#[test]
fn number_parses_a_text_field_as_a_c_integer_literal() {
    let facts = [
        call(1, "htons", &["0x50"]),
        call(2, "htons", &[""]),
        call(3, "htons", &["8080"]),
        call(4, "htons", &["0120"]),
        call(5, "htons", &["443u"]),
        call(6, "htons", &["PORT"]),
        call(7, "htons", &[]),
    ];
    assert_eq!(
        lines(&run("number-text", &facts)),
        [Some(1), Some(4), Some(5)]
    );
}

#[test]
fn guard_contains_reads_every_frame_whichever_side() {
    let guarded = Fact::Call {
        file_kind: FileKind::Source,
        location: src(5),
        callee: t("f"),
        args: Vec::new(),
        guards: vec![windows_else_chain()],
    };
    let facts = [call(1, "f", &[]), guarded];
    assert_eq!(lines(&run("guard-contains", &facts)), [Some(5)]);
}

#[test]
fn a_kind_alone_holds_for_every_fact_of_that_kind() {
    let facts = [import("dlopen"), call(1, "dlopen", &[]), import("puts")];
    assert_eq!(run("exists", &facts).len(), 2);
}

#[test]
fn all_needs_every_predicate_on_the_same_fact() {
    let facts = [
        call(1, "setuid", &["0"]),
        call(2, "setuid", &["1"]),
        call(3, "setgid", &["0"]),
    ];
    assert_eq!(lines(&run("all", &facts)), [Some(1)]);
}

#[test]
fn any_needs_one_predicate() {
    let facts = [call(1, "setuid", &[]), import("setuid"), import("puts")];
    assert_eq!(run("any", &facts).len(), 2);
}

#[test]
fn not_inverts_its_predicate() {
    let facts = [call(1, "setuid", &[]), call(2, "puts", &[]), import("puts")];
    assert_eq!(lines(&run("not", &facts)), [Some(2)]);
}

#[test]
fn a_cross_kind_all_without_co_occurs_never_holds() {
    let facts = [call(1, "setuid", &[]), include(2, "unistd.h", true)];
    assert!(run("cross-kind-all", &facts).is_empty());
}

#[test]
fn an_absent_optional_field_is_false_and_its_negation_true() {
    let facts = [
        config_key(1, "port", None),
        config_key(2, "port", Some("80")),
    ];
    assert_eq!(lines(&run("missing-optional", &facts)), [Some(2)]);
    assert_eq!(lines(&run("not-missing-optional", &facts)), [Some(1)]);
}

#[test]
fn an_index_out_of_range_is_false() {
    let facts = [call(1, "f", &["1"]), call(2, "f", &["1", "2", "3", ""])];
    assert_eq!(lines(&run("index-out-of-range", &facts)), [Some(2)]);
}

#[test]
fn function_scope_holds_only_in_the_function_with_every_witness() {
    let facts = [
        call_at(src(2), "socket", &[]),
        call_at(at("src/main.c", 3, function("a")), "socket", &[]),
        call_at(at("src/main.c", 4, function("a")), "bind", &[]),
        call_at(at("src/main.c", 8, function("b")), "socket", &[]),
        call_at(at("src/main.c", 9, function("c")), "bind", &[]),
        call_at(src(12), "bind", &[]),
    ];
    let found = run("co-occurs-function", &facts);
    assert_eq!(lines(&found), [Some(3)]);
    assert_eq!(found[0].location.scope, function("a"));
}

#[test]
fn a_function_in_another_file_is_another_group() {
    let facts = [
        call_at(at("src/a.c", 3, function("a")), "socket", &[]),
        call_at(at("src/b.c", 4, function("a")), "bind", &[]),
    ];
    assert!(run("co-occurs-function", &facts).is_empty());
}

#[test]
fn a_package_script_is_not_the_same_file_as_entry_zero() {
    let facts = [package_file("/opt/app", None), package_script()];
    assert!(run("co-occurs-file", &facts).is_empty());
}

#[test]
fn file_scope_holds_across_functions_of_one_file() {
    let facts = [
        call_at(at("src/a.c", 3, function("a")), "socket", &[]),
        call_at(at("src/a.c", 9, function("b")), "bind", &[]),
        call_at(at("src/b.c", 1, Scope::File), "bind", &[]),
    ];
    assert_eq!(lines(&run("co-occurs-file-calls", &facts)), [Some(3)]);
}

#[test]
fn a_group_finding_sits_at_its_first_witness() {
    let facts = [
        call_at(at("src/a.c", 4, Scope::File), "bind", &[]),
        call_at(at("src/a.c", 9, Scope::File), "socket", &[]),
    ];
    let found = run("co-occurs-file-calls", &facts);
    assert_eq!(lines(&found), [Some(4)]);
}

#[test]
fn package_scope_needs_a_package_frame() {
    let facts = [call(1, "socket", &[]), call(2, "bind", &[])];
    assert!(run("co-occurs-package-source", &facts).is_empty());
}

#[test]
fn package_scope_spans_the_files_of_one_package() {
    let in_package = |frames: Vec<Frame>| Fact::Call {
        file_kind: FileKind::Source,
        location: Location {
            frames,
            ..at("src/main.c", 7, Scope::File)
        },
        callee: t("socket"),
        args: Vec::new(),
        guards: Vec::new(),
    };
    let mut nested = rpm();
    nested.push(Frame {
        kind: FrameKind::Archive,
        name: t("src"),
    });
    let facts = [package_file("/opt/app", None), in_package(nested)];
    let found = run("co-occurs-package", &facts);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].location, package_location(Scope::File));

    let other = vec![Frame {
        kind: FrameKind::Deb,
        name: t("other"),
    }];
    let facts = [package_file("/opt/app", None), in_package(other)];
    assert!(run("co-occurs-package", &facts).is_empty());
}

#[test]
fn a_nested_co_occurs_is_a_predicate_on_each_anchor() {
    let facts = [
        call_at(at("src/main.c", 3, function("a")), "socket", &[]),
        call_at(at("src/main.c", 4, function("a")), "bind", &[]),
        call_at(at("src/main.c", 5, function("a")), "bind", &[]),
        call_at(at("src/main.c", 9, function("b")), "bind", &[]),
    ];
    assert_eq!(lines(&run("co-occurs-nested", &facts)), [Some(4), Some(5)]);
}

#[test]
fn found_wins_over_suspected_for_one_fact() {
    let facts = [call(1, "setuid", &["0"]), call(2, "setuid", &["uid"])];
    let found = run("found-and-suspected", &facts);
    let got: Vec<(Option<u32>, Confidence, &str)> = found
        .iter()
        .map(|f| {
            (
                f.location.line,
                f.confidence,
                f.evidence.definition.as_str(),
            )
        })
        .collect();
    assert_eq!(
        got,
        [
            (Some(1), Confidence::Found, "literal-zero"),
            (Some(2), Confidence::Suspected, "any-call"),
        ]
    );
}

#[test]
fn found_wins_over_suspected_for_one_group() {
    let facts = [
        call_at(at("src/a.c", 1, Scope::File), "socket", &[]),
        call_at(at("src/a.c", 2, Scope::File), "bind", &[]),
        call_at(at("src/b.c", 5, Scope::File), "bind", &[]),
    ];
    let found = run("found-and-suspected-groups", &facts);
    let got: Vec<(Option<u32>, Confidence)> = found
        .iter()
        .map(|f| (f.location.line, f.confidence))
        .collect();
    assert_eq!(
        got,
        [
            (Some(1), Confidence::Found),
            (Some(5), Confidence::Suspected)
        ]
    );
}

#[test]
fn a_finding_carries_the_rule_and_the_anchor() {
    let guarded = Fact::Call {
        file_kind: FileKind::Source,
        location: src(5),
        callee: t("f"),
        args: Vec::new(),
        guards: vec![windows_else_chain()],
    };
    let found = run("guard-contains", &[guarded]);
    assert_eq!(found.len(), 1);
    let f = &found[0];
    assert_eq!(f.location, src(5));
    assert_eq!(f.rule.as_str(), "eval/guard-contains");
    assert_eq!(f.behaviour.as_str(), "b");
    assert_eq!(f.confidence, Confidence::Found);
    assert_eq!(f.evidence.definition.as_str(), "guard-contains-found");
    assert_eq!(f.guards, [windows_else_chain()]);
    assert_eq!(f.fix, "Fix guard-contains.");
}

#[test]
fn common_fields_resolve_to_text() {
    let facts = [
        literal_at(binary("usr/bin/app", Scope::File), FileKind::Binary, "x"),
        literal_at(src(1), FileKind::Source, "x"),
    ];
    assert_eq!(run("file-kind", &facts).len(), 1);

    let setting = |key: &str| Fact::ServiceSetting {
        file_kind: FileKind::Service,
        location: at(
            "app.service",
            2,
            Scope::UnitSection {
                section: t("Service"),
                key: t(key),
            },
        ),
        section: t("Service"),
        key: t(key),
        value: t("app"),
    };
    assert_eq!(
        run("scope-text", &[setting("User"), setting("Group")]).len(),
        1
    );

    let facts = [
        call_at(at("src/a.c", 1, function("ns::serve")), "f", &[]),
        call_at(at("src/a.c", 2, function("serve")), "f", &[]),
        call_at(at("src/a.c", 3, Scope::File), "f", &[]),
    ];
    assert_eq!(lines(&run("scope-function", &facts)), [Some(1)]);
    assert_eq!(lines(&run("scope-file", &facts)), [Some(3)]);
    assert_eq!(run("scope-script", &[package_script()]).len(), 1);
}

#[test]
fn dynamic_load_comes_from_binary_imports_only() {
    let facts = [
        call(1, "dlopen", &["\"libx.so\""]),
        import("dlopen"),
        import("dlsym"),
    ];
    let matched = evaluate(&only("equals-text"), &facts);
    assert_eq!(
        matched.observations.as_slice(),
        [Observation::DynamicLoad {
            location: binary("usr/bin/app", Scope::Symbol(t("dlopen"))),
            symbol: t("dlopen"),
        }]
    );
}

#[test]
fn the_same_facts_give_byte_identical_output() {
    let facts = [
        call_at(at("src/a.c", 1, function("a")), "socket", &[]),
        call_at(at("src/a.c", 2, function("a")), "bind", &["0"]),
        call(3, "setuid", &["0"]),
        call(4, "htons", &["80"]),
        import("dlopen"),
        package_file("/opt/x", Some(0o4755)),
        package_script(),
        config_key(1, "port", None),
    ];
    let corpus = corpus();
    let first: RulesOutput = evaluate(&corpus, &facts);
    let second = evaluate(&corpus, &facts);
    assert!(!first.findings.is_empty());
    assert_eq!(
        serde_json::to_vec(&first).unwrap(),
        serde_json::to_vec(&second).unwrap()
    );
}

fn in_input(index: u32, mut fact: Fact) -> Fact {
    let location = match &mut fact {
        Fact::Call { location, .. } | Fact::PackageFile { location, .. } => location,
        _ => unreachable!(),
    };
    location.input.index = index;
    fact
}

#[test]
fn a_suspected_group_holding_a_found_fact_gives_no_finding() {
    let facts = [
        call_at(at("src/a.c", 1, Scope::File), "socket", &[]),
        call_at(at("src/a.c", 2, Scope::File), "bind", &[]),
        call_at(at("src/b.c", 5, Scope::File), "socket", &[]),
    ];
    let got: Vec<(Option<u32>, Confidence)> = run("mixed-found-fact", &facts)
        .iter()
        .map(|f| (f.location.line, f.confidence))
        .collect();
    assert_eq!(
        got,
        [
            (Some(2), Confidence::Found),
            (Some(5), Confidence::Suspected)
        ]
    );
}

#[test]
fn a_suspected_fact_inside_a_found_group_gives_no_finding() {
    let facts = [
        call_at(at("src/a.c", 1, Scope::File), "socket", &[]),
        call_at(at("src/a.c", 2, Scope::File), "bind", &[]),
        call_at(at("src/b.c", 5, Scope::File), "bind", &[]),
    ];
    let got: Vec<(Option<u32>, Confidence)> = run("mixed-found-group", &facts)
        .iter()
        .map(|f| (f.location.line, f.confidence))
        .collect();
    assert_eq!(
        got,
        [
            (Some(1), Confidence::Found),
            (Some(5), Confidence::Suspected)
        ]
    );
}

#[test]
fn another_input_is_another_file_and_another_package() {
    let facts = [
        call_at(at("src/a.c", 1, Scope::File), "socket", &[]),
        in_input(1, call_at(at("src/a.c", 2, Scope::File), "bind", &[])),
    ];
    assert!(run("co-occurs-file-calls", &facts).is_empty());

    let packaged = Fact::Call {
        file_kind: FileKind::Source,
        location: Location {
            frames: rpm(),
            ..at("src/main.c", 7, Scope::File)
        },
        callee: t("socket"),
        args: Vec::new(),
        guards: Vec::new(),
    };
    let facts = [
        package_file("/opt/app", None),
        in_input(1, packaged.clone()),
    ];
    assert!(run("co-occurs-package", &facts).is_empty());
    let facts = [package_file("/opt/app", None), packaged];
    assert_eq!(run("co-occurs-package", &facts).len(), 1);
}

#[test]
fn another_signature_is_another_function() {
    let scope = |signature: &str| Scope::Function {
        qualified_name: t("serve"),
        signature: t(signature),
    };
    let facts = [
        call_at(at("src/a.c", 3, scope("(int)")), "socket", &[]),
        call_at(at("src/a.c", 9, scope("(long)")), "bind", &[]),
    ];
    assert!(run("co-occurs-function", &facts).is_empty());
}

#[test]
fn a_kind_on_a_top_level_co_occurs_needs_a_member_of_that_kind() {
    let facts = [
        include(1, "sys/socket.h", true),
        call(3, "socket", &[]),
        call_at(at("src/other.c", 2, Scope::File), "socket", &[]),
    ];
    let found = run("co-occurs-kind", &facts);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].location, src(3));
}

#[test]
fn one_symbol_imported_twice_at_one_location_is_one_observation() {
    let versioned = |version: &str| Fact::Import {
        file_kind: FileKind::Binary,
        location: binary("usr/bin/app", Scope::Symbol(t("dlopen"))),
        symbol: t("dlopen"),
        version: Some(t(version)),
    };
    let facts = [versioned("GLIBC_2.2.5"), versioned("GLIBC_2.34")];
    let matched = evaluate(&only("equals-text"), &facts);
    assert_eq!(matched.observations.len(), 1);
}

fn frame(directive: Directive, text: &str, has_token: bool) -> GuardFrame {
    let mut identifiers: Vec<Text> = text
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .skip(2)
        .filter(|w| {
            !w.is_empty() && *w != "defined" && !w.starts_with(|c: char| c.is_ascii_digit())
        })
        .map(t)
        .collect();
    identifiers.sort();
    identifiers.dedup();
    GuardFrame {
        directive,
        text: t(text),
        identifiers: identifiers.into(),
        holds_code: false,
        has_token,
    }
}

fn chain(mut frames: Vec<GuardFrame>, later: Vec<GuardFrame>, has_alternative: bool) -> GuardChain {
    if let Some(code) = frames.last_mut() {
        code.holds_code = true;
    }
    let chain = GuardChain {
        frames,
        later,
        has_alternative,
        region: RegionId(0),
    };
    chain.validate().unwrap();
    chain
}

const FIXTURES: &str = "../../fixtures/source/windows-branches";

struct Row {
    fixture: &'static str,
    line: u32,
    callee: &'static str,
    guards: Vec<GuardChain>,
}

fn windows_matrix() -> Vec<(Row, bool)> {
    use Directive::{Elif, Else, If, Ifdef, Ifndef};
    let win = || frame(Ifdef, "#ifdef _WIN32", true);
    let other = |holds| frame(Else, "#else", holds);
    let reg = "RegOpenKeyExA";
    let row = |fixture, line, guards| Row {
        fixture,
        line,
        callee: reg,
        guards,
    };
    vec![
        (
            row(
                "plain-else.c",
                7,
                vec![chain(vec![win()], vec![other(true)], true)],
            ),
            true,
        ),
        (row("no-guard.c", 4, Vec::new()), false),
        (
            row(
                "error-else.c",
                5,
                vec![chain(vec![win()], vec![other(false)], false)],
            ),
            false,
        ),
        (
            row(
                "empty-else.c",
                5,
                vec![chain(vec![win()], vec![other(false)], false)],
            ),
            false,
        ),
        (
            row(
                "negated.c",
                9,
                vec![chain(
                    vec![frame(Ifndef, "#ifndef _WIN32", true), other(true)],
                    Vec::new(),
                    true,
                )],
            ),
            true,
        ),
        (
            row(
                "disjunction.c",
                7,
                vec![chain(
                    vec![frame(If, "#if defined(_WIN32) || defined(_WIN64)", true)],
                    vec![other(true)],
                    true,
                )],
            ),
            true,
        ),
        (
            row(
                "linux-elif.c",
                7,
                vec![chain(
                    vec![win()],
                    vec![frame(Elif, "#elif defined(__linux__)", true)],
                    true,
                )],
            ),
            true,
        ),
        (
            row(
                "windows-only-elif.c",
                5,
                vec![chain(
                    vec![win()],
                    vec![frame(Elif, "#elif defined(_WIN64)", true)],
                    true,
                )],
            ),
            false,
        ),
        (
            Row {
                callee: "RegOpenKeyExW",
                ..row(
                    "windows-only-elif.c",
                    7,
                    vec![chain(
                        vec![
                            frame(Ifdef, "#ifdef _WIN32", true),
                            frame(Elif, "#elif defined(_WIN64)", true),
                        ],
                        Vec::new(),
                        true,
                    )],
                )
            },
            false,
        ),
        (
            row(
                "nested-under-non-windows.c",
                8,
                vec![
                    chain(
                        vec![frame(Ifdef, "#ifdef USE_REG", true)],
                        Vec::new(),
                        false,
                    ),
                    chain(vec![win()], vec![other(true)], true),
                ],
            ),
            true,
        ),
        (
            row(
                "windows-nested-no-alternative.c",
                6,
                vec![
                    chain(
                        vec![frame(Ifndef, "#ifndef NOREG", true)],
                        vec![other(true)],
                        true,
                    ),
                    chain(vec![win()], Vec::new(), false),
                ],
            ),
            false,
        ),
        (
            row(
                "windows-in-windows.c",
                6,
                vec![
                    chain(vec![win()], Vec::new(), false),
                    chain(
                        vec![frame(Ifdef, "#ifdef _WIN64", true)],
                        vec![other(true)],
                        true,
                    ),
                ],
            ),
            false,
        ),
        (
            row(
                "corpus-macro.c",
                7,
                vec![chain(
                    vec![frame(Ifdef, "#ifdef MY_WIN", true)],
                    vec![other(true)],
                    true,
                )],
            ),
            false,
        ),
    ]
}

fn row_fact(row: &Row) -> Fact {
    Fact::Call {
        file_kind: FileKind::Source,
        location: at(row.fixture, row.line, function("read_setting")),
        callee: t(row.callee),
        args: Vec::new(),
        guards: row.guards.clone(),
    }
}

fn windows_corpus(dir: &str) -> Corpus {
    let mut corpus = load_dir(&corpora().join(dir)).unwrap();
    corpus.rules.retain(|r| r.id.as_str() == "eval/windows-api");
    assert_eq!(corpus.rules.len(), 1);
    corpus
}

#[test]
fn the_matrix_rows_mirror_the_windows_branch_fixtures() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(FIXTURES);
    let mut named: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    named.sort();
    let mut covered: Vec<String> = windows_matrix()
        .iter()
        .map(|(row, _)| row.fixture.to_owned())
        .collect();
    covered.sort();
    covered.dedup();
    assert_eq!(named, covered);
    for (row, _) in windows_matrix() {
        let source = std::fs::read_to_string(dir.join(row.fixture)).unwrap();
        let lines: Vec<&str> = source.lines().collect();
        let line = lines[row.line as usize - 1];
        assert!(
            line.contains(&format!("{}(", row.callee)),
            "{}:{}",
            row.fixture,
            row.line
        );
        for chain in &row.guards {
            for frame in chain.frames.iter().chain(&chain.later) {
                let text = frame.text.as_str().unwrap();
                assert!(lines.contains(&text), "{}: {text}", row.fixture);
            }
            chain.validate().unwrap();
        }
    }
}

#[test]
fn each_guard_form_is_a_windows_branch_or_a_finding() {
    let corpus = windows_corpus("eval");
    for (row, branch) in windows_matrix() {
        let output = evaluate(&corpus, &[row_fact(&row)]);
        let (findings, branches) = (output.findings.len(), output.observations.len());
        let want = if branch { (0, 1) } else { (1, 0) };
        assert_eq!((findings, branches), want, "{}:{}", row.fixture, row.line);
    }
}

#[test]
fn a_windows_branch_observation_carries_location_guards_condition_behaviour_and_api() {
    let corpus = windows_corpus("eval");
    let matrix = windows_matrix();
    let conditions: Vec<(&str, Option<String>)> = matrix
        .iter()
        .filter(|(_, branch)| *branch)
        .map(|(row, _)| {
            let fact = row_fact(row);
            let output = evaluate(&corpus, std::slice::from_ref(&fact));
            match &output.observations[0] {
                Observation::WindowsBranch {
                    location,
                    guards,
                    condition,
                    behaviour,
                    api,
                } => {
                    assert_eq!(location, fact.location());
                    assert_eq!(guards.as_slice(), fact.guards());
                    assert_eq!(behaviour.as_str(), "c");
                    assert_eq!(api.as_ref().unwrap().as_str(), Some(row.callee));
                    (row.fixture, condition.as_str().map(str::to_owned))
                }
                other => panic!("{other:?}"),
            }
        })
        .collect();
    let s = |c: &str| Some(c.to_owned());
    assert_eq!(
        conditions,
        [
            ("plain-else.c", s("#ifdef _WIN32")),
            ("negated.c", s("#ifndef _WIN32, #else")),
            ("disjunction.c", s("#if defined(_WIN32) || defined(_WIN64)")),
            ("linux-elif.c", s("#ifdef _WIN32")),
            ("nested-under-non-windows.c", s("#ifdef _WIN32")),
        ]
    );
}

#[test]
fn a_macro_added_to_the_corpus_manifest_makes_a_windows_branch() {
    let row = windows_matrix()
        .into_iter()
        .find(|(row, _)| row.fixture == "corpus-macro.c")
        .unwrap()
        .0;
    let fact = row_fact(&row);
    let without = evaluate(&windows_corpus("eval"), std::slice::from_ref(&fact));
    assert_eq!((without.findings.len(), without.observations.len()), (1, 0));
    let with = evaluate(
        &windows_corpus("eval-windows-macro"),
        std::slice::from_ref(&fact),
    );
    assert_eq!((with.findings.len(), with.observations.len()), (0, 1));
}

#[test]
fn guard_forms_outside_the_listed_ones_are_neither_side() {
    use Directive::{Elif, Elifdef, Else, If, Ifdef, Ifndef};
    let corpus = windows_corpus("eval");
    let posix = || frame(Else, "#else", true);
    let class = |guards: Vec<GuardChain>| {
        let output = evaluate(
            &corpus,
            &[Fact::Call {
                file_kind: FileKind::Source,
                location: src(1),
                callee: t("RegOpenKeyExA"),
                args: Vec::new(),
                guards,
            }],
        );
        (output.findings.len(), output.observations.len())
    };
    for text in [
        "#if _WIN32",
        "#if (defined(_WIN32))",
        "#if defined _WIN32",
        "#if defined(_WIN32) || (_WIN64)",
    ] {
        assert_eq!(
            class(vec![chain(
                vec![frame(If, text, true)],
                vec![posix()],
                true
            )]),
            (0, 1),
            "{text}"
        );
    }
    for text in [
        "#if defined(_WIN32) && defined(X)",
        "#if defined(_WIN32) || defined(X)",
        "#if _WIN32 >= 1",
        "#if !defined(_WIN32)",
        "#if",
    ] {
        assert_eq!(
            class(vec![chain(
                vec![frame(If, text, true)],
                vec![posix()],
                true
            )]),
            (1, 0),
            "{text}"
        );
    }
    for text in ["#if !defined(_WIN32)", "#if !_WIN32", "#ifndef _WIN32"] {
        let directive = if text.starts_with("#ifndef") {
            Ifndef
        } else {
            If
        };
        assert_eq!(
            class(vec![chain(
                vec![frame(directive, text, true), posix()],
                Vec::new(),
                true
            )]),
            (0, 1),
            "{text}"
        );
    }
    assert_eq!(
        class(vec![chain(
            vec![
                frame(Ifdef, "#ifdef X", true),
                frame(Elifdef, "#elifdef _WIN32", true)
            ],
            Vec::new(),
            true
        )]),
        (0, 1)
    );
    assert_eq!(
        class(vec![chain(
            vec![frame(Ifdef, "#ifdef _WIN32", true), posix()],
            Vec::new(),
            true
        )]),
        (1, 0)
    );
    assert_eq!(
        class(vec![chain(
            vec![frame(Ifdef, "#ifdef _WIN32", true)],
            vec![
                frame(Elif, "#elif defined(_WIN64)", true),
                frame(Else, "#else", true)
            ],
            true
        )]),
        (0, 1)
    );
}

fn classify(guards: Vec<GuardChain>) -> (usize, Vec<String>) {
    let output = evaluate(
        &windows_corpus("eval"),
        &[Fact::Call {
            file_kind: FileKind::Source,
            location: src(1),
            callee: t("RegOpenKeyExA"),
            args: Vec::new(),
            guards,
        }],
    );
    let conditions = output
        .observations
        .iter()
        .map(|o| match o {
            Observation::WindowsBranch { condition, .. } => condition.as_str().unwrap().to_owned(),
            other => panic!("{other:?}"),
        })
        .collect();
    (output.findings.len(), conditions)
}

fn branch(condition: &str) -> (usize, Vec<String>) {
    (0, vec![condition.to_owned()])
}

const FINDING: (usize, Vec<String>) = (1, Vec::new());

#[test]
fn a_linux_branch_before_a_windows_elif_is_an_alternative() {
    use Directive::{Elif, Ifdef};
    assert_eq!(
        classify(vec![chain(
            vec![
                frame(Ifdef, "#ifdef __linux__", true),
                frame(Elif, "#elif defined(_WIN64)", true),
                frame(Elif, "#elif defined(_WIN32)", true),
            ],
            Vec::new(),
            true,
        )]),
        branch("#elif defined(_WIN32)")
    );
}

#[test]
fn branches_reached_only_after_a_windows_negative_frame_are_no_alternative() {
    use Directive::{Elif, Elifdef, Else, Ifdef, Ifndef};
    assert_eq!(
        classify(vec![chain(
            vec![
                frame(Ifndef, "#ifndef _WIN32", false),
                frame(Elifdef, "#elifdef X", true),
                frame(Else, "#else", true),
            ],
            Vec::new(),
            true,
        )]),
        FINDING
    );
    assert_eq!(
        classify(vec![chain(
            vec![
                frame(Ifdef, "#ifdef X", false),
                frame(Elif, "#elif !defined(_WIN32)", false),
                frame(Else, "#else", true),
            ],
            Vec::new(),
            false,
        )]),
        FINDING
    );
    assert_eq!(
        classify(vec![chain(
            vec![
                frame(Ifndef, "#ifndef _WIN32", true),
                frame(Elifdef, "#elifdef X", true),
                frame(Else, "#else", true),
            ],
            Vec::new(),
            true,
        )]),
        branch("#ifndef _WIN32, #else")
    );
}

#[test]
fn elifndef_is_windows_negative_like_elif_not_defined() {
    use Directive::{Elif, Elifndef, Else, Ifdef};
    for (directive, text) in [
        (Elifndef, "#elifndef _WIN32"),
        (Elif, "#elif !defined(_WIN32)"),
    ] {
        assert_eq!(
            classify(vec![chain(
                vec![
                    frame(Ifdef, "#ifdef X", false),
                    frame(directive, text, true),
                    frame(Else, "#else", true),
                ],
                Vec::new(),
                true,
            )]),
            branch(&format!("{text}, #else")),
            "{text}"
        );
    }
}

#[test]
fn a_parenthesized_negation_is_windows_negative() {
    use Directive::{Else, If};
    assert_eq!(
        classify(vec![chain(
            vec![
                frame(If, "#if !(defined(_WIN32))", true),
                frame(Else, "#else", true),
            ],
            Vec::new(),
            true,
        )]),
        branch("#if !(defined(_WIN32)), #else")
    );
}

#[test]
fn the_first_of_two_negative_frames_supplies_the_condition() {
    use Directive::{Elifndef, Else, Ifndef};
    assert_eq!(
        classify(vec![chain(
            vec![
                frame(Ifndef, "#ifndef _WIN32", true),
                frame(Elifndef, "#elifndef _WIN64", false),
                frame(Else, "#else", true),
            ],
            Vec::new(),
            true,
        )]),
        branch("#ifndef _WIN32, #else")
    );
}

#[test]
fn the_outermost_qualifying_chain_supplies_the_condition() {
    use Directive::{Else, If, Ifdef};
    assert_eq!(
        classify(vec![
            chain(
                vec![frame(If, "#if defined(_WIN64)", true)],
                vec![frame(Else, "#else", true)],
                true,
            ),
            chain(
                vec![frame(Ifdef, "#ifdef _WIN32", true)],
                vec![frame(Else, "#else", true)],
                true,
            ),
        ]),
        branch("#if defined(_WIN64)")
    );
}

#[test]
fn deeply_nested_parentheses_are_neither_side() {
    use Directive::{Else, If};
    let depth = 100_000;
    let text = format!(
        "#if {}defined(_WIN32){}",
        "(".repeat(depth),
        ")".repeat(depth)
    );
    assert_eq!(
        classify(vec![chain(
            vec![frame(If, &text, true)],
            vec![frame(Else, "#else", true)],
            true,
        )]),
        FINDING
    );
    let shallow = format!("#if {}defined(_WIN32){}", "(".repeat(8), ")".repeat(8));
    assert_eq!(
        classify(vec![chain(
            vec![frame(If, &shallow, true)],
            vec![frame(Else, "#else", true)],
            true,
        )]),
        branch(&shallow)
    );
}

#[test]
fn a_found_hit_that_became_a_windows_branch_still_suppresses_its_suspected_hit() {
    let fact = Fact::Call {
        file_kind: FileKind::Source,
        location: src(1),
        callee: t("RegOpenKeyExA"),
        args: Vec::new(),
        guards: vec![chain(
            vec![frame(Directive::Ifdef, "#ifdef _WIN32", true)],
            vec![frame(Directive::Else, "#else", true)],
            true,
        )],
    };
    let output = evaluate(&only("windows-found-suspected"), &[fact]);
    assert!(output.findings.is_empty(), "{:?}", output.findings);
    assert_eq!(output.observations.len(), 1);
}

#[test]
fn a_negated_windows_disjunction_or_conjunction_is_windows_negative() {
    use Directive::{Else, If};
    for text in [
        "#if !(defined(_WIN32) || defined(_WIN64))",
        "#if !defined(_WIN32) && !defined(_WIN64)",
        "#if !_WIN32 && !(defined(_WIN64))",
    ] {
        assert_eq!(
            classify(vec![chain(
                vec![frame(If, text, true), frame(Else, "#else", true)],
                Vec::new(),
                true,
            )]),
            branch(&format!("{text}, #else")),
            "{text}"
        );
    }
    for text in [
        "#if !(defined(_WIN32) || defined(X))",
        "#if !defined(_WIN32) && defined(X)",
        "#if !defined(_WIN32) && !defined(X)",
        "#if !defined(_WIN32) || !defined(_WIN64)",
    ] {
        assert_eq!(
            classify(vec![chain(
                vec![frame(If, text, true), frame(Else, "#else", true)],
                Vec::new(),
                true,
            )]),
            FINDING,
            "{text}"
        );
    }
}

#[test]
fn an_else_inside_a_windows_only_block_is_no_alternative() {
    use Directive::{Else, Ifdef};
    assert_eq!(
        classify(vec![
            chain(vec![frame(Ifdef, "#ifdef _WIN32", true)], Vec::new(), false),
            chain(
                vec![frame(Ifdef, "#ifdef _WIN64", true)],
                vec![frame(Else, "#else", true)],
                true,
            ),
        ]),
        FINDING
    );
}

#[test]
fn nesting_is_counted_to_64_on_both_paths() {
    use Directive::{Else, If};
    let wrapped =
        |depth: usize| format!("{}defined(_WIN32){}", "(".repeat(depth), ")".repeat(depth));
    for (depth, positive) in [(64, true), (65, false)] {
        let text = format!("#if {}", wrapped(depth));
        assert_eq!(
            classify(vec![chain(
                vec![frame(If, &text, true)],
                vec![frame(Else, "#else", true)],
                true,
            )]),
            if positive { branch(&text) } else { FINDING },
            "positive at {depth}"
        );
        let text = format!("#if !{}", wrapped(depth));
        assert_eq!(
            classify(vec![chain(
                vec![frame(If, &text, true), frame(Else, "#else", true)],
                Vec::new(),
                true,
            )]),
            if positive {
                branch(&format!("{text}, #else"))
            } else {
                FINDING
            },
            "negated at {depth}"
        );
    }
}

#[test]
fn a_fact_that_is_not_a_call_becomes_a_windows_branch_with_no_api() {
    let fact = Fact::StringLiteral {
        file_kind: FileKind::Source,
        location: src(4),
        value: t("SOFTWARE\\App"),
        guards: vec![chain(
            vec![frame(Directive::Ifdef, "#ifdef _WIN32", true)],
            vec![frame(Directive::Else, "#else", true)],
            true,
        )],
    };
    let output = evaluate(&only("windows-literal"), &[fact]);
    assert!(output.findings.is_empty());
    match output.observations.as_slice() {
        [Observation::WindowsBranch { api, condition, .. }] => {
            assert_eq!(*api, None);
            assert_eq!(condition.as_str(), Some("#ifdef _WIN32"));
        }
        other => panic!("{other:?}"),
    }
}
