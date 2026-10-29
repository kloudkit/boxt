use std::fmt;

use serde::{Deserialize, Serialize};

use crate::location::Location;
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
    }
}

closed_enum! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub enum Branch {
        Then,
        Else,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct GuardFrame {
    pub directive: Directive,
    pub branch: Branch,
    pub text: Text,
    pub identifiers: SortedVec<Text>,
    pub has_alternative: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Fact {
    Call {
        file_kind: FileKind,
        location: Location,
        callee: Text,
        args: Vec<Text>,
        region: Option<RegionId>,
        guards: Vec<GuardFrame>,
    },
    Include {
        file_kind: FileKind,
        location: Location,
        target: Text,
        system: bool,
        region: Option<RegionId>,
        guards: Vec<GuardFrame>,
    },
    StringLiteral {
        file_kind: FileKind,
        location: Location,
        value: Text,
        region: Option<RegionId>,
        guards: Vec<GuardFrame>,
    },
    ConfigKey {
        file_kind: FileKind,
        location: Location,
        key: Text,
        value: Option<Text>,
        region: Option<RegionId>,
        guards: Vec<GuardFrame>,
    },
    GuardRegion {
        file_kind: FileKind,
        location: Location,
        region: RegionId,
        guards: Vec<GuardFrame>,
        end_line: u32,
    },
    Import {
        file_kind: FileKind,
        location: Location,
        symbol: Text,
    },
    Export {
        file_kind: FileKind,
        location: Location,
        symbol: Text,
    },
    NeededLibrary {
        file_kind: FileKind,
        location: Location,
        name: Text,
    },
    PackageFile {
        file_kind: FileKind,
        location: Location,
        path: Text,
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
        name: Text,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FactKindSpec {
    pub kind: FactKind,
    pub name: &'static str,
    pub file_kinds: &'static [FileKind],
    pub fields: &'static [&'static str],
}

pub const COMMON_FIELDS: &[&str] = &["file_kind", "location"];

const SOURCE: &[FileKind] = &[FileKind::Source];
const BINARY: &[FileKind] = &[FileKind::Binary];
const PACKAGE: &[FileKind] = &[FileKind::Package];
const SERVICE: &[FileKind] = &[FileKind::Service];

pub const FACT_KINDS: &[FactKindSpec] = &[
    FactKindSpec {
        kind: FactKind::Call,
        name: "call",
        file_kinds: SOURCE,
        fields: &["callee", "args", "region", "guards"],
    },
    FactKindSpec {
        kind: FactKind::Include,
        name: "include",
        file_kinds: SOURCE,
        fields: &["target", "system", "region", "guards"],
    },
    FactKindSpec {
        kind: FactKind::StringLiteral,
        name: "string_literal",
        file_kinds: &[FileKind::Source, FileKind::Binary],
        fields: &["value", "region", "guards"],
    },
    FactKindSpec {
        kind: FactKind::ConfigKey,
        name: "config_key",
        file_kinds: &[FileKind::Source, FileKind::Config],
        fields: &["key", "value", "region", "guards"],
    },
    FactKindSpec {
        kind: FactKind::GuardRegion,
        name: "guard_region",
        file_kinds: SOURCE,
        fields: &["region", "guards", "end_line"],
    },
    FactKindSpec {
        kind: FactKind::Import,
        name: "import",
        file_kinds: BINARY,
        fields: &["symbol"],
    },
    FactKindSpec {
        kind: FactKind::Export,
        name: "export",
        file_kinds: BINARY,
        fields: &["symbol"],
    },
    FactKindSpec {
        kind: FactKind::NeededLibrary,
        name: "needed_library",
        file_kinds: BINARY,
        fields: &["name"],
    },
    FactKindSpec {
        kind: FactKind::PackageFile,
        name: "package_file",
        file_kinds: PACKAGE,
        fields: &["path", "mode", "user", "group"],
    },
    FactKindSpec {
        kind: FactKind::PackageDependency,
        name: "package_dependency",
        file_kinds: PACKAGE,
        fields: &["name", "constraint"],
    },
    FactKindSpec {
        kind: FactKind::PackageScript,
        name: "package_script",
        file_kinds: PACKAGE,
        fields: &["name", "interpreter"],
    },
    FactKindSpec {
        kind: FactKind::ServiceSetting,
        name: "service_setting",
        file_kinds: SERVICE,
        fields: &["section", "key", "value"],
    },
    FactKindSpec {
        kind: FactKind::ShellCommand,
        name: "shell_command",
        file_kinds: &[FileKind::Package, FileKind::Service],
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

    pub fn validate(&self) -> Result<(), FactError> {
        let kind = self.kind();
        let file_kind = self.file_kind();
        if kind.file_kinds().contains(&file_kind) {
            Ok(())
        } else {
            Err(FactError::FileKindNotAllowed { kind, file_kind })
        }
    }
}
