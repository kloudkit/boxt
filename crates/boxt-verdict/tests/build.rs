use boxt_verdict::boxt_core::fact::FileKind;
use boxt_verdict::boxt_core::identity::Identity;
use boxt_verdict::boxt_core::input_scope::{self, InputScope};
use boxt_verdict::boxt_core::location::{Frame, FrameKind, InputRef, Location, RelPath, Scope};
use boxt_verdict::boxt_core::message::{
    CanonicalKey, CollectedFile, EntryPath, Region, RegionReason,
};
use boxt_verdict::boxt_core::outcome::{
    Count, Details, FileOutcome, NotCheckedReason, Outcome, SpecialKind,
};
use boxt_verdict::boxt_core::verdict::{
    BehaviourId, BehaviourTier, Confidence, Control, ControlId, Evidence, EvidenceId, Finding,
    GroupKey, LineRange, NotCheckedEntry, Observation, Profile, ProjectFile, Result, RuleId,
    TargetId, Tier, TierRange,
};
use boxt_verdict::boxt_core::{SortedVec, Text};
use boxt_verdict::{VerdictError, VerdictInput, build, prepare};

fn id(s: &str) -> TargetId {
    TargetId::new(s).unwrap()
}

fn behaviour(s: &str) -> BehaviourId {
    BehaviourId::new(s).unwrap()
}

fn profile(target: &str, controls: &[(&str, &[(&str, Tier)])], allowed: &[&str]) -> Profile {
    Profile {
        id: id(target),
        version: "provisional".to_owned(),
        controls: controls
            .iter()
            .map(|(control, behaviours)| Control {
                id: ControlId::new(*control).unwrap(),
                behaviours: behaviours
                    .iter()
                    .map(|(b, tier)| BehaviourTier {
                        behaviour: behaviour(b),
                        tier: *tier,
                    })
                    .collect(),
                not_detectable: None,
            })
            .collect(),
        allowed: allowed.iter().map(|b| behaviour(b)).collect(),
    }
}

fn four_profiles() -> Vec<Profile> {
    vec![
        profile(
            "docker-default",
            &[("linux-runtime", &[("registry", Tier::NeedsWindows)])],
            &["needs-root", "setuid", "low-port"],
        ),
        profile(
            "kubernetes-restricted",
            &[
                ("run-as-non-root", &[("needs-root", Tier::Blocked)]),
                ("no-privilege-escalation", &[("setuid", Tier::Blocked)]),
                ("unprivileged-ports", &[("low-port", Tier::ReadyWithConfig)]),
            ],
            &[],
        ),
        profile(
            "openshift-restricted-v2",
            &[("run-as-non-root", &[("needs-root", Tier::Blocked)])],
            &[],
        ),
        profile(
            "rootless-podman",
            &[("run-as-non-root", &[("needs-root", Tier::ReadyWithConfig)])],
            &[],
        ),
    ]
}

fn location(index: u32, given: &str, path: &str, line: Option<u32>, scope: Scope) -> Location {
    Location {
        input: InputRef {
            index,
            given: Text::from(given),
        },
        frames: Vec::new(),
        path: Some(RelPath::try_from(path).unwrap()),
        line,
        scope,
    }
}

fn in_main() -> Scope {
    Scope::Function {
        qualified_name: Text::from("main"),
        signature: Text::from("()"),
    }
}

fn finding(b: &str, confidence: Confidence, loc: Location) -> Finding {
    Finding {
        location: loc,
        rule: RuleId::new(format!("test/{b}")).unwrap(),
        behaviour: behaviour(b),
        confidence,
        evidence: Evidence {
            definition: EvidenceId::new("call").unwrap(),
        },
        guards: Vec::new(),
        fix: "fix it".to_owned(),
    }
}

fn file(path: &str, outcome: Outcome, regions: Vec<Region>) -> CollectedFile {
    file_in(0, "/src", path, Vec::new(), outcome, regions)
}

fn file_in(
    index: u32,
    given: &str,
    path: &str,
    frames: Vec<Frame>,
    outcome: Outcome,
    regions: Vec<Region>,
) -> CollectedFile {
    let mut loc = location(index, given, path, None, Scope::File);
    loc.frames = frames;
    CollectedFile {
        key: CanonicalKey::at(index, RelPath::try_from(path).unwrap(), EntryPath::job()),
        location: loc,
        outcome,
        regions,
        notes: Vec::new(),
    }
}

fn read_source(path: &str, lines: u64) -> CollectedFile {
    file(
        path,
        Outcome::new(FileOutcome::Read, Some(FileKind::Source)).with_lines(lines),
        Vec::new(),
    )
}

struct Run {
    targets: SortedVec<TargetId>,
    profiles: Vec<Profile>,
    files: SortedVec<CollectedFile>,
    findings: SortedVec<Finding>,
    observations: SortedVec<Observation>,
    roots: Vec<Text>,
    host_dependent_trees: SortedVec<u32>,
    compile_database_used: bool,
    project_file: ProjectFile,
}

impl Run {
    fn new(findings: Vec<Finding>) -> Run {
        Run {
            targets: SortedVec::new(),
            profiles: four_profiles(),
            files: vec![read_source("main.c", 40)].into(),
            findings: findings.into(),
            observations: SortedVec::new(),
            roots: vec![Text::from("/src")],
            host_dependent_trees: SortedVec::new(),
            compile_database_used: false,
            project_file: ProjectFile::NoneFound,
        }
    }

    fn build(&self) -> std::result::Result<Result, VerdictError> {
        let none = SortedVec::new();
        let prepared = prepare(&self.profiles, &self.targets)?;
        build(
            &prepared,
            &VerdictInput {
                files: &self.files,
                findings: &self.findings,
                observations: &self.observations,
                macro_tables_cut: &none,
                host_dependent_trees: &self.host_dependent_trees,
                roots: &self.roots,
                compile_database_used: self.compile_database_used,
                project_file: &self.project_file,
            },
        )
    }
}

fn target<'a>(result: &'a Result, t: &str) -> &'a boxt_verdict::boxt_core::verdict::TargetVerdict {
    result.targets.iter().find(|v| v.target == id(t)).unwrap()
}

#[test]
fn no_target_named_covers_every_profile() {
    let result = Run::new(Vec::new()).build().unwrap();
    let names: Vec<&str> = result.targets.iter().map(|t| t.target.as_str()).collect();
    assert_eq!(
        names,
        [
            "docker-default",
            "kubernetes-restricted",
            "openshift-restricted-v2",
            "rootless-podman"
        ]
    );
}

#[test]
fn named_targets_are_exactly_those() {
    let mut run = Run::new(Vec::new());
    run.targets = vec![id("rootless-podman"), id("docker-default")].into();
    let result = run.build().unwrap();
    let names: Vec<&str> = result.targets.iter().map(|t| t.target.as_str()).collect();
    assert_eq!(names, ["docker-default", "rootless-podman"]);
}

#[test]
fn an_unknown_target_names_every_known_id() {
    let mut run = Run::new(Vec::new());
    run.targets = vec![id("docker")].into();
    assert_eq!(
        prepare(&run.profiles, &run.targets),
        Err(VerdictError::UnknownTarget {
            id: id("docker"),
            known: vec![
                id("docker-default"),
                id("kubernetes-restricted"),
                id("openshift-restricted-v2"),
                id("rootless-podman"),
            ]
            .into(),
        })
    );
}

#[test]
fn prepare_refuses_an_unknown_target_before_any_file_is_read() {
    let mut run = Run::new(Vec::new());
    run.targets = vec![id("docker")].into();
    assert!(matches!(
        prepare(&run.profiles, &run.targets),
        Err(VerdictError::UnknownTarget { .. })
    ));
}

#[test]
fn prepared_profiles_are_the_named_targets_in_target_id_order() {
    let mut run = Run::new(Vec::new());
    run.targets = vec![id("rootless-podman"), id("docker-default")].into();
    let prepared = prepare(&run.profiles, &run.targets).unwrap();
    let ids: Vec<&str> = prepared.profiles().iter().map(|p| p.id.as_str()).collect();
    assert_eq!(ids, ["docker-default", "rootless-podman"]);
}

#[test]
fn the_result_carries_the_project_file() {
    let mut run = Run::new(Vec::new());
    assert_eq!(run.build().unwrap().project_file, ProjectFile::NoneFound);
    let found = ProjectFile::Found {
        path: Text::from("/repo/boxt.yaml"),
        above_vcs_root: true,
    };
    run.project_file = found.clone();
    assert_eq!(run.build().unwrap().project_file, found);
}

#[test]
fn the_worst_control_applies() {
    let mut run = Run::new(vec![finding(
        "bind",
        Confidence::Found,
        location(0, "/src", "main.c", Some(3), in_main()),
    )]);
    run.profiles = vec![profile(
        "t",
        &[
            ("a", &[("bind", Tier::ReadyWithConfig)]),
            ("b", &[("bind", Tier::Blocked)]),
        ],
        &[],
    )];
    let result = run.build().unwrap();
    assert_eq!(
        target(&result, "t").range,
        TierRange {
            found: Tier::Blocked,
            with_suspected: Tier::Blocked
        }
    );
}

#[test]
fn an_allowed_behaviour_is_ready_and_counted_as_allowed() {
    let run = Run::new(vec![
        finding(
            "setuid",
            Confidence::Found,
            location(0, "/src", "main.c", Some(3), in_main()),
        ),
        finding(
            "needs-root",
            Confidence::Suspected,
            location(0, "/src", "main.c", Some(4), in_main()),
        ),
    ]);
    let result = run.build().unwrap();
    let docker = target(&result, "docker-default");
    assert_eq!(docker.range.found, Tier::Ready);
    assert_eq!((docker.found, docker.suspected, docker.allowed), (0, 0, 2));
    assert!(docker.unmapped.is_empty());
    let podman = target(&result, "rootless-podman");
    assert_eq!(podman.unmapped.as_slice(), &[behaviour("setuid")]);
    assert_eq!((podman.found, podman.suspected, podman.allowed), (0, 1, 0));
}

#[test]
fn a_behaviour_mapped_to_ready_counts_as_allowed() {
    let mut run = Run::new(vec![
        finding(
            "bind",
            Confidence::Found,
            location(0, "/src", "main.c", Some(3), in_main()),
        ),
        finding(
            "bind",
            Confidence::Suspected,
            location(0, "/src", "main.c", Some(5), in_main()),
        ),
        finding(
            "root",
            Confidence::Found,
            location(0, "/src", "main.c", Some(7), in_main()),
        ),
    ]);
    run.profiles = vec![profile(
        "t",
        &[
            ("a", &[("bind", Tier::Ready)]),
            ("b", &[("root", Tier::NeedsCodeChange)]),
        ],
        &[],
    )];
    let t = run.build().unwrap().targets[0].clone();
    assert_eq!((t.found, t.suspected, t.allowed), (1, 0, 2));
    assert_eq!(t.range.found, Tier::NeedsCodeChange);
}

#[test]
fn an_unmapped_behaviour_sets_no_tier_and_is_not_counted() {
    let run = Run::new(vec![
        finding(
            "low-port",
            Confidence::Found,
            location(0, "/src", "main.c", Some(3), in_main()),
        ),
        finding(
            "low-port",
            Confidence::Suspected,
            location(0, "/src", "main.c", Some(4), in_main()),
        ),
    ]);
    let result = run.build().unwrap();
    let openshift = target(&result, "openshift-restricted-v2");
    assert_eq!(openshift.unmapped.as_slice(), &[behaviour("low-port")]);
    assert_eq!(
        openshift.range,
        TierRange {
            found: Tier::Ready,
            with_suspected: Tier::Ready
        }
    );
    assert_eq!(
        (openshift.found, openshift.suspected, openshift.allowed),
        (0, 0, 0)
    );
    let kubernetes = target(&result, "kubernetes-restricted");
    assert!(kubernetes.unmapped.is_empty());
    assert_eq!(kubernetes.range.found, Tier::ReadyWithConfig);
}

#[test]
fn the_range_runs_from_found_to_found_and_suspected() {
    let mut run = Run::new(vec![
        finding(
            "port",
            Confidence::Found,
            location(0, "/src", "main.c", Some(3), in_main()),
        ),
        finding(
            "root",
            Confidence::Suspected,
            location(0, "/src", "main.c", Some(5), in_main()),
        ),
        finding(
            "root",
            Confidence::Suspected,
            location(0, "/src", "main.c", Some(7), in_main()),
        ),
    ]);
    run.profiles = vec![profile(
        "t",
        &[
            ("p", &[("port", Tier::NeedsCodeChange)]),
            ("r", &[("root", Tier::Blocked)]),
        ],
        &[],
    )];
    let result = run.build().unwrap();
    let t = target(&result, "t");
    assert_eq!(
        t.range,
        TierRange {
            found: Tier::NeedsCodeChange,
            with_suspected: Tier::Blocked
        }
    );
    assert_eq!((t.found, t.suspected, t.allowed), (1, 2, 0));
}

#[test]
fn a_clean_run_is_ready_with_its_not_checked_list() {
    let result = Run::new(Vec::new()).build().unwrap();
    for t in result.targets.iter() {
        assert_eq!(
            t.range,
            TierRange {
                found: Tier::Ready,
                with_suspected: Tier::Ready
            }
        );
    }
    assert!(result.not_checked.is_empty());
    let json = serde_json::to_value(&result).unwrap();
    assert_eq!(json["not_checked"], serde_json::json!([]));
    assert_eq!(result.counts.read, 1);
    assert_eq!(result.counts.not_checked, Count::Exact(0));
}

#[test]
fn not_checked_lists_files_regions_and_dynamic_loads_with_reasons() {
    let mut run = Run::new(Vec::new());
    let fifo = file(
        "pipe",
        Outcome::new(
            FileOutcome::NotChecked {
                reason: NotCheckedReason::SpecialFile(SpecialKind::Fifo),
                details: Details::None,
            },
            None,
        ),
        Vec::new(),
    );
    let unparseable = file(
        "broken.c",
        Outcome::new(
            FileOutcome::NotChecked {
                reason: NotCheckedReason::Unparseable,
                details: Details::None,
            },
            Some(FileKind::Source),
        )
        .with_lines(30),
        Vec::new(),
    );
    let region = |first_line, last_line| Region {
        first_line,
        last_line,
        reason: RegionReason::Unparseable,
    };
    let partly = file(
        "partly.c",
        Outcome::new(FileOutcome::Read, Some(FileKind::Source)).with_lines(50),
        vec![region(30, 31), region(10, 20), region(41, 41)],
    );
    run.files = vec![fifo.clone(), unparseable.clone(), partly.clone()].into();
    let dl_location = location(
        1,
        "/bin/app",
        "app",
        None,
        Scope::Symbol(Text::from("dlopen")),
    );
    run.observations = vec![Observation::DynamicLoad {
        location: dl_location.clone(),
        symbol: Text::from("dlopen"),
    }]
    .into();
    let result = run.build().unwrap();
    let expected = [
        NotCheckedEntry::File {
            location: unparseable.location.clone(),
            outcome: unparseable.outcome.clone(),
        },
        NotCheckedEntry::File {
            location: fifo.location.clone(),
            outcome: fifo.outcome.clone(),
        },
        NotCheckedEntry::Regions {
            location: partly.location.clone(),
            reason: RegionReason::Unparseable,
            ranges: vec![
                LineRange {
                    first: 10,
                    last: 20,
                },
                LineRange {
                    first: 30,
                    last: 31,
                },
                LineRange {
                    first: 41,
                    last: 41,
                },
            ],
        },
        NotCheckedEntry::DynamicLoad {
            location: dl_location,
            symbol: Text::from("dlopen"),
        },
    ];
    assert_eq!(result.not_checked.as_slice(), &expected);
    assert_eq!(result.counts.not_checked, Count::Exact(2));
    assert_eq!(result.observations.len(), 1);
}

#[test]
fn an_aggregate_is_one_not_checked_entry() {
    let mut run = Run::new(Vec::new());
    let cut = file(
        "z.c",
        Outcome::new(
            FileOutcome::Aggregate {
                reason: NotCheckedReason::LimitExceeded(
                    boxt_verdict::boxt_core::limits::LimitName::TotalEntries,
                ),
                count: Some(7),
                at_least: 7,
            },
            Some(FileKind::Source),
        ),
        Vec::new(),
    );
    run.files.insert(cut.clone());
    let result = run.build().unwrap();
    assert_eq!(result.not_checked.len(), 1);
    assert_eq!(result.counts.not_checked, Count::Exact(7));
}

#[test]
fn nothing_read_is_an_error() {
    let mut run = Run::new(Vec::new());
    run.files = SortedVec::new();
    assert_eq!(run.build(), Err(VerdictError::NothingRead));
    run.files = vec![file(
        "pipe",
        Outcome::new(
            FileOutcome::NotChecked {
                reason: NotCheckedReason::SpecialFile(SpecialKind::Fifo),
                details: Details::None,
            },
            None,
        ),
        Vec::new(),
    )]
    .into();
    assert_eq!(run.build(), Err(VerdictError::NothingRead));
}

#[test]
fn sm3_is_none_with_no_source_lines() {
    let mut run = Run::new(Vec::new());
    run.files = vec![file(
        "app.conf",
        Outcome::new(FileOutcome::Read, Some(FileKind::Config)).with_lines(3),
        Vec::new(),
    )]
    .into();
    let result = run.build().unwrap();
    assert_eq!(result.sm3.lines, None);
    assert_eq!(result.input_scope, InputScope::ConfigurationOnly);
}

#[test]
fn a_source_only_run_has_the_source_only_text() {
    let result = Run::new(Vec::new()).build().unwrap();
    assert_eq!(result.input_scope, InputScope::SourceOnly);
    assert_eq!(
        result.input_scope_text,
        "Source only: results describe the code, not the built program. Add binaries or packages for a richer result."
    );
    let mut run = Run::new(Vec::new());
    run.compile_database_used = true;
    let result = run.build().unwrap();
    assert_eq!(
        result.input_scope_text,
        input_scope::text(InputScope::SourceOnly, true)
    );
    assert!(
        result
            .input_scope_text
            .ends_with("some may not be in the build.")
    );
}

#[test]
fn the_scope_counts_read_files_only_and_binaries_outside_packages() {
    let read = |kind| Outcome::new(FileOutcome::Read, Some(kind));
    let rpm = vec![Frame {
        kind: FrameKind::Rpm,
        name: Text::from("app"),
    }];
    let tar = vec![Frame {
        kind: FrameKind::Archive,
        name: Text::from("bundle"),
    }];
    let mut run = Run::new(Vec::new());
    run.files = vec![
        file_in(
            0,
            "/p",
            "usr/bin/app",
            rpm.clone(),
            read(FileKind::Binary),
            Vec::new(),
        ),
        file(
            "skipped.c",
            Outcome::new(
                FileOutcome::NotChecked {
                    reason: NotCheckedReason::Unparseable,
                    details: Details::None,
                },
                Some(FileKind::Source),
            ),
            Vec::new(),
        ),
    ]
    .into();
    assert_eq!(
        run.build().unwrap().input_scope,
        InputScope::ConfigurationOnly
    );
    run.files.insert(file_in(
        0,
        "/p",
        "x",
        rpm,
        read(FileKind::Package),
        Vec::new(),
    ));
    assert_eq!(run.build().unwrap().input_scope, InputScope::PackagesOnly);
    run.files.insert(file_in(
        1,
        "/t",
        "bin/tool",
        tar,
        read(FileKind::Binary),
        Vec::new(),
    ));
    assert_eq!(
        run.build().unwrap().input_scope,
        InputScope::BinariesAndPackages
    );
    run.files.insert(read_source("main.c", 3));
    assert_eq!(
        run.build().unwrap().input_scope,
        InputScope::SourceBinariesAndPackages
    );
}

#[test]
fn findings_sharing_an_identity_are_numbered_in_location_order() {
    let run = Run::new(vec![
        finding(
            "bind",
            Confidence::Found,
            location(0, "/src", "main.c", Some(9), in_main()),
        ),
        finding(
            "bind",
            Confidence::Found,
            location(0, "/src", "main.c", Some(4), in_main()),
        ),
        finding(
            "bind",
            Confidence::Found,
            location(0, "/src", "other.c", Some(4), in_main()),
        ),
    ]);
    let result = run.build().unwrap();
    let ids: Vec<(Option<u32>, &str, String)> = result
        .findings
        .iter()
        .map(|r| {
            (
                r.finding.location.line,
                std::str::from_utf8(r.finding.location.path.as_ref().unwrap().as_bytes()).unwrap(),
                r.id.to_string(),
            )
        })
        .collect();
    assert_eq!(ids[0].0, Some(4));
    assert_eq!(ids[1].0, Some(9));
    let (first, second) = (&ids[0].2, &ids[1].2);
    assert_eq!(first[..12], second[..12]);
    assert!(first.ends_with("-1") && second.ends_with("-2"), "{ids:?}");
    assert_ne!(ids[2].2[..12], first[..12]);
    assert!(ids[2].2.ends_with("-1"));
    for (_, _, id) in &ids {
        assert_eq!(id.len(), 14);
        assert!(
            id[..12]
                .bytes()
                .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
        );
    }
}

#[test]
fn rule_ids_break_the_tie_after_location() {
    let at = || location(0, "/src", "main.c", Some(4), in_main());
    let mut user = finding("bind", Confidence::Found, at());
    user.rule = RuleId::new("user/bind").unwrap();
    let mut corpus_b = finding("bind", Confidence::Found, at());
    corpus_b.rule = RuleId::new("net/zbind").unwrap();
    let mut corpus_a = finding("bind", Confidence::Found, at());
    corpus_a.rule = RuleId::new("net/abind").unwrap();
    let result = Run::new(vec![user, corpus_b, corpus_a]).build().unwrap();
    let by_rule = |r: &str| {
        result
            .findings
            .iter()
            .find(|f| f.finding.rule.as_str() == r)
            .unwrap()
            .id
            .ordinal()
            .get()
    };
    assert_eq!(by_rule("net/abind"), 1);
    assert_eq!(by_rule("net/zbind"), 2);
    assert_eq!(by_rule("user/bind"), 3);
}

fn ids_by_input(order: [(&str, u32, u32); 2]) -> Vec<(String, Option<u32>, String)> {
    let mut run = Run::new(Vec::new());
    run.roots = order
        .iter()
        .map(|(given, _, _)| Text::from(*given))
        .collect();
    run.files = order
        .iter()
        .map(|(given, index, _)| {
            file_in(
                *index,
                given,
                "main.c",
                Vec::new(),
                Outcome::new(FileOutcome::Read, Some(FileKind::Source)).with_lines(10),
                Vec::new(),
            )
        })
        .collect();
    run.findings = order
        .iter()
        .map(|(given, index, line)| {
            finding(
                "bind",
                Confidence::Found,
                location(*index, given, "main.c", Some(*line), in_main()),
            )
        })
        .collect();
    let result = run.build().unwrap();
    let mut ids: Vec<(String, Option<u32>, String)> = result
        .findings
        .iter()
        .map(|r| {
            (
                String::from_utf8(r.finding.location.input.given.as_bytes().to_vec()).unwrap(),
                r.finding.location.line,
                r.id.to_string(),
            )
        })
        .collect();
    ids.sort();
    ids
}

#[test]
fn ids_do_not_depend_on_input_order() {
    let pq = ids_by_input([("/a/x", 0, 3), ("/b/x", 1, 3)]);
    let qp = ids_by_input([("/b/x", 0, 3), ("/a/x", 1, 3)]);
    assert_eq!(pq, qp);
    assert!(pq[0].2.ends_with("-1") && pq[1].2.ends_with("-2"), "{pq:?}");
}

#[test]
fn the_same_root_given_twice_numbers_the_same_in_both_orders() {
    let first = ids_by_input([("/s/p", 0, 9), ("/s/p", 1, 4)]);
    let second = ids_by_input([("/s/p", 0, 4), ("/s/p", 1, 9)]);
    assert_eq!(first, second);
    assert_eq!(first[0].1, Some(4));
    assert!(
        first[0].2.ends_with("-1") && first[1].2.ends_with("-2"),
        "{first:?}"
    );
}

#[test]
fn the_same_input_gives_byte_identical_json() {
    let run = Run::new(vec![finding(
        "needs-root",
        Confidence::Suspected,
        location(0, "/src", "main.c", Some(3), in_main()),
    )]);
    let a = serde_json::to_vec(&run.build().unwrap()).unwrap();
    let b = serde_json::to_vec(&run.build().unwrap()).unwrap();
    assert_eq!(a, b);
    let back: Result = serde_json::from_slice(&a).unwrap();
    assert_eq!(back, run.build().unwrap());
    let json: serde_json::Value = serde_json::from_slice(&a).unwrap();
    let reported = &json["findings"][0];
    assert!(reported["id"].is_string());
    assert_eq!(reported["behaviour"], "needs-root");
    assert!(reported.get("finding").is_none());
    assert_eq!(json["input_scope"], "source_only");
}

#[test]
fn host_dependent_trees_mark_the_result() {
    let mut run = Run::new(Vec::new());
    assert!(!run.build().unwrap().host_dependent);
    run.host_dependent_trees = vec![0].into();
    assert!(run.build().unwrap().host_dependent);
}

#[test]
fn a_host_dependent_outcome_marks_the_result() {
    let mut run = Run::new(Vec::new());
    let mut outcome = Outcome::new(FileOutcome::Read, Some(FileKind::Source)).with_lines(5);
    outcome.host_dependent = true;
    run.files.insert(file("slow.c", outcome, Vec::new()));
    assert!(run.build().unwrap().host_dependent);
}

#[test]
fn a_fixed_identity_has_a_fixed_id() {
    let f = finding(
        "bind",
        Confidence::Found,
        location(0, "/src", "main.c", Some(3), in_main()),
    );
    assert_eq!(
        Identity::of(&f).canonical_text().as_bytes(),
        b"behaviour bind\npath main.c\nscope function\nname main\nsignature ()"
    );
    let result = Run::new(vec![f]).build().unwrap();
    assert_eq!(result.findings[0].id.as_str(), "5a5d9d7ad72c-1");
}

fn with_rule(mut f: Finding, rule: &str, fix: &str) -> Finding {
    f.rule = RuleId::new(rule).unwrap();
    fix.clone_into(&mut f.fix);
    f
}

#[test]
fn findings_group_by_behaviour_with_one_section_per_rule() {
    let at = |path: &str, line| location(0, "/src", path, Some(line), in_main());
    let findings = vec![
        with_rule(
            finding("setuid", Confidence::Found, at("0.c", 4)),
            "user/setuid-x",
            "user fix",
        ),
        with_rule(
            finding("setuid", Confidence::Suspected, at("a.c", 9)),
            "identity/setuid",
            "corpus fix",
        ),
        with_rule(
            finding("setuid", Confidence::Found, at("a.c", 2)),
            "identity/setuid",
            "corpus fix",
        ),
        finding("needs-root", Confidence::Found, at("c.c", 1)),
        with_rule(
            finding("needs-root", Confidence::Suspected, at("c.c", 2)),
            "identity/needs-root",
            "root fix",
        ),
        finding("low-port", Confidence::Suspected, at("d.c", 1)),
    ];
    let result = Run::new(findings).build().unwrap();
    let keys: Vec<&GroupKey> = result.groups.iter().map(|g| &g.key).collect();
    assert_eq!(
        keys,
        [
            &GroupKey::Behaviour(behaviour("low-port")),
            &GroupKey::Behaviour(behaviour("needs-root")),
            &GroupKey::Behaviour(behaviour("setuid")),
        ]
    );
    let setuid = &result.groups[2];
    assert_eq!((setuid.found, setuid.suspected), (2, 1));
    let rules: Vec<(&str, &str)> = setuid
        .rules
        .iter()
        .map(|r| (r.rule.as_str(), r.fix.as_str()))
        .collect();
    assert_eq!(
        rules,
        [
            ("identity/setuid", "corpus fix"),
            ("user/setuid-x", "user fix")
        ]
    );
    let in_order: Vec<_> = result
        .findings
        .iter()
        .filter(|r| r.finding.rule.as_str() == "identity/setuid")
        .map(|r| r.id.clone())
        .collect();
    assert_eq!(setuid.rules[0].findings, in_order);
    assert_eq!(setuid.rules[0].findings.len(), 2);
    assert_eq!(
        result.findings[0].finding.rule.as_str(),
        "user/setuid-x",
        "the user rule's finding comes first in findings order"
    );
    assert_eq!(result.groups[1].rules.len(), 2);
    for reported in result.findings.iter() {
        let holders: Vec<_> = result
            .groups
            .iter()
            .flat_map(|g| g.rules.iter().map(move |r| (g, r)))
            .filter(|(_, r)| r.findings.contains(&reported.id))
            .collect();
        assert_eq!(holders.len(), 1, "{}", reported.id);
        let (group, section) = holders[0];
        assert_eq!(
            group.key,
            GroupKey::Behaviour(reported.finding.behaviour.clone())
        );
        assert_eq!(section.rule, reported.finding.rule);
        assert_eq!(
            section
                .findings
                .iter()
                .filter(|id| **id == reported.id)
                .count(),
            1
        );
    }
    assert_eq!((result.groups[0].found, result.groups[0].suspected), (0, 1));
}

#[test]
fn no_findings_give_no_groups() {
    assert!(Run::new(Vec::new()).build().unwrap().groups.is_empty());
}

#[test]
fn files_no_reader_understands_are_counted() {
    let mut run = Run::new(Vec::new());
    let no_reader = |path: &str| {
        file(
            path,
            Outcome::new(
                FileOutcome::NotChecked {
                    reason: NotCheckedReason::NoReader,
                    details: Details::None,
                },
                None,
            ),
            Vec::new(),
        )
    };
    let unparseable = file(
        "broken.c",
        Outcome::new(
            FileOutcome::NotChecked {
                reason: NotCheckedReason::Unparseable,
                details: Details::None,
            },
            Some(FileKind::Source),
        )
        .with_lines(3),
        Vec::new(),
    );
    for f in [no_reader("README"), no_reader("x.txt"), unparseable] {
        run.files.insert(f);
    }
    let result = run.build().unwrap();
    assert_eq!(result.no_reader, 2);
    assert_eq!(result.not_checked.len(), 3);
    assert_eq!(Run::new(Vec::new()).build().unwrap().no_reader, 0);
}
