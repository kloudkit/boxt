#![allow(dead_code)]

use std::path::{Path, PathBuf};

use boxt_core::Text;
use boxt_core::location::RelPath;
use boxt_rules::{Corpus, CorpusError, LoadError, Problem, load_dir};

pub fn corpora() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpora")
}

pub fn corpus_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus")
}

pub fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

pub fn write(root: &Path, rel: &str, contents: impl AsRef<[u8]>) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

pub fn rel(path: &str) -> RelPath {
    RelPath::new(Text::from(path)).unwrap()
}

pub fn refused(result: Result<Corpus, LoadError>) -> Vec<CorpusError> {
    match result {
        Err(LoadError::Refused(errors)) => errors,
        Err(other) => panic!("expected refusals, got {other}"),
        Ok(corpus) => panic!("expected refusals, got {corpus:#?}"),
    }
}

pub fn problems(result: Result<Corpus, LoadError>) -> Vec<(String, Problem)> {
    refused(result)
        .into_iter()
        .map(|e| {
            (
                String::from_utf8(e.file.as_bytes().to_vec()).unwrap(),
                e.problem,
            )
        })
        .collect()
}

pub const MANIFEST: &str =
    "corpus_version: 1.0.0\nbehaviours:\n  - { id: needs-root, dimension: identity }\n";

pub fn good_rule(id: &str) -> String {
    format!(
        "schema: \"1.0\"\nid: {id}\nbehaviour: needs-root\ntitle: T\nfix: F\n\
         evidence:\n  found: {{ id: e, text: t }}\n\
         match:\n  found: {{ kind: call, field: callee, equals: setuid }}\n"
    )
}

pub fn single(rule: &str) -> Vec<(String, Problem)> {
    let dir = scratch(&format!("single-{:x}", fxhash(rule)));
    write(&dir, "manifest.yaml", MANIFEST);
    write(&dir, "rules/r.yaml", rule);
    match load_dir(&dir) {
        Ok(_) => Vec::new(),
        result => problems(result),
    }
}

fn fxhash(s: &str) -> u64 {
    s.bytes().fold(0xcbf29ce484222325u64, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x100000001b3)
    })
}
