use std::borrow::Cow;
use std::fmt;
use std::hash::{Hash, Hasher};

use globset::{Candidate, GlobBuilder, GlobMatcher};

use crate::location::normalize_value_path;
use crate::text::Text;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PatternMode {
    Path,
    Text,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PatternError {
    Empty,
    Glob(String),
}

impl fmt::Display for PatternError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PatternError::Empty => f.write_str("the pattern is empty"),
            PatternError::Glob(e) => write!(f, "the pattern is not a valid glob: {e}"),
        }
    }
}

impl std::error::Error for PatternError {}

#[derive(Debug, Clone)]
pub struct Pattern {
    source: String,
    mode: PatternMode,
    under: bool,
    matchers: Vec<GlobMatcher>,
}

const BACKSLASH: char = '\u{E000}';

fn encode_pattern(source: &str) -> String {
    let mut out = String::with_capacity(source.len());
    let mut chars = source.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('\\') => out.push(BACKSLASH),
            Some(next) => {
                out.push('\\');
                out.push(next);
            }
            None => out.push('\\'),
        }
    }
    out
}

fn encode_value(value: &[u8]) -> Cow<'_, [u8]> {
    if !value.contains(&b'\\') {
        return Cow::Borrowed(value);
    }
    let mut buf = [0u8; 4];
    let marker = BACKSLASH.encode_utf8(&mut buf).as_bytes();
    let mut out = Vec::with_capacity(value.len() + 8);
    for &b in value {
        if b == b'\\' {
            out.extend_from_slice(marker);
        } else {
            out.push(b);
        }
    }
    Cow::Owned(out)
}

fn escape(literal: &str) -> String {
    let mut out = String::with_capacity(literal.len());
    for c in literal.chars() {
        if matches!(c, '*' | '?' | '[' | ']' | '{' | '}' | '\\' | '!' | ',') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

fn glob(source: &str, mode: PatternMode) -> Result<GlobMatcher, PatternError> {
    GlobBuilder::new(&encode_pattern(source))
        .literal_separator(mode == PatternMode::Path)
        .backslash_escape(true)
        .case_insensitive(false)
        .build()
        .map(|g| g.compile_matcher())
        .map_err(|e| PatternError::Glob(e.kind().to_string()))
}

pub fn compile(source: &str, mode: PatternMode) -> Result<Pattern, PatternError> {
    if source.is_empty() {
        return Err(PatternError::Empty);
    }
    Ok(Pattern {
        source: source.to_owned(),
        mode,
        under: false,
        matchers: vec![glob(source, mode)?],
    })
}

pub fn path_under(prefix: &str) -> Result<Pattern, PatternError> {
    if prefix.is_empty() {
        return Err(PatternError::Empty);
    }
    let normalized = normalize_value_path(&Text::from(prefix));
    let literal = escape(&String::from_utf8_lossy(normalized.as_bytes()));
    let below = if literal.ends_with('/') {
        format!("{literal}**")
    } else {
        format!("{literal}/**")
    };
    Ok(Pattern {
        source: prefix.to_owned(),
        mode: PatternMode::Path,
        under: true,
        matchers: vec![
            glob(&literal, PatternMode::Path)?,
            glob(&below, PatternMode::Path)?,
        ],
    })
}

impl Pattern {
    pub fn source(&self) -> &str {
        &self.source
    }

    pub fn mode(&self) -> PatternMode {
        self.mode
    }

    pub fn is_under(&self) -> bool {
        self.under
    }

    pub fn is_match(&self, value: &[u8]) -> bool {
        let value = encode_value(value);
        let candidate = Candidate::from_bytes(value.as_ref());
        self.matchers
            .iter()
            .any(|m| m.is_match_candidate(&candidate))
    }

    fn key(&self) -> (&str, PatternMode, bool) {
        (&self.source, self.mode, self.under)
    }
}

impl PartialEq for Pattern {
    fn eq(&self, other: &Self) -> bool {
        self.key() == other.key()
    }
}

impl Eq for Pattern {}

impl PartialOrd for Pattern {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Pattern {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.key().cmp(&other.key())
    }
}

impl Hash for Pattern {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.key().hash(state);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_mode_stars_stop_at_slashes() {
        let p = compile("src/*.c", PatternMode::Path).unwrap();
        assert!(p.is_match(b"src/a.c"));
        assert!(!p.is_match(b"src/sub/a.c"));
        assert!(
            compile("src/**/*.c", PatternMode::Path)
                .unwrap()
                .is_match(b"src/sub/a.c")
        );
    }

    #[test]
    fn text_mode_matches_the_whole_value() {
        let p = compile("*bind*", PatternMode::Text).unwrap();
        assert!(p.is_match(b"net/bind_any"));
        assert!(
            !compile("bind", PatternMode::Text)
                .unwrap()
                .is_match(b"rebind")
        );
        assert!(
            !compile("Bind", PatternMode::Text)
                .unwrap()
                .is_match(b"bind")
        );
    }

    #[test]
    fn path_under_is_the_path_or_below_it() {
        let p = path_under("./tests/").unwrap();
        assert!(p.is_match(b"tests"));
        assert!(p.is_match(b"tests/a/b.c"));
        assert!(!p.is_match(b"tests2/a.c"));
        assert!(path_under("a*b").unwrap().is_match(b"a*b/c"));
        assert!(!path_under("a*b").unwrap().is_match(b"axb/c"));
        assert!(path_under("a\\b").unwrap().is_match(b"a\\b/c"));
        assert!(path_under("{x,y}").unwrap().is_match(b"{x,y}"));
    }

    #[test]
    fn a_backslash_is_an_ordinary_byte_on_every_host() {
        let p = compile("a\\\\*", PatternMode::Path).unwrap();
        assert!(p.is_match(b"a\\b"));
        assert!(!p.is_match(b"a/b"));
        assert!(!compile("a/b", PatternMode::Path).unwrap().is_match(b"a\\b"));
        assert!(
            compile("a\\*b", PatternMode::Text)
                .unwrap()
                .is_match(b"a*b")
        );
        assert!(
            !compile("a\\*b", PatternMode::Text)
                .unwrap()
                .is_match(b"axb")
        );
    }

    #[test]
    fn empty_and_invalid_patterns_are_refused() {
        assert_eq!(
            compile("", PatternMode::Text).unwrap_err(),
            PatternError::Empty
        );
        assert_eq!(path_under("").unwrap_err(), PatternError::Empty);
        assert!(matches!(
            compile("[a", PatternMode::Text),
            Err(PatternError::Glob(_))
        ));
        assert!(matches!(
            compile("{a", PatternMode::Path),
            Err(PatternError::Glob(_))
        ));
    }
}
