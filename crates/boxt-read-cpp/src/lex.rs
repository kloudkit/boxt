use boxt_core::SortedVec;
use boxt_core::Text;
use boxt_core::fact::{Directive, GuardChain, GuardFrame, RegionId};

pub const CODE: u8 = 0;
pub const COMMENT: u8 = 1;
pub const LITERAL: u8 = 2;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    If,
    Ifdef,
    Ifndef,
    Elif,
    Elifdef,
    Elifndef,
    Else,
    Endif,
    Error,
    Include,
    Define,
    Other,
}

impl Kind {
    fn of(name: &[u8]) -> Kind {
        match name {
            b"if" => Kind::If,
            b"ifdef" => Kind::Ifdef,
            b"ifndef" => Kind::Ifndef,
            b"elif" => Kind::Elif,
            b"elifdef" => Kind::Elifdef,
            b"elifndef" => Kind::Elifndef,
            b"else" => Kind::Else,
            b"endif" => Kind::Endif,
            b"error" => Kind::Error,
            b"include" | b"include_next" => Kind::Include,
            b"define" => Kind::Define,
            _ => Kind::Other,
        }
    }

    pub fn directive(self) -> Option<Directive> {
        Some(match self {
            Kind::If => Directive::If,
            Kind::Ifdef => Directive::Ifdef,
            Kind::Ifndef => Directive::Ifndef,
            Kind::Elif => Directive::Elif,
            Kind::Elifdef => Directive::Elifdef,
            Kind::Elifndef => Directive::Elifndef,
            Kind::Else => Directive::Else,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone)]
pub struct Dir {
    pub kind: Kind,
    pub start: usize,
    pub end: usize,
    pub hash: usize,
    pub line: u32,
    pub text: String,
}

#[derive(Debug, Clone)]
pub struct Branch {
    pub dir: usize,
    pub body: (usize, usize),
    pub has_token: bool,
    pub region: RegionId,
}

#[derive(Debug, Clone)]
pub struct Group {
    pub branches: Vec<Branch>,
    pub endif: Option<usize>,
    pub parent: Option<(usize, usize)>,
}

pub struct Lex {
    pub mask: Vec<u8>,
    pub cls: Vec<u8>,
    pub line_start: Vec<usize>,
    pub dirs: Vec<Dir>,
    pub groups: Vec<Group>,
}

pub fn is_ident(b: u8) -> bool {
    b == b'_' || b == b'$' || b.is_ascii_alphanumeric()
}

pub fn is_ws(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\r' | b'\x0b' | b'\x0c' | b'\n')
}

pub fn splice_end(src: &[u8], i: usize) -> Option<usize> {
    if src.get(i) != Some(&b'\\') {
        return None;
    }
    let mut j = i + 1;
    if src.get(j) == Some(&b'\r') {
        j += 1;
    }
    (src.get(j) == Some(&b'\n')).then_some(j + 1)
}

fn escaped_newline(src: &[u8], k: usize) -> bool {
    (k >= 1 && src[k - 1] == b'\\') || (k >= 2 && src[k - 1] == b'\r' && src[k - 2] == b'\\')
}

pub fn raw_string_end(t: &[u8], quote: usize) -> Option<usize> {
    if quote == 0 || t[quote - 1] != b'R' {
        return None;
    }
    let mut p = quote - 1;
    if p >= 2 && &t[p - 2..p] == b"u8" {
        p -= 2;
    } else if p >= 1 && matches!(t[p - 1], b'u' | b'U' | b'L') {
        p -= 1;
    }
    if p > 0 && is_ident(t[p - 1]) {
        return None;
    }
    let open = t[quote + 1..].iter().take(17).position(|&b| b == b'(')?;
    let delim = &t[quote + 1..quote + 1 + open];
    if delim
        .iter()
        .any(|&b| matches!(b, b' ' | b')' | b'\\' | b'\t' | b'\n'))
    {
        return None;
    }
    let mut close = Vec::with_capacity(delim.len() + 2);
    close.push(b')');
    close.extend_from_slice(delim);
    close.push(b'"');
    let body = quote + 2 + open;
    t[body..]
        .windows(close.len())
        .position(|w| w == close.as_slice())
        .map(|k| body + k + close.len())
        .or(Some(t.len()))
}

fn digit_separator(t: &[u8], i: usize) -> bool {
    if i == 0
        || i + 1 >= t.len()
        || !t[i - 1].is_ascii_alphanumeric()
        || !t[i + 1].is_ascii_alphanumeric()
    {
        return false;
    }
    let mut s = i;
    while s > 0 && (is_ident(t[s - 1]) || t[s - 1] == b'\'') {
        s -= 1;
    }
    t[s].is_ascii_digit()
}

fn classify(src: &[u8], cpp: bool) -> Vec<u8> {
    let n = src.len();
    let mut cls = vec![CODE; n];
    let mut i = 0;
    while i < n {
        if let Some(j) = splice_end(src, i) {
            i = j;
            continue;
        }
        let c = src[i];
        if c == b'/' && src.get(i + 1) == Some(&b'*') {
            let end = src[i + 2..]
                .windows(2)
                .position(|w| w == b"*/")
                .map_or(n, |k| i + 2 + k + 2);
            cls[i..end].fill(COMMENT);
            i = end;
            continue;
        }
        if c == b'/' && src.get(i + 1) == Some(&b'/') {
            let mut k = i;
            let end = loop {
                match src[k..].iter().position(|&b| b == b'\n') {
                    None => break n,
                    Some(off) => {
                        let nl = k + off;
                        if escaped_newline(src, nl) {
                            k = nl + 1;
                        } else {
                            break nl;
                        }
                    }
                }
            };
            cls[i..end].fill(COMMENT);
            i = end;
            continue;
        }
        if cpp
            && c == b'"'
            && let Some(end) = raw_string_end(src, i)
        {
            if end >= i + 2 {
                cls[i + 1..end - 1].fill(LITERAL);
            }
            i = end;
            continue;
        }
        if c == b'\'' && digit_separator(src, i) {
            i += 1;
            continue;
        }
        if c == b'"' || c == b'\'' {
            let mut j = i + 1;
            while j < n && src[j] != c && src[j] != b'\n' {
                if let Some(k) = splice_end(src, j) {
                    j = k;
                    continue;
                }
                if src[j] == b'\\' {
                    j += 1;
                }
                j += 1;
            }
            let stop = j.min(n);
            cls[i + 1..stop].fill(LITERAL);
            i = if j < n && src[j] == c { j + 1 } else { stop };
            continue;
        }
        i += 1;
    }
    cls
}

pub fn line_starts(src: &[u8]) -> Vec<usize> {
    let mut v = vec![0];
    for (k, &b) in src.iter().enumerate() {
        if b == b'\n' {
            v.push(k + 1);
        }
    }
    if v.len() > 1 && v.last() == Some(&src.len()) {
        v.pop();
    }
    v
}

pub fn line_count(src: &[u8]) -> u32 {
    if src.is_empty() {
        0
    } else {
        u32::try_from(line_starts(src).len()).unwrap_or(u32::MAX)
    }
}

impl Lex {
    pub fn line_of(&self, off: usize) -> u32 {
        let k = match self.line_start.binary_search(&off) {
            Ok(k) => k + 1,
            Err(k) => k,
        };
        u32::try_from(k).unwrap_or(u32::MAX)
    }

    fn frame(&self, dir: usize, holds_code: bool) -> GuardFrame {
        let d = &self.dirs[dir];
        GuardFrame {
            directive: d.kind.directive().unwrap_or(Directive::Else),
            text: Text::from(d.text.as_str()),
            identifiers: identifiers(&d.text),
            holds_code,
        }
    }

    pub fn chain(&self, g: usize, j: usize) -> GuardChain {
        let group = &self.groups[g];
        let frames = group.branches[..=j]
            .iter()
            .enumerate()
            .map(|(i, b)| self.frame(b.dir, i == j))
            .collect();
        let has_alternative = group
            .branches
            .iter()
            .enumerate()
            .any(|(i, b)| i != j && b.has_token);
        GuardChain {
            frames,
            has_alternative,
            region: group.branches[j].region,
        }
    }

    #[cfg(test)]
    pub fn stack_at(&self, p: usize) -> Vec<GuardChain> {
        let mut chains = Vec::new();
        for (g, group) in self.groups.iter().enumerate() {
            if let Some(j) = group
                .branches
                .iter()
                .position(|b| b.body.0 <= p && p < b.body.1)
            {
                chains.push(self.chain(g, j));
            }
        }
        chains
    }

    pub fn max_branches(&self) -> usize {
        self.groups
            .iter()
            .map(|g| g.branches.len())
            .max()
            .unwrap_or(1)
    }

    pub fn branch_end_line(&self, g: usize, j: usize, last_line: u32) -> u32 {
        let group = &self.groups[g];
        let next = group.branches.get(j + 1).map(|b| b.dir).or(group.endif);
        next.map_or(last_line, |d| self.dirs[d].line)
    }
}

fn identifiers(text: &str) -> SortedVec<Text> {
    let t = text.as_bytes();
    let mut i = t.iter().position(|&b| b == b'#').map_or(0, |p| p + 1);
    while i < t.len() && is_ws(t[i]) {
        i += 1;
    }
    while i < t.len() && is_ident(t[i]) {
        i += 1;
    }
    let mut out: Vec<Text> = Vec::new();
    while i < t.len() {
        let c = t[i];
        if c == b'"' || c == b'\'' {
            i += 1;
            while i < t.len() && t[i] != c {
                if t[i] == b'\\' {
                    i += 1;
                }
                i += 1;
            }
            i += 1;
            continue;
        }
        if c.is_ascii_digit() {
            while i < t.len()
                && (is_ident(t[i])
                    || t[i] == b'.'
                    || (t[i] == b'\'' && t.get(i + 1).is_some_and(u8::is_ascii_alphanumeric)))
            {
                i += 1;
            }
            continue;
        }
        if is_ident(c) {
            let s = i;
            while i < t.len() && is_ident(t[i]) {
                i += 1;
            }
            let word = &t[s..i];
            let prefix = matches!(word, b"L" | b"u" | b"U" | b"u8");
            if prefix && matches!(t.get(i), Some(b'\'' | b'"')) {
                continue;
            }
            if word != b"defined" {
                out.push(Text::from(word));
            }
            continue;
        }
        i += 1;
    }
    out.sort();
    out.dedup();
    SortedVec::from(out)
}

fn normalize(src: &[u8], cls: &[u8], from: usize, to: usize) -> String {
    let mut out = Vec::with_capacity(to - from);
    let mut i = from;
    while i < to {
        if cls[i] == COMMENT {
            while i < to && cls[i] == COMMENT {
                i += 1;
            }
            out.push(b' ');
            continue;
        }
        if let Some(j) = splice_end(src, i) {
            i = j;
            continue;
        }
        out.push(src[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out)
        .split_ascii_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn lex(src: &[u8], cpp: bool) -> Lex {
    let n = src.len();
    let cls = classify(src, cpp);
    let mut mask = src.to_vec();
    for i in 0..n {
        if cls[i] != CODE && src[i] != b'\n' {
            mask[i] = b' ';
        }
        if cls[i] == CODE && splice_end(src, i).is_some() {
            mask[i] = b' ';
        }
    }
    let line_start = line_starts(src);
    let mut lx = Lex {
        mask,
        cls,
        line_start,
        dirs: Vec::new(),
        groups: Vec::new(),
    };

    let mut s = 0;
    while s < n {
        let mut e = s;
        while e < n && !(src[e] == b'\n' && lx.cls[e] == CODE && !escaped_newline(src, e)) {
            e += 1;
        }
        let first = (s..e).find(|&p| lx.cls[p] == CODE && !is_ws(lx.mask[p]));
        if let Some(p) = first
            && lx.mask[p] == b'#'
        {
            let mut q = p + 1;
            while q < e && is_ws(lx.mask[q]) {
                q += 1;
            }
            let ns = q;
            while q < e && is_ident(lx.mask[q]) {
                q += 1;
            }
            let kind = Kind::of(&lx.mask[ns..q]);
            let text_end = match kind {
                Kind::If | Kind::Elif => e,
                Kind::Ifdef | Kind::Ifndef | Kind::Elifdef | Kind::Elifndef => {
                    let mut r = q;
                    while r < e && is_ws(lx.mask[r]) {
                        r += 1;
                    }
                    let a = r;
                    while r < e && is_ident(lx.mask[r]) {
                        r += 1;
                    }
                    if r > a { r } else { q }
                }
                _ => q,
            };
            let text = if kind.directive().is_some() {
                normalize(src, &lx.cls, p, text_end)
            } else {
                String::new()
            };
            let line = lx.line_of(p);
            lx.dirs.push(Dir {
                kind,
                start: s,
                end: e,
                hash: p,
                line,
                text,
            });
        }
        s = e + 1;
    }

    let mut open: Vec<usize> = Vec::new();
    for (k, d) in lx.dirs.iter().enumerate() {
        let body_start = (d.end + 1).min(n);
        let branch = |dir| Branch {
            dir,
            body: (body_start, n),
            has_token: false,
            region: RegionId(0),
        };
        match d.kind {
            Kind::If | Kind::Ifdef | Kind::Ifndef => {
                let parent = open.last().map(|&g| (g, lx.groups[g].branches.len() - 1));
                lx.groups.push(Group {
                    branches: vec![branch(k)],
                    endif: None,
                    parent,
                });
                open.push(lx.groups.len() - 1);
            }
            Kind::Elif | Kind::Elifdef | Kind::Elifndef | Kind::Else => {
                if let Some(&g) = open.last() {
                    let br = &mut lx.groups[g].branches;
                    if let Some(last) = br.last_mut() {
                        last.body.1 = d.start;
                    }
                    br.push(branch(k));
                }
            }
            Kind::Endif => {
                if let Some(g) = open.pop() {
                    if let Some(last) = lx.groups[g].branches.last_mut() {
                        last.body.1 = d.start;
                    }
                    lx.groups[g].endif = Some(k);
                }
            }
            _ => {}
        }
    }

    let mut order: Vec<(usize, usize, usize)> = Vec::new();
    for (g, group) in lx.groups.iter().enumerate() {
        for (j, b) in group.branches.iter().enumerate() {
            order.push((b.dir, g, j));
        }
    }
    order.sort_unstable();
    for (index, &(_, g, j)) in order.iter().enumerate() {
        lx.groups[g].branches[j].region = RegionId(u32::try_from(index).unwrap_or(u32::MAX));
    }

    let dirs = &lx.dirs;
    let mask = &lx.mask;
    for g in &mut lx.groups {
        for b in &mut g.branches {
            let (bs, be) = b.body;
            let mut pos = bs;
            let mut tok = false;
            let first = dirs.partition_point(|d| d.start < bs);
            for d in dirs[first..].iter().take_while(|d| d.start < be) {
                if mask[pos.min(d.start)..d.start].iter().any(|&c| !is_ws(c))
                    || d.kind != Kind::Error
                {
                    tok = true;
                    break;
                }
                pos = (d.end + 1).min(be).max(pos);
            }
            if !tok && pos < be {
                tok = mask[pos..be].iter().any(|&c| !is_ws(c));
            }
            b.has_token = tok;
        }
    }
    lx
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(d: Directive, t: &str, h: bool) -> (Directive, String, bool) {
        (d, t.to_string(), h)
    }

    type Shape = (bool, Vec<(Directive, String, bool)>);

    fn shape(chains: &[GuardChain]) -> Vec<Shape> {
        chains
            .iter()
            .map(|c| {
                (
                    c.has_alternative,
                    c.frames
                        .iter()
                        .map(|fr| {
                            (
                                fr.directive,
                                fr.text.as_str().unwrap().to_string(),
                                fr.holds_code,
                            )
                        })
                        .collect(),
                )
            })
            .collect()
    }

    #[test]
    fn matrix_cases() {
        let src = "void t(void) {\n#if A\n  x();\n#else /* !A */\n#ifdef B\n  m();\n#endif\n#endif\n#ifdef X\n  h();\n#elif Y\n  k();\n#endif\n#ifdef _WIN32\n  f();\n#else\n#error no\n#endif\n#if defined(X) && \\\n  FOO(1) // c\n  z();\n#endif\n}\n";
        let lx = lex(src.as_bytes(), false);
        let at = |needle: &str| lx.stack_at(src.find(needle).unwrap());
        assert_eq!(
            shape(&at("m()")),
            vec![
                (
                    true,
                    vec![
                        f(Directive::If, "#if A", false),
                        f(Directive::Else, "#else", true)
                    ]
                ),
                (false, vec![f(Directive::Ifdef, "#ifdef B", true)]),
            ]
        );
        assert_eq!(
            shape(&at("x()")),
            vec![(true, vec![f(Directive::If, "#if A", true)])]
        );
        assert_eq!(
            shape(&at("k()")),
            vec![(
                true,
                vec![
                    f(Directive::Ifdef, "#ifdef X", false),
                    f(Directive::Elif, "#elif Y", true)
                ]
            )]
        );
        assert_eq!(
            shape(&at("f()")),
            vec![(false, vec![f(Directive::Ifdef, "#ifdef _WIN32", true)])]
        );
        let z = at("z()");
        assert_eq!(
            shape(&z),
            vec![(
                false,
                vec![f(Directive::If, "#if defined(X) && FOO(1)", true)]
            )]
        );
        assert_eq!(
            z[0].frames[0].identifiers.as_slice(),
            &[Text::from("FOO"), Text::from("X")]
        );
        assert!(at("void").is_empty());
        for chain in at("m()") {
            chain.validate().unwrap();
        }
    }

    #[test]
    fn region_ids_follow_source_order() {
        let src = b"#if A\n#ifdef B\nb();\n#endif\n#else\nc();\n#endif\n#if C\n#endif\n";
        let lx = lex(src, false);
        let ids: Vec<(u32, u32)> = lx
            .groups
            .iter()
            .flat_map(|g| g.branches.iter().map(|b| (lx.dirs[b.dir].line, b.region.0)))
            .collect();
        assert_eq!(ids, vec![(1, 0), (5, 2), (2, 1), (8, 3)]);
        assert_eq!(lx.branch_end_line(0, 0, 9), 5);
        assert_eq!(lx.branch_end_line(0, 1, 9), 7);
    }

    #[test]
    fn comments_and_strings_hide_directives() {
        let src = b"/* \n#if A\n */\nchar *s = \"\\\n#if B\";\n// x \\\n#if C\n  /* c\n */ #ifdef D\nq();\n#endif\n";
        let lx = lex(src, false);
        let kinds: Vec<Kind> = lx.dirs.iter().map(|d| d.kind).collect();
        assert_eq!(kinds, vec![Kind::Ifdef, Kind::Endif]);
        assert_eq!(lx.dirs[0].line, 9);
        assert_eq!(lx.dirs[0].text, "#ifdef D");
    }

    #[test]
    fn multiline_comment_in_directive_does_not_end_it() {
        let src = b"#if A /* x\n y */ && (B)\nq();\n#endif\n";
        let lx = lex(src, false);
        assert_eq!(lx.dirs[0].text, "#if A && (B)");
        assert_eq!(
            lx.groups[0].branches[0].body.0,
            src.iter().position(|&b| b == b'q').unwrap()
        );
    }

    #[test]
    fn literal_prefixes_and_separators_name_nothing() {
        let src = b"#if X == L'a' && Y > 1'000'000 && u8\"s\" && Z\nq();\n#endif\n";
        for cpp in [false, true] {
            let lx = lex(src, cpp);
            assert_eq!(lx.dirs.len(), 2);
            let frame = &lx.chain(0, 0).frames[0];
            assert_eq!(
                frame.identifiers.as_slice(),
                &[Text::from("X"), Text::from("Y"), Text::from("Z")]
            );
        }
        let lx = lex(b"int x = 1'000;\n#if A\n#endif\n", false);
        assert_eq!(lx.dirs.len(), 2);
    }

    #[test]
    fn raw_strings_only_in_cpp() {
        let src = b"auto s = R\"x(\n#if A\n)x\";\n";
        assert!(lex(src, true).dirs.is_empty());
        assert_eq!(lex(src, false).dirs.len(), 1);
    }

    #[test]
    fn error_only_branch_is_no_alternative() {
        let src = b"#ifdef _WIN32\nf();\n#else\n#error x\n#endif\n";
        let lx = lex(src, false);
        let at = src.windows(3).position(|w| w == b"f()").unwrap();
        let chain = &lx.stack_at(at)[0];
        assert!(!chain.has_alternative);
    }
}
