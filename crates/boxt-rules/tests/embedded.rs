mod common;

use std::time::{Duration, SystemTime};

use boxt_rules::{CorpusHash, Origin, corpus_hash, embedded, load_tar, pack, prepare};

use common::*;

#[test]
fn the_embedded_corpus_loads_with_no_errors() {
    let corpus = load_tar(embedded()).unwrap();
    assert_eq!(corpus.origin, Origin::Corpus);
}

#[test]
fn prepare_loads_and_validates_the_embedded_corpus() {
    assert_eq!(prepare().unwrap(), load_tar(embedded()).unwrap());
}

#[test]
fn the_embedded_tar_is_pack_of_the_corpus_folder() {
    let packed = pack(&corpus_dir()).unwrap();
    assert_eq!(packed, embedded());
    assert_eq!(CorpusHash::of(&packed), corpus_hash());
    assert_eq!(corpus_hash().to_string().len(), 64);
}

#[test]
fn two_packs_of_one_folder_are_byte_identical() {
    let dir = scratch("pack-twice");
    write(&dir, "manifest.yaml", MANIFEST);
    write(&dir, "rules/b/z.yaml", good_rule("identity/z"));
    write(&dir, "rules/a.yaml", good_rule("identity/a"));
    write(&dir, "profiles/.gitkeep", "");
    let first = pack(&dir).unwrap();
    let file = std::fs::File::options()
        .write(true)
        .open(dir.join("rules/a.yaml"))
        .unwrap();
    file.set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000_000))
        .unwrap();
    drop(file);
    let second = pack(&dir).unwrap();
    assert_eq!(first, second);
    assert_eq!(CorpusHash::of(&first), CorpusHash::of(&second));

    let mut archive = tar::Archive::new(first.as_slice());
    let mut paths = Vec::new();
    for entry in archive.entries().unwrap() {
        let entry = entry.unwrap();
        let header = entry.header();
        assert_eq!(header.mtime().unwrap(), 0);
        assert_eq!(header.uid().unwrap(), 0);
        assert_eq!(header.gid().unwrap(), 0);
        assert_eq!(header.mode().unwrap(), 0o644);
        assert!(header.as_ustar().is_some());
        assert!(header.entry_type().is_file());
        paths.push(String::from_utf8(entry.path_bytes().into_owned()).unwrap());
    }
    assert_eq!(
        paths,
        [
            "manifest.yaml",
            "profiles/.gitkeep",
            "rules/a.yaml",
            "rules/b/z.yaml"
        ]
    );
}

#[test]
fn a_path_too_long_for_ustar_is_refused_not_extended() {
    let dir = scratch("pack-long");
    let long = format!("rules/{}/{}.yaml", "d".repeat(150), "f".repeat(120));
    write(&dir, &long, "x");
    assert!(pack(&dir).is_err());
}
