//! Corpus loader, validator, matcher, guard classes and coverage.
#![forbid(unsafe_code)]

mod error;
mod format;
mod load;
mod pack;
mod schema;
mod validate;

use std::fmt;
use std::sync::OnceLock;

use sha2::{Digest, Sha256};

pub use error::{CorpusError, LoadError, Problem};
pub use format::{CoScope, Literal, NumberOp};
pub use load::{MAX_DEPTH, MAX_FILE_BYTES, MAX_FOLDER_DEPTH, load_dir, load_tar, load_user_dir};
pub use pack::pack;
pub use schema::{generate as generate_schema, rule_schema, schema_file_name};
pub use validate::{
    Behaviour, COMMON_TEST_FIELDS, CommonField, Corpus, CorpusVersion, EvidenceDef, FieldPath,
    FieldTest, Manifest, Matcher, Origin, Rule, RuleMatch, Test, field_path,
};

static EMBEDDED: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/corpus.tar"));

pub fn embedded() -> &'static [u8] {
    EMBEDDED
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CorpusHash(pub [u8; 32]);

impl CorpusHash {
    pub fn of(bytes: &[u8]) -> CorpusHash {
        CorpusHash(Sha256::digest(bytes).into())
    }
}

impl fmt::Display for CorpusHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for b in self.0 {
            write!(f, "{b:02x}")?;
        }
        Ok(())
    }
}

pub fn corpus_hash() -> CorpusHash {
    static HASH: OnceLock<CorpusHash> = OnceLock::new();
    *HASH.get_or_init(|| CorpusHash::of(EMBEDDED))
}
