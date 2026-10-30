use crate::lex::{Lex, is_ws};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Rule {
    Foreach,
    Proto,
    ClassAttr,
    Decls,
    TrailingAttr,
    LoneInvocation,
    LoneMacro,
    Linkage,
    DeclAttr,
    Qualifier,
}

impl Rule {
    pub fn is_invocation(self) -> bool {
        matches!(
            self,
            Rule::Foreach | Rule::TrailingAttr | Rule::LoneInvocation
        )
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Pseudo {
    pub start: usize,
    pub name_end: usize,
    pub end: usize,
    pub rule: Rule,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum T {
    Ident,
    Num,
    Punct(u8),
}

#[derive(Clone, Copy, Debug)]
struct Tok {
    t: T,
    s: usize,
    e: usize,
    line: u32,
}

pub const KEYWORDS: &[&str] = &[
    "auto",
    "break",
    "case",
    "char",
    "const",
    "continue",
    "default",
    "do",
    "double",
    "else",
    "enum",
    "extern",
    "float",
    "for",
    "goto",
    "if",
    "inline",
    "int",
    "long",
    "register",
    "restrict",
    "return",
    "short",
    "signed",
    "sizeof",
    "static",
    "struct",
    "switch",
    "typedef",
    "union",
    "unsigned",
    "void",
    "volatile",
    "while",
    "_Bool",
    "_Complex",
    "_Atomic",
    "_Noreturn",
    "_Static_assert",
    "_Thread_local",
    "_Alignof",
    "_Alignas",
    "alignas",
    "alignof",
    "bool",
    "class",
    "constexpr",
    "decltype",
    "delete",
    "explicit",
    "export",
    "friend",
    "mutable",
    "namespace",
    "new",
    "noexcept",
    "operator",
    "private",
    "protected",
    "public",
    "template",
    "this",
    "throw",
    "try",
    "typeid",
    "typename",
    "using",
    "virtual",
    "co_await",
    "co_return",
    "co_yield",
    "typeof",
    "__typeof__",
    "defined",
];

const DECL_KEYWORDS: &[&str] = &[
    "static",
    "extern",
    "const",
    "volatile",
    "inline",
    "int",
    "char",
    "void",
    "unsigned",
    "signed",
    "long",
    "short",
    "float",
    "double",
    "struct",
    "union",
    "enum",
    "class",
    "typedef",
    "register",
    "bool",
    "_Bool",
    "virtual",
    "explicit",
    "friend",
    "template",
    "constexpr",
];

const TYPE_KEYWORDS: &[&str] = &[
    "void", "int", "char", "short", "long", "unsigned", "signed", "float", "double", "bool",
    "_Bool",
];

const SCAN: usize = 4000;

fn all_caps(w: &[u8]) -> bool {
    w.len() >= 2
        && (w[0].is_ascii_uppercase() || w[0] == b'_')
        && w.iter()
            .all(|&b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
        && w.iter().any(|b| b.is_ascii_uppercase())
}

fn tokens(lx: &Lex) -> Vec<Tok> {
    let n = lx.mask.len();
    let mut in_dir = vec![false; n];
    for d in &lx.dirs {
        in_dir[d.start..d.end].fill(true);
    }
    let m = &lx.mask;
    let mut out = Vec::new();
    let mut i = 0;
    while i < n {
        let c = m[i];
        if in_dir[i] || is_ws(c) {
            i += 1;
            continue;
        }
        let s = i;
        let t = if c == b'_' || c == b'$' || c.is_ascii_alphabetic() {
            while i < n
                && !in_dir[i]
                && (m[i] == b'_' || m[i] == b'$' || m[i].is_ascii_alphanumeric())
            {
                i += 1;
            }
            T::Ident
        } else if c.is_ascii_digit() {
            while i < n
                && !in_dir[i]
                && (m[i].is_ascii_alphanumeric() || m[i] == b'.' || m[i] == b'_')
            {
                i += 1;
            }
            T::Num
        } else {
            i += 1;
            T::Punct(c)
        };
        out.push(Tok {
            t,
            s,
            e: i,
            line: lx.line_of(s),
        });
    }
    out
}

fn matching(toks: &[Tok], open: usize) -> Option<usize> {
    let mut depth = 0i64;
    for (k, t) in toks.iter().enumerate().skip(open).take(SCAN) {
        match t.t {
            T::Punct(b'(') => depth += 1,
            T::Punct(b')') => {
                depth -= 1;
                if depth == 0 {
                    return Some(k);
                }
            }
            _ => {}
        }
    }
    None
}

fn matching_back(toks: &[Tok], close: usize) -> Option<usize> {
    let mut depth = 0i64;
    let lo = close.saturating_sub(SCAN);
    for k in (lo..=close).rev() {
        match toks[k].t {
            T::Punct(b')') => depth += 1,
            T::Punct(b'(') => {
                depth -= 1;
                if depth == 0 {
                    return Some(k);
                }
            }
            _ => {}
        }
    }
    None
}

fn is_p(t: Option<&Tok>, c: u8) -> bool {
    t.is_some_and(|t| t.t == T::Punct(c))
}

fn foreach_name(w: &[u8]) -> bool {
    all_caps(w) && (w.ends_with(b"_FOREACH") || w.ends_with(b"_FOREACH_SAFE"))
}

fn decls_name(w: &[u8]) -> bool {
    all_caps(w)
        && [
            &b"BEGIN_DECLS"[..],
            b"END_DECLS",
            b"BEGINDECLS",
            b"ENDDECLS",
        ]
        .iter()
        .any(|suf| w.ends_with(suf) && (w.len() == suf.len() || w[w.len() - suf.len() - 1] == b'_'))
}

fn blank(out: &mut [u8], s: usize, e: usize) {
    for b in &mut out[s..e] {
        if *b != b'\n' {
            *b = b' ';
        }
    }
}

pub fn rewrite(src: &[u8], lx: &Lex) -> (Vec<u8>, Vec<Pseudo>) {
    let toks = tokens(lx);
    let word = |t: &Tok| &src[t.s..t.e];
    let kw = |t: &Tok, list: &[&str]| {
        t.t == T::Ident && std::str::from_utf8(word(t)).is_ok_and(|w| list.contains(&w))
    };
    let mut out = src.to_vec();
    let mut gone = vec![false; toks.len()];
    let mut pseudo = Vec::new();
    let lines = toks.last().map_or(0, |t| t.line as usize);
    let mut per_line = vec![0usize; lines + 1];
    for t in &toks {
        per_line[t.line as usize] += 1;
    }
    let cont: &[u8] = b",;=([)].:?+-*/%&|^<>!}";
    let first_on_line = |k: usize| k == 0 || toks[k - 1].line != toks[k].line;
    let last_on_line = |k: usize| toks.get(k + 1).is_none_or(|t| t.line != toks[k].line);

    for k in 0..toks.len() {
        if gone[k] || toks[k].t != T::Ident {
            continue;
        }
        let w = word(&toks[k]);
        let live_prev = (0..k).rev().find(|&p| !gone[p]);
        let prev = live_prev.map(|p| &toks[p]);
        let at_start = match live_prev {
            None => true,
            Some(p) => match toks[p].t {
                T::Punct(b';' | b'{' | b'}') => true,
                T::Punct(b':') => p == 0 || toks[p - 1].t != T::Punct(b':'),
                _ => false,
            },
        };
        let next = toks.get(k + 1);
        let next2 = toks.get(k + 2);
        let name_end = toks[k].e;
        let mut push = |rule: Rule, start: usize, end: usize| {
            pseudo.push(Pseudo {
                start,
                name_end,
                end,
                rule,
            });
        };

        if foreach_name(w)
            && is_p(next, b'(')
            && let Some(close) = matching(&toks, k + 1)
            && is_p(toks.get(close + 1), b'{')
        {
            let s = toks[k].s;
            out[s..s + 5].copy_from_slice(b"while");
            blank(&mut out, s + 5, toks[k].e);
            push(Rule::Foreach, s, toks[close].e);
            continue;
        }

        if all_caps(w)
            && is_p(next, b'(')
            && is_p(next2, b'(')
            && prev.is_some_and(|p| (p.t == T::Ident && !kw(p, KEYWORDS)) || p.t == T::Punct(b')'))
            && let Some(outer) = matching(&toks, k + 1)
            && let Some(inner) = matching(&toks, k + 2)
            && inner + 1 == outer
        {
            blank(&mut out, toks[k].s, toks[k + 1].e);
            blank(&mut out, toks[outer].s, toks[outer].e);
            gone[k] = true;
            gone[k + 1] = true;
            gone[outer] = true;
            push(Rule::Proto, toks[k].s, toks[outer].e);
            continue;
        }

        if all_caps(w)
            && prev.is_some_and(|p| p.t == T::Ident && matches!(word(p), b"class" | b"struct"))
            && next.is_some_and(|n| n.t == T::Ident)
            && next2.is_some_and(|n2| {
                (n2.t == T::Punct(b':') && !is_p(toks.get(k + 3), b':'))
                    || n2.t == T::Punct(b'{')
                    || (n2.t == T::Ident && word(n2) == b"final")
            })
        {
            blank(&mut out, toks[k].s, toks[k].e);
            gone[k] = true;
            push(Rule::ClassAttr, toks[k].s, toks[k].e);
            continue;
        }

        if decls_name(w) && !is_p(next, b'(') {
            blank(&mut out, toks[k].s, toks[k].e);
            gone[k] = true;
            push(Rule::Decls, toks[k].s, toks[k].e);
            continue;
        }

        if all_caps(w)
            && prev.is_some_and(|p| p.t == T::Punct(b')'))
            && is_p(next, b'(')
            && let Some(close) = matching(&toks, k + 1)
            && !toks[k + 2..close].iter().any(|t| t.t == T::Punct(b'('))
            && toks
                .get(close + 1)
                .is_some_and(|t| matches!(t.t, T::Punct(b';' | b'{' | b',')))
            && let Some(open) = live_prev.and_then(|p| matching_back(&toks, p))
            && open > 0
            && ((toks[open - 1].t == T::Ident && !kw(&toks[open - 1], KEYWORDS))
                || toks[open - 1].t == T::Punct(b')'))
        {
            blank(&mut out, toks[k].s, toks[close].e);
            gone[k..=close].fill(true);
            push(Rule::TrailingAttr, toks[k].s, toks[close].e);
            continue;
        }

        if all_caps(w)
            && at_start
            && first_on_line(k)
            && is_p(next, b'(')
            && let Some(close) = matching(&toks, k + 1)
            && !toks[k + 2..close].iter().any(|t| t.t == T::Punct(b'('))
            && last_on_line(close)
            && toks.get(close + 1).is_none_or(|a| match a.t {
                T::Punct(c) => c == b'}' || !b",;=([)].:?+-*/%&|^<>!{".contains(&c),
                T::Ident => !is_p(toks.get(close + 2), b'(') || all_caps(word(a)),
                T::Num => false,
            })
        {
            blank(&mut out, toks[k].s, toks[close].e);
            gone[k..=close].fill(true);
            push(Rule::LoneInvocation, toks[k].s, toks[close].e);
            continue;
        }

        let alone = per_line[toks[k].line as usize] == 1;
        let next_ok = next.is_none_or(|n| match n.t {
            T::Punct(c) => !cont.contains(&c),
            T::Ident => !is_p(next2, b'(') || all_caps(word(n)),
            T::Num => false,
        });
        if all_caps(w) && alone && at_start && next_ok {
            blank(&mut out, toks[k].s, toks[k].e);
            gone[k] = true;
            push(Rule::LoneMacro, toks[k].s, toks[k].e);
            continue;
        }

        if all_caps(w) && at_start && next.is_some_and(|n| kw(n, DECL_KEYWORDS)) {
            blank(&mut out, toks[k].s, toks[k].e);
            gone[k] = true;
            push(Rule::Linkage, toks[k].s, toks[k].e);
            continue;
        }

        let ident = |t: Option<&Tok>| t.is_some_and(|t| t.t == T::Ident && !kw(t, KEYWORDS));
        let at = |i: usize| toks.get(i);
        let prefix = at_start
            && ident(next)
            && (ident(next2) || ((is_p(next2, b'*') || is_p(next2, b'&')) && ident(at(k + 3))));
        let typeish = |t: Option<&Tok>| {
            t.is_some_and(|t| {
                kw(t, TYPE_KEYWORDS)
                    || (t.t == T::Ident && !kw(t, KEYWORDS))
                    || t.t == T::Punct(b'*')
            })
        };
        let between = typeish(prev)
            && !at_start
            && ((ident(next) && is_p(next2, b'('))
                || (is_p(next, b'*') && ident(next2) && is_p(at(k + 3), b'('))
                || (!is_p(prev, b'*')
                    && is_p(next, b'(')
                    && is_p(next2, b'*')
                    && ident(at(k + 3))
                    && is_p(at(k + 4), b')')
                    && is_p(at(k + 5), b'(')));
        let fnptr = is_p(prev, b'(')
            && live_prev.is_some_and(|p| p > 0 && typeish(at(p - 1)) && !is_p(at(p - 1), b'*'))
            && is_p(next, b'*')
            && ident(next2)
            && is_p(at(k + 3), b')')
            && is_p(at(k + 4), b'(');
        if all_caps(w) && (prefix || between || fnptr) {
            blank(&mut out, toks[k].s, toks[k].e);
            gone[k] = true;
            push(Rule::DeclAttr, toks[k].s, toks[k].e);
            continue;
        }

        let shift =
            |a: usize, c: u8| is_p(at(a), c) && is_p(at(a + 1), c) && toks[a].e == toks[a + 1].s;
        let member = |a: usize| {
            is_p(at(a), b'.')
                || (is_p(at(a), b'-') && is_p(at(a + 1), b'>') && toks[a].e == toks[a + 1].s)
        };
        let after_shift =
            live_prev.is_some_and(|p| p >= 1 && (shift(p - 1, b'<') || shift(p - 1, b'>')));
        if all_caps(w)
            && ident(next)
            && ((at_start && (shift(k + 2, b'<') || shift(k + 2, b'>') || member(k + 2)))
                || after_shift)
        {
            blank(&mut out, toks[k].s, toks[k].e);
            gone[k] = true;
            push(Rule::Qualifier, toks[k].s, toks[k].e);
        }
    }
    (out, pseudo)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lex::lex;

    fn rw(src: &str) -> (String, Vec<String>) {
        let lx = lex(src.as_bytes(), true);
        let (out, p) = rewrite(src.as_bytes(), &lx);
        assert_eq!(out.len(), src.len());
        (
            String::from_utf8(out).unwrap(),
            p.into_iter()
                .map(|p| format!("{:?}:{}", p.rule, &src[p.start..p.name_end]))
                .collect(),
        )
    }

    #[test]
    fn shapes() {
        let (o, p) = rw(
            "NS_BEGIN\nclass LIB_EXPORT Foo : public Bar {\n};\nint f __P((int a, char *b));\nvoid g() {\n  TAILQ_FOREACH(x, &h, e) {\n    use(x);\n  }\n}\nNS_END\n",
        );
        assert_eq!(
            o,
            "        \nclass            Foo : public Bar {\n};\nint f     (int a, char *b) ;\nvoid g() {\n  while        (x, &h, e) {\n    use(x);\n  }\n}\n      \n"
        );
        assert_eq!(
            p,
            vec![
                "LoneMacro:NS_BEGIN",
                "ClassAttr:LIB_EXPORT",
                "Proto:__P",
                "Foreach:TAILQ_FOREACH",
                "LoneMacro:NS_END"
            ]
        );
    }

    #[test]
    fn leaves_ordinary_code_alone() {
        let src = "enum e {\n  A,\n  B\n};\nint x = {\n  MAX\n};\nstruct FOO bar;\n}\n\nDWORD\nGetFoo(void)\n{\n  return MAX((a), (b));\n}\n/* LONE */\n#define X \\\n  Y\n";
        let (o, p) = rw(src);
        assert_eq!(o, src);
        assert!(p.is_empty());
    }

    #[test]
    fn statement_and_attribute_shapes() {
        let (o, p) = rw(
            "NS_USE\nNS_BEGIN\nclass X;\nGLOBAL int g;\nstatic void f(int) NTP_PRINTF(1, 2);\nclass A {\n  DECL_SER(A)\n};\nvoid h() {\n  if (a) FOO(x);\n}\n",
        );
        assert_eq!(
            p,
            vec![
                "LoneMacro:NS_USE",
                "LoneMacro:NS_BEGIN",
                "Linkage:GLOBAL",
                "TrailingAttr:NTP_PRINTF",
                "LoneInvocation:DECL_SER"
            ]
        );
        assert!(o.contains("if (a) FOO(x);"));
        let (o2, p2) = rw("void f() {\n  SAVE_ERRNO(\n    close(0);\n  )\n}\n");
        assert!(p2.is_empty() && o2.contains("close(0)"));
        assert!(o.contains(&format!("static void f(int){};", " ".repeat(17))));
    }

    #[test]
    fn declaration_attribute_and_qualifier_shapes() {
        let (_, p) = rw(
            "LIB_DATA isc_t *isc_x = NULL;\nextern void WINAPI f(int);\ntypedef ULONG (WINAPI *PFN)(ULONG a);\nvoid g(UINT32 *p);\nvoid h() {\n  STD_Q cout << \"x\" << STD_Q endl;\n  BOOL ok = f(1);\n  x = a * MAX(*p, 1);\n}\nstatic void WINAPI (*g)(int);\n",
        );
        assert_eq!(
            p,
            vec![
                "DeclAttr:LIB_DATA",
                "DeclAttr:WINAPI",
                "DeclAttr:WINAPI",
                "Qualifier:STD_Q",
                "Qualifier:STD_Q",
                "DeclAttr:WINAPI"
            ]
        );
    }

    #[test]
    fn decls_brackets() {
        let (o, _) = rw("__BEGIN_DECLS int f(void); ISC_LANG_ENDDECLS\n");
        assert_eq!(o, "              int f(void);                  \n");
    }
}
