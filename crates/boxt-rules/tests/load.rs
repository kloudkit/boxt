mod common;

use boxt_core::fact::{FactKind, FieldType};
use boxt_core::pattern;
use boxt_core::verdict::{BehaviourId, Confidence, ControlId, IdError, TargetId, Tier};
use boxt_core::version::{SchemaVersion, UnsupportedReason, UnsupportedVersion};
use boxt_rules::{
    COMMON_TEST_FIELDS, CommonField, CorpusVersion, FieldPath, FieldTest, Literal, LoadError,
    MAX_DEPTH, MAX_FILE_BYTES, MAX_FOLDER_DEPTH, Origin, Problem, Test, load_dir, load_tar,
    load_user_dir, pack,
};

use common::*;

fn behaviour(id: &str) -> BehaviourId {
    BehaviourId::new(id).unwrap()
}

#[test]
fn a_valid_corpus_loads_its_manifest_rules_and_profiles() {
    let corpus = load_dir(&corpora().join("valid")).unwrap();
    assert_eq!(corpus.origin, Origin::Corpus);
    assert_eq!(
        corpus.manifest.corpus_version,
        CorpusVersion::parse("1.2.3").unwrap()
    );
    assert_eq!(
        corpus
            .manifest
            .behaviours
            .iter()
            .map(|b| b.id.as_str())
            .collect::<Vec<_>>(),
        ["fixed-port", "needs-root"]
    );
    assert_eq!(corpus.manifest.windows_guard_macros, ["_WIN32", "_MSC_VER"]);
    assert_eq!(
        corpus.manifest.dynamic_load_symbols,
        ["dlopen", "LoadLibraryA"]
    );

    let ids: Vec<&str> = corpus.rules.iter().map(|r| r.id.as_str()).collect();
    assert_eq!(ids, ["identity/needs-root-setuid", "network/low-port"]);
    let setuid = &corpus.rules[0];
    assert_eq!(setuid.file, rel("rules/identity/needs-root-setuid.yaml"));
    assert_eq!(setuid.behaviour, behaviour("needs-root"));
    assert_eq!(setuid.title, "Calls setuid(0)");
    assert_eq!(
        setuid.fix,
        "Run as a non-root user and drop the setuid call."
    );
    assert_eq!(setuid.matches.len(), 2);
    let found = &setuid.matches[0];
    assert_eq!(found.confidence, Confidence::Found);
    assert_eq!(found.evidence.id.as_str(), "setuid-zero");
    assert_eq!(found.matcher.kind, Some(FactKind::Call));
    let Test::All(tests) = &found.matcher.test else {
        panic!("{:?}", found.matcher.test)
    };
    assert_eq!(
        tests[1].test,
        Test::Field {
            field: FieldPath::Kind {
                name: "args",
                index: Some(0)
            },
            test: FieldTest::Equals(Literal::Text("0".into())),
        }
    );
    assert_eq!(setuid.matches[1].confidence, Confidence::Suspected);

    let port = &corpus.rules[1];
    let Test::Any(alternatives) = &port.matches[0].matcher.test else {
        panic!()
    };
    assert_eq!(alternatives.len(), 4);
    let Test::All(inner) = &alternatives[0].test else {
        panic!()
    };
    let Test::Not(not) = &inner[1].test else {
        panic!()
    };
    assert_eq!(
        not.test,
        Test::Field {
            field: FieldPath::Common(CommonField::LocationPath),
            test: FieldTest::PathUnder(pattern::path_under("tests").unwrap()),
        }
    );
    assert_eq!(inner[2].test, Test::GuardContains("HAVE_SOCKETS".into()));
    assert!(matches!(alternatives[1].test, Test::CoOccurs { .. }));
    assert_eq!(alternatives[3].kind, Some(FactKind::Import));
    assert_eq!(alternatives[3].test, Test::Exists);

    assert_eq!(corpus.profiles.len(), 1);
    let profile = &corpus.profiles[0];
    assert_eq!(
        profile.id,
        TargetId::new("openshift-restricted-v2").unwrap()
    );
    assert_eq!(profile.version, "2026.1");
    assert_eq!(profile.allowed.as_slice(), [behaviour("fixed-port")]);
    let controls = profile.controls.as_slice();
    assert_eq!(controls.len(), 2);
    assert_eq!(controls[0].id, ControlId::new("run-as-non-root").unwrap());
    assert_eq!(controls[0].behaviours[0].tier, Tier::Blocked);
    assert_eq!(controls[0].not_detectable, None);
    assert!(controls[1].behaviours.is_empty());
    assert!(controls[1].not_detectable.is_some());
}

#[test]
fn every_refusal_names_its_file_and_problem_and_all_are_reported() {
    let got = problems(load_dir(&corpora().join("refused")));
    let want: Vec<(String, Problem)> = vec![
        (
            "profiles/empty-control.yaml".into(),
            Problem::ControlWithoutBehaviours(ControlId::new("nothing").unwrap()),
        ),
        (
            "profiles/newer-major.yaml".into(),
            Problem::UnsupportedVersion(UnsupportedVersion {
                supported: SchemaVersion::new(1, 0),
                found: SchemaVersion::new(2, 0),
                reason: UnsupportedReason::OtherMajor,
            }),
        ),
        (
            "rules/names-target.yaml".into(),
            Problem::NamesTarget("targets"),
        ),
        ("rules/names-tier.yaml".into(), Problem::NamesTier("tier")),
        (
            "rules/newer-schema.yaml".into(),
            Problem::UnsupportedVersion(UnsupportedVersion {
                supported: SchemaVersion::new(1, 0),
                found: SchemaVersion::new(1, 1),
                reason: UnsupportedReason::NewerMinor,
            }),
        ),
        (
            "rules/no-evidence.yaml".into(),
            Problem::ConfidenceWithoutEvidence(Confidence::Suspected),
        ),
        ("rules/no-fix.yaml".into(), Problem::NoFix),
        ("rules/no-title-no-fix.yaml".into(), Problem::NoTitle),
        ("rules/no-title-no-fix.yaml".into(), Problem::NoFix),
        (
            "rules/unknown-behaviour.yaml".into(),
            Problem::UnknownBehaviour(behaviour("needs-roots")),
        ),
        (
            "rules/unknown-field.yaml".into(),
            Problem::UnknownField {
                kind: FactKind::Call,
                field: "calee".into(),
            },
        ),
        (
            "rules/unknown-key.yaml".into(),
            Problem::UnknownKey("weight".into()),
        ),
        (
            "rules/unknown-kind.yaml".into(),
            Problem::UnknownFactKind("syscall".into()),
        ),
        (
            "rules/user-behaviour.yaml".into(),
            Problem::UserBehaviourOutsideUserRegistry(behaviour("user/needs-root")),
        ),
    ];
    assert_eq!(got, want);
}

#[test]
fn a_refused_version_names_both_versions() {
    let errors = refused(load_dir(&corpora().join("refused")));
    let newer = errors
        .iter()
        .find(|e| e.file == rel("rules/newer-schema.yaml"))
        .unwrap()
        .to_string();
    assert!(newer.contains("1.1") && newer.contains("1.0"), "{newer}");
}

#[test]
fn a_tier_or_target_key_is_its_own_problem_not_an_unknown_key() {
    let base = good_rule("identity/x");
    for key in ["target", "targets", "tier", "tiers"] {
        let rule = base.replace("title: T", &format!("title: T\n{key}: ~"));
        let want = if key.starts_with("target") {
            Problem::NamesTarget(key)
        } else {
            Problem::NamesTier(key)
        };
        assert_eq!(single(&rule), [("rules/r.yaml".to_owned(), want)], "{key}");
    }
}

#[test]
fn match_node_shape_is_checked() {
    let with = |found: &str| {
        good_rule("identity/x").replace(
            "found: { kind: call, field: callee, equals: setuid }",
            &format!("found: {found}"),
        )
    };
    let misfit = |field: &str, field_type: FieldType, test: &'static str| Problem::TestDoesNotFit {
        field: field.into(),
        field_type,
        test,
    };
    let unknown = |field: &str| Problem::UnknownField {
        kind: FactKind::Call,
        field: field.into(),
    };
    let cases: Vec<(&str, Problem)> = vec![
        ("{ field: callee, equals: x }", Problem::NoKind),
        ("{ kind: call, field: callee }", Problem::FieldWithoutTest),
        ("{ kind: call, equals: x }", Problem::TestWithoutField),
        (
            "{ kind: call, field: callee, equals: x, pattern: y }",
            Problem::SeveralTests,
        ),
        ("{ any: [] }", Problem::EmptyList("any")),
        ("{ all: [] }", Problem::EmptyList("all")),
        (
            "{ co_occurs: { scope: file, all: [] } }",
            Problem::EmptyList("co_occurs.all"),
        ),
        (
            "{ kind: call, field: callee, one_of: [] }",
            Problem::EmptyList("one_of"),
        ),
        ("{ not: { field: callee, equals: x } }", Problem::NoKind),
        ("{ all: [ { not: {} } ] }", Problem::NoTest),
        (
            "{ kind: import, guard_contains: X }",
            misfit("guards", FieldType::Guards, "guard_contains"),
        ),
        (
            "{ kind: call, field: location, equals: x }",
            unknown("location"),
        ),
        (
            "{ kind: call, field: args.01, equals: x }",
            unknown("args.01"),
        ),
        (
            "{ kind: call, field: callee.0, equals: x }",
            unknown("callee.0"),
        ),
        (
            "{ kind: call, field: guards.0, equals: x }",
            unknown("guards.0"),
        ),
        (
            "{ kind: call, field: location.column, equals: x }",
            unknown("location.column"),
        ),
        (
            "{ kind: call, field: callee, number: { op: \"<\", value: 3 } }",
            misfit("callee", FieldType::Text, "number"),
        ),
        (
            "{ kind: call, field: callee, path_under: src }",
            misfit("callee", FieldType::Text, "path_under"),
        ),
        (
            "{ kind: call, field: callee, equals: 1 }",
            misfit("callee", FieldType::Text, "equals"),
        ),
        (
            "{ kind: call, field: callee, one_of: [a, true] }",
            misfit("callee", FieldType::Text, "one_of"),
        ),
        (
            "{ kind: call, field: args, equals: x }",
            misfit("args", FieldType::TextList, "equals"),
        ),
        (
            "{ kind: call, field: guards, pattern: x }",
            misfit("guards", FieldType::Guards, "pattern"),
        ),
        (
            "{ kind: include, field: system, equals: \"true\" }",
            Problem::TestDoesNotFit {
                field: "system".into(),
                field_type: FieldType::Bool,
                test: "equals",
            },
        ),
        (
            "{ kind: shell_command, field: assignments, equals: x }",
            Problem::TestDoesNotFit {
                field: "assignments".into(),
                field_type: FieldType::Records,
                test: "equals",
            },
        ),
        (
            "{ kind: call, field: location.line, path_under: x }",
            misfit("location.line", FieldType::Integer, "path_under"),
        ),
        (
            "{ kind: call, field: callee, pattern: \" \" }",
            Problem::BlankValue("pattern"),
        ),
        (
            "{ kind: call, field: location.path, path_under: \"\" }",
            Problem::BlankValue("path_under"),
        ),
        (
            "{ kind: call, guard_contains: \"\" }",
            Problem::BlankValue("guard_contains"),
        ),
    ];
    for (node, want) in cases {
        let got = single(&with(node));
        assert_eq!(got, [("rules/r.yaml".to_owned(), want)], "{node}");
    }
    let bad = single(&with("{ kind: call, field: callee, pattern: \"[a\" }"));
    assert!(
        matches!(&bad[..], [(_, Problem::BadPattern { key: "pattern", .. })]),
        "{bad:?}"
    );
}

#[test]
fn every_field_accepts_the_tests_its_type_allows() {
    for kind in FactKind::ALL {
        let fields = kind
            .fields()
            .iter()
            .map(|f| (f.name, f.ty))
            .chain(COMMON_TEST_FIELDS.iter().map(|(_, n, ty)| (*n, *ty)));
        for (field, ty) in fields {
            let tests: Vec<String> = match ty {
                FieldType::Text => vec![
                    format!("field: {field}, equals: x"),
                    format!("field: {field}, one_of: [x, y]"),
                    format!("field: {field}, pattern: \"x*\""),
                ],
                FieldType::Path => vec![
                    format!("field: {field}, equals: x"),
                    format!("field: {field}, pattern: \"src/*.c\""),
                    format!("field: {field}, path_under: src"),
                ],
                FieldType::Integer => vec![
                    format!("field: {field}, equals: 3"),
                    format!("field: {field}, number: {{ op: \">=\", value: 3 }}"),
                ],
                FieldType::Bool => vec![format!("field: {field}, equals: true")],
                FieldType::TextList => vec![format!("field: {field}.0, equals: x")],
                FieldType::Guards => vec!["guard_contains: X".to_owned()],
                FieldType::Records => vec![],
            };
            for test in tests {
                let rule = good_rule("identity/x").replace(
                    "found: { kind: call, field: callee, equals: setuid }",
                    &format!("found: {{ kind: {}, {test} }}", kind.name()),
                );
                assert!(single(&rule).is_empty(), "{} {test}", kind.name());
            }
        }
    }
}

fn with_kind_only() -> String {
    good_rule("identity/x").replace(
        "found: { kind: call, field: callee, equals: setuid }",
        "found: { all: [ { not: { kind: call } } ] }",
    )
}

#[test]
fn ids_are_checked_and_duplicates_refused() {
    let dir = scratch("ids");
    write(&dir, "manifest.yaml", MANIFEST);
    write(&dir, "rules/a.yaml", good_rule("identity/same"));
    write(&dir, "rules/b.yaml", good_rule("identity/same"));
    write(&dir, "rules/c.yaml", good_rule("single"));
    let got = problems(load_dir(&dir));
    assert_eq!(
        got,
        [
            (
                "rules/b.yaml".to_owned(),
                Problem::DuplicateId("identity/same".into())
            ),
            (
                "rules/c.yaml".to_owned(),
                Problem::BadId {
                    key: "id",
                    value: "single".into(),
                    error: IdError::TooFewSegments
                }
            ),
        ]
    );
}

#[test]
fn missing_and_unparsable_schema_versions_are_refused() {
    let rule = good_rule("identity/x");
    assert_eq!(
        single(&rule.replace("schema: \"1.0\"\n", "")),
        [("rules/r.yaml".to_owned(), Problem::NoSchemaVersion)]
    );
    assert_eq!(
        single(&rule.replace("schema: \"1.0\"", "schema: \"one\"")),
        [(
            "rules/r.yaml".to_owned(),
            Problem::BadSchemaVersion("one".into())
        )]
    );
    assert!(single(&rule.replace("schema: \"1.0\"", "schema: 1.0")).is_empty());
    assert!(single(&with_kind_only()).is_empty());
}

#[test]
fn a_document_too_deep_or_too_big_is_refused() {
    let dir = scratch("limits");
    write(&dir, "manifest.yaml", MANIFEST);
    let mut deep = String::from(
        "schema: \"1.0\"\nid: identity/deep\nbehaviour: needs-root\ntitle: T\nfix: F\nmatch:\n  found: ",
    );
    for _ in 0..100 {
        deep.push_str("{ not: ");
    }
    deep.push_str("{ kind: call }");
    for _ in 0..100 {
        deep.push_str(" }");
    }
    deep.push('\n');
    write(&dir, "rules/deep.yaml", &deep);
    let mut block = String::from("schema: \"1.0\"\nid: identity/deep-block\n");
    for depth in 0..100 {
        block.push_str(&" ".repeat(depth * 2));
        block.push_str("k:\n");
    }
    write(&dir, "rules/deep-block.yaml", &block);
    let big = format!(
        "{}# {}\n",
        good_rule("identity/big"),
        "x".repeat(2 * 1024 * 1024)
    );
    write(&dir, "rules/big.yaml", &big);
    let got = problems(load_dir(&dir));
    assert_eq!(
        got,
        [
            (
                "rules/big.yaml".to_owned(),
                Problem::TooLarge {
                    size: big.len() as u64,
                    limit: MAX_FILE_BYTES
                }
            ),
            (
                "rules/deep-block.yaml".to_owned(),
                Problem::TooDeep { limit: MAX_DEPTH }
            ),
            (
                "rules/deep.yaml".to_owned(),
                Problem::TooDeep { limit: MAX_DEPTH }
            ),
        ]
    );
    let tar = pack(&dir).unwrap();
    assert_eq!(problems(load_tar(&tar)), got);
}

#[test]
fn malformed_yaml_and_bad_files_are_refused() {
    let dir = scratch("malformed");
    write(&dir, "manifest.yaml", MANIFEST);
    write(&dir, "rules/bad.yaml", "schema: \"1.0\"\nid: [unclosed\n");
    write(
        &dir,
        "rules/two.yaml",
        format!(
            "{}---\n{}",
            good_rule("identity/a"),
            good_rule("identity/b")
        ),
    );
    write(
        &dir,
        "rules/dup.yaml",
        format!("{}title: Again\n", good_rule("identity/dup")),
    );
    write(
        &dir,
        "rules/latin1.yaml",
        b"schema: \"1.0\"\ntitle: caf\xe9\n",
    );
    write(&dir, "rules/notes.txt", "not a rule");
    write(&dir, "rules/.gitkeep", "");
    write(&dir, "profiles/nested/p.yaml", "schema: \"1.0\"\n");
    write(&dir, "schema/anything.json", "{}");
    write(&dir, "README.md", "ignored");
    let got = problems(load_dir(&dir));
    let files: Vec<&str> = got.iter().map(|(f, _)| f.as_str()).collect();
    assert_eq!(
        files,
        [
            "profiles/nested/p.yaml",
            "rules/bad.yaml",
            "rules/dup.yaml",
            "rules/latin1.yaml",
            "rules/notes.txt",
            "rules/two.yaml",
        ]
    );
    assert_eq!(got[0].1, Problem::UnexpectedFile);
    assert!(
        matches!(got[1].1, Problem::Malformed { line: 2.., .. }),
        "{:?}",
        got[1]
    );
    assert!(
        matches!(got[2].1, Problem::Malformed { .. }),
        "{:?}",
        got[2]
    );
    assert_eq!(got[3].1, Problem::NotUtf8);
    assert_eq!(got[4].1, Problem::UnexpectedFile);
    assert!(
        matches!(got[5].1, Problem::Malformed { .. }),
        "{:?}",
        got[5]
    );
}

#[test]
fn merge_keys_cannot_bring_in_unknown_fields() {
    let rule = format!("base: &base {{ weight: 1 }}\n{}", good_rule("identity/m"));
    assert_eq!(
        single(&rule),
        [(
            "rules/r.yaml".to_owned(),
            Problem::UnknownKey("base".into())
        )]
    );
    let merged = good_rule("identity/m").replace(
        "match:\n  found: { kind: call, field: callee, equals: setuid }",
        "x: &x { weight: 2 }\nmatch:\n  found: { <<: *x, kind: call, field: callee, equals: setuid }",
    );
    let got = single(&merged);
    assert_eq!(got.len(), 1, "{got:?}");
    assert!(matches!(got[0].1, Problem::UnknownKey(_)), "{got:?}");
}

#[test]
fn a_manifest_is_checked() {
    let dir = scratch("manifest");
    write(
        &dir,
        "manifest.yaml",
        "corpus_version: 1.0\nbehaviours:\n  - { id: user/x, dimension: identity }\n  - { id: a, dimension: d }\n  - { id: a, dimension: d }\n",
    );
    write(&dir, "registry.yaml", "corpus_version: 0.0.0\n");
    let got = problems(load_dir(&dir));
    assert_eq!(
        got,
        [
            (
                "manifest.yaml".to_owned(),
                Problem::BadCorpusVersion("1.0".into())
            ),
            (
                "manifest.yaml".to_owned(),
                Problem::UserBehaviourOutsideUserRegistry(behaviour("user/x"))
            ),
            ("manifest.yaml".to_owned(), Problem::DuplicateId("a".into())),
            ("registry.yaml".to_owned(), Problem::UnexpectedFile),
        ]
    );
}

#[test]
fn a_user_folder_uses_its_own_user_registry() {
    let base = load_dir(&corpora().join("valid")).unwrap();
    let user = load_user_dir(&corpora().join("user"), &base).unwrap();
    assert_eq!(user.origin, Origin::User);
    assert_eq!(user.user_version, CorpusVersion::parse("0.1.0"));
    assert_eq!(
        user.rules
            .iter()
            .map(|r| r.behaviour.as_str())
            .collect::<Vec<_>>(),
        ["needs-root", "user/site-daemon"]
    );
    assert_eq!(
        user.manifest
            .behaviours
            .iter()
            .map(|b| b.id.as_str())
            .collect::<Vec<_>>(),
        ["fixed-port", "needs-root", "user/site-daemon"]
    );
    assert_eq!(
        user.manifest.windows_guard_macros,
        base.manifest.windows_guard_macros
    );
    assert_eq!(
        user.manifest.dynamic_load_symbols,
        base.manifest.dynamic_load_symbols
    );
    assert_eq!(user.manifest.corpus_version, base.manifest.corpus_version);

    let empty_base = load_dir(&{
        let d = scratch("empty-base");
        write(&d, "manifest.yaml", "corpus_version: 0.0.0\n");
        d
    })
    .unwrap();
    assert_eq!(
        problems(load_user_dir(&corpora().join("user"), &empty_base)),
        [(
            "rules/root.yaml".to_owned(),
            Problem::UnknownBehaviour(behaviour("needs-root"))
        )]
    );

    let dir = scratch("user-registry");
    write(
        &dir,
        "registry.yaml",
        "behaviours:\n  - { id: needs-root, dimension: identity }\n",
    );
    assert_eq!(
        problems(load_user_dir(&dir, &base)),
        [(
            "registry.yaml".to_owned(),
            Problem::NotUserBehaviour(behaviour("needs-root"))
        )]
    );

    let empty = scratch("user-empty");
    let corpus = load_user_dir(&empty, &base).unwrap();
    assert_eq!(corpus.origin, Origin::User);
    assert_eq!(corpus.user_version, None);
    assert!(corpus.rules.is_empty());
}

#[test]
fn the_caller_decides_corpus_or_user() {
    let base = load_dir(&corpora().join("valid")).unwrap();
    let empty = scratch("origin-empty");
    assert_eq!(
        problems(load_dir(&empty)),
        [("manifest.yaml".to_owned(), Problem::NoManifest)]
    );
    assert_eq!(
        problems(load_tar(&pack(&empty).unwrap())),
        [("manifest.yaml".to_owned(), Problem::NoManifest)]
    );
    let user = scratch("origin-user-manifest");
    write(&user, "manifest.yaml", MANIFEST);
    assert_eq!(
        problems(load_user_dir(&user, &base)),
        [("manifest.yaml".to_owned(), Problem::UnexpectedFile)]
    );
    assert_eq!(
        problems(load_dir(&corpora().join("user"))),
        [
            ("manifest.yaml".to_owned(), Problem::NoManifest),
            ("registry.yaml".to_owned(), Problem::UnexpectedFile),
            (
                "rules/root.yaml".to_owned(),
                Problem::UnknownBehaviour(behaviour("needs-root"))
            ),
            (
                "rules/site-daemon.yaml".to_owned(),
                Problem::UserBehaviourOutsideUserRegistry(behaviour("user/site-daemon"))
            ),
        ]
    );
}

#[test]
fn user_ids_need_the_user_prefix_and_may_not_collide() {
    let base = load_dir(&corpora().join("valid")).unwrap();
    let dir = scratch("user-ids");
    write(
        &dir,
        "rules/plain.yaml",
        good_rule("identity/needs-root-setuid"),
    );
    write(&dir, "rules/fine.yaml", good_rule("user/fine"));
    write(
        &dir,
        "profiles/plain.yaml",
        "schema: \"1.0\"\nid: openshift-restricted-v2\nversion: \"1\"\n",
    );
    write(
        &dir,
        "profiles/fine.yaml",
        "schema: \"1.0\"\nid: user/site\nversion: \"1\"\n",
    );
    assert_eq!(
        problems(load_user_dir(&dir, &base)),
        [
            (
                "profiles/plain.yaml".to_owned(),
                Problem::NotUserId("openshift-restricted-v2".into())
            ),
            (
                "profiles/plain.yaml".to_owned(),
                Problem::CollidesWithCorpus("openshift-restricted-v2".into())
            ),
            (
                "rules/plain.yaml".to_owned(),
                Problem::NotUserId("identity/needs-root-setuid".into())
            ),
            (
                "rules/plain.yaml".to_owned(),
                Problem::CollidesWithCorpus("identity/needs-root-setuid".into())
            ),
        ]
    );
}

#[cfg(unix)]
#[test]
fn a_symlinked_rule_in_a_user_folder_is_not_a_regular_file() {
    let base = load_dir(&corpora().join("valid")).unwrap();
    let dir = scratch("user-symlink");
    write(&dir, "elsewhere.txt", good_rule("user/linked"));
    std::fs::create_dir_all(dir.join("rules")).unwrap();
    std::os::unix::fs::symlink(dir.join("elsewhere.txt"), dir.join("rules/linked.yaml")).unwrap();
    assert_eq!(
        problems(load_user_dir(&dir, &base)),
        [("rules/linked.yaml".to_owned(), Problem::NotRegularFile)]
    );
}

#[test]
fn a_tar_and_its_folder_load_alike() {
    for name in ["valid", "refused"] {
        let dir = corpora().join(name);
        let tar = pack(&dir).unwrap();
        match (load_dir(&dir), load_tar(&tar)) {
            (Ok(a), Ok(b)) => assert_eq!(a, b),
            (Err(LoadError::Refused(a)), Err(LoadError::Refused(b))) => assert_eq!(a, b),
            (a, b) => panic!("{name}: {a:?} vs {b:?}"),
        }
    }
}

#[test]
fn a_missing_root_is_a_root_error() {
    let missing = scratch("missing").join("nope");
    assert!(matches!(load_dir(&missing), Err(LoadError::Root(_))));
    let mut garbage = vec![0x55u8; 512];
    garbage.extend([0u8; 1024]);
    assert!(matches!(load_tar(&garbage), Err(LoadError::Tar(_))));
}

#[test]
fn corpus_versions_parse_and_order_as_semver() {
    for good in [
        "0.0.0",
        "1.2.3",
        "1.0.0-alpha.1",
        "1.0.0+build.5",
        "10.20.30-rc.1+x",
    ] {
        assert_eq!(CorpusVersion::parse(good).unwrap().to_string(), good);
    }
    for bad in [
        "1.0", "01.0.0", "1.0.0-", "1.0.0-01", "a.b.c", "1.0.0.0", "",
    ] {
        assert!(CorpusVersion::parse(bad).is_none(), "{bad}");
    }
    let order = [
        "1.0.0-alpha",
        "1.0.0-alpha.1",
        "1.0.0-alpha.beta",
        "1.0.0-beta",
        "1.0.0-beta.2",
        "1.0.0-beta.11",
        "1.0.0-rc.1",
        "1.0.0",
        "1.0.1",
        "1.1.0",
    ];
    for pair in order.windows(2) {
        let a = CorpusVersion::parse(pair[0]).unwrap();
        let b = CorpusVersion::parse(pair[1]).unwrap();
        assert_eq!(
            a.precedence(&b),
            std::cmp::Ordering::Less,
            "{} < {}",
            pair[0],
            pair[1]
        );
    }
}

fn rule_with(evidence: &str, matches: &str) -> String {
    format!(
        "schema: \"1.0\"\nid: identity/x\nbehaviour: needs-root\ntitle: T\nfix: F\n{evidence}{matches}"
    )
}

#[test]
fn match_and_evidence_must_pair_up() {
    let r = |p: Problem| vec![("rules/r.yaml".to_owned(), p)];
    let ev = "evidence:\n  found: { id: e, text: t }\n";
    assert_eq!(
        single(&rule_with(ev, "")),
        [
            ("rules/r.yaml".to_owned(), Problem::NoMatch),
            (
                "rules/r.yaml".to_owned(),
                Problem::EvidenceWithoutMatch(Confidence::Found)
            ),
        ]
    );
    assert_eq!(single(&rule_with("", "")), r(Problem::NoMatch));
    assert_eq!(
        single(&rule_with(ev, "match: {}\n")),
        [
            ("rules/r.yaml".to_owned(), Problem::NoMatch),
            (
                "rules/r.yaml".to_owned(),
                Problem::EvidenceWithoutMatch(Confidence::Found)
            ),
        ]
    );
    let found = "match:\n  found: { kind: call, field: callee, equals: setuid }\n";
    assert_eq!(
        single(&rule_with(
            "evidence:\n  found: { id: e, text: t }\n  suspected: { id: s, text: t }\n",
            found
        )),
        r(Problem::EvidenceWithoutMatch(Confidence::Suspected))
    );
    assert_eq!(
        single(&rule_with(
            "evidence:\n  found: { id: e, text: \" \" }\n",
            found
        )),
        r(Problem::NoEvidenceText(Confidence::Found))
    );
    assert_eq!(
        single(&rule_with(
            "evidence:\n  found: { id: e, text: t }\n  suspected: { id: e, text: u }\n",
            "match:\n  found: { kind: call, field: callee, equals: a }\n  suspected: { kind: call, field: callee, equals: b }\n"
        )),
        r(Problem::DuplicateId("e".into()))
    );
}

#[test]
fn a_tier_is_reported_alongside_an_unknown_key() {
    let rule = good_rule("identity/x").replace("title: T", "title: T\ntier: blocked\nweight: 2");
    assert_eq!(
        single(&rule),
        [
            ("rules/r.yaml".to_owned(), Problem::NamesTier("tier")),
            (
                "rules/r.yaml".to_owned(),
                Problem::UnknownKey("weight".into())
            ),
        ]
    );
}

fn profile(body: &str) -> String {
    format!("schema: \"1.0\"\nid: p\nversion: \"1\"\n{body}")
}

#[test]
fn profiles_are_checked() {
    let dir = scratch("profiles");
    write(&dir, "manifest.yaml", MANIFEST);
    write(
        &dir,
        "profiles/a.yaml",
        profile(
            "allowed: [needs-roots]\ncontrols:\n  - id: c\n    behaviours: [ { behaviour: nope, tier: blocked } ]\n",
        ),
    );
    write(&dir, "profiles/b.yaml", profile(""));
    write(&dir, "profiles/b2.yaml", profile(""));
    write(
        &dir,
        "profiles/c.yaml",
        "schema: \"1.0\"\nid: q\nversion: \" \"\nallowed: [needs-root]\ncontrols:\n  - id: c\n    behaviours: [ { behaviour: needs-root, tier: blocked } ]\n  - id: d\n    behaviours: [ { behaviour: needs-root, tier: ready } ]\n    not_detectable: Also set by the runtime.\n",
    );
    assert_eq!(
        problems(load_dir(&dir)),
        [
            (
                "profiles/a.yaml".to_owned(),
                Problem::UnknownBehaviour(behaviour("needs-roots"))
            ),
            (
                "profiles/a.yaml".to_owned(),
                Problem::UnknownBehaviour(behaviour("nope"))
            ),
            (
                "profiles/b2.yaml".to_owned(),
                Problem::DuplicateId("p".into())
            ),
            ("profiles/c.yaml".to_owned(), Problem::BlankValue("version")),
            (
                "profiles/c.yaml".to_owned(),
                Problem::AllowedAndTiered(behaviour("needs-root"))
            ),
            (
                "profiles/c.yaml".to_owned(),
                Problem::ControlTieredAndNotDetectable(ControlId::new("d").unwrap())
            ),
        ]
    );
}

#[test]
fn manifest_lists_hold_no_blank_or_repeated_entries() {
    let dir = scratch("manifest-lists");
    write(
        &dir,
        "manifest.yaml",
        "corpus_version: 1.0.0\nwindows_guard_macros: [_WIN32, \" \", _WIN32]\ndynamic_load_symbols: [\"\"]\n",
    );
    assert_eq!(
        problems(load_dir(&dir)),
        [
            (
                "manifest.yaml".to_owned(),
                Problem::BlankEntry("windows_guard_macros")
            ),
            (
                "manifest.yaml".to_owned(),
                Problem::DuplicateEntry {
                    key: "windows_guard_macros",
                    value: "_WIN32".into()
                }
            ),
            (
                "manifest.yaml".to_owned(),
                Problem::BlankEntry("dynamic_load_symbols")
            ),
        ]
    );
    let missing = scratch("manifest-no-version");
    write(&missing, "manifest.yaml", "behaviours: []\n");
    assert_eq!(
        problems(load_dir(&missing)),
        [(
            "manifest.yaml".to_owned(),
            Problem::MissingKey("corpus_version".into())
        )]
    );
}

#[test]
fn corpus_versions_compare_by_precedence() {
    use std::collections::BTreeSet;
    let a = CorpusVersion::parse("1.0.0+a").unwrap();
    let b = CorpusVersion::parse("1.0.0+b").unwrap();
    assert_eq!(a, b);
    assert_eq!(a.cmp(&b), std::cmp::Ordering::Equal);
    let hash = |v: &CorpusVersion| {
        use std::hash::{BuildHasher, BuildHasherDefault};
        BuildHasherDefault::<std::collections::hash_map::DefaultHasher>::default().hash_one(v)
    };
    assert_eq!(hash(&a), hash(&b));
    let sorted: Vec<String> = [
        "1.0.0",
        "1.0.0-rc.1",
        "1.0.0-beta.11",
        "1.0.0-beta.2",
        "0.9.9",
    ]
    .iter()
    .map(|s| CorpusVersion::parse(s).unwrap())
    .collect::<BTreeSet<_>>()
    .iter()
    .map(|v| v.to_string())
    .collect();
    assert_eq!(
        sorted,
        [
            "0.9.9",
            "1.0.0-beta.2",
            "1.0.0-beta.11",
            "1.0.0-rc.1",
            "1.0.0"
        ]
    );
}

#[test]
fn only_known_names_are_allowed_at_the_root() {
    let dir = scratch("root-names");
    write(&dir, "manifest.yaml", MANIFEST);
    write(&dir, "manifest.yml", MANIFEST);
    write(&dir, "extra.yaml", "a: 1\n");
    write(&dir, "rule/r.yaml", good_rule("identity/a"));
    write(&dir, "Rules/r.yaml", good_rule("identity/b"));
    write(&dir, "profile/p.yaml", "x: 1\n");
    write(&dir, "README.md", "ignored");
    write(&dir, "LICENSE", "ignored");
    write(&dir, "schema/rule-v9.9.schema.json", "{}");
    let want = vec![
        ("Rules".to_owned(), Problem::UnexpectedFolder),
        ("extra.yaml".to_owned(), Problem::UnexpectedFile),
        ("manifest.yml".to_owned(), Problem::UnexpectedFile),
        ("profile".to_owned(), Problem::UnexpectedFolder),
        ("rule".to_owned(), Problem::UnexpectedFolder),
    ];
    assert_eq!(problems(load_dir(&dir)), want);
    assert_eq!(problems(load_tar(&pack(&dir).unwrap())), want);
}

fn tar_of(entries: &[(&[u8], &str)]) -> Vec<u8> {
    let mut builder = tar::Builder::new(Vec::new());
    for (path, data) in entries {
        let mut header = tar::Header::new_gnu();
        header.as_old_mut().name[..path.len()].copy_from_slice(path);
        header.set_size(data.len() as u64);
        header.set_mode(0o644);
        header.set_entry_type(tar::EntryType::Regular);
        header.set_cksum();
        builder.append(&header, data.as_bytes()).unwrap();
    }
    builder.into_inner().unwrap()
}

#[test]
fn tar_members_are_named_once_and_lossily() {
    let rule = good_rule("identity/a");
    let tar = tar_of(&[
        (b"./manifest.yaml", MANIFEST),
        (b"./rules/a.yaml", &rule),
        (b"rules/a.yaml", &rule),
        (b"rules/\xff.yaml", &rule),
        (b"\xfe.txt", "ignored"),
    ]);
    assert_eq!(
        problems(load_tar(&tar)),
        [
            ("rules/a.yaml".to_owned(), Problem::DuplicateMember),
            ("rules/\u{fffd}.yaml".to_owned(), Problem::BadFileName),
        ]
    );
    let fine = tar_of(&[(b"./manifest.yaml", MANIFEST), (b"./rules/a.yaml", &rule)]);
    assert_eq!(load_tar(&fine).unwrap().rules.len(), 1);
}

#[cfg(unix)]
#[test]
fn non_utf8_names_are_refused_only_where_they_matter() {
    use std::os::unix::ffi::OsStrExt;
    let dir = scratch("non-utf8");
    write(&dir, "manifest.yaml", MANIFEST);
    std::fs::write(dir.join(std::ffi::OsStr::from_bytes(b"\xfe.txt")), "x").unwrap();
    std::fs::create_dir_all(dir.join("rules")).unwrap();
    assert!(load_dir(&dir).is_ok());
    std::fs::write(
        dir.join("rules")
            .join(std::ffi::OsStr::from_bytes(b"\xff.yaml")),
        "x",
    )
    .unwrap();
    assert_eq!(
        problems(load_dir(&dir)),
        [("rules/\u{fffd}.yaml".to_owned(), Problem::BadFileName)]
    );
}

#[test]
fn deep_folders_are_cut_and_reported() {
    let dir = scratch("deep-folders");
    write(&dir, "manifest.yaml", MANIFEST);
    let deep = (0..MAX_FOLDER_DEPTH)
        .map(|i| format!("d{i}"))
        .collect::<Vec<_>>();
    write(
        &dir,
        &format!("rules/{}/r.yaml", deep.join("/")),
        good_rule("identity/deep"),
    );
    let got = problems(load_dir(&dir));
    assert_eq!(got.len(), 1, "{got:?}");
    assert_eq!(
        got[0].1,
        Problem::FolderTooDeep {
            limit: MAX_FOLDER_DEPTH
        }
    );
    let tar = problems(load_tar(&pack(&dir).unwrap()));
    assert_eq!(tar.len(), 1, "{tar:?}");
    assert_eq!(
        tar[0].1,
        Problem::FolderTooDeep {
            limit: MAX_FOLDER_DEPTH
        }
    );
}

#[cfg(unix)]
#[test]
fn an_unreadable_folder_is_one_error_and_the_load_goes_on() {
    use std::os::unix::fs::PermissionsExt;
    let dir = scratch("unreadable");
    write(&dir, "manifest.yaml", MANIFEST);
    write(&dir, "rules/locked/r.yaml", good_rule("identity/locked"));
    write(
        &dir,
        "rules/open.yaml",
        "schema: \"1.0\"\nid: identity/open\n",
    );
    let locked = dir.join("rules/locked");
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
    let readable = std::fs::read_dir(&locked).is_ok();
    let got = problems(load_dir(&dir));
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
    if readable {
        return;
    }
    let files: Vec<&str> = got.iter().map(|(f, _)| f.as_str()).collect();
    assert_eq!(files, ["rules/locked", "rules/open.yaml"], "{got:?}");
    assert_eq!(got[0].1, Problem::Io(std::io::ErrorKind::PermissionDenied));
}
