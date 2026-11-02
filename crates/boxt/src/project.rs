use std::fmt;
use std::fs;
use std::io::{self, Read};
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};

use boxt_collector::boxt_core::decode::encoding_from_name;
use boxt_collector::boxt_core::limits::{LIMITS, LimitError, LimitName, Limits};
use boxt_collector::boxt_core::pattern::{PatternError, path_pattern};
use boxt_collector::boxt_core::settings::{EncodingDecl, PathPattern};
use boxt_collector::boxt_core::verdict::{IdError, ProjectFile, TargetId};
use boxt_collector::boxt_core::version::{PROJECT_FILE_VERSION, SchemaVersion, UnsupportedVersion};
use boxt_collector::boxt_core::{SortedVec, Text};
use boxt_collector::path::{display_text_from_path, lexical, match_text};
use boxt_render::escape;
use serde::Deserialize;
use serde::de::{self, Deserializer, MapAccess, SeqAccess, Visitor};
use serde_saphyr::budget::{Budget, BudgetBreach};
use serde_saphyr::options::{AliasLimits, DuplicateKeyPolicy, MergeKeyPolicy};
use serde_saphyr::{Error, Options};

pub const NAMES: [&str; 2] = ["boxt.yaml", ".boxt.yaml"];
pub const MAX_FILE_BYTES: u64 = 1024 * 1024;
pub const MAX_DEPTH: usize = 64;
const VCS_MARKERS: [&str; 3] = [".git", ".hg", ".svn"];
const KEYS: &str = "schema, targets, jobs, limits, exclude and encodings";
const DECL_KEYS: &str = "path and encoding";
pub const ENCODING_NAMES: &str =
    "utf-8, utf-16le, utf-16be, latin-1, or windows-NNNN for a code page boxt decodes";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FileSettings {
    pub targets: Option<SortedVec<TargetId>>,
    pub jobs: Option<NonZeroU32>,
    pub limits: Vec<(LimitName, u64)>,
    pub exclude: Vec<PathPattern>,
    pub encodings: Vec<EncodingDecl>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Project {
    pub file: ProjectFile,
    pub settings: FileSettings,
    pub vcs_root: Option<PathBuf>,
    pub own_path: Option<Text>,
}

impl Project {
    pub fn warning(&self) -> Option<String> {
        let (ProjectFile::Found { path, .. }, Some(root)) = (&self.file, &self.vcs_root) else {
            return None;
        };
        self.file.above_vcs_root().then(|| {
            format!(
                "the project file {} lies above {}, the version-control root of the first input",
                escape(path),
                shown(root)
            )
        })
    }
}

#[derive(Debug)]
pub enum ProjectError {
    Absolute { input: PathBuf, error: io::Error },
    Probe { path: PathBuf, error: io::Error },
    BothNames { first: PathBuf, second: PathBuf },
    Read { path: PathBuf, error: io::Error },
    Invalid { path: PathBuf, problem: Problem },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Problem {
    TooLarge {
        size: u64,
    },
    NotUtf8,
    TooDeep,
    Malformed {
        line: u64,
        column: u64,
        message: String,
    },
    UnknownKey(String),
    UnknownLimit(String),
    UnknownDeclKey(String),
    MissingKey(String),
    BadPattern {
        key: String,
        value: String,
        error: PatternError,
    },
    BadEncoding {
        key: String,
        value: String,
    },
    BadValue {
        key: String,
        expected: &'static str,
        found: String,
    },
    BadTarget {
        key: String,
        value: String,
        error: IdError,
    },
    BadSchemaVersion(String),
    UnsupportedVersion(UnsupportedVersion),
    Limit {
        key: String,
        error: LimitError,
    },
}

pub fn encoding_problem(key: &str, value: &str) -> String {
    if value.eq_ignore_ascii_case("utf-16") {
        format!(
            "{key}: {} names no byte order; use utf-16le or utf-16be",
            quoted(value)
        )
    } else {
        format!(
            "{key}: {} is not an encoding boxt reads; use {ENCODING_NAMES}",
            quoted(value)
        )
    }
}

fn shown(path: &Path) -> String {
    escape(&display_text_from_path(path))
}

fn quoted(s: &str) -> String {
    format!("'{}'", escape(&Text::from(s)))
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Problem::TooLarge { size } => write!(
                f,
                "the file is {size} bytes, over the {MAX_FILE_BYTES}-byte limit"
            ),
            Problem::NotUtf8 => f.write_str("the file is not UTF-8"),
            Problem::TooDeep => write!(
                f,
                "the document nests deeper than the depth limit of {MAX_DEPTH}"
            ),
            Problem::Malformed {
                line,
                column,
                message,
            } => write!(f, "malformed YAML at {line}:{column}: {message}"),
            Problem::UnknownKey(key) => write!(
                f,
                "unknown key {}; a project file holds only {KEYS}",
                quoted(key)
            ),
            Problem::UnknownLimit(name) => {
                write!(
                    f,
                    "unknown key {}; limits are",
                    quoted(&format!("limits.{name}"))
                )?;
                for (i, spec) in LIMITS.iter().enumerate() {
                    f.write_str(if i == 0 { " " } else { ", " })?;
                    f.write_str(spec.flag)?;
                }
                Ok(())
            }
            Problem::UnknownDeclKey(key) => write!(
                f,
                "unknown key {}; an encoding declaration holds only {DECL_KEYS}",
                quoted(key)
            ),
            Problem::MissingKey(key) => write!(f, "missing key '{key}'"),
            Problem::BadPattern { key, value, error } => {
                write!(f, "{key}: {} is refused: {error}", quoted(value))
            }
            Problem::BadEncoding { key, value } => f.write_str(&encoding_problem(key, value)),
            Problem::BadValue {
                key,
                expected,
                found,
            } => write!(f, "{key}: expected {expected}, found {found}"),
            Problem::BadTarget { key, value, error } => {
                write!(f, "{key}: {} is not a target id: {error}", quoted(value))
            }
            Problem::BadSchemaVersion(v) => write!(
                f,
                "schema: {} is not <major>.<minor>, such as \"{PROJECT_FILE_VERSION}\"",
                quoted(v)
            ),
            Problem::UnsupportedVersion(v) => write!(f, "schema: {v}"),
            Problem::Limit { key, error } => match error {
                LimitError::Zero(_) => write!(f, "{key}: must be at least 1"),
                LimitError::AboveMax { max, .. } => write!(f, "{key}: must be at most {max}"),
                other => write!(f, "{key}: {other}"),
            },
        }
    }
}

impl fmt::Display for ProjectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProjectError::Absolute { input, error } => write!(
                f,
                "{}: cannot make the input path absolute to look for a project file: {error}",
                shown(input)
            ),
            ProjectError::Probe { path, error } => {
                write!(
                    f,
                    "{}: cannot look for a project file: {error}",
                    shown(path)
                )
            }
            ProjectError::BothNames { first, second } => write!(
                f,
                "{} and {}: one folder holds two project files; keep one",
                shown(first),
                shown(second)
            ),
            ProjectError::Read { path, error } => {
                write!(f, "{}: cannot read the project file: {error}", shown(path))
            }
            ProjectError::Invalid { path, problem } => write!(f, "{}: {problem}", shown(path)),
        }
    }
}

impl std::error::Error for ProjectError {}

fn own_path(path: &Path) -> Result<Text, ProjectError> {
    match_text(path).map_err(|error| ProjectError::Absolute {
        input: path.to_path_buf(),
        error,
    })
}

pub fn load(first_input: &Path, named: Option<&Path>) -> Result<Project, ProjectError> {
    if let Some(path) = named {
        return Ok(Project {
            file: ProjectFile::Named {
                path: display_text_from_path(path),
            },
            settings: read(path)?,
            vcs_root: None,
            own_path: Some(own_path(path)?),
        });
    }
    let start = search_start(first_input)?;
    let vcs_root = vcs_root(&start)?;
    let Some(found) = search(&start)? else {
        return Ok(Project {
            file: ProjectFile::NoneFound,
            settings: FileSettings::default(),
            vcs_root,
            own_path: None,
        });
    };
    let above_vcs_root = found
        .parent()
        .zip(vcs_root.as_deref())
        .is_some_and(|(folder, root)| folder != root && root.starts_with(folder));
    Ok(Project {
        file: ProjectFile::Found {
            path: display_text_from_path(&found),
            above_vcs_root,
        },
        settings: read(&found)?,
        vcs_root,
        own_path: Some(own_path(&found)?),
    })
}

fn search_start(first_input: &Path) -> Result<PathBuf, ProjectError> {
    let absolute = std::path::absolute(first_input).map_err(|error| ProjectError::Absolute {
        input: first_input.to_path_buf(),
        error,
    })?;
    let absolute = lexical(&absolute);
    let is_file = fs::metadata(&absolute).is_ok_and(|m| m.is_file());
    match absolute.parent() {
        Some(parent) if is_file => Ok(parent.to_path_buf()),
        _ => Ok(absolute),
    }
}

fn exists(path: &Path) -> Result<bool, ProjectError> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(e)
            if matches!(
                e.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
            ) =>
        {
            Ok(false)
        }
        Err(error) => Err(ProjectError::Probe {
            path: path.to_path_buf(),
            error,
        }),
    }
}

fn vcs_root(start: &Path) -> Result<Option<PathBuf>, ProjectError> {
    for folder in start.ancestors() {
        for marker in VCS_MARKERS {
            if exists(&folder.join(marker))? {
                return Ok(Some(folder.to_path_buf()));
            }
        }
    }
    Ok(None)
}

fn search(start: &Path) -> Result<Option<PathBuf>, ProjectError> {
    for folder in start.ancestors() {
        let mut here = Vec::with_capacity(NAMES.len());
        for name in NAMES {
            let candidate = folder.join(name);
            if exists(&candidate)? {
                here.push(candidate);
            }
        }
        let mut here = here.into_iter();
        match (here.next(), here.next()) {
            (Some(first), Some(second)) => return Err(ProjectError::BothNames { first, second }),
            (Some(only), None) => return Ok(Some(only)),
            _ => {}
        }
    }
    Ok(None)
}

fn read(path: &Path) -> Result<FileSettings, ProjectError> {
    let invalid = |problem| ProjectError::Invalid {
        path: path.to_path_buf(),
        problem,
    };
    let read_error = |error| ProjectError::Read {
        path: path.to_path_buf(),
        error,
    };
    let file = fs::File::open(path).map_err(read_error)?;
    let mut bytes = Vec::new();
    file.take(MAX_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(read_error)?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        let size = fs::metadata(path).map_or(bytes.len() as u64, |m| m.len());
        return Err(invalid(Problem::TooLarge { size }));
    }
    let text = String::from_utf8(bytes).map_err(|_| invalid(Problem::NotUtf8))?;
    let folder = own_path(path)?;
    let base = match folder.as_bytes().iter().rposition(|&b| b == b'/') {
        Some(0) => Text::from("/"),
        Some(i) => Text::from_bytes(folder.as_bytes()[..i].to_vec()),
        None => folder,
    };
    parse(&text, &base).map_err(invalid)
}

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

fn problem_of(error: &Error) -> Problem {
    match error {
        Error::WithSnippet { error, .. } => problem_of(error),
        Error::AliasError { error, .. } => problem_of(error),
        Error::Budget {
            breach: BudgetBreach::Depth { .. },
            ..
        } => Problem::TooDeep,
        Error::AliasReplayStackDepthExceeded { .. } => Problem::TooDeep,
        Error::SerdeUnknownField { field, .. } => Problem::UnknownKey(field.clone()),
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

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Raw {
    #[serde(default, deserialize_with = "present")]
    schema: Option<Value>,
    #[serde(default, deserialize_with = "present")]
    targets: Option<Value>,
    #[serde(default, deserialize_with = "present")]
    jobs: Option<Value>,
    #[serde(default, deserialize_with = "present")]
    limits: Option<Value>,
    #[serde(default, deserialize_with = "present")]
    exclude: Option<Value>,
    #[serde(default, deserialize_with = "present")]
    encodings: Option<Value>,
}

fn present<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<Value>, D::Error> {
    Value::deserialize(deserializer).map(Some)
}

#[derive(Debug, Clone, PartialEq)]
enum Value {
    Null,
    Bool(bool),
    Int(i128),
    Float(f64),
    Str(String),
    Seq(Vec<Value>),
    Map(Vec<(Value, Value)>),
}

impl Value {
    fn found(&self) -> String {
        match self {
            Value::Null => "nothing".to_owned(),
            Value::Bool(b) => b.to_string(),
            Value::Int(n) => n.to_string(),
            Value::Float(x) => format!("{x:?}"),
            Value::Str(s) => quoted(s),
            Value::Seq(items) if items.is_empty() => "an empty list".to_owned(),
            Value::Seq(_) => "a list".to_owned(),
            Value::Map(_) => "a mapping".to_owned(),
        }
    }
}

struct ValueVisitor;

impl<'de> Visitor<'de> for ValueVisitor {
    type Value = Value;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("any YAML value")
    }

    fn visit_unit<E: de::Error>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }

    fn visit_none<E: de::Error>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }

    fn visit_some<D: Deserializer<'de>>(self, deserializer: D) -> Result<Value, D::Error> {
        Value::deserialize(deserializer)
    }

    fn visit_bool<E: de::Error>(self, v: bool) -> Result<Value, E> {
        Ok(Value::Bool(v))
    }

    fn visit_i64<E: de::Error>(self, v: i64) -> Result<Value, E> {
        Ok(Value::Int(i128::from(v)))
    }

    fn visit_u64<E: de::Error>(self, v: u64) -> Result<Value, E> {
        Ok(Value::Int(i128::from(v)))
    }

    fn visit_i128<E: de::Error>(self, v: i128) -> Result<Value, E> {
        Ok(Value::Int(v))
    }

    fn visit_u128<E: de::Error>(self, v: u128) -> Result<Value, E> {
        Ok(i128::try_from(v).map_or(Value::Float(v as f64), Value::Int))
    }

    fn visit_f64<E: de::Error>(self, v: f64) -> Result<Value, E> {
        Ok(Value::Float(v))
    }

    fn visit_str<E: de::Error>(self, v: &str) -> Result<Value, E> {
        Ok(Value::Str(v.to_owned()))
    }

    fn visit_string<E: de::Error>(self, v: String) -> Result<Value, E> {
        Ok(Value::Str(v))
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Value, A::Error> {
        let mut items = Vec::new();
        while let Some(item) = seq.next_element()? {
            items.push(item);
        }
        Ok(Value::Seq(items))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Value, A::Error> {
        let mut entries = Vec::new();
        while let Some(entry) = map.next_entry()? {
            entries.push(entry);
        }
        Ok(Value::Map(entries))
    }
}

impl<'de> Deserialize<'de> for Value {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(ValueVisitor)
    }
}

pub fn parse(text: &str, base: &Text) -> Result<FileSettings, Problem> {
    let raw: Raw =
        serde_saphyr::from_str_with_options(text, options()).map_err(|e| problem_of(&e))?;
    check_schema(raw.schema)?;
    Ok(FileSettings {
        targets: raw.targets.map(targets).transpose()?,
        jobs: raw.jobs.map(jobs).transpose()?,
        limits: raw.limits.map(limits).transpose()?.unwrap_or_default(),
        exclude: raw
            .exclude
            .map(|v| exclude(v, base))
            .transpose()?
            .unwrap_or_default(),
        encodings: raw
            .encodings
            .map(|v| encodings(v, base))
            .transpose()?
            .unwrap_or_default(),
    })
}

fn list(key: &str, value: Value, expected: &'static str) -> Result<Vec<Value>, Problem> {
    match value {
        Value::Seq(items) => Ok(items),
        other => Err(Problem::BadValue {
            key: key.to_owned(),
            expected,
            found: other.found(),
        }),
    }
}

fn pattern(key: String, value: Value, base: &Text) -> Result<PathPattern, Problem> {
    let Value::Str(source) = value else {
        return Err(Problem::BadValue {
            key,
            expected: "a path pattern",
            found: value.found(),
        });
    };
    let given = PathPattern {
        base: base.clone(),
        pattern: Text::from(source.as_str()),
    };
    path_pattern(&given.base, &given.pattern).map_err(|error| Problem::BadPattern {
        key,
        value: source,
        error,
    })?;
    Ok(given)
}

fn exclude(value: Value, base: &Text) -> Result<Vec<PathPattern>, Problem> {
    list("exclude", value, "a list of path patterns")?
        .into_iter()
        .enumerate()
        .map(|(i, item)| pattern(format!("exclude[{i}]"), item, base))
        .collect()
}

fn encodings(value: Value, base: &Text) -> Result<Vec<EncodingDecl>, Problem> {
    list(
        "encodings",
        value,
        "a list of {path: PATTERN, encoding: NAME} mappings",
    )?
    .into_iter()
    .enumerate()
    .map(|(i, item)| declaration(i, item, base))
    .collect()
}

fn declaration(i: usize, value: Value, base: &Text) -> Result<EncodingDecl, Problem> {
    let key = format!("encodings[{i}]");
    let Value::Map(entries) = value else {
        return Err(Problem::BadValue {
            key,
            expected: "a mapping with path and encoding",
            found: value.found(),
        });
    };
    let mut path = None;
    let mut name = None;
    for (field, value) in entries {
        match field {
            Value::Str(f) if f == "path" => path = Some(value),
            Value::Str(f) if f == "encoding" => name = Some(value),
            Value::Str(f) => return Err(Problem::UnknownDeclKey(format!("{key}.{f}"))),
            other => {
                return Err(Problem::BadValue {
                    key,
                    expected: "path and encoding as keys",
                    found: other.found(),
                });
            }
        }
    }
    let path = path.ok_or_else(|| Problem::MissingKey(format!("{key}.path")))?;
    let name = name.ok_or_else(|| Problem::MissingKey(format!("{key}.encoding")))?;
    let pattern = pattern(format!("{key}.path"), path, base)?;
    let key = format!("{key}.encoding");
    let Value::Str(name) = name else {
        return Err(Problem::BadValue {
            key,
            expected: "an encoding name",
            found: name.found(),
        });
    };
    let encoding = encoding_from_name(&name).ok_or(Problem::BadEncoding { key, value: name })?;
    Ok(EncodingDecl { pattern, encoding })
}

fn check_schema(value: Option<Value>) -> Result<(), Problem> {
    let raw = match value {
        None => return Err(Problem::MissingKey("schema".to_owned())),
        Some(Value::Str(raw)) => raw,
        Some(other) => {
            return Err(Problem::BadValue {
                key: "schema".to_owned(),
                expected: "a quoted version such as \"1.0\"",
                found: other.found(),
            });
        }
    };
    let found = raw
        .parse::<SchemaVersion>()
        .map_err(|_| Problem::BadSchemaVersion(raw))?;
    PROJECT_FILE_VERSION
        .check(found)
        .map_err(Problem::UnsupportedVersion)
}

fn targets(value: Value) -> Result<SortedVec<TargetId>, Problem> {
    let items = match value {
        Value::Seq(items) if !items.is_empty() => items,
        _ => {
            return Err(Problem::BadValue {
                key: "targets".to_owned(),
                expected: "a list of one or more target ids",
                found: value.found(),
            });
        }
    };
    items
        .into_iter()
        .enumerate()
        .map(|(i, item)| {
            let key = format!("targets[{i}]");
            match item {
                Value::Str(s) => TargetId::new(s.as_str()).map_err(|error| Problem::BadTarget {
                    key,
                    value: s,
                    error,
                }),
                other => Err(Problem::BadValue {
                    key,
                    expected: "a target id",
                    found: other.found(),
                }),
            }
        })
        .collect()
}

fn jobs(value: Value) -> Result<NonZeroU32, Problem> {
    match value {
        Value::Int(n) => u32::try_from(n).ok().and_then(NonZeroU32::new),
        _ => None,
    }
    .ok_or_else(|| Problem::BadValue {
        key: "jobs".to_owned(),
        expected: "a whole number from 1 to 4294967295",
        found: value.found(),
    })
}

fn limits(value: Value) -> Result<Vec<(LimitName, u64)>, Problem> {
    let Value::Map(entries) = value else {
        return Err(Problem::BadValue {
            key: "limits".to_owned(),
            expected: "a mapping of limit names to whole numbers",
            found: value.found(),
        });
    };
    let mut scratch = Limits::default();
    let mut out = Vec::with_capacity(entries.len());
    for (key, value) in entries {
        let Value::Str(flag) = key else {
            return Err(Problem::BadValue {
                key: "limits".to_owned(),
                expected: "limit names as keys",
                found: key.found(),
            });
        };
        let name =
            LimitName::from_flag(&flag).ok_or_else(|| Problem::UnknownLimit(flag.clone()))?;
        let key = format!("limits.{flag}");
        let number = match &value {
            Value::Int(n) => u64::try_from(*n).ok(),
            _ => None,
        }
        .ok_or_else(|| Problem::BadValue {
            key: key.clone(),
            expected: "a whole number",
            found: value.found(),
        })?;
        scratch
            .set(name, number)
            .map_err(|error| Problem::Limit { key, error })?;
        out.push((name, number));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use boxt_collector::boxt_core::settings::Encoding;

    use super::*;

    fn parse(text: &str) -> Result<FileSettings, Problem> {
        super::parse(text, &Text::from("/repo"))
    }

    fn problem(text: &str) -> String {
        parse(text).unwrap_err().to_string()
    }

    #[test]
    fn every_key_parses() {
        let settings = parse(
            "schema: \"1.0\"\ntargets: [rootless-podman, docker-default]\njobs: 3\nlimits:\n  work: 5\n  file-size: 1024\n",
        )
        .unwrap();
        assert_eq!(
            settings
                .targets
                .unwrap()
                .iter()
                .map(|t| t.as_str())
                .collect::<Vec<_>>(),
            ["docker-default", "rootless-podman"]
        );
        assert_eq!(settings.jobs, NonZeroU32::new(3));
        assert_eq!(
            settings.limits,
            [(LimitName::Work, 5), (LimitName::FileSize, 1024)]
        );
    }

    #[test]
    fn only_the_schema_is_required() {
        assert_eq!(parse("schema: \"1.0\"\n").unwrap(), FileSettings::default());
        assert_eq!(problem("jobs: 2\n"), "missing key 'schema'");
    }

    #[test]
    fn every_limit_can_be_set_by_its_flag_name() {
        let mut text = String::from("schema: \"1.0\"\nlimits:\n");
        for spec in LIMITS {
            text.push_str(&format!("  {}: {}\n", spec.flag, spec.default));
        }
        assert_eq!(parse(&text).unwrap().limits.len(), LimitName::ALL.len());
    }

    #[test]
    fn unknown_keys_are_named() {
        let top = problem("schema: \"1.0\"\ncolour: red\n");
        assert!(top.starts_with("unknown key 'colour'"), "{top}");
        let limit = problem("schema: \"1.0\"\nlimits: {wrk: 5}\n");
        assert!(
            limit.starts_with("unknown key 'limits.wrk'; limits are file-size, work,"),
            "{limit}"
        );
        let snake = problem("schema: \"1.0\"\nlimits: {file_size: 5}\n");
        assert!(
            snake.starts_with("unknown key 'limits.file_size'"),
            "{snake}"
        );
        assert!(problem("schema: \"1.0\"\ninputs: [src]\n").starts_with("unknown key 'inputs'"));
    }

    #[test]
    fn bad_values_name_their_key() {
        for (text, expected) in [
            (
                "jobs: many",
                "jobs: expected a whole number from 1 to 4294967295, found 'many'",
            ),
            (
                "jobs: 0",
                "jobs: expected a whole number from 1 to 4294967295, found 0",
            ),
            (
                "jobs: -1",
                "jobs: expected a whole number from 1 to 4294967295, found -1",
            ),
            (
                "jobs: true",
                "jobs: expected a whole number from 1 to 4294967295, found true",
            ),
            (
                "jobs:",
                "jobs: expected a whole number from 1 to 4294967295, found nothing",
            ),
            ("limits: {work: 0}", "limits.work: must be at least 1"),
            (
                "limits: {work: many}",
                "limits.work: expected a whole number, found 'many'",
            ),
            (
                "limits: {work: 1.5}",
                "limits.work: expected a whole number, found 1.5",
            ),
            (
                "limits: {entry-message-bytes: 4294967296}",
                "limits.entry-message-bytes: must be at most 4294967295",
            ),
            (
                "limits: [work]",
                "limits: expected a mapping of limit names to whole numbers, found a list",
            ),
            (
                "targets: []",
                "targets: expected a list of one or more target ids, found an empty list",
            ),
            (
                "targets: docker-default",
                "targets: expected a list of one or more target ids, found 'docker-default'",
            ),
            (
                "targets: [Docker]",
                "targets[0]: 'Docker' is not a target id: ",
            ),
            (
                "targets: [[a]]",
                "targets[0]: expected a target id, found a list",
            ),
        ] {
            let got = problem(&format!("schema: \"1.0\"\n{text}\n"));
            assert!(got.starts_with(expected), "{text}: {got}");
        }
    }

    #[test]
    fn the_version_is_checked() {
        let newer = problem("schema: \"1.9\"\n");
        assert!(newer.contains("1.9") && newer.contains("1.0"), "{newer}");
        assert!(problem("schema: \"2.0\"\n").contains("2.0"));
        assert!(problem("schema: \"one\"\n").starts_with("schema: 'one' is not <major>.<minor>"));
        assert!(problem("schema: 1.0\n").starts_with("schema: expected a quoted version"));
        assert!(parse("schema: \"1.0\"\n").is_ok());
    }

    #[test]
    fn the_corpus_loader_strictness_applies() {
        assert!(problem("schema: \"1.0\"\njobs: 2\njobs: 3\n").starts_with("malformed YAML"));
        assert!(problem("schema: \"1.0\"\n---\nschema: \"1.0\"\n").starts_with("malformed YAML"));
        for word in ["yes", "on"] {
            let got = problem(&format!("schema: \"1.0\"\njobs: {word}\n"));
            assert!(got.ends_with(&format!("found '{word}'")), "{got}");
        }
        assert!(parse("schema: \"1.0\"\njobs: !foo 3\n").is_err());
        let deep = format!(
            "schema: \"1.0\"\nlimits: {}{}\n",
            "[".repeat(100),
            "]".repeat(100)
        );
        assert_eq!(problem(&deep), Problem::TooDeep.to_string());
    }

    #[test]
    fn the_documented_example_parses_and_every_limit_is_documented() {
        let doc = include_str!("../PROJECT-FILE.md");
        let example = doc
            .split("```yaml\n")
            .nth(1)
            .unwrap()
            .split("```")
            .next()
            .unwrap();
        let example = parse(example).unwrap();
        assert!(example.targets.is_some());
        assert!(!example.exclude.is_empty());
        assert!(!example.encodings.is_empty());
        assert!(doc.contains(&format!("now `{PROJECT_FILE_VERSION}`")));
        for spec in LIMITS {
            assert!(doc.contains(&format!("`{}`", spec.flag)), "{}", spec.flag);
        }
        for flag in [
            "`--target ID`",
            "`--jobs N`",
            "`--limit NAME=VALUE`",
            "`--project FILE`",
            "`--exclude PATTERN`",
            "`--encoding PATTERN=NAME`",
        ] {
            assert!(doc.contains(flag), "{flag}");
        }
    }

    #[test]
    fn excludes_and_encodings_keep_the_file_base_and_their_order() {
        let settings = parse(
            "schema: \"1.0\"\nexclude: [third_party/**, \"tests/*.c\"]\nencodings:\n  - {path: legacy/**, encoding: Windows-1252}\n  - {path: \"**/*.c\", encoding: utf-16le}\n",
        )
        .unwrap();
        let at = |p: &str| PathPattern {
            base: Text::from("/repo"),
            pattern: Text::from(p),
        };
        assert_eq!(settings.exclude, [at("third_party/**"), at("tests/*.c")]);
        assert_eq!(
            settings.encodings,
            [
                EncodingDecl {
                    pattern: at("legacy/**"),
                    encoding: Encoding::WindowsCodePage(1252),
                },
                EncodingDecl {
                    pattern: at("**/*.c"),
                    encoding: Encoding::Utf16Le,
                },
            ]
        );
        assert_eq!(parse("schema: \"1.0\"\nexclude: []\n").unwrap().exclude, []);
    }

    #[test]
    fn bad_patterns_and_encodings_name_their_key() {
        for (text, expected) in [
            (
                "exclude: [\"../x/**\"]",
                "exclude[0]: '../x/**' is refused: the pattern has a '.' or '..' segment",
            ),
            (
                "exclude: [a/**, \"\"]",
                "exclude[1]: '' is refused: the pattern is empty",
            ),
            (
                "exclude: [/abs/**]",
                "exclude[0]: '/abs/**' is refused: the pattern is absolute",
            ),
            (
                "exclude: [\"[a\"]",
                "exclude[0]: '[a' is refused: the pattern is not a valid glob",
            ),
            (
                "exclude: [3]",
                "exclude[0]: expected a path pattern, found 3",
            ),
            (
                "exclude: third_party",
                "exclude: expected a list of path patterns, found 'third_party'",
            ),
            (
                "encodings: [{path: a/**, encoding: windows-437}]",
                "encodings[0].encoding: 'windows-437' is not an encoding boxt reads; use utf-8,",
            ),
            (
                "encodings: [{path: a/**, encoding: UTF-16}]",
                "encodings[0].encoding: 'UTF-16' names no byte order; use utf-16le or utf-16be",
            ),
            (
                "encodings: [{path: a/**, encoding: cp1252}]",
                "encodings[0].encoding: 'cp1252' is not an encoding",
            ),
            (
                "encodings: [{path: a/**, encoding: utf-8}, {path: ../b, encoding: utf-8}]",
                "encodings[1].path: '../b' is refused:",
            ),
            (
                "encodings: [{path: a/**}]",
                "missing key 'encodings[0].encoding'",
            ),
            (
                "encodings: [{encoding: utf-8}]",
                "missing key 'encodings[0].path'",
            ),
            (
                "encodings: [{path: a, encoding: utf-8, colour: red}]",
                "unknown key 'encodings[0].colour'; an encoding declaration holds only path and encoding",
            ),
            (
                "encodings: [a/**]",
                "encodings[0]: expected a mapping with path and encoding, found 'a/**'",
            ),
            (
                "encodings: {a: utf-8}",
                "encodings: expected a list of {path: PATTERN, encoding: NAME} mappings, found a mapping",
            ),
        ] {
            let got = problem(&format!("schema: \"1.0\"\n{text}\n"));
            assert!(got.starts_with(expected), "{text}: {got}");
        }
    }
}
