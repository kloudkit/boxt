#[path = "src/pack.rs"]
mod pack;

use std::path::PathBuf;

fn main() {
    let manifest =
        PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let corpus = manifest.join("../../corpus");
    println!("cargo::rerun-if-changed={}", corpus.display());
    let tar = pack::pack(&corpus).unwrap_or_else(|e| panic!("packing {}: {e}", corpus.display()));
    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR"));
    std::fs::write(out.join("corpus.tar"), tar).expect("writing corpus.tar");
}
