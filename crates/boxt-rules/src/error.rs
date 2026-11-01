use std::fmt;
use std::io;

use boxt_core::fact::{FactKind, FieldType};
use boxt_core::location::RelPath;
use boxt_core::verdict::{BehaviourId, Confidence, ControlId, IdError};
use boxt_core::version::UnsupportedVersion;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CorpusError {
    pub file: RelPath,
    pub problem: Problem,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Problem {
    NoTitle,
    NoFix,
    NamesTarget(&'static str),
    NamesTier(&'static str),
    NoMatch,
    ConfidenceWithoutEvidence(Confidence),
    NoEvidenceText(Confidence),
    EvidenceWithoutMatch(Confidence),
    TestDoesNotFit {
        field: String,
        field_type: FieldType,
        test: &'static str,
    },
    BadPattern {
        key: &'static str,
        message: String,
    },
    BlankValue(&'static str),
    BlankEntry(&'static str),
    DuplicateEntry {
        key: &'static str,
        value: String,
    },
    NotUserId(String),
    CollidesWithCorpus(String),
    AllowedAndTiered(BehaviourId),
    ControlTieredAndNotDetectable(ControlId),
    NoManifest,
    DuplicateMember,
    UnexpectedFolder,
    FolderTooDeep {
        limit: usize,
    },
    UnknownFactKind(String),
    UnknownField {
        kind: FactKind,
        field: String,
    },
    NoKind,
    NoTest,
    SeveralTests,
    FieldWithoutTest,
    TestWithoutField,
    EmptyList(&'static str),
    UnknownBehaviour(BehaviourId),
    UserBehaviourOutsideUserRegistry(BehaviourId),
    NotUserBehaviour(BehaviourId),
    ControlWithoutBehaviours(ControlId),
    BadId {
        key: &'static str,
        value: String,
        error: IdError,
    },
    DuplicateId(String),
    UnknownKey(String),
    MissingKey(String),
    NoSchemaVersion,
    BadSchemaVersion(String),
    UnsupportedVersion(UnsupportedVersion),
    BadCorpusVersion(String),
    Malformed {
        line: u64,
        column: u64,
        message: String,
    },
    NotUtf8,
    TooLarge {
        size: u64,
        limit: u64,
    },
    TooDeep {
        limit: usize,
    },
    UnexpectedFile,
    NotRegularFile,
    BadFileName,
    Io(io::ErrorKind),
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Problem::NoTitle => f.write_str("the rule has no title"),
            Problem::NoFix => f.write_str("the rule has no fix"),
            Problem::NamesTarget(key) => write!(
                f,
                "the rule names a target ('{key}'); only profiles map behaviours to targets"
            ),
            Problem::NamesTier(key) => write!(
                f,
                "the rule names a tier ('{key}'); only profiles map behaviours to tiers"
            ),
            Problem::NoMatch => f.write_str("the rule has no match for any confidence"),
            Problem::ConfidenceWithoutEvidence(c) => write!(
                f,
                "the rule matches at confidence '{}' but has no evidence for it",
                confidence_name(*c)
            ),
            Problem::NoEvidenceText(c) => write!(
                f,
                "the evidence for confidence '{}' has no text",
                confidence_name(*c)
            ),
            Problem::EvidenceWithoutMatch(c) => write!(
                f,
                "the rule has evidence for confidence '{}' but no match for it",
                confidence_name(*c)
            ),
            Problem::TestDoesNotFit {
                field,
                field_type,
                test,
            } => write!(
                f,
                "test '{test}' does not fit field '{field}', which is {}",
                field_type.name()
            ),
            Problem::BadPattern { key, message } => write!(f, "'{key}': {message}"),
            Problem::BlankValue(key) => write!(f, "'{key}' is blank"),
            Problem::BlankEntry(key) => write!(f, "'{key}' holds a blank entry"),
            Problem::DuplicateEntry { key, value } => {
                write!(f, "'{key}' lists '{value}' more than once")
            }
            Problem::NotUserId(id) => {
                write!(f, "id '{id}' in a user folder must start with 'user/'")
            }
            Problem::CollidesWithCorpus(id) => {
                write!(f, "id '{id}' is already defined by the corpus")
            }
            Problem::AllowedAndTiered(b) => write!(
                f,
                "behaviour '{b}' is both allowed and given a tier by a control"
            ),
            Problem::ControlTieredAndNotDetectable(c) => write!(
                f,
                "control '{c}' lists behaviours and is also marked not_detectable"
            ),
            Problem::NoManifest => f.write_str("the corpus has no manifest.yaml"),
            Problem::DuplicateMember => f.write_str("the tar holds this path more than once"),
            Problem::UnexpectedFolder => {
                f.write_str("unexpected folder: a corpus holds only rules/, profiles/ and schema/")
            }
            Problem::FolderTooDeep { limit } => {
                write!(f, "the folder nests deeper than {limit} levels")
            }
            Problem::UnknownFactKind(kind) => write!(f, "unknown fact kind '{kind}'"),
            Problem::UnknownField { kind, field } => {
                write!(f, "fact kind '{}' has no field '{field}'", kind.name())
            }
            Problem::NoKind => f.write_str("a test has no fact kind: give the node a 'kind'"),
            Problem::NoTest => f.write_str("a match node has no test or combinator"),
            Problem::SeveralTests => {
                f.write_str("a match node holds more than one test or combinator")
            }
            Problem::FieldWithoutTest => f.write_str("a 'field' has no field test"),
            Problem::TestWithoutField => f.write_str("a field test has no 'field'"),
            Problem::EmptyList(key) => write!(f, "'{key}' is empty"),
            Problem::UnknownBehaviour(b) => write!(f, "behaviour '{b}' is not in the registry"),
            Problem::UserBehaviourOutsideUserRegistry(b) => write!(
                f,
                "user behaviour '{b}' is only allowed in a user folder's own registry"
            ),
            Problem::NotUserBehaviour(b) => write!(
                f,
                "a user registry may only add 'user/' behaviours, not '{b}'"
            ),
            Problem::ControlWithoutBehaviours(c) => write!(
                f,
                "control '{c}' lists no behaviours and has no not_detectable mark"
            ),
            Problem::BadId { key, value, error } => {
                write!(f, "'{key}' value '{value}' is not a valid id: {error}")
            }
            Problem::DuplicateId(id) => write!(f, "id '{id}' is defined more than once"),
            Problem::UnknownKey(key) => write!(f, "unknown key '{key}'"),
            Problem::MissingKey(key) => write!(f, "missing key '{key}'"),
            Problem::NoSchemaVersion => f.write_str("missing key 'schema'"),
            Problem::BadSchemaVersion(v) => {
                write!(f, "schema version '{v}' is not <major>.<minor>")
            }
            Problem::UnsupportedVersion(v) => write!(
                f,
                "schema version {} is not supported (this loader reads {}): {v}",
                v.found, v.supported
            ),
            Problem::BadCorpusVersion(v) => {
                write!(f, "corpus_version '{v}' is not a semantic version")
            }
            Problem::Malformed {
                line,
                column,
                message,
            } => write!(f, "malformed YAML at {line}:{column}: {message}"),
            Problem::NotUtf8 => f.write_str("the file is not UTF-8"),
            Problem::TooLarge { size, limit } => {
                write!(f, "the file is {size} bytes, over the {limit}-byte limit")
            }
            Problem::TooDeep { limit } => {
                write!(
                    f,
                    "the document nests deeper than the depth limit of {limit}"
                )
            }
            Problem::UnexpectedFile => f.write_str(
                "unexpected file: a corpus holds manifest.yaml (registry.yaml in a user folder), \
                 rules/**/*.yaml and profiles/*.yaml",
            ),
            Problem::NotRegularFile => f.write_str("not a regular file"),
            Problem::BadFileName => f.write_str("the file name is not a valid relative UTF-8 path"),
            Problem::Io(kind) => write!(f, "could not read the file: {kind}"),
        }
    }
}

fn confidence_name(c: Confidence) -> &'static str {
    match c {
        Confidence::Found => "found",
        Confidence::Suspected => "suspected",
    }
}

impl fmt::Display for CorpusError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}: {}",
            String::from_utf8_lossy(self.file.as_bytes()),
            self.problem
        )
    }
}

impl std::error::Error for CorpusError {}

#[derive(Debug)]
pub enum LoadError {
    Root(io::Error),
    Tar(io::Error),
    Refused(Vec<CorpusError>),
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LoadError::Root(e) => write!(f, "could not read the corpus folder: {e}"),
            LoadError::Tar(e) => write!(f, "could not read the corpus tar: {e}"),
            LoadError::Refused(errors) => {
                write!(f, "{} corpus file problem(s)", errors.len())?;
                for e in errors {
                    write!(f, "\n  {e}")?;
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for LoadError {}
