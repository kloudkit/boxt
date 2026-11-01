use std::ffi::OsString;
use std::path::{Path, PathBuf};

use boxt_collector::boxt_core::fact::Fact;
use boxt_collector::boxt_core::limits::Limits;
use boxt_collector::boxt_core::settings::Settings;
use boxt_collector::boxt_core::verdict::Confidence;
use boxt_collector::path::text_from_path;
use boxt_collector::worker::WORKER_SUBCOMMAND;
use boxt_collector::{RunOptions, run_with};
use boxt_rules::{Matched, embedded, evaluate, load_tar};

fn seed_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/source/seed")
}

fn scan(dir: &Path) -> Matched {
    let settings = Settings {
        inputs: vec![text_from_path(dir)],
        targets: Default::default(),
        limits: Limits::default(),
        encodings: Vec::new(),
        excludes: Vec::new(),
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
    let corpus = load_tar(embedded()).unwrap();
    evaluate(&corpus, &facts)
}

#[test]
fn every_seed_behaviour_is_found_from_source() {
    let matched = scan(&seed_dir());
    let found: Vec<(String, String, Option<u32>)> = matched
        .findings
        .iter()
        .filter(|f| f.confidence == Confidence::Found)
        .map(|f| {
            let path = f.location.path.as_ref().unwrap().as_bytes();
            (
                f.behaviour.to_string(),
                String::from_utf8(path.to_vec()).unwrap(),
                f.location.line,
            )
        })
        .collect();
    for (behaviour, file, line) in [
        ("needs-root", "needs-root.c", 6),
        ("setuid", "needs-root.c", 6),
        ("needs-root", "needs-root.c", 13),
        ("setuid", "needs-root.c", 13),
        ("setuid", "negative.c", 12),
        ("setuid", "setuid.c", 7),
        ("setuid", "setuid.c", 9),
        ("setuid", "setuid.c", 11),
        ("low-port", "low-port.c", 11),
        ("registry", "registry.c", 8),
        ("registry", "registry.c", 10),
        ("registry", "registry.c", 11),
    ] {
        assert!(
            found.contains(&(behaviour.to_owned(), file.to_owned(), Some(line))),
            "{behaviour} at {file}:{line} not in {found:#?}"
        );
    }
    assert_eq!(found.len(), 12, "{found:#?}");
    let near_misses: Vec<_> = matched
        .findings
        .iter()
        .filter(|f| {
            f.location.path.as_ref().map(|p| p.as_bytes()) == Some(b"negative.c".as_slice())
        })
        .map(|f| (f.behaviour.to_string(), f.location.line))
        .collect();
    assert_eq!(near_misses, [("setuid".to_owned(), Some(12))]);
    let suspected: Vec<_> = matched
        .findings
        .iter()
        .filter(|f| f.confidence == Confidence::Suspected)
        .map(|f| (f.behaviour.to_string(), f.location.line))
        .collect();
    assert_eq!(suspected, [("needs-root".to_owned(), Some(8))]);
    assert!(matched.observations.is_empty());
}

#[test]
fn a_seed_scan_twice_is_byte_identical() {
    let first = serde_json::to_vec(&scan(&seed_dir())).unwrap();
    let second = serde_json::to_vec(&scan(&seed_dir())).unwrap();
    assert_eq!(first, second);
}
