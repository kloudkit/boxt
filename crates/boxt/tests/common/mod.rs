use std::ffi::OsString;
use std::path::{Path, PathBuf};

use boxt_collector::boxt_core::fact::Fact;
use boxt_collector::boxt_core::limits::Limits;
use boxt_collector::boxt_core::settings::Settings;
use boxt_collector::boxt_core::verdict::Result;
use boxt_collector::boxt_core::{SortedVec, Text};
use boxt_collector::path::text_from_path;
use boxt_collector::worker::WORKER_SUBCOMMAND;
use boxt_collector::{RunOptions, run_with};
use boxt_rules::{embedded, evaluate, load_tar};
use boxt_verdict::{VerdictInput, build};

pub fn seed_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .unwrap()
        .join("fixtures")
        .join("source")
        .join("seed")
}

pub fn verdict(inputs: &[PathBuf]) -> Result {
    let given: Vec<Text> = inputs.iter().map(|p| text_from_path(p)).collect();
    let roots: Vec<Text> = inputs
        .iter()
        .map(|p| text_from_path(&std::path::absolute(p).unwrap()))
        .collect();
    let settings = Settings {
        inputs: given.clone(),
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
    let matched = evaluate(&corpus, &facts);
    build(&VerdictInput {
        targets: &SortedVec::new(),
        profiles: &corpus.profiles,
        files: &collected.files,
        findings: &matched.findings,
        observations: &matched.observations,
        macro_tables_cut: &collected.macro_tables_cut,
        host_dependent_trees: &collected.host_dependent_trees,
        roots: &roots,
        compile_database_used: false,
    })
    .unwrap()
}
