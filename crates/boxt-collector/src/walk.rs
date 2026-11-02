use std::fs::{self, Metadata};
use std::io;
use std::path::{Path, PathBuf};

use boxt_core::Text;
use boxt_core::location::{InputRef, RelPath};
use boxt_core::message::JobFile;
use boxt_core::outcome::{
    IoKind, LinkTarget, NotCheckedReason, OutOfScopeCause, OwnFileRole, SpecialKind,
};
use boxt_core::pattern::Pattern;
use boxt_core::settings::PathPattern;

use crate::path::{join_match_text, match_text, matches, path_from_text, text_from_path};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WalkedInput {
    pub input: InputRef,
    pub items: Vec<Item>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub file: JobFile,
    pub kind: ItemKind,
}

impl Item {
    pub fn path(&self) -> &RelPath {
        match &self.file {
            JobFile::InInput(path) | JobFile::WholeInput(path) => path,
            JobFile::Setting(_) => unreachable!("the walk makes no setting jobs"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ItemKind {
    File { size: u64 },
    NotChecked(NotCheckedReason),
    OutOfScope(OutOfScopeCause),
}

#[derive(Debug, Clone, Default)]
pub struct Scope {
    pub own_files: Vec<(Pattern, OwnFileRole)>,
    pub excludes: Vec<(PathPattern, Pattern)>,
}

impl Scope {
    pub fn is_empty(&self) -> bool {
        self.own_files.is_empty() && self.excludes.is_empty()
    }

    pub fn cause(&self, path: &Text) -> Option<OutOfScopeCause> {
        if let Some((_, role)) = self.own_files.iter().find(|(p, _)| matches(p, path)) {
            return Some(OutOfScopeCause::OwnFile(*role));
        }
        self.excludes
            .iter()
            .find(|(_, p)| matches(p, path))
            .map(|(given, _)| OutOfScopeCause::Pattern(given.clone()))
    }

    fn below_cause(&self, path: &Text) -> Option<OutOfScopeCause> {
        let mut probe = path.as_bytes().to_vec();
        probe.extend_from_slice(FOLDER_PROBE.as_bytes());
        let probe = Text::from_bytes(probe);
        self.excludes
            .iter()
            .find(|(_, p)| matches(p, &probe))
            .map(|(given, _)| OutOfScopeCause::Pattern(given.clone()))
    }
}

const FOLDER_PROBE: &str = "/\u{E001}";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WalkError {
    Input { index: u32, kind: IoKind },
    InputPath { index: u32 },
    InputName { index: u32 },
    EntryName { index: u32 },
}

impl std::fmt::Display for WalkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WalkError::Input { index, kind } => {
                write!(f, "input {index} cannot be read ({kind:?})")
            }
            WalkError::InputPath { index } => {
                write!(f, "input {index} is not a path this system can open")
            }
            WalkError::InputName { index } => {
                write!(f, "input {index} names no file or directory")
            }
            WalkError::EntryName { index } => {
                write!(
                    f,
                    "input {index} holds an entry whose name is not a safe path"
                )
            }
        }
    }
}

impl std::error::Error for WalkError {}

pub fn walk(inputs: &[Text], scope: &Scope) -> Result<Vec<WalkedInput>, WalkError> {
    inputs
        .iter()
        .enumerate()
        .map(|(index, given)| {
            let index = u32::try_from(index).unwrap_or(u32::MAX);
            let mut walked = walk_input(index, given)?;
            mark_out_of_scope(&mut walked, scope)?;
            Ok(walked)
        })
        .collect()
}

fn mark_out_of_scope(walked: &mut WalkedInput, scope: &Scope) -> Result<(), WalkError> {
    if scope.is_empty() {
        return Ok(());
    }
    let index = walked.input.index;
    let root = path_from_text(&walked.input.given).ok_or(WalkError::InputPath { index })?;
    let root = match_text(&root).map_err(|e| WalkError::Input {
        index,
        kind: IoKind::from_io(e.kind()),
    })?;
    for item in &mut walked.items {
        let path = match &item.file {
            JobFile::WholeInput(_) => root.clone(),
            JobFile::InInput(rel) => join_match_text(&root, rel),
            JobFile::Setting(_) => continue,
        };
        let cause = scope.cause(&path).or_else(|| {
            matches!(item.kind, ItemKind::NotChecked(_))
                .then(|| scope.below_cause(&path))
                .flatten()
        });
        if let Some(cause) = cause {
            item.kind = ItemKind::OutOfScope(cause);
        }
    }
    Ok(())
}

fn walk_input(index: u32, given: &Text) -> Result<WalkedInput, WalkError> {
    let input = InputRef {
        index,
        given: given.clone(),
    };
    let root = path_from_text(given).ok_or(WalkError::InputPath { index })?;
    let io_error = |e: io::Error| WalkError::Input {
        index,
        kind: IoKind::from_io(e.kind()),
    };
    let meta = fs::metadata(&root).map_err(io_error)?;
    if !meta.is_dir() {
        let name = root
            .file_name()
            .map(|n| text_from_path(Path::new(n)))
            .and_then(|n| RelPath::new(n).ok())
            .ok_or(WalkError::InputName { index })?;
        let kind = if is_reparse_point(&meta) {
            ItemKind::NotChecked(NotCheckedReason::SpecialFile(SpecialKind::ReparsePoint))
        } else if meta.is_file() {
            ItemKind::File { size: meta.len() }
        } else {
            ItemKind::NotChecked(NotCheckedReason::SpecialFile(special_kind(&meta)))
        };
        return Ok(WalkedInput {
            input,
            items: vec![Item {
                file: JobFile::WholeInput(name),
                kind,
            }],
        });
    }
    let real_root = fs::canonicalize(&root).map_err(io_error)?;
    let mut items = Vec::new();
    let mut dirs: Vec<Subdir> = vec![(root, real_root, Vec::new())];
    while let Some((dir, real_dir, prefix)) = dirs.pop() {
        match list_dir(&dir, &real_dir, &prefix) {
            Ok(listed) if listed.unsafe_name && prefix.is_empty() => {
                return Err(WalkError::EntryName { index });
            }
            Ok(listed) => {
                items.extend(listed.items);
                dirs.extend(listed.subdirs);
                if listed.unsafe_name {
                    push_dir(&mut items, prefix, NotCheckedReason::UnsafeEntryName);
                }
            }
            Err(e) if prefix.is_empty() => return Err(io_error(e)),
            Err(e) => push_dir(
                &mut items,
                prefix,
                NotCheckedReason::Unreadable(IoKind::from_io(e.kind())),
            ),
        }
    }
    items.sort_by(|a, b| a.path().cmp(b.path()));
    Ok(WalkedInput { input, items })
}

fn push_dir(items: &mut Vec<Item>, prefix: Vec<u8>, reason: NotCheckedReason) {
    if let Ok(path) = RelPath::new(Text::from_bytes(prefix)) {
        items.push(Item {
            file: JobFile::InInput(path),
            kind: ItemKind::NotChecked(reason),
        });
    }
}

type Subdir = (PathBuf, PathBuf, Vec<u8>);

struct Listed {
    items: Vec<Item>,
    subdirs: Vec<Subdir>,
    unsafe_name: bool,
}

/// Lists one directory whole or not at all: an I/O error while listing it keeps none of its
/// entries. An entry whose name is not a `RelPath` is left out, and the directory itself
/// gets one `UnsafeEntryName` outcome for it.
fn list_dir(dir: &Path, real_dir: &Path, prefix: &[u8]) -> io::Result<Listed> {
    let mut items = Vec::new();
    let mut subdirs = Vec::new();
    let mut unsafe_name = false;
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let mut rel = prefix.to_vec();
        if !rel.is_empty() {
            rel.push(b'/');
        }
        rel.extend_from_slice(text_from_path(Path::new(&name)).as_bytes());
        let Ok(path) = RelPath::new(Text::from_bytes(rel.clone())) else {
            unsafe_name = true;
            continue;
        };
        let on_disk = entry.path();
        let kind = match fs::symlink_metadata(&on_disk) {
            Ok(meta) => match classify(&meta) {
                Class::Dir => {
                    subdirs.push((on_disk, real_dir.join(&name), rel));
                    continue;
                }
                Class::File(size) => ItemKind::File { size },
                Class::NotChecked(reason) => ItemKind::NotChecked(reason),
                Class::Link => {
                    ItemKind::NotChecked(NotCheckedReason::Symlink(link_target(real_dir, &name)))
                }
            },
            Err(e) => ItemKind::NotChecked(NotCheckedReason::Unreadable(IoKind::from_io(e.kind()))),
        };
        items.push(Item {
            file: JobFile::InInput(path),
            kind,
        });
    }
    Ok(Listed {
        items,
        subdirs,
        unsafe_name,
    })
}

enum Class {
    File(u64),
    Dir,
    Link,
    NotChecked(NotCheckedReason),
}

#[cfg(windows)]
pub fn is_reparse_point(meta: &Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;
    meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
pub fn is_reparse_point(_meta: &Metadata) -> bool {
    false
}

fn classify(meta: &Metadata) -> Class {
    if is_reparse_point(meta) {
        return Class::NotChecked(NotCheckedReason::SpecialFile(SpecialKind::ReparsePoint));
    }
    let kind = meta.file_type();
    if kind.is_symlink() {
        Class::Link
    } else if kind.is_dir() {
        Class::Dir
    } else if kind.is_file() {
        Class::File(meta.len())
    } else {
        Class::NotChecked(NotCheckedReason::SpecialFile(special_kind(meta)))
    }
}

#[cfg(unix)]
pub fn special_kind(meta: &Metadata) -> SpecialKind {
    use std::os::unix::fs::FileTypeExt;
    let kind = meta.file_type();
    if kind.is_fifo() {
        SpecialKind::Fifo
    } else if kind.is_socket() {
        SpecialKind::Socket
    } else if kind.is_block_device() {
        SpecialKind::BlockDevice
    } else if kind.is_char_device() {
        SpecialKind::CharDevice
    } else {
        SpecialKind::Other
    }
}

#[cfg(not(unix))]
pub fn special_kind(meta: &Metadata) -> SpecialKind {
    if is_reparse_point(meta) {
        SpecialKind::ReparsePoint
    } else {
        SpecialKind::Other
    }
}

#[cfg(unix)]
const MAX_LINK_HOPS: u32 = 40;

#[cfg(unix)]
fn link_target(real_dir: &Path, name: &std::ffi::OsStr) -> LinkTarget {
    use std::ffi::OsString;
    use std::path::Component;

    let mut current = real_dir.to_path_buf();
    let mut rest: Vec<OsString> = vec![name.to_owned()];
    let mut hops = 0;
    while let Some(part) = rest.pop() {
        if part == ".." {
            current.pop();
            continue;
        }
        if part == "." || part.is_empty() {
            continue;
        }
        let candidate = current.join(&part);
        let meta = match fs::symlink_metadata(&candidate) {
            Ok(meta) => meta,
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
                ) =>
            {
                return LinkTarget::Missing;
            }
            Err(_) => return LinkTarget::Unresolved,
        };
        let kind = meta.file_type();
        if kind.is_symlink() {
            hops += 1;
            if hops > MAX_LINK_HOPS {
                return LinkTarget::Loop;
            }
            let Ok(target) = fs::read_link(&candidate) else {
                return LinkTarget::Unresolved;
            };
            if target.is_absolute() {
                current = PathBuf::from("/");
            }
            let parts: Vec<OsString> = target
                .components()
                .filter_map(|c| match c {
                    Component::Normal(n) => Some(n.to_owned()),
                    Component::ParentDir => Some(OsString::from("..")),
                    Component::CurDir | Component::RootDir | Component::Prefix(_) => None,
                })
                .collect();
            rest.extend(parts.into_iter().rev());
            continue;
        }
        if kind.is_dir() {
            current = candidate;
            continue;
        }
        if rest.iter().any(|p| p != "." && !p.is_empty()) {
            return LinkTarget::Missing;
        }
        return if kind.is_file() {
            LinkTarget::File
        } else {
            LinkTarget::Special(special_kind(&meta))
        };
    }
    LinkTarget::Directory
}

#[cfg(not(unix))]
fn link_target(_real_dir: &Path, _name: &std::ffi::OsStr) -> LinkTarget {
    LinkTarget::Unresolved
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    struct Dir(PathBuf);

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn dir(name: &str) -> Dir {
        let root = std::env::temp_dir().join(format!("boxt-walk-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        Dir(root)
    }

    fn kinds(walked: &WalkedInput) -> Vec<(String, ItemKind)> {
        walked
            .items
            .iter()
            .map(|i| {
                (
                    String::from_utf8_lossy(i.path().as_bytes()).into_owned(),
                    i.kind.clone(),
                )
            })
            .collect()
    }

    #[test]
    fn links_and_special_files_are_listed_and_never_followed() {
        use std::os::unix::fs::symlink;
        let d = dir("links");
        let root = &d.0;
        fs::create_dir_all(root.join("sub")).unwrap();
        fs::write(root.join("real.c"), b"int x;\n").unwrap();
        fs::write(root.join("sub/b.c"), b"int y;\n").unwrap();
        symlink("real.c", root.join("a.c")).unwrap();
        symlink("x", root.join("x")).unwrap();
        symlink("sub", root.join("dir-link")).unwrap();
        symlink("nowhere", root.join("gone")).unwrap();
        symlink("hop", root.join("hop2")).unwrap();
        symlink("sub/../real.c", root.join("hop")).unwrap();
        rustix::fs::mkfifoat(rustix::fs::CWD, root.join("p"), rustix::fs::Mode::RUSR).unwrap();
        let walked = walk(&[text_from_path(root)], &Scope::default()).unwrap();
        let link = |t| ItemKind::NotChecked(NotCheckedReason::Symlink(t));
        assert_eq!(
            kinds(&walked[0]),
            vec![
                ("a.c".into(), link(LinkTarget::File)),
                ("dir-link".into(), link(LinkTarget::Directory)),
                ("gone".into(), link(LinkTarget::Missing)),
                ("hop".into(), link(LinkTarget::File)),
                ("hop2".into(), link(LinkTarget::File)),
                (
                    "p".into(),
                    ItemKind::NotChecked(NotCheckedReason::SpecialFile(SpecialKind::Fifo))
                ),
                ("real.c".into(), ItemKind::File { size: 7 }),
                ("sub/b.c".into(), ItemKind::File { size: 7 }),
                ("x".into(), link(LinkTarget::Loop)),
            ]
        );
    }

    #[test]
    fn a_linked_root_is_followed_once_and_a_file_input_is_its_name() {
        use std::os::unix::fs::symlink;
        let d = dir("root-link");
        let real = d.0.join("real");
        fs::create_dir_all(&real).unwrap();
        fs::write(real.join("a.c"), b"int x;\n").unwrap();
        symlink("real", d.0.join("root")).unwrap();
        let walked = walk(
            &[
                text_from_path(&d.0.join("root")),
                text_from_path(&real.join("a.c")),
            ],
            &Scope::default(),
        )
        .unwrap();
        assert_eq!(
            kinds(&walked[0]),
            vec![("a.c".into(), ItemKind::File { size: 7 })]
        );
        assert_eq!(walked[1].input.index, 1);
        assert!(matches!(walked[1].items[0].file, JobFile::WholeInput(_)));
        assert_eq!(
            walk(&[text_from_path(&d.0.join("missing"))], &Scope::default()),
            Err(WalkError::Input {
                index: 0,
                kind: IoKind::NotFound
            })
        );
    }

    #[test]
    fn own_files_come_before_excludes_and_the_first_exclude_wins() {
        use boxt_core::pattern::{literal_path, path_pattern};
        let d = dir("scope");
        let root = &d.0;
        fs::create_dir_all(root.join("gen/deep")).unwrap();
        fs::write(root.join("boxt.yaml"), b"schema: \"1.0\"\n").unwrap();
        fs::write(root.join("a.c"), b"int a;\n").unwrap();
        fs::write(root.join("gen/b.c"), b"int b;\n").unwrap();
        fs::write(root.join("gen/deep/c.c"), b"int c;\n").unwrap();
        let base = match_text(root).unwrap();
        let given = |p: &str| PathPattern {
            base: base.clone(),
            pattern: Text::from(p),
        };
        let compiled = |p: &str| (given(p), path_pattern(&base, &Text::from(p)).unwrap());
        let scope = Scope {
            own_files: vec![(
                literal_path(&match_text(&root.join("boxt.yaml")).unwrap()).unwrap(),
                OwnFileRole::ProjectFile,
            )],
            excludes: vec![compiled("gen/*.c"), compiled("gen/**"), compiled("*")],
        };
        let walked = walk(&[text_from_path(root)], &scope).unwrap();
        let out = |p: &str| ItemKind::OutOfScope(OutOfScopeCause::Pattern(given(p)));
        assert_eq!(
            kinds(&walked[0]),
            vec![
                ("a.c".into(), out("*")),
                (
                    "boxt.yaml".into(),
                    ItemKind::OutOfScope(OutOfScopeCause::OwnFile(OwnFileRole::ProjectFile))
                ),
                ("gen/b.c".into(), out("gen/*.c")),
                ("gen/deep/c.c".into(), out("gen/**")),
            ]
        );
    }

    #[test]
    fn a_not_checked_item_is_excluded_when_its_contents_would_be() {
        use std::os::unix::fs::symlink;

        use boxt_core::pattern::path_pattern;
        let d = dir("folder-probe");
        let root = &d.0;
        fs::create_dir_all(root.join("real")).unwrap();
        fs::create_dir_all(root.join("third_party")).unwrap();
        fs::write(root.join("a.c"), b"int a;\n").unwrap();
        fs::write(root.join("real/b.c"), b"int b;\n").unwrap();
        symlink("real", root.join("gen")).unwrap();
        symlink("../real", root.join("third_party/link")).unwrap();
        let base = match_text(root).unwrap();
        let given = |p: &str| PathPattern {
            base: base.clone(),
            pattern: Text::from(p),
        };
        let compiled = |p: &str| (given(p), path_pattern(&base, &Text::from(p)).unwrap());
        let scope = Scope {
            own_files: Vec::new(),
            excludes: vec![compiled("gen/**"), compiled("third_party/**")],
        };
        let walked = walk(&[text_from_path(root)], &scope).unwrap();
        let out = |p: &str| ItemKind::OutOfScope(OutOfScopeCause::Pattern(given(p)));
        assert_eq!(
            kinds(&walked[0]),
            vec![
                ("a.c".into(), ItemKind::File { size: 7 }),
                ("gen".into(), out("gen/**")),
                ("real/b.c".into(), ItemKind::File { size: 7 }),
                ("third_party/link".into(), out("third_party/**")),
            ]
        );
    }
}
