use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

use boxt_collector::boxt_core::RulesOutput;
use boxt_collector::boxt_core::fact::Fact;
use boxt_collector::boxt_core::limits::Limits;
use boxt_collector::boxt_core::settings::Settings;
use boxt_collector::boxt_core::verdict::Observation;
use boxt_collector::path::text_from_path;
use boxt_collector::worker::WORKER_SUBCOMMAND;
use boxt_collector::{RunOptions, run_with};
use boxt_rules::{Corpus, embedded, evaluate, load_tar};

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/source/windows-branches")
}

fn scan(corpus: &Corpus) -> RulesOutput {
    let settings = Settings {
        inputs: vec![text_from_path(&fixtures())],
        targets: Default::default(),
        limits: Limits::default(),
        encodings: Vec::new(),
        excludes: Vec::new(),
        own_files: Vec::new(),
        ignored_macros: Default::default(),
        jobs: None,
        compile_database: None,
    };
    let options = RunOptions {
        program: PathBuf::from(env!("CARGO_BIN_EXE_boxt")),
        args: vec![OsString::from(WORKER_SUBCOMMAND)],
        env: Vec::new(),
        measure: false,
    };
    let (collected, _) = run_with(&settings, &options).unwrap();
    let facts: Vec<Fact> = collected
        .facts
        .into_iter()
        .flat_map(|file| file.facts)
        .collect();
    evaluate(corpus, &facts)
}

type Row = (String, u32, &'static str);

fn rows(output: &RulesOutput) -> Vec<Row> {
    let name = |loc: &boxt_collector::boxt_core::location::Location| {
        String::from_utf8(loc.path.as_ref().unwrap().as_bytes().to_vec()).unwrap()
    };
    let mut out: Vec<Row> = output
        .findings
        .iter()
        .filter(|f| f.behaviour.as_str() == "registry")
        .map(|f| (name(&f.location), f.location.line.unwrap(), "finding"))
        .chain(output.observations.iter().filter_map(|o| match o {
            Observation::WindowsBranch {
                location,
                behaviour,
                ..
            } if behaviour.as_str() == "registry" => {
                Some((name(location), location.line.unwrap(), "windows branch"))
            }
            _ => None,
        }))
        .collect();
    out.sort();
    out
}

fn row(file: &str, line: u32, class: &'static str) -> Row {
    (file.to_owned(), line, class)
}

fn matrix(corpus_macro: &'static str) -> Vec<Row> {
    let mut want = vec![
        row("corpus-macro.c", 7, corpus_macro),
        row("disjunction.c", 7, "windows branch"),
        row("empty-else.c", 5, "finding"),
        row("error-else.c", 5, "finding"),
        row("linux-elif.c", 7, "windows branch"),
        row("negated.c", 9, "windows branch"),
        row("nested-under-non-windows.c", 8, "windows branch"),
        row("no-guard.c", 4, "finding"),
        row("plain-else.c", 7, "windows branch"),
        row("windows-nested-no-alternative.c", 6, "finding"),
        row("windows-only-elif.c", 5, "finding"),
        row("windows-only-elif.c", 7, "finding"),
        row("windows-in-windows.c", 6, "finding"),
    ];
    want.sort();
    want
}

#[test]
fn every_guard_form_in_the_fixtures_gets_its_class() {
    let corpus = load_tar(embedded()).unwrap();
    assert_eq!(rows(&scan(&corpus)), matrix("finding"));
}

#[test]
fn a_macro_added_to_the_corpus_changes_the_class_with_no_code_change() {
    let mut corpus = load_tar(embedded()).unwrap();
    corpus
        .manifest
        .windows_guard_macros
        .push("MY_WIN".to_owned());
    assert_eq!(rows(&scan(&corpus)), matrix("windows branch"));
}

#[test]
fn a_windows_branch_carries_its_condition_and_api() {
    let corpus = load_tar(embedded()).unwrap();
    let output = scan(&corpus);
    let negated = output
        .observations
        .iter()
        .find_map(|o| match o {
            Observation::WindowsBranch {
                location,
                condition,
                api,
                guards,
                ..
            } if location.path.as_ref().unwrap().as_bytes() == b"negated.c" => {
                Some((condition.clone(), api.clone(), guards.len()))
            }
            _ => None,
        })
        .unwrap();
    assert_eq!(negated.0.as_str(), Some("#ifndef _WIN32, #else"));
    assert_eq!(negated.1.unwrap().as_str(), Some("RegOpenKeyExA"));
    assert_eq!(negated.2, 1);
}

#[test]
fn the_report_lists_each_windows_branch_under_its_heading() {
    let out = Command::new(env!("CARGO_BIN_EXE_boxt"))
        .arg(fixtures().join("plain-else.c"))
        .args(["--target", "docker-default"])
        .env_remove("CLICOLOR_FORCE")
        .output()
        .unwrap();
    let text = String::from_utf8(out.stdout).unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let section = text
        .split_once("\nWindows branches (1)\n")
        .unwrap_or_else(|| panic!("{text}"))
        .1;
    let line = section.lines().next().unwrap();
    assert_eq!(
        line.split_whitespace().collect::<Vec<_>>(),
        ["plain-else.c:7", "#ifdef", "_WIN32", "RegOpenKeyExA"],
        "{text}"
    );
    assert!(text.contains("\n Findings   none\n"), "{text}");
    assert!(text.contains("  docker-default   [ready] "), "{text}");
    assert!(!text.contains("registry"), "{text}");
    let again = Command::new(env!("CARGO_BIN_EXE_boxt"))
        .arg(fixtures().join("plain-else.c"))
        .args(["--target", "docker-default"])
        .env_remove("CLICOLOR_FORCE")
        .output()
        .unwrap();
    assert_eq!(again.stdout, text.as_bytes());
}
