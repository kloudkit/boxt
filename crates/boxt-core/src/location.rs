use std::fmt;

use serde::{Deserialize, Serialize};

use crate::text::Text;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct InputRef {
    pub index: u32,
    pub given: Text,
}

closed_enum! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub enum FrameKind {
        Rpm,
        Deb,
        Archive,
    }
}

impl FrameKind {
    pub const fn name(self) -> &'static str {
        match self {
            FrameKind::Rpm => "rpm",
            FrameKind::Deb => "deb",
            FrameKind::Archive => "archive",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Frame {
    pub kind: FrameKind,
    pub name: Text,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "Text", into = "Text")]
pub struct RelPath(Text);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelPathError {
    Empty,
    Absolute,
    TrailingSlash,
    EmptySegment,
    DotSegment,
    DotDotSegment,
    Nul,
}

impl fmt::Display for RelPathError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Empty => "relative path is empty",
            Self::Absolute => "relative path starts with '/'",
            Self::TrailingSlash => "relative path ends with '/'",
            Self::EmptySegment => "relative path has an empty segment",
            Self::DotSegment => "relative path has a '.' segment",
            Self::DotDotSegment => "relative path has a '..' segment",
            Self::Nul => "relative path holds a NUL byte",
        })
    }
}

impl std::error::Error for RelPathError {}

impl RelPath {
    pub fn new(text: Text) -> Result<Self, RelPathError> {
        validate_rel_path(text.as_bytes())?;
        Ok(Self(text))
    }

    pub fn as_text(&self) -> &Text {
        &self.0
    }

    pub fn as_bytes(&self) -> &[u8] {
        self.0.as_bytes()
    }

    pub fn file_name(&self) -> &[u8] {
        let b = self.0.as_bytes();
        match b.iter().rposition(|&c| c == b'/') {
            Some(i) => &b[i + 1..],
            None => b,
        }
    }
}

pub fn validate_rel_path(bytes: &[u8]) -> Result<(), RelPathError> {
    if bytes.is_empty() {
        return Err(RelPathError::Empty);
    }
    if bytes.contains(&0) {
        return Err(RelPathError::Nul);
    }
    if bytes[0] == b'/' {
        return Err(RelPathError::Absolute);
    }
    if bytes[bytes.len() - 1] == b'/' {
        return Err(RelPathError::TrailingSlash);
    }
    for segment in bytes.split(|&c| c == b'/') {
        match segment {
            b"" => return Err(RelPathError::EmptySegment),
            b"." => return Err(RelPathError::DotSegment),
            b".." => return Err(RelPathError::DotDotSegment),
            _ => {}
        }
    }
    Ok(())
}

impl TryFrom<Text> for RelPath {
    type Error = RelPathError;

    fn try_from(text: Text) -> Result<Self, RelPathError> {
        Self::new(text)
    }
}

impl TryFrom<&str> for RelPath {
    type Error = RelPathError;

    fn try_from(s: &str) -> Result<Self, RelPathError> {
        Self::new(Text::from(s))
    }
}

impl From<RelPath> for Text {
    fn from(p: RelPath) -> Text {
        p.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    File,
    Function {
        qualified_name: Text,
        signature: Text,
    },
    Symbol(Text),
    UnitSection {
        section: Text,
        key: Text,
    },
    ConfigKey(Text),
    Script(ScriptName),
}

closed_enum! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub enum ScopeKind {
        File,
        Function,
        Symbol,
        UnitSection,
        ConfigKey,
        Script,
    }
}

impl ScopeKind {
    pub const fn name(self) -> &'static str {
        match self {
            ScopeKind::File => "file",
            ScopeKind::Function => "function",
            ScopeKind::Symbol => "symbol",
            ScopeKind::UnitSection => "unit_section",
            ScopeKind::ConfigKey => "config_key",
            ScopeKind::Script => "script",
        }
    }
}

impl Scope {
    pub const fn kind(&self) -> ScopeKind {
        match self {
            Scope::File => ScopeKind::File,
            Scope::Function { .. } => ScopeKind::Function,
            Scope::Symbol(_) => ScopeKind::Symbol,
            Scope::UnitSection { .. } => ScopeKind::UnitSection,
            Scope::ConfigKey(_) => ScopeKind::ConfigKey,
            Scope::Script(_) => ScopeKind::Script,
        }
    }
}

closed_enum! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub enum ScriptName {
        PreTransaction,
        PreInstall,
        PostInstall,
        PreRemove,
        PostRemove,
        PostTransaction,
        PreRemoveTransaction,
        PostRemoveTransaction,
        Verify,
        TriggerPreInstall,
        TriggerInstall,
        TriggerRemove,
        TriggerPostRemove,
        FileTriggerInstall,
        FileTriggerRemove,
        FileTriggerPostRemove,
        TransactionFileTriggerInstall,
        TransactionFileTriggerRemove,
        TransactionFileTriggerPostRemove,
        Config,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScriptNameSpec {
    pub format: FrameKind,
    pub native: &'static str,
    pub name: ScriptName,
}

const fn rpm(native: &'static str, name: ScriptName) -> ScriptNameSpec {
    ScriptNameSpec {
        format: FrameKind::Rpm,
        native,
        name,
    }
}

const fn deb(native: &'static str, name: ScriptName) -> ScriptNameSpec {
    ScriptNameSpec {
        format: FrameKind::Deb,
        native,
        name,
    }
}

pub const SCRIPT_NAMES: &[ScriptNameSpec] = &[
    rpm("%pretrans", ScriptName::PreTransaction),
    rpm("%pre", ScriptName::PreInstall),
    rpm("%post", ScriptName::PostInstall),
    rpm("%preun", ScriptName::PreRemove),
    rpm("%postun", ScriptName::PostRemove),
    rpm("%posttrans", ScriptName::PostTransaction),
    rpm("%preuntrans", ScriptName::PreRemoveTransaction),
    rpm("%postuntrans", ScriptName::PostRemoveTransaction),
    rpm("%verifyscript", ScriptName::Verify),
    rpm("%triggerprein", ScriptName::TriggerPreInstall),
    rpm("%triggerin", ScriptName::TriggerInstall),
    rpm("%triggerun", ScriptName::TriggerRemove),
    rpm("%triggerpostun", ScriptName::TriggerPostRemove),
    rpm("%filetriggerin", ScriptName::FileTriggerInstall),
    rpm("%filetriggerun", ScriptName::FileTriggerRemove),
    rpm("%filetriggerpostun", ScriptName::FileTriggerPostRemove),
    rpm(
        "%transfiletriggerin",
        ScriptName::TransactionFileTriggerInstall,
    ),
    rpm(
        "%transfiletriggerun",
        ScriptName::TransactionFileTriggerRemove,
    ),
    rpm(
        "%transfiletriggerpostun",
        ScriptName::TransactionFileTriggerPostRemove,
    ),
    deb("preinst", ScriptName::PreInstall),
    deb("postinst", ScriptName::PostInstall),
    deb("prerm", ScriptName::PreRemove),
    deb("postrm", ScriptName::PostRemove),
    deb("config", ScriptName::Config),
];

impl ScriptName {
    pub const fn name(self) -> &'static str {
        match self {
            ScriptName::PreTransaction => "pre_transaction",
            ScriptName::PreInstall => "pre_install",
            ScriptName::PostInstall => "post_install",
            ScriptName::PreRemove => "pre_remove",
            ScriptName::PostRemove => "post_remove",
            ScriptName::PostTransaction => "post_transaction",
            ScriptName::PreRemoveTransaction => "pre_remove_transaction",
            ScriptName::PostRemoveTransaction => "post_remove_transaction",
            ScriptName::Verify => "verify",
            ScriptName::TriggerPreInstall => "trigger_pre_install",
            ScriptName::TriggerInstall => "trigger_install",
            ScriptName::TriggerRemove => "trigger_remove",
            ScriptName::TriggerPostRemove => "trigger_post_remove",
            ScriptName::FileTriggerInstall => "file_trigger_install",
            ScriptName::FileTriggerRemove => "file_trigger_remove",
            ScriptName::FileTriggerPostRemove => "file_trigger_post_remove",
            ScriptName::TransactionFileTriggerInstall => "transaction_file_trigger_install",
            ScriptName::TransactionFileTriggerRemove => "transaction_file_trigger_remove",
            ScriptName::TransactionFileTriggerPostRemove => "transaction_file_trigger_post_remove",
            ScriptName::Config => "config",
        }
    }

    pub fn from_native(format: FrameKind, native: &[u8]) -> Option<ScriptName> {
        SCRIPT_NAMES
            .iter()
            .find(|s| s.format == format && s.native.as_bytes() == native)
            .map(|s| s.name)
    }

    pub fn native(self, format: FrameKind) -> Option<&'static str> {
        SCRIPT_NAMES
            .iter()
            .find(|s| s.format == format && s.name == self)
            .map(|s| s.native)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Location {
    pub input: InputRef,
    pub frames: Vec<Frame>,
    pub path: Option<RelPath>,
    pub line: Option<u32>,
    pub scope: Scope,
}

impl Location {
    pub fn file(input: InputRef, frames: Vec<Frame>, path: RelPath) -> Self {
        Self {
            input,
            frames,
            path: Some(path),
            line: None,
            scope: Scope::File,
        }
    }

    pub fn frame(input: InputRef, frames: Vec<Frame>) -> Self {
        Self {
            input,
            frames,
            path: None,
            line: None,
            scope: Scope::File,
        }
    }

    pub fn at(&self, line: Option<u32>, scope: Scope) -> Self {
        Self {
            line,
            scope,
            ..self.clone()
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(from = "Text")]
pub struct PathText(Text);

impl PathText {
    pub fn as_text(&self) -> &Text {
        &self.0
    }

    pub fn as_bytes(&self) -> &[u8] {
        self.0.as_bytes()
    }
}

impl From<Text> for PathText {
    fn from(text: Text) -> Self {
        normalize_value_path(&text)
    }
}

impl From<&str> for PathText {
    fn from(s: &str) -> Self {
        normalize_value_path(&Text::from(s))
    }
}

pub fn normalize_value_path(text: &Text) -> PathText {
    let bytes = text.as_bytes();
    if bytes.is_empty() {
        return PathText(Text::default());
    }
    let absolute = bytes[0] == b'/';
    let mut kept: Vec<&[u8]> = Vec::new();
    for segment in bytes.split(|&c| c == b'/') {
        match segment {
            b"" | b"." => {}
            b".." => match kept.last() {
                Some(last) if *last != b".." => {
                    kept.pop();
                }
                _ if absolute => {}
                _ => kept.push(segment),
            },
            _ => kept.push(segment),
        }
    }
    let mut out = Vec::with_capacity(bytes.len());
    if absolute {
        out.push(b'/');
    }
    out.extend_from_slice(&kept.join(&b'/'));
    if out.is_empty() {
        out.push(b'.');
    }
    PathText(Text::from_bytes(out))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryNameError {
    Empty,
    DotDotSegment,
    Nul,
}

impl fmt::Display for EntryNameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Empty => "entry name names no file",
            Self::DotDotSegment => "entry name has a '..' segment",
            Self::Nul => "entry name holds a NUL byte",
        })
    }
}

impl std::error::Error for EntryNameError {}

pub fn entry_path(raw: &[u8]) -> Result<RelPath, EntryNameError> {
    if raw.contains(&0) {
        return Err(EntryNameError::Nul);
    }
    let mut kept: Vec<&[u8]> = Vec::new();
    for segment in raw.split(|&c| c == b'/') {
        match segment {
            b"" | b"." => {}
            b".." => return Err(EntryNameError::DotDotSegment),
            _ => kept.push(segment),
        }
    }
    if kept.is_empty() {
        return Err(EntryNameError::Empty);
    }
    Ok(RelPath(Text::from_bytes(kept.join(&b'/'))))
}

pub const ARCHIVE_SUFFIXES: &[&str] = &[
    ".tar.gz", ".tgz", ".tar.xz", ".txz", ".tar.bz2", ".tbz2", ".tar.zst", ".tar", ".zip", ".gz",
    ".xz", ".bz2", ".zst",
];

pub fn frame_name(file_name: &Text) -> Option<Text> {
    let full = file_name.as_bytes();
    let name = match full.iter().rposition(|&c| c == b'/') {
        Some(i) => &full[i + 1..],
        None => full,
    };
    let mut suffixes: Vec<&[u8]> = ARCHIVE_SUFFIXES.iter().map(|s| s.as_bytes()).collect();
    suffixes.sort_by_key(|s| std::cmp::Reverse(s.len()));
    let stripped = suffixes
        .iter()
        .find(|s| name.len() > s.len() && name.ends_with(s))
        .map_or(name, |s| &name[..name.len() - s.len()]);
    let cut = version_cut(stripped).map_or(stripped, |i| &stripped[..i]);
    let chosen = if cut.is_empty() { stripped } else { cut };
    (!chosen.is_empty()).then(|| Text::from_bytes(chosen.to_vec()))
}

fn version_cut(name: &[u8]) -> Option<usize> {
    let digit_at = |i: usize| name.get(i).is_some_and(u8::is_ascii_digit);
    (0..name.len()).find(|&i| match name[i] {
        b'-' => digit_at(i + 1) || (name.get(i + 1) == Some(&b'v') && digit_at(i + 2)),
        b'_' => digit_at(i + 1),
        _ => false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_names_follow_the_fixture_table() {
        let table = [
            ("app-1.2.tar.gz", "app"),
            ("app-v2.1.tar.gz", "app"),
            ("app_1.2.orig.tar.gz", "app"),
            ("app.1.2.tar.gz", "app.1.2"),
            ("app-1.2-x86_64.tgz", "app"),
            ("app-x86_64.zip", "app-x86"),
            ("app.tar", "app"),
            ("app.zip", "app"),
            ("libfoo-dev.tar.zst", "libfoo-dev"),
            ("app-v.tar.gz", "app-v"),
            ("app.tar.bz2", "app"),
            ("app.tbz2", "app"),
            ("app.txz", "app"),
            ("app.gz", "app"),
            ("app.xz", "app"),
            ("app.bz2", "app"),
            ("app.zst", "app"),
            ("dir-1/app.tar", "app"),
            ("app-1.0.tar.xz", "app"),
            ("-1.0.tar", "-1.0"),
            (".tar.gz", ".tar"),
            ("app", "app"),
        ];
        for (input, expected) in table {
            assert_eq!(
                frame_name(&Text::from(input)),
                Some(Text::from(expected)),
                "{input}"
            );
        }
    }

    #[test]
    fn e16_frame_name_of_nothing_is_none() {
        assert_eq!(frame_name(&Text::from("")), None);
        assert_eq!(frame_name(&Text::from("dir/")), None);
    }

    #[test]
    fn v6_suffixes_match_in_exact_case() {
        for (input, expected) in [
            ("app.TAR.GZ", "app.TAR.GZ"),
            ("app.Zip", "app.Zip"),
            ("app.TGZ", "app.TGZ"),
            ("APP-1.0.TAR.GZ", "APP"),
            ("app.tar.GZ", "app.tar.GZ"),
        ] {
            assert_eq!(
                frame_name(&Text::from(input)),
                Some(Text::from(expected)),
                "{input}"
            );
        }
    }

    #[test]
    fn frame_name_keeps_raw_bytes() {
        let name = Text::from_bytes(b"\xffapp-1.0.tar".to_vec());
        assert_eq!(frame_name(&name).unwrap().as_bytes(), b"\xffapp");
    }

    #[test]
    fn rel_path_validator() {
        assert!(RelPath::try_from("src/a.c").is_ok());
        assert!(RelPath::try_from("a.c").is_ok());
        assert!(RelPath::try_from("dir\\name").is_ok());
        assert!(RelPath::try_from("..a/b..").is_ok());
        assert_eq!(RelPath::try_from(""), Err(RelPathError::Empty));
        assert_eq!(
            RelPath::try_from("/etc/passwd"),
            Err(RelPathError::Absolute)
        );
        assert_eq!(RelPath::try_from("a/"), Err(RelPathError::TrailingSlash));
        assert_eq!(RelPath::try_from("a//b"), Err(RelPathError::EmptySegment));
        assert_eq!(RelPath::try_from("./a"), Err(RelPathError::DotSegment));
        assert_eq!(
            RelPath::try_from("a/../b"),
            Err(RelPathError::DotDotSegment)
        );
        assert_eq!(RelPath::try_from("a\0b"), Err(RelPathError::Nul));
        assert!(serde_json::from_str::<RelPath>(r#""../x""#).is_err());
        let p = RelPath::try_from("src/main.c").unwrap();
        assert_eq!(p.file_name(), b"main.c");
    }

    #[test]
    fn location_compares_byte_exactly() {
        let at = |path: &str| {
            Location::file(
                InputRef {
                    index: 0,
                    given: Text::from("in"),
                },
                vec![],
                RelPath::try_from(path).unwrap(),
            )
        };
        assert_ne!(at("Src/a.c"), at("src/a.c"));
        assert!(at("Src/a.c") < at("src/a.c"));
        assert!(at("a-b") < at("a/b"));
    }
}
