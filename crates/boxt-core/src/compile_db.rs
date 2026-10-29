use std::fmt;

use serde::{Deserialize, Serialize};

use crate::outcome::{FaultKind, FileOutcome, NotCheckedReason, Outcome};
use crate::text::Text;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct CompileUnit {
    pub directory: Text,
    pub file: Text,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CxxClass {
    Header,
    Source,
}

pub const SOURCE_EXTENSIONS: &[&str] = &[
    "c", "cc", "cp", "cpp", "cxx", "c++", "cppm", "ixx", "ccm", "cxxm", "c++m", "mpp",
];

pub const HEADER_EXTENSIONS: &[&str] = &[
    "h", "hh", "hp", "hpp", "hxx", "h++", "inl", "ipp", "tcc", "tpp", "txx",
];

pub fn extension_class(file_name: &[u8]) -> Option<CxxClass> {
    let name = match file_name.iter().rposition(|&c| c == b'/') {
        Some(i) => &file_name[i + 1..],
        None => file_name,
    };
    let dot = name.iter().rposition(|&c| c == b'.').filter(|&i| i > 0)?;
    let ext = &name[dot + 1..];
    let listed = |table: &[&str]| table.iter().any(|e| e.as_bytes().eq_ignore_ascii_case(ext));
    if listed(SOURCE_EXTENSIONS) {
        Some(CxxClass::Source)
    } else if listed(HEADER_EXTENSIONS) {
        Some(CxxClass::Header)
    } else {
        None
    }
}

pub fn classify(file_name: &[u8]) -> CxxClass {
    extension_class(file_name).unwrap_or(CxxClass::Header)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CompileDatabaseError {
    NotRead(NotCheckedReason),
    MatchesNoFile,
}

impl fmt::Display for CompileDatabaseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CompileDatabaseError::NotRead(reason) => {
                write!(f, "the compile database could not be read ({reason:?})")
            }
            CompileDatabaseError::MatchesNoFile => {
                f.write_str("the compile database matches no file in its tree")
            }
        }
    }
}

impl std::error::Error for CompileDatabaseError {}

impl CompileDatabaseError {
    pub fn check(job_done: Option<&Outcome>) -> Result<(), CompileDatabaseError> {
        match job_done.map(|o| &o.file) {
            Some(FileOutcome::Read) => Ok(()),
            Some(
                FileOutcome::NotChecked { reason, .. } | FileOutcome::Aggregate { reason, .. },
            ) => Err(CompileDatabaseError::NotRead(*reason)),
            Some(FileOutcome::OutOfScope { .. }) | None => Err(CompileDatabaseError::NotRead(
                NotCheckedReason::Fault(FaultKind::BadFrame),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extension_table() {
        assert_eq!(classify(b"src/main.c"), CxxClass::Source);
        assert_eq!(classify(b"a.CPP"), CxxClass::Source);
        assert_eq!(classify(b"a.C"), CxxClass::Source);
        assert_eq!(classify(b"x.c++"), CxxClass::Source);
        assert_eq!(classify(b"include/a.h"), CxxClass::Header);
        assert_eq!(classify(b"a.H"), CxxClass::Header);
        assert_eq!(classify(b"detail/impl.inl"), CxxClass::Header);
        assert_eq!(classify(b"t.tcc"), CxxClass::Header);
        assert_eq!(extension_class(b"README"), None);
        assert_eq!(extension_class(b".c"), None);
        assert_eq!(extension_class(b"dir.c/file"), None);
    }

    #[test]
    fn b15_module_extensions_are_sources_and_only_slash_splits() {
        for name in [
            "m.ccm", "m.cxxm", "m.c++m", "m.mpp", "m.cppm", "m.ixx", "M.CCM",
        ] {
            assert_eq!(
                extension_class(name.as_bytes()),
                Some(CxxClass::Source),
                "{name}"
            );
        }
        assert_eq!(extension_class(b"dir\\file.c"), Some(CxxClass::Source));
        assert_eq!(extension_class(b"a.c\\file"), None);
        assert_eq!(classify(b"a.c\\file"), CxxClass::Header);
    }

    #[test]
    fn unlisted_cxx_file_is_a_header() {
        assert_eq!(classify(b"include/vector"), CxxClass::Header);
        assert_eq!(classify(b"weird.def"), CxxClass::Header);
        assert_eq!(classify(b".c"), CxxClass::Header);
    }
}
