use std::io;
use std::path::{Component, Path, PathBuf};

use boxt_core::Text;
use boxt_core::location::RelPath;
use boxt_core::message::{Job, JobFile};
use boxt_core::pattern::Pattern;

#[cfg(unix)]
pub fn text_from_path(path: &Path) -> Text {
    use std::os::unix::ffi::OsStrExt;
    Text::from_bytes(path.as_os_str().as_bytes().to_vec())
}

#[cfg(windows)]
pub fn text_from_path(path: &Path) -> Text {
    use std::os::windows::ffi::OsStrExt;
    Text::from_wide(&path.as_os_str().encode_wide().collect::<Vec<u16>>())
}

pub fn display_text_from_path(path: &Path) -> Text {
    let text = text_from_path(path);
    if cfg!(windows) {
        Text::from_bytes(
            text.into_bytes()
                .into_iter()
                .map(|b| if b == b'\\' { b'/' } else { b })
                .collect(),
        )
    } else {
        text
    }
}

#[cfg(unix)]
pub fn path_from_text(text: &Text) -> Option<PathBuf> {
    use std::os::unix::ffi::OsStrExt;
    Some(PathBuf::from(std::ffi::OsStr::from_bytes(text.as_bytes())))
}

#[cfg(windows)]
pub fn path_from_text(text: &Text) -> Option<PathBuf> {
    use std::os::windows::ffi::OsStringExt;
    Some(PathBuf::from(std::ffi::OsString::from_wide(
        &text.to_wide()?,
    )))
}

pub fn rel_path(root: &Path, file: &Path) -> Option<RelPath> {
    let rel = file.strip_prefix(root).ok()?;
    let mut bytes = Vec::new();
    for part in rel.components() {
        if !bytes.is_empty() {
            bytes.push(b'/');
        }
        bytes.extend_from_slice(text_from_path(Path::new(part.as_os_str())).as_bytes());
    }
    RelPath::new(Text::from_bytes(bytes)).ok()
}

pub fn job_path(job: &Job) -> Option<PathBuf> {
    let given = path_from_text(&job.input.given)?;
    match &job.file {
        JobFile::WholeInput(_) => Some(given),
        JobFile::InInput(rel) => Some(given.join(path_from_text(rel.as_text())?)),
        JobFile::Setting(file) => path_from_text(file),
    }
}

pub fn lexical(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if out.parent().is_some() {
                    out.pop();
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

pub fn match_text(path: &Path) -> io::Result<Text> {
    let absolute = std::path::absolute(path)?;
    Ok(display_text_from_path(&lexical(&absolute)))
}

pub fn join_match_text(root: &Text, rel: &RelPath) -> Text {
    let mut bytes = root.as_bytes().to_vec();
    if bytes.last() != Some(&b'/') {
        bytes.push(b'/');
    }
    bytes.extend_from_slice(rel.as_bytes());
    Text::from_bytes(bytes)
}

pub fn matches(pattern: &Pattern, path: &Text) -> bool {
    pattern.is_match(path.as_bytes()) || (cfg!(windows) && pattern.is_match_folded(path.as_bytes()))
}

#[cfg(test)]
mod tests {
    use boxt_core::pattern::path_pattern;

    use super::*;

    #[cfg(windows)]
    #[test]
    fn a_shown_windows_path_uses_forward_slashes() {
        let shown = display_text_from_path(Path::new(r"C:\Users\me\repo\boxt.yaml"));
        assert_eq!(shown.as_str(), Some("C:/Users/me/repo/boxt.yaml"));
    }

    #[cfg(unix)]
    #[test]
    fn a_shown_unix_path_is_unchanged() {
        let shown = display_text_from_path(Path::new("/home/me/a\\b/boxt.yaml"));
        assert_eq!(shown.as_str(), Some("/home/me/a\\b/boxt.yaml"));
    }

    #[test]
    fn lexical_drops_dots_and_pops_parents() {
        assert_eq!(lexical(Path::new("/a/./b/../c")), Path::new("/a/c"));
        assert_eq!(lexical(Path::new("/../a")), Path::new("/a"));
    }

    #[cfg(unix)]
    #[test]
    fn a_match_text_is_absolute_and_lexical() {
        let here = std::env::current_dir().unwrap();
        let text = match_text(Path::new("a/./b/../c.c")).unwrap();
        let expected = display_text_from_path(&here.join("a/c.c"));
        assert_eq!(text, expected);
        let joined = join_match_text(&Text::from("/"), &RelPath::new(Text::from("x/y")).unwrap());
        assert_eq!(joined, Text::from("/x/y"));
        let joined = join_match_text(&Text::from("/r"), &RelPath::new(Text::from("x")).unwrap());
        assert_eq!(joined, Text::from("/r/x"));
    }

    #[test]
    fn matching_folds_case_only_on_windows() {
        let p = path_pattern(&Text::from("/repo"), &Text::from("third_party/**")).unwrap();
        assert!(matches(&p, &Text::from("/repo/third_party/a.c")));
        assert_eq!(
            matches(&p, &Text::from("/repo/Third_Party/a.c")),
            cfg!(windows)
        );
        assert!(!matches(&p, &Text::from("/repo/src/a.c")));
    }
}
