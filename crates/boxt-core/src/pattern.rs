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
    Absolute,
    DotSegment,
    LeadingDot,
    EmptySegment,
    NotUtf8,
}

impl fmt::Display for PatternError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PatternError::Empty => f.write_str("the pattern is empty"),
            PatternError::Glob(e) => write!(f, "the pattern is not a valid glob: {e}"),
            PatternError::Absolute => {
                f.write_str("the pattern is absolute; it must be relative to its base folder")
            }
            PatternError::DotSegment => f.write_str("the pattern has a '.' or '..' segment"),
            PatternError::LeadingDot => f.write_str(
                "the pattern starts with './'; drop it, since a pattern is already relative to its base folder",
            ),
            PatternError::EmptySegment => f.write_str(
                "the pattern has an empty segment, such as a trailing '/' or '//'; write a folder as 'name/**'",
            ),
            PatternError::NotUtf8 => f.write_str("the pattern or its base folder is not UTF-8"),
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
    folded: Option<Vec<GlobMatcher>>,
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
    build(source, mode, false)
}

fn build(source: &str, mode: PatternMode, fold: bool) -> Result<GlobMatcher, PatternError> {
    GlobBuilder::new(&encode_pattern(source))
        .literal_separator(mode == PatternMode::Path)
        .backslash_escape(true)
        .case_insensitive(fold)
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
        folded: None,
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
        folded: None,
    })
}

fn is_absolute(pattern: &str) -> bool {
    let bytes = pattern.as_bytes();
    bytes.first() == Some(&b'/')
        || (bytes.len() >= 2
            && bytes[0].is_ascii_alphabetic()
            && bytes[1] == b':'
            && matches!(bytes.get(2), None | Some(b'/')))
}

fn normalized_base(base: &Text) -> Result<String, PatternError> {
    let normalized = normalize_value_path(base);
    let text = std::str::from_utf8(normalized.as_bytes()).map_err(|_| PatternError::NotUtf8)?;
    Ok(escape(text.trim_end_matches('/')))
}

fn path_matcher(source: String, glob_source: &str) -> Result<Pattern, PatternError> {
    Ok(Pattern {
        source,
        mode: PatternMode::Path,
        under: false,
        matchers: vec![build(glob_source, PatternMode::Path, false)?],
        folded: Some(vec![build(glob_source, PatternMode::Path, true)?]),
    })
}

pub fn path_pattern(base: &Text, pattern: &Text) -> Result<Pattern, PatternError> {
    let pattern = pattern.as_str().ok_or(PatternError::NotUtf8)?;
    if pattern.is_empty() {
        return Err(PatternError::Empty);
    }
    if is_absolute(pattern) {
        return Err(PatternError::Absolute);
    }
    if pattern.starts_with("./") {
        return Err(PatternError::LeadingDot);
    }
    if pattern.split('/').any(|s| s == "." || s == "..") {
        return Err(PatternError::DotSegment);
    }
    if pattern.split('/').any(str::is_empty) {
        return Err(PatternError::EmptySegment);
    }
    let glob_source = format!("{}/{pattern}", normalized_base(base)?);
    path_matcher(pattern.to_owned(), &glob_source)
}

pub fn literal_path(path: &Text) -> Result<Pattern, PatternError> {
    if path.as_bytes().is_empty() {
        return Err(PatternError::Empty);
    }
    let literal = normalized_base(path)?;
    path_matcher(
        String::from_utf8_lossy(path.as_bytes()).into_owned(),
        &literal,
    )
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

    pub fn is_match_folded(&self, value: &[u8]) -> bool {
        let Some(folded) = &self.folded else {
            return self.is_match(value);
        };
        let value = encode_value(value);
        let candidate = Candidate::from_bytes(value.as_ref());
        folded.iter().any(|m| m.is_match_candidate(&candidate))
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

    fn at(base: &str, pattern: &str) -> Pattern {
        path_pattern(&Text::from(base), &Text::from(pattern)).unwrap()
    }

    #[test]
    fn a_path_pattern_matches_below_its_base_only() {
        let p = at("/repo/./sub/", "gen/**");
        assert!(p.is_match(b"/repo/sub/gen/x.c"));
        assert!(p.is_match(b"/repo/sub/gen/a/b.c"));
        assert!(!p.is_match(b"/repo/gen/x.c"));
        assert!(!p.is_match(b"/repo/sub/gen"));
        let star = at("/repo", "src/*.c");
        assert!(star.is_match(b"/repo/src/a.c"));
        assert!(!star.is_match(b"/repo/src/a/b.c"));
        assert!(at("/", "repo/**").is_match(b"/repo/a"));
        assert!(at("C:/Users/me", "x/*").is_match(b"C:/Users/me/x/a"));
        assert_eq!(at("/repo", "gen/**").source(), "gen/**");
    }

    #[test]
    fn a_base_is_taken_literally() {
        let p = at("/a*b/[x]{y,z}", "*.c");
        assert!(p.is_match(b"/a*b/[x]{y,z}/m.c"));
        assert!(!p.is_match(b"/axb/x/m.c"));
        assert!(at("/a\\b", "*.c").is_match(b"/a\\b/m.c"));
    }

    #[test]
    fn folding_is_a_separate_matcher() {
        let p = at("/repo", "third_party/**");
        assert!(!p.is_match(b"/repo/Third_Party/a.c"));
        assert!(p.is_match_folded(b"/repo/Third_Party/a.c"));
        assert!(!at("/Repo", "x").is_match(b"/repo/x"));
        assert!(at("/Repo", "x").is_match_folded(b"/repo/x"));
        let text = compile("Bind", PatternMode::Text).unwrap();
        assert!(!text.is_match_folded(b"bind"));
    }

    #[test]
    fn a_literal_path_matches_only_itself() {
        let p = literal_path(&Text::from("/repo/./boxt.yaml")).unwrap();
        assert!(p.is_match(b"/repo/boxt.yaml"));
        assert!(!p.is_match(b"/repo/boxt.yamlx"));
        assert!(!p.is_match(b"/repo/sub/boxt.yaml"));
        assert!(p.is_match_folded(b"/REPO/Boxt.yaml"));
        assert!(
            literal_path(&Text::from("/r/[a]*.yaml"))
                .unwrap()
                .is_match(b"/r/[a]*.yaml")
        );
        assert_eq!(
            literal_path(&Text::default()).unwrap_err(),
            PatternError::Empty
        );
    }

    #[test]
    fn bad_path_patterns_are_refused() {
        let base = Text::from("/repo");
        let refused = |p: &str| path_pattern(&base, &Text::from(p)).unwrap_err();
        assert_eq!(refused(""), PatternError::Empty);
        assert_eq!(refused("/x/**"), PatternError::Absolute);
        assert_eq!(refused("C:/x"), PatternError::Absolute);
        assert_eq!(refused("c:"), PatternError::Absolute);
        assert_eq!(refused("../x/**"), PatternError::DotSegment);
        assert_eq!(refused("a/./b"), PatternError::DotSegment);
        assert_eq!(refused("a/.."), PatternError::DotSegment);
        assert_eq!(refused("./gen/**"), PatternError::LeadingDot);
        assert_eq!(refused("."), PatternError::DotSegment);
        assert_eq!(refused("gen/"), PatternError::EmptySegment);
        assert_eq!(refused("a//b"), PatternError::EmptySegment);
        assert!(refused("gen/").to_string().contains("'name/**'"));
        assert!(refused("./a").to_string().contains("drop it"));
        assert!(matches!(refused("[a"), PatternError::Glob(_)));
        assert!(path_pattern(&base, &Text::from("..x/.y")).is_ok());
        assert!(path_pattern(&base, &Text::from("c:x")).is_ok());
        #[cfg(unix)]
        assert_eq!(
            path_pattern(&Text::from_bytes(b"/r\xff".to_vec()), &Text::from("x")).unwrap_err(),
            PatternError::NotUtf8
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
