use std::fmt;

use serde::{Deserialize, Serialize};

use crate::location::{Location, PathText, Scope, ScopeKind, ScriptName};
use crate::shell::{Assignment, Redirection};
use crate::sorted::SortedVec;
use crate::text::Text;

closed_enum! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub enum FileKind {
        Source,
        Binary,
        Package,
        Service,
        Config,
    }
}

impl FileKind {
    pub const fn name(self) -> &'static str {
        match self {
            FileKind::Source => "source",
            FileKind::Binary => "binary",
            FileKind::Package => "package",
            FileKind::Service => "service",
            FileKind::Config => "config",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RegionId(pub u32);

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum KeySegment {
    Name(Text),
    Index(u64),
    Attribute(Text),
}

fn needs_quoting(bytes: &[u8]) -> bool {
    bytes.is_empty()
        || bytes[0] == b'@'
        || bytes
            .iter()
            .any(|&c| c == b'.' || c == b'[' || c < 0x20 || c == 0x7f)
}

fn push_quoted(out: &mut Vec<u8>, bytes: &[u8]) {
    out.extend_from_slice(b"[\"");
    for &c in bytes {
        match c {
            b'"' => out.extend_from_slice(b"\\\""),
            b'\\' => out.extend_from_slice(b"\\\\"),
            b'\n' => out.extend_from_slice(b"\\n"),
            b'\r' => out.extend_from_slice(b"\\r"),
            b'\t' => out.extend_from_slice(b"\\t"),
            0x08 => out.extend_from_slice(b"\\b"),
            0x0c => out.extend_from_slice(b"\\f"),
            c if c < 0x20 || c == 0x7f => {
                out.extend_from_slice(format!("\\u{c:04x}").as_bytes());
            }
            c => out.push(c),
        }
    }
    out.extend_from_slice(b"\"]");
}

pub fn config_key_path(segments: &[KeySegment]) -> Text {
    let mut out = Vec::new();
    for (i, segment) in segments.iter().enumerate() {
        let bare = |out: &mut Vec<u8>, bytes: &[u8]| {
            if i > 0 {
                out.push(b'.');
            }
            out.extend_from_slice(bytes);
        };
        match segment {
            KeySegment::Index(n) => out.extend_from_slice(format!("[{n}]").as_bytes()),
            KeySegment::Name(name) if needs_quoting(name.as_bytes()) => {
                push_quoted(&mut out, name.as_bytes());
            }
            KeySegment::Name(name) => bare(&mut out, name.as_bytes()),
            KeySegment::Attribute(name) => {
                let mut at = vec![b'@'];
                at.extend_from_slice(name.as_bytes());
                if needs_quoting(&at[1..]) {
                    push_quoted(&mut out, &at);
                } else {
                    bare(&mut out, &at);
                }
            }
        }
    }
    Text::from_bytes(out)
}

closed_enum! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub enum Directive {
        If,
        Ifdef,
        Ifndef,
        Elif,
        Elifdef,
        Elifndef,
        Else,
    }
}

impl Directive {
    pub const fn opens_chain(self) -> bool {
        matches!(self, Directive::If | Directive::Ifdef | Directive::Ifndef)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct GuardFrame {
    pub directive: Directive,
    pub text: Text,
    pub identifiers: SortedVec<Text>,
    pub holds_code: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct GuardChain {
    pub frames: Vec<GuardFrame>,
    pub has_alternative: bool,
    pub region: RegionId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuardError {
    Empty,
    NotOpenedByIf,
    OpenerInsideChain,
    ElseNotLast,
    HoldsCodeNotLast,
}

impl fmt::Display for GuardError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            GuardError::Empty => "guard chain has no directive",
            GuardError::NotOpenedByIf => "guard chain does not open with #if, #ifdef or #ifndef",
            GuardError::OpenerInsideChain => "guard chain has a second opening directive",
            GuardError::ElseNotLast => "guard chain has a directive after #else",
            GuardError::HoldsCodeNotLast => {
                "guard chain must hold the code in its last directive only"
            }
        })
    }
}

impl std::error::Error for GuardError {}

impl GuardChain {
    pub fn validate(&self) -> Result<(), GuardError> {
        let (last, rest) = self.frames.split_last().ok_or(GuardError::Empty)?;
        if !self.frames[0].directive.opens_chain() {
            return Err(GuardError::NotOpenedByIf);
        }
        if self.frames[1..].iter().any(|f| f.directive.opens_chain()) {
            return Err(GuardError::OpenerInsideChain);
        }
        if rest.iter().any(|f| f.directive == Directive::Else) {
            return Err(GuardError::ElseNotLast);
        }
        if !last.holds_code || rest.iter().any(|f| f.holds_code) {
            return Err(GuardError::HoldsCodeNotLast);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Fact {
    Call {
        file_kind: FileKind,
        location: Location,
        callee: Text,
        args: Vec<Text>,
        guards: Vec<GuardChain>,
    },
    Include {
        file_kind: FileKind,
        location: Location,
        target: Text,
        system: bool,
        guards: Vec<GuardChain>,
    },
    StringLiteral {
        file_kind: FileKind,
        location: Location,
        value: Text,
        guards: Vec<GuardChain>,
    },
    ConfigKey {
        file_kind: FileKind,
        location: Location,
        key: Text,
        value: Option<Text>,
    },
    GuardRegion {
        file_kind: FileKind,
        location: Location,
        region: RegionId,
        guards: Vec<GuardChain>,
        end_line: u32,
    },
    Import {
        file_kind: FileKind,
        location: Location,
        symbol: Text,
        version: Option<Text>,
    },
    Export {
        file_kind: FileKind,
        location: Location,
        symbol: Text,
        version: Option<Text>,
    },
    NeededLibrary {
        file_kind: FileKind,
        location: Location,
        name: Text,
    },
    PackageFile {
        file_kind: FileKind,
        location: Location,
        path: PathText,
        mode: Option<u32>,
        user: Option<Text>,
        group: Option<Text>,
    },
    PackageDependency {
        file_kind: FileKind,
        location: Location,
        name: Text,
        constraint: Option<Text>,
    },
    PackageScript {
        file_kind: FileKind,
        location: Location,
        name: ScriptName,
        interpreter: Option<Text>,
    },
    ServiceSetting {
        file_kind: FileKind,
        location: Location,
        section: Text,
        key: Text,
        value: Text,
    },
    ShellCommand {
        file_kind: FileKind,
        location: Location,
        assignments: Vec<Assignment>,
        argv: Vec<Text>,
        redirections: Vec<Redirection>,
    },
}

closed_enum! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub enum FactKind {
        Call,
        Include,
        StringLiteral,
        ConfigKey,
        GuardRegion,
        Import,
        Export,
        NeededLibrary,
        PackageFile,
        PackageDependency,
        PackageScript,
        ServiceSetting,
        ShellCommand,
    }
}

closed_enum! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub enum PathRule {
        File,
        Frame,
    }
}

impl PathRule {
    pub const fn name(self) -> &'static str {
        match self {
            PathRule::File => "file",
            PathRule::Frame => "frame",
        }
    }
}

closed_enum! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub enum LineRule {
        Required,
        Absent,
    }
}

impl LineRule {
    pub const fn name(self) -> &'static str {
        match self {
            LineRule::Required => "required",
            LineRule::Absent => "absent",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceRule {
    pub file_kind: FileKind,
    pub scopes: &'static [ScopeKind],
    pub path: PathRule,
    pub line: LineRule,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FactKindSpec {
    pub kind: FactKind,
    pub name: &'static str,
    pub sources: &'static [SourceRule],
    pub fields: &'static [&'static str],
}

pub const COMMON_FIELDS: &[&str] = &["file_kind", "location"];

const IN_CODE: &[ScopeKind] = &[ScopeKind::File, ScopeKind::Function];
const FILE: &[ScopeKind] = &[ScopeKind::File];
const SYMBOL: &[ScopeKind] = &[ScopeKind::Symbol];
const SCRIPT: &[ScopeKind] = &[ScopeKind::Script];

const fn row(
    file_kind: FileKind,
    scopes: &'static [ScopeKind],
    path: PathRule,
    line: LineRule,
) -> SourceRule {
    SourceRule {
        file_kind,
        scopes,
        path,
        line,
    }
}

const SOURCE_CODE: &[SourceRule] = &[row(
    FileKind::Source,
    IN_CODE,
    PathRule::File,
    LineRule::Required,
)];
const BINARY_SYMBOL: &[SourceRule] = &[row(
    FileKind::Binary,
    SYMBOL,
    PathRule::File,
    LineRule::Absent,
)];
const PACKAGE_HEADER: &[SourceRule] = &[row(
    FileKind::Package,
    FILE,
    PathRule::Frame,
    LineRule::Absent,
)];

pub const FACT_KINDS: &[FactKindSpec] = &[
    FactKindSpec {
        kind: FactKind::Call,
        name: "call",
        sources: SOURCE_CODE,
        fields: &["callee", "args", "guards"],
    },
    FactKindSpec {
        kind: FactKind::Include,
        name: "include",
        sources: SOURCE_CODE,
        fields: &["target", "system", "guards"],
    },
    FactKindSpec {
        kind: FactKind::StringLiteral,
        name: "string_literal",
        sources: &[
            row(
                FileKind::Source,
                IN_CODE,
                PathRule::File,
                LineRule::Required,
            ),
            row(FileKind::Binary, FILE, PathRule::File, LineRule::Absent),
        ],
        fields: &["value", "guards"],
    },
    FactKindSpec {
        kind: FactKind::ConfigKey,
        name: "config_key",
        sources: &[row(
            FileKind::Config,
            &[ScopeKind::ConfigKey],
            PathRule::File,
            LineRule::Required,
        )],
        fields: &["key", "value"],
    },
    FactKindSpec {
        kind: FactKind::GuardRegion,
        name: "guard_region",
        sources: SOURCE_CODE,
        fields: &["region", "guards", "end_line"],
    },
    FactKindSpec {
        kind: FactKind::Import,
        name: "import",
        sources: BINARY_SYMBOL,
        fields: &["symbol", "version"],
    },
    FactKindSpec {
        kind: FactKind::Export,
        name: "export",
        sources: BINARY_SYMBOL,
        fields: &["symbol", "version"],
    },
    FactKindSpec {
        kind: FactKind::NeededLibrary,
        name: "needed_library",
        sources: &[row(
            FileKind::Binary,
            FILE,
            PathRule::File,
            LineRule::Absent,
        )],
        fields: &["name"],
    },
    FactKindSpec {
        kind: FactKind::PackageFile,
        name: "package_file",
        sources: PACKAGE_HEADER,
        fields: &["path", "mode", "user", "group"],
    },
    FactKindSpec {
        kind: FactKind::PackageDependency,
        name: "package_dependency",
        sources: PACKAGE_HEADER,
        fields: &["name", "constraint"],
    },
    FactKindSpec {
        kind: FactKind::PackageScript,
        name: "package_script",
        sources: &[row(
            FileKind::Package,
            SCRIPT,
            PathRule::Frame,
            LineRule::Absent,
        )],
        fields: &["name", "interpreter"],
    },
    FactKindSpec {
        kind: FactKind::ServiceSetting,
        name: "service_setting",
        sources: &[row(
            FileKind::Service,
            &[ScopeKind::UnitSection],
            PathRule::File,
            LineRule::Required,
        )],
        fields: &["section", "key", "value"],
    },
    FactKindSpec {
        kind: FactKind::ShellCommand,
        name: "shell_command",
        sources: &[
            row(
                FileKind::Package,
                SCRIPT,
                PathRule::Frame,
                LineRule::Required,
            ),
            row(FileKind::Service, FILE, PathRule::File, LineRule::Required),
        ],
        fields: &["assignments", "argv", "redirections"],
    },
];

impl FactKind {
    pub fn spec(self) -> &'static FactKindSpec {
        &FACT_KINDS[self as usize]
    }

    pub fn name(self) -> &'static str {
        self.spec().name
    }

    pub fn file_kinds(self) -> Vec<FileKind> {
        self.spec().sources.iter().map(|r| r.file_kind).collect()
    }

    pub fn source(self, file_kind: FileKind) -> Option<&'static SourceRule> {
        self.spec()
            .sources
            .iter()
            .find(|r| r.file_kind == file_kind)
    }

    pub fn fields(self) -> &'static [&'static str] {
        self.spec().fields
    }

    pub fn from_name(name: &str) -> Option<FactKind> {
        FACT_KINDS.iter().find(|s| s.name == name).map(|s| s.kind)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuardRegionError {
    NoGuards,
    EndBeforeLine,
    RegionMismatch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FactError {
    FileKindNotAllowed { kind: FactKind, file_kind: FileKind },
    ScopeNotAllowed { kind: FactKind, scope: ScopeKind },
    PathNotAllowed { kind: FactKind, rule: PathRule },
    LineNotAllowed { kind: FactKind, rule: LineRule },
    NoFrame { kind: FactKind },
    LineZero { kind: FactKind },
    ScopeDiffersFromFields { kind: FactKind },
    CarriageReturn { kind: FactKind },
    ScriptNameMismatch,
    ScriptNameNotForFormat { kind: FactKind },
    GuardRegion(GuardRegionError),
    Guard { kind: FactKind, error: GuardError },
}

impl fmt::Display for FactError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FactError::FileKindNotAllowed { kind, file_kind } => write!(
                f,
                "fact kind {} may not come from a {} file",
                kind.name(),
                file_kind.name()
            ),
            FactError::ScopeNotAllowed { kind, scope } => write!(
                f,
                "fact kind {} may not have scope {} for its file kind",
                kind.name(),
                scope.name()
            ),
            FactError::PathNotAllowed { kind, rule } => write!(
                f,
                "fact kind {} breaks its location path rule {}",
                kind.name(),
                rule.name()
            ),
            FactError::LineNotAllowed { kind, rule } => write!(
                f,
                "fact kind {} breaks its line rule {}",
                kind.name(),
                rule.name()
            ),
            FactError::NoFrame { kind } => write!(
                f,
                "fact kind {} names its frame but its location has no frame",
                kind.name()
            ),
            FactError::LineZero { kind } => {
                write!(f, "fact kind {} has line 0; lines are 1-based", kind.name())
            }
            FactError::ScopeDiffersFromFields { kind } => write!(
                f,
                "fact kind {} has a scope that differs from its own fields",
                kind.name()
            ),
            FactError::CarriageReturn { kind } => write!(
                f,
                "fact kind {} holds a CR in its function scope",
                kind.name()
            ),
            FactError::ScriptNameMismatch => {
                f.write_str("package_script name differs from its location's script scope")
            }
            FactError::ScriptNameNotForFormat { kind } => write!(
                f,
                "fact kind {} names a script its package format does not have",
                kind.name()
            ),
            FactError::GuardRegion(error) => write!(f, "guard_region: {error:?}"),
            FactError::Guard { kind, error } => write!(f, "fact kind {}: {error}", kind.name()),
        }
    }
}

impl std::error::Error for FactError {}

impl Fact {
    pub fn kind(&self) -> FactKind {
        match self {
            Fact::Call { .. } => FactKind::Call,
            Fact::Include { .. } => FactKind::Include,
            Fact::StringLiteral { .. } => FactKind::StringLiteral,
            Fact::ConfigKey { .. } => FactKind::ConfigKey,
            Fact::GuardRegion { .. } => FactKind::GuardRegion,
            Fact::Import { .. } => FactKind::Import,
            Fact::Export { .. } => FactKind::Export,
            Fact::NeededLibrary { .. } => FactKind::NeededLibrary,
            Fact::PackageFile { .. } => FactKind::PackageFile,
            Fact::PackageDependency { .. } => FactKind::PackageDependency,
            Fact::PackageScript { .. } => FactKind::PackageScript,
            Fact::ServiceSetting { .. } => FactKind::ServiceSetting,
            Fact::ShellCommand { .. } => FactKind::ShellCommand,
        }
    }

    pub fn file_kind(&self) -> FileKind {
        match self {
            Fact::Call { file_kind, .. }
            | Fact::Include { file_kind, .. }
            | Fact::StringLiteral { file_kind, .. }
            | Fact::ConfigKey { file_kind, .. }
            | Fact::GuardRegion { file_kind, .. }
            | Fact::Import { file_kind, .. }
            | Fact::Export { file_kind, .. }
            | Fact::NeededLibrary { file_kind, .. }
            | Fact::PackageFile { file_kind, .. }
            | Fact::PackageDependency { file_kind, .. }
            | Fact::PackageScript { file_kind, .. }
            | Fact::ServiceSetting { file_kind, .. }
            | Fact::ShellCommand { file_kind, .. } => *file_kind,
        }
    }

    pub fn location(&self) -> &Location {
        match self {
            Fact::Call { location, .. }
            | Fact::Include { location, .. }
            | Fact::StringLiteral { location, .. }
            | Fact::ConfigKey { location, .. }
            | Fact::GuardRegion { location, .. }
            | Fact::Import { location, .. }
            | Fact::Export { location, .. }
            | Fact::NeededLibrary { location, .. }
            | Fact::PackageFile { location, .. }
            | Fact::PackageDependency { location, .. }
            | Fact::PackageScript { location, .. }
            | Fact::ServiceSetting { location, .. }
            | Fact::ShellCommand { location, .. } => location,
        }
    }

    pub fn guards(&self) -> &[GuardChain] {
        match self {
            Fact::Call { guards, .. }
            | Fact::Include { guards, .. }
            | Fact::StringLiteral { guards, .. }
            | Fact::GuardRegion { guards, .. } => guards,
            Fact::ConfigKey { .. }
            | Fact::Import { .. }
            | Fact::Export { .. }
            | Fact::NeededLibrary { .. }
            | Fact::PackageFile { .. }
            | Fact::PackageDependency { .. }
            | Fact::PackageScript { .. }
            | Fact::ServiceSetting { .. }
            | Fact::ShellCommand { .. } => &[],
        }
    }

    fn holds_carriage_return(&self) -> bool {
        let cr = |t: &Text| t.as_bytes().contains(&b'\r');
        match &self.location().scope {
            Scope::Function {
                qualified_name,
                signature,
            } => cr(qualified_name) || cr(signature),
            _ => false,
        }
    }

    fn scope_matches_fields(&self) -> bool {
        let scope = &self.location().scope;
        match self {
            Fact::Import { symbol, .. } | Fact::Export { symbol, .. } => {
                matches!(scope, Scope::Symbol(s) if s == symbol)
            }
            Fact::ConfigKey { key, .. } => matches!(scope, Scope::ConfigKey(k) if k == key),
            Fact::ServiceSetting { section, key, .. } => matches!(
                scope,
                Scope::UnitSection { section: s, key: k } if s == section && k == key
            ),
            _ => true,
        }
    }

    pub fn validate(&self) -> Result<(), FactError> {
        let kind = self.kind();
        let file_kind = self.file_kind();
        let rule = kind
            .source(file_kind)
            .ok_or(FactError::FileKindNotAllowed { kind, file_kind })?;
        let location = self.location();
        let scope = location.scope.kind();
        if !rule.scopes.contains(&scope) {
            return Err(FactError::ScopeNotAllowed { kind, scope });
        }
        let path_ok = match rule.path {
            PathRule::File => location.path.is_some(),
            PathRule::Frame => location.path.is_none(),
        };
        if !path_ok {
            return Err(FactError::PathNotAllowed {
                kind,
                rule: rule.path,
            });
        }
        if rule.path == PathRule::Frame && location.frames.is_empty() {
            return Err(FactError::NoFrame { kind });
        }
        if location.line == Some(0) || matches!(self, Fact::GuardRegion { end_line: 0, .. }) {
            return Err(FactError::LineZero { kind });
        }
        let line_ok = match rule.line {
            LineRule::Required => location.line.is_some(),
            LineRule::Absent => location.line.is_none(),
        };
        if !line_ok {
            return Err(FactError::LineNotAllowed {
                kind,
                rule: rule.line,
            });
        }
        if self.holds_carriage_return() {
            return Err(FactError::CarriageReturn { kind });
        }
        if !self.scope_matches_fields() {
            return Err(FactError::ScopeDiffersFromFields { kind });
        }
        if let Fact::PackageScript { name, .. } = self
            && location.scope != Scope::Script(*name)
        {
            return Err(FactError::ScriptNameMismatch);
        }
        if let Scope::Script(name) = location.scope {
            let format = location.frames.last().map(|f| f.kind);
            if format.and_then(|f| name.native(f)).is_none() {
                return Err(FactError::ScriptNameNotForFormat { kind });
            }
        }
        if let Fact::GuardRegion {
            region,
            guards,
            end_line,
            ..
        } = self
        {
            let last = guards
                .last()
                .ok_or(FactError::GuardRegion(GuardRegionError::NoGuards))?;
            if location.line.is_some_and(|line| *end_line < line) {
                return Err(FactError::GuardRegion(GuardRegionError::EndBeforeLine));
            }
            if last.region != *region {
                return Err(FactError::GuardRegion(GuardRegionError::RegionMismatch));
            }
        }
        for chain in self.guards() {
            chain
                .validate()
                .map_err(|error| FactError::Guard { kind, error })?;
        }
        Ok(())
    }
}
