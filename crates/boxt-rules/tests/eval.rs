mod common;

use boxt_core::Text;
use boxt_core::fact::{Directive, Fact, FileKind, GuardChain, GuardFrame, RegionId};
use boxt_core::location::{
    Frame, FrameKind, InputRef, Location, PathText, RelPath, Scope, ScriptName,
};
use boxt_core::verdict::{Confidence, Finding, Observation};
use boxt_rules::{Corpus, Matched, evaluate, load_dir};

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
            },
            GuardFrame {
                directive: Directive::Else,
                text: t("#else"),
                identifiers: Vec::new().into(),
                holds_code: true,
            },
        ],
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
    let first: Matched = evaluate(&corpus, &facts);
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
