use std::collections::BTreeSet;
use std::fs;
use std::io::{self, Read};
use std::path::Path;

use boxt_core::Text;
use boxt_core::location::RelPath;
use serde::de::DeserializeOwned;
use serde_saphyr::budget::{Budget, BudgetBreach};
use serde_saphyr::options::{AliasLimits, DuplicateKeyPolicy, MergeKeyPolicy};
use serde_saphyr::{Error, Options};

use crate::error::{CorpusError, LoadError, Problem};
use crate::validate::{Corpus, Origin, Role, SourceFile, validate};

pub const MAX_FILE_BYTES: u64 = 1024 * 1024;
pub const MAX_DEPTH: usize = 64;
pub const MAX_FOLDER_DEPTH: usize = 16;

fn options() -> Options {
    let mut budget = Budget::default();
    budget.max_depth = MAX_DEPTH;
    budget.max_documents = 1;
    budget.max_reader_input_bytes = Some(MAX_FILE_BYTES as usize);
    let mut aliases = AliasLimits::default();
    aliases.max_replay_stack_depth = MAX_DEPTH;
    let mut options = Options::default();
    options.budget = Some(budget);
    options.duplicate_keys = DuplicateKeyPolicy::Error;
    options.merge_keys = MergeKeyPolicy::Merge;
    options.alias_limits = aliases;
    options.strict_booleans = true;
    options.reject_unsupported_tags = true;
    options.with_snippet = false;
    options
}

pub(crate) fn parse<T: DeserializeOwned>(text: &str) -> Result<T, Problem> {
    serde_saphyr::from_str_with_options(text, options()).map_err(|e| problem_of(&e))
}

fn problem_of(error: &Error) -> Problem {
    match error {
        Error::WithSnippet { error, .. } => problem_of(error),
        Error::AliasError { error, .. } => problem_of(error),
        Error::Budget {
            breach: BudgetBreach::Depth { .. },
            ..
        } => Problem::TooDeep { limit: MAX_DEPTH },
        Error::AliasReplayStackDepthExceeded { .. } => Problem::TooDeep { limit: MAX_DEPTH },
        Error::SerdeUnknownField { field, .. } => Problem::UnknownKey(field.clone()),
        Error::SerdeMissingField { field, .. } => Problem::MissingKey((*field).to_owned()),
        other => {
            let (line, column) = other
                .location()
                .map(|l| (l.line(), l.column()))
                .unwrap_or((0, 0));
            Problem::Malformed {
                line,
                column,
                message: other.without_snippet().to_string(),
            }
        }
    }
}

enum Place {
    File(Role),
    Ignore,
    UnexpectedFile,
    UnexpectedFolder(String),
}

fn place(path: &str) -> Place {
    let Some((top, rest)) = path.split_once('/') else {
        return match path {
            "manifest.yaml" => Place::File(Role::Manifest),
            "registry.yaml" => Place::File(Role::Registry),
            _ if path.ends_with(".yaml") || path.ends_with(".yml") => Place::UnexpectedFile,
            _ => Place::Ignore,
        };
    };
    let name = rest.rsplit('/').next().unwrap_or(rest);
    match top {
        "schema" => Place::Ignore,
        "rules" | "profiles" if name == ".gitkeep" => Place::Ignore,
        "rules" if rest.ends_with(".yaml") => Place::File(Role::Rule),
        "profiles" if !rest.contains('/') && rest.ends_with(".yaml") => Place::File(Role::Profile),
        "rules" | "profiles" => Place::UnexpectedFile,
        _ => Place::UnexpectedFolder(top.to_owned()),
    }
}

fn text_of(bytes: Vec<u8>) -> Result<String, Problem> {
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err(Problem::TooLarge {
            size: bytes.len() as u64,
            limit: MAX_FILE_BYTES,
        });
    }
    String::from_utf8(bytes).map_err(|_| Problem::NotUtf8)
}

fn rel(path: &str) -> RelPath {
    RelPath::new(Text::from(path)).unwrap_or_else(|_| {
        RelPath::new(Text::from(format!("<{}>", path.replace('/', "|"))))
            .unwrap_or_else(|_| RelPath::new(Text::from("<unnamed>")).expect("valid path"))
    })
}

fn join(prefix: &str, name: &str) -> String {
    if prefix.is_empty() {
        name.to_owned()
    } else {
        format!("{prefix}/{name}")
    }
}

struct Collector {
    files: Vec<SourceFile>,
    errors: Vec<CorpusError>,
    folders: BTreeSet<String>,
}

impl Collector {
    fn new() -> Collector {
        Collector {
            files: Vec::new(),
            errors: Vec::new(),
            folders: BTreeSet::new(),
        }
    }

    fn refuse(&mut self, path: &str, problem: Problem) {
        self.errors.push(CorpusError {
            file: rel(path),
            problem,
        });
    }

    fn add(&mut self, path: &str, utf8: bool, read: impl FnOnce() -> Result<String, Problem>) {
        let placed = place(path);
        if !utf8 {
            if !matches!(placed, Place::Ignore) {
                self.refuse(path, Problem::BadFileName);
            }
            return;
        }
        match placed {
            Place::Ignore => {}
            Place::UnexpectedFile => self.refuse(path, Problem::UnexpectedFile),
            Place::UnexpectedFolder(top) => {
                if self.folders.insert(top.clone()) {
                    self.refuse(&top, Problem::UnexpectedFolder);
                }
            }
            Place::File(role) => self.files.push(SourceFile {
                path: rel(path),
                role,
                text: read(),
            }),
        }
    }

    fn finish(mut self, origin: Origin, base: Option<&Corpus>) -> Result<Corpus, LoadError> {
        self.files.sort_by(|a, b| a.path.cmp(&b.path));
        let result = validate(self.files, origin, base);
        let mut errors = self.errors;
        match result {
            Ok(corpus) if errors.is_empty() => Ok(corpus),
            Ok(_) => {
                errors.sort_by(|a, b| a.file.cmp(&b.file));
                Err(LoadError::Refused(errors))
            }
            Err(more) => {
                errors.extend(more);
                errors.sort_by(|a, b| a.file.cmp(&b.file));
                Err(LoadError::Refused(errors))
            }
        }
    }
}

pub fn load_dir(path: &Path) -> Result<Corpus, LoadError> {
    collect_dir(path)?.finish(Origin::Corpus, None)
}

pub fn load_tar(bytes: &[u8]) -> Result<Corpus, LoadError> {
    collect_tar(bytes)?.finish(Origin::Corpus, None)
}

pub fn load_user_dir(path: &Path, corpus: &Corpus) -> Result<Corpus, LoadError> {
    collect_dir(path)?.finish(Origin::User, Some(corpus))
}

fn collect_dir(root: &Path) -> Result<Collector, LoadError> {
    let meta = fs::metadata(root).map_err(LoadError::Root)?;
    if !meta.is_dir() {
        return Err(LoadError::Root(io::Error::new(
            io::ErrorKind::NotADirectory,
            "the corpus path is not a directory",
        )));
    }
    let mut collector = Collector::new();
    let entries = list(root).map_err(LoadError::Root)?;
    walk("", 0, entries, &mut collector);
    Ok(collector)
}

fn list(dir: &Path) -> io::Result<Vec<(std::ffi::OsString, std::path::PathBuf)>> {
    let mut entries = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        entries.push((entry.file_name(), entry.path()));
    }
    entries.sort_by(|a, b| a.0.as_encoded_bytes().cmp(b.0.as_encoded_bytes()));
    Ok(entries)
}

fn walk(
    prefix: &str,
    depth: usize,
    entries: Vec<(std::ffi::OsString, std::path::PathBuf)>,
    collector: &mut Collector,
) {
    for (name, full) in entries {
        let utf8 = name.to_str().is_some();
        let path = join(prefix, &name.to_string_lossy().replace('/', "|"));
        let meta = match fs::symlink_metadata(&full) {
            Ok(m) => m,
            Err(e) => {
                collector.add(&path, utf8, || Err(Problem::Io(e.kind())));
                continue;
            }
        };
        if !meta.is_dir() {
            if meta.is_file() {
                let size = meta.len();
                collector.add(&path, utf8, || read_file(&full, size));
            } else {
                collector.add(&path, utf8, || Err(Problem::NotRegularFile));
            }
            continue;
        }
        if prefix.is_empty() && !matches!(path.as_str(), "rules" | "profiles") {
            if path != "schema" {
                let problem = if utf8 {
                    Problem::UnexpectedFolder
                } else {
                    Problem::BadFileName
                };
                collector.refuse(&path, problem);
            }
            continue;
        }
        if !utf8 {
            collector.refuse(&path, Problem::BadFileName);
            continue;
        }
        if depth + 1 >= MAX_FOLDER_DEPTH {
            collector.refuse(
                &path,
                Problem::FolderTooDeep {
                    limit: MAX_FOLDER_DEPTH,
                },
            );
            continue;
        }
        match list(&full) {
            Ok(children) => walk(&path, depth + 1, children, collector),
            Err(e) => collector.refuse(&path, Problem::Io(e.kind())),
        }
    }
}

fn read_file(path: &Path, size: u64) -> Result<String, Problem> {
    if size > MAX_FILE_BYTES {
        return Err(Problem::TooLarge {
            size,
            limit: MAX_FILE_BYTES,
        });
    }
    let file = fs::File::open(path).map_err(|e| Problem::Io(e.kind()))?;
    let mut bytes = Vec::new();
    file.take(MAX_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| Problem::Io(e.kind()))?;
    text_of(bytes)
}

fn collect_tar(bytes: &[u8]) -> Result<Collector, LoadError> {
    let mut collector = Collector::new();
    let mut seen = BTreeSet::new();
    let mut archive = tar::Archive::new(bytes);
    for entry in archive.entries().map_err(LoadError::Tar)? {
        let mut entry = entry.map_err(LoadError::Tar)?;
        let raw = entry.path_bytes().into_owned();
        let utf8 = std::str::from_utf8(&raw).is_ok();
        let lossy = String::from_utf8_lossy(&raw);
        let mut path = lossy.trim_end_matches('/');
        while let Some(rest) = path.strip_prefix("./") {
            path = rest;
        }
        let path = path.to_owned();
        let kind = entry.header().entry_type();
        if path.is_empty() || path == "." || kind.is_dir() {
            continue;
        }
        if !seen.insert(path.clone()) {
            collector.refuse(&path, Problem::DuplicateMember);
            continue;
        }
        if path.split('/').count() > MAX_FOLDER_DEPTH {
            collector.refuse(
                &path,
                Problem::FolderTooDeep {
                    limit: MAX_FOLDER_DEPTH,
                },
            );
            continue;
        }
        if !kind.is_file() {
            collector.add(&path, utf8, || Err(Problem::NotRegularFile));
            continue;
        }
        let size = entry.header().size().map_err(LoadError::Tar)?;
        collector.add(&path, utf8, || {
            if size > MAX_FILE_BYTES {
                return Err(Problem::TooLarge {
                    size,
                    limit: MAX_FILE_BYTES,
                });
            }
            let mut data = Vec::new();
            (&mut entry)
                .take(MAX_FILE_BYTES + 1)
                .read_to_end(&mut data)
                .map_err(|e| Problem::Io(e.kind()))?;
            text_of(data)
        });
    }
    Ok(collector)
}
