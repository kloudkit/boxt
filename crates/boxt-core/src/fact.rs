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
        FrameForScripts,
    }
}

impl PathRule {
    pub const fn name(self) -> &'static str {
        match self {
            PathRule::File => "file",
            PathRule::Frame => "frame",
            PathRule::FrameForScripts => "frame_for_scripts",
        }
    }

    pub fn allows(self, has_path: bool, scope: ScopeKind) -> bool {
        match self {
            PathRule::File => has_path,
            PathRule::Frame => !has_path,
            PathRule::FrameForScripts => has_path != (scope == ScopeKind::Script),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FactKindSpec {
    pub kind: FactKind,
    pub name: &'static str,
    pub file_kinds: &'static [FileKind],
    pub scopes: &'static [ScopeKind],
    pub path: PathRule,
    pub fields: &'static [&'static str],
}

pub const COMMON_FIELDS: &[&str] = &["file_kind", "location"];

const SOURCE: &[FileKind] = &[FileKind::Source];
const BINARY: &[FileKind] = &[FileKind::Binary];
const PACKAGE: &[FileKind] = &[FileKind::Package];
const SERVICE: &[FileKind] = &[FileKind::Service];
const CONFIG: &[FileKind] = &[FileKind::Config];

const IN_CODE: &[ScopeKind] = &[ScopeKind::File, ScopeKind::Function];
const FILE: &[ScopeKind] = &[ScopeKind::File];
const SYMBOL: &[ScopeKind] = &[ScopeKind::Symbol];

pub const FACT_KINDS: &[FactKindSpec] = &[
    FactKindSpec {
        kind: FactKind::Call,
        name: "call",
        file_kinds: SOURCE,
        scopes: IN_CODE,
        path: PathRule::File,
        fields: &["callee", "args", "guards"],
    },
    FactKindSpec {
        kind: FactKind::Include,
        name: "include",
        file_kinds: SOURCE,
        scopes: IN_CODE,
        path: PathRule::File,
        fields: &["target", "system", "guards"],
    },
    FactKindSpec {
        kind: FactKind::StringLiteral,
        name: "string_literal",
        file_kinds: &[FileKind::Source, FileKind::Binary],
        scopes: IN_CODE,
        path: PathRule::File,
        fields: &["value", "guards"],
    },
    FactKindSpec {
        kind: FactKind::ConfigKey,
        name: "config_key",
        file_kinds: CONFIG,
        scopes: &[ScopeKind::ConfigKey],
        path: PathRule::File,
        fields: &["key", "value"],
    },
    FactKindSpec {
        kind: FactKind::GuardRegion,
        name: "guard_region",
        file_kinds: SOURCE,
        scopes: IN_CODE,
        path: PathRule::File,
        fields: &["region", "guards", "end_line"],
    },
    FactKindSpec {
        kind: FactKind::Import,
        name: "import",
        file_kinds: BINARY,
        scopes: SYMBOL,
        path: PathRule::File,
        fields: &["symbol", "version"],
    },
    FactKindSpec {
        kind: FactKind::Export,
        name: "export",
        file_kinds: BINARY,
        scopes: SYMBOL,
        path: PathRule::File,
        fields: &["symbol", "version"],
    },
    FactKindSpec {
        kind: FactKind::NeededLibrary,
        name: "needed_library",
        file_kinds: BINARY,
        scopes: FILE,
        path: PathRule::File,
        fields: &["name"],
    },
    FactKindSpec {
        kind: FactKind::PackageFile,
        name: "package_file",
        file_kinds: PACKAGE,
        scopes: FILE,
        path: PathRule::Frame,
        fields: &["path", "mode", "user", "group"],
    },
    FactKindSpec {
        kind: FactKind::PackageDependency,
        name: "package_dependency",
        file_kinds: PACKAGE,
        scopes: FILE,
        path: PathRule::Frame,
        fields: &["name", "constraint"],
    },
    FactKindSpec {
        kind: FactKind::PackageScript,
        name: "package_script",
        file_kinds: PACKAGE,
        scopes: &[ScopeKind::Script],
        path: PathRule::Frame,
        fields: &["name", "interpreter"],
    },
    FactKindSpec {
        kind: FactKind::ServiceSetting,
        name: "service_setting",
        file_kinds: SERVICE,
        scopes: &[ScopeKind::UnitSection],
        path: PathRule::File,
        fields: &["section", "key", "value"],
    },
    FactKindSpec {
        kind: FactKind::ShellCommand,
        name: "shell_command",
        file_kinds: &[FileKind::Package, FileKind::Service],
        scopes: &[ScopeKind::Script, ScopeKind::File],
        path: PathRule::FrameForScripts,
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

    pub fn file_kinds(self) -> &'static [FileKind] {
        self.spec().file_kinds
    }

    pub fn scopes(self) -> &'static [ScopeKind] {
        self.spec().scopes
    }

    pub fn fields(self) -> &'static [&'static str] {
        self.spec().fields
    }

    pub fn from_name(name: &str) -> Option<FactKind> {
        FACT_KINDS.iter().find(|s| s.name == name).map(|s| s.kind)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FactError {
    FileKindNotAllowed { kind: FactKind, file_kind: FileKind },
    ScopeNotAllowed { kind: FactKind, scope: ScopeKind },
    PathNotAllowed { kind: FactKind, rule: PathRule },
    LineZero { kind: FactKind },
    ScriptNameMismatch,
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
                "fact kind {} may not have scope {}",
                kind.name(),
                scope.name()
            ),
            FactError::PathNotAllowed { kind, rule } => write!(
                f,
                "fact kind {} breaks its location path rule {}",
                kind.name(),
                rule.name()
            ),
            FactError::LineZero { kind } => {
                write!(f, "fact kind {} has line 0; lines are 1-based", kind.name())
            }
            FactError::ScriptNameMismatch => {
                f.write_str("package_script name differs from its location's script scope")
            }
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

    pub fn validate(&self) -> Result<(), FactError> {
        let kind = self.kind();
        let spec = kind.spec();
        let file_kind = self.file_kind();
        if !spec.file_kinds.contains(&file_kind) {
            return Err(FactError::FileKindNotAllowed { kind, file_kind });
        }
        let location = self.location();
        let scope = location.scope.kind();
        if !spec.scopes.contains(&scope) {
            return Err(FactError::ScopeNotAllowed { kind, scope });
        }
        if !spec.path.allows(location.path.is_some(), scope) {
            return Err(FactError::PathNotAllowed {
                kind,
                rule: spec.path,
            });
        }
        let end_zero = matches!(self, Fact::GuardRegion { end_line: 0, .. });
        if location.line == Some(0) || end_zero {
            return Err(FactError::LineZero { kind });
        }
        if let Fact::PackageScript { name, .. } = self
            && location.scope != Scope::Script(*name)
        {
            return Err(FactError::ScriptNameMismatch);
        }
        for chain in self.guards() {
            chain
                .validate()
                .map_err(|error| FactError::Guard { kind, error })?;
        }
        Ok(())
    }
}
