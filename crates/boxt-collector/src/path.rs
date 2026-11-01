use std::path::{Path, PathBuf};

use boxt_core::Text;
use boxt_core::location::RelPath;
use boxt_core::message::{Job, JobFile};
use boxt_core::settings::PathPattern;

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

struct Glob<'a> {
    pattern: &'a [u8],
    path: &'a [u8],
    memo: Vec<Option<bool>>,
}

impl Glob<'_> {
    fn at(&mut self, pi: usize, si: usize) -> bool {
        let slot = pi * (self.path.len() + 1) + si;
        if let Some(known) = self.memo[slot] {
            return known;
        }
        let (pattern, path) = (self.pattern, self.path);
        let found = match pattern.get(pi) {
            None => si == path.len(),
            Some(b'*') if pattern.get(pi + 1) == Some(&b'*') => {
                let next = if pattern.get(pi + 2) == Some(&b'/') {
                    pi + 3
                } else {
                    pi + 2
                };
                (si..=path.len()).any(|i| self.at(next, i))
            }
            Some(b'*') => (si..=path.len())
                .take_while(|&i| i == si || path[i - 1] != b'/')
                .any(|i| self.at(pi + 1, i)),
            Some(b'?') => path.get(si).is_some_and(|&c| c != b'/') && self.at(pi + 1, si + 1),
            Some(&c) => path.get(si) == Some(&c) && self.at(pi + 1, si + 1),
        };
        self.memo[slot] = Some(found);
        found
    }
}

fn glob(pattern: &[u8], path: &[u8]) -> bool {
    let mut g = Glob {
        pattern,
        path,
        memo: vec![None; (pattern.len() + 1) * (path.len() + 1)],
    };
    g.at(0, 0)
}

fn separators(bytes: &[u8]) -> Vec<u8> {
    let mut out = bytes.to_vec();
    if cfg!(windows) {
        for b in &mut out {
            if *b == b'\\' {
                *b = b'/';
            }
        }
    }
    while out.len() > 1 && out.last() == Some(&b'/') {
        out.pop();
    }
    out
}

pub fn declared_for(pattern: &PathPattern, on_disk: &Text) -> bool {
    let path = separators(on_disk.as_bytes());
    let base = separators(pattern.base.as_bytes());
    let rel = if base.is_empty() {
        &path[..]
    } else {
        match path.strip_prefix(base.as_slice()).and_then(|r| {
            r.strip_prefix(b"/")
                .or_else(|| base.ends_with(b"/").then_some(r))
        }) {
            Some(rel) => rel,
            None => return false,
        }
    };
    glob(&separators(pattern.pattern.as_bytes()), rel)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decl(base: &str, pattern: &str) -> PathPattern {
        PathPattern {
            base: Text::from(base),
            pattern: Text::from(pattern),
        }
    }

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
    fn encoding_patterns_match_under_their_base() {
        let file = Text::from("/repo/legacy/win/a.c");
        let matches = |base: &str, pattern: &str| declared_for(&decl(base, pattern), &file);
        assert!(matches("/repo", "legacy/**"));
        assert!(matches("/repo/", "legacy/*/a.c"));
        assert!(matches("/repo", "**/*.c"));
        assert!(!matches("/repo", "legacy/*.c"));
        assert!(!matches("/other", "**"));
        assert!(!matches("/rep", "**"));
        assert!(matches("/repo", "legacy/win/?.c"));
        assert!(matches("/repo/legacy", "win/*.c"));
        assert!(matches("/", "repo/**"));
        let long = Text::from(format!("/repo/{}b", "a".repeat(200)).as_str());
        let started = std::time::Instant::now();
        assert!(!declared_for(&decl("/repo", &"*a".repeat(30)), &long));
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
    }
}
