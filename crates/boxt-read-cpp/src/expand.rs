use std::collections::BTreeMap;
use std::sync::OnceLock;

use boxt_core::Text;
use boxt_core::location::{Location, Scope};
use boxt_core::message::{MacroDef, MacroParams, MacroTable, Variadic};
use boxt_core::read::Exceeded;

use crate::idioms::KEYWORDS;
use crate::lex::{COMMENT, Kind, LITERAL, Lex, is_ws, raw_string_end, splice_end};

pub const DEPTH: usize = 16;
pub const MAX_TOKENS: usize = 4096;
pub const ATTEMPTS: usize = 4;
pub const SCAN: usize = 4000;

const VA_ARGS: &str = "__VA_ARGS__";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum K {
    Ident,
    Lit,
    Num,
    Punct,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct PTok {
    pub text: String,
    pub kind: K,
    pub sp: bool,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Def {
    pub params: Option<Vec<String>>,
    pub variadic: Option<String>,
    pub body: Vec<PTok>,
}

pub struct Defs<'t> {
    table: &'t MacroTable,
    cache: BTreeMap<String, Option<Vec<Def>>>,
}

fn std_defs() -> &'static [(String, Def)] {
    static STD: OnceLock<Vec<(String, Def)>> = OnceLock::new();
    STD.get_or_init(|| {
        let mut v = Vec::new();
        let sizes = [
            "8", "16", "32", "64", "LEAST8", "LEAST16", "LEAST32", "LEAST64", "FAST8", "FAST16",
            "FAST32", "FAST64", "MAX", "PTR",
        ];
        for pre in ["PRI", "SCN"] {
            for c in "diouxX".chars() {
                for sz in sizes {
                    v.push(format!("#define {pre}{c}{sz} \"{c}\""));
                }
            }
        }
        v.push("#define noreturn _Noreturn".into());
        v.push("#define va_arg(ap, type) (*(type *)0)".into());
        v.push("#define offsetof(type, member) ((size_t)&((type *)0)->member)".into());
        v.into_iter()
            .filter_map(|s| {
                let raw = parse_define(s.as_bytes())?;
                Some((raw.name, raw.def))
            })
            .collect()
    })
}

impl<'t> Defs<'t> {
    pub fn new(table: &'t MacroTable) -> Self {
        Defs {
            table,
            cache: BTreeMap::new(),
        }
    }

    fn load(&self, name: &str) -> Option<Vec<Def>> {
        if is_keyword(name) {
            return None;
        }
        let defs = self.table.as_slice();
        let from = defs.partition_point(|d| d.name.as_bytes() < name.as_bytes());
        let mut list: Vec<Def> = Vec::new();
        for d in defs[from..]
            .iter()
            .take_while(|d| d.name.as_bytes() == name.as_bytes())
        {
            if let Some(def) = from_table(d)
                && !list.contains(&def)
            {
                list.push(def);
            }
        }
        for (n, def) in std_defs() {
            if n == name && !list.contains(def) {
                list.push(def.clone());
            }
        }
        (!list.is_empty()).then_some(list)
    }

    pub fn get(&mut self, name: &str) -> Option<&[Def]> {
        if !self.cache.contains_key(name) {
            let loaded = self.load(name);
            self.cache.insert(name.to_string(), loaded);
        }
        self.cache.get(name).and_then(|d| d.as_deref())
    }

    fn pick(&mut self, name: &str, k: usize) -> Option<Def> {
        let list = self.get(name)?;
        list.get(k.min(list.len() - 1)).cloned()
    }
}

fn from_table(d: &MacroDef) -> Option<Def> {
    let text = |t: &Text| t.as_str().map(str::to_string);
    let (params, variadic) = match &d.params {
        None => (None, None),
        Some(p) => {
            let names = p.names.iter().map(text).collect::<Option<Vec<_>>>()?;
            let variadic = match &p.variadic {
                None => None,
                Some(Variadic::Ellipsis) => Some(VA_ARGS.to_string()),
                Some(Variadic::Named(n)) => Some(text(n)?),
            };
            (Some(names), variadic)
        }
    };
    let body = strip_comments(d.body.as_bytes());
    Some(Def {
        params,
        variadic,
        body: tokenize(&body),
    })
}

pub fn strip_comments(s: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len());
    let mut i = 0;
    while i < s.len() {
        let c = s[i];
        if c == b'/' && s.get(i + 1) == Some(&b'*') {
            i = s[i + 2..]
                .windows(2)
                .position(|w| w == b"*/")
                .map_or(s.len(), |k| i + 2 + k + 2);
            out.push(b' ');
            continue;
        }
        if c == b'/' && s.get(i + 1) == Some(&b'/') {
            i = s[i..]
                .iter()
                .position(|&b| b == b'\n')
                .map_or(s.len(), |k| i + k);
            out.push(b' ');
            continue;
        }
        if c == b'"'
            && let Some(end) = raw_string_end(s, i)
        {
            out.extend_from_slice(&s[i..end]);
            i = end;
            continue;
        }
        if c == b'"' || c == b'\'' {
            let st = i;
            i += 1;
            while i < s.len() && s[i] != c && s[i] != b'\n' {
                if s[i] == b'\\' {
                    i += 1;
                }
                i += 1;
            }
            i = (i + 1).min(s.len());
            out.extend_from_slice(&s[st..i]);
            continue;
        }
        out.push(c);
        i += 1;
    }
    out
}

fn is_id_start(b: u8) -> bool {
    b == b'_' || b == b'$' || b.is_ascii_alphabetic()
}

fn is_id(b: u8) -> bool {
    b == b'_' || b == b'$' || b.is_ascii_alphanumeric()
}

pub fn is_keyword(w: &str) -> bool {
    KEYWORDS.contains(&w) || w == VA_ARGS || w == "__VA_OPT__"
}

fn is_prefix(w: &[u8]) -> bool {
    matches!(
        w,
        b"L" | b"u" | b"U" | b"u8" | b"R" | b"LR" | b"uR" | b"UR" | b"u8R"
    )
}

fn pp_number(s: &[u8], mut i: usize, quote_ok: &dyn Fn(usize) -> bool) -> usize {
    i += 1;
    while i < s.len() {
        let c = s[i];
        let exponent = matches!(c, b'+' | b'-') && matches!(s[i - 1], b'e' | b'E' | b'p' | b'P');
        let separator =
            c == b'\'' && s.get(i + 1).is_some_and(|d| d.is_ascii_alphanumeric()) && quote_ok(i);
        if is_id(c) || c == b'.' || exponent || separator {
            i += 1;
        } else {
            break;
        }
    }
    i
}

fn skip_lit(s: &[u8], i: usize) -> usize {
    let q = s[i];
    let mut j = i + 1;
    while j < s.len() && s[j] != q {
        if s[j] == b'\\' {
            j += 1;
        }
        j += 1;
    }
    (j + 1).min(s.len())
}

pub fn tokenize(s: &[u8]) -> Vec<PTok> {
    let n = s.len();
    let mut out = Vec::new();
    let mut i = 0;
    let mut sp = false;
    while i < n {
        let c = s[i];
        if is_ws(c) {
            sp = true;
            i += 1;
            continue;
        }
        let st = i;
        let kind = if is_id_start(c) {
            while i < n && is_id(s[i]) {
                i += 1;
            }
            if i < n && matches!(s[i], b'"' | b'\'') && is_prefix(&s[st..i]) {
                i = match raw_string_end(s, i) {
                    Some(end) if s[i] == b'"' => end,
                    _ => skip_lit(s, i),
                };
                K::Lit
            } else {
                K::Ident
            }
        } else if c.is_ascii_digit()
            || (c == b'.' && s.get(i + 1).is_some_and(|d| d.is_ascii_digit()))
        {
            i = pp_number(s, i, &|_| true);
            K::Num
        } else if c == b'"' || c == b'\'' {
            i = skip_lit(s, i);
            K::Lit
        } else {
            i += 1;
            K::Punct
        };
        out.push(PTok {
            text: String::from_utf8_lossy(&s[st..i]).into_owned(),
            kind,
            sp,
        });
        sp = false;
    }
    out
}

#[derive(Clone, Copy, Debug)]
pub struct STok {
    pub kind: K,
    pub s: usize,
    pub e: usize,
    pub sp: bool,
    pub line: u32,
}

fn in_directive(n: usize, lx: &Lex) -> Vec<bool> {
    let mut v = vec![false; n];
    for d in &lx.dirs {
        v[d.start..(d.end + 1).min(n)].fill(true);
    }
    v
}

pub fn src_tokens(src: &[u8], lx: &Lex) -> Vec<STok> {
    let n = src.len();
    let in_dir = in_directive(n, lx);
    let cls = &lx.cls;
    let mut out = Vec::new();
    let mut i = 0;
    let mut sp = true;
    let lit_end = |i: usize| {
        let mut j = i + 1;
        while j < n && cls[j] == LITERAL {
            j += 1;
        }
        if j < n && src[j] == src[i] { j + 1 } else { j }
    };
    while i < n {
        if let Some(j) = splice_end(src, i)
            && !in_dir[i]
        {
            i = j;
            continue;
        }
        if in_dir[i] || cls[i] == COMMENT || is_ws(src[i]) {
            sp = true;
            i += 1;
            continue;
        }
        let st = i;
        let c = src[i];
        let kind = if c == b'"' || c == b'\'' {
            i = lit_end(i);
            K::Lit
        } else if is_id_start(c) {
            while i < n && is_id(src[i]) {
                i += 1;
            }
            if i < n && matches!(src[i], b'"' | b'\'') && is_prefix(&src[st..i]) {
                i = lit_end(i);
                K::Lit
            } else {
                K::Ident
            }
        } else if c.is_ascii_digit()
            || (c == b'.' && src.get(i + 1).is_some_and(|d| d.is_ascii_digit()))
        {
            i = pp_number(src, i, &|q| cls.get(q + 1) != Some(&LITERAL));
            K::Num
        } else {
            i += 1;
            K::Punct
        };
        out.push(STok {
            kind,
            s: st,
            e: i,
            sp,
            line: lx.line_of(st),
        });
        sp = false;
    }
    out
}

fn strip_splices(b: &[u8]) -> String {
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if let Some(j) = splice_end(b, i) {
            i = j;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn ptok_of(src: &[u8], t: &STok) -> PTok {
    PTok {
        text: strip_splices(&src[t.s..t.e]),
        kind: t.kind,
        sp: t.sp,
    }
}

pub struct RawDef {
    pub name: String,
    pub def: Def,
    pub params: Option<MacroParams>,
    pub body: Vec<u8>,
}

fn parse_define(t: &[u8]) -> Option<RawDef> {
    let n = t.len();
    let mut i = 0;
    let ws = |i: &mut usize| {
        while *i < n && is_ws(t[*i]) {
            *i += 1;
        }
    };
    let ident = |i: &mut usize| {
        let s = *i;
        if *i < n && is_id_start(t[*i]) {
            while *i < n && is_id(t[*i]) {
                *i += 1;
            }
        }
        String::from_utf8_lossy(&t[s..*i]).into_owned()
    };
    ws(&mut i);
    if t.get(i) != Some(&b'#') {
        return None;
    }
    i += 1;
    ws(&mut i);
    if ident(&mut i) != "define" {
        return None;
    }
    ws(&mut i);
    let name = ident(&mut i);
    if name.is_empty() {
        return None;
    }
    let mut params = None;
    let mut variadic: Option<String> = None;
    let mut written = None;
    if t.get(i) == Some(&b'(') {
        i += 1;
        let mut ps = Vec::new();
        let mut var = None;
        loop {
            ws(&mut i);
            if t.get(i) == Some(&b')') {
                i += 1;
                break;
            }
            if t[i.min(n)..].starts_with(b"...") {
                variadic = Some(VA_ARGS.to_string());
                var = Some(Variadic::Ellipsis);
                i += 3;
            } else {
                let p = ident(&mut i);
                if p.is_empty() {
                    return None;
                }
                ws(&mut i);
                if t[i.min(n)..].starts_with(b"...") {
                    var = Some(Variadic::Named(Text::from(p.as_str())));
                    variadic = Some(p);
                    i += 3;
                } else {
                    ps.push(p);
                }
            }
            ws(&mut i);
            match t.get(i) {
                Some(b',') => i += 1,
                Some(b')') => {
                    i += 1;
                    break;
                }
                _ => return None,
            }
        }
        written = Some(MacroParams {
            names: ps.iter().map(|p| Text::from(p.as_str())).collect(),
            variadic: var,
        });
        params = Some(ps);
    }
    let rest = &t[i.min(n)..];
    let a = rest.iter().position(|&b| !is_ws(b)).unwrap_or(rest.len());
    let z = rest.iter().rposition(|&b| !is_ws(b)).map_or(a, |p| p + 1);
    let body = rest[a..z.max(a)].to_vec();
    let tokens = tokenize(&strip_comments(&body));
    Some(RawDef {
        name,
        def: Def {
            params,
            variadic,
            body: tokens,
        },
        params: written,
        body,
    })
}

pub fn definitions(src: &[u8], lx: &Lex, base: &Location) -> Vec<MacroDef> {
    let mut out = Vec::new();
    for d in lx.dirs.iter().filter(|d| d.kind == Kind::Define) {
        let end = d.end.min(src.len());
        let mut t = Vec::with_capacity(end - d.hash);
        let mut i = d.hash;
        while i < end {
            if lx.cls[i] != COMMENT
                && let Some(j) = splice_end(src, i)
            {
                i = j;
                continue;
            }
            t.push(src[i]);
            i += 1;
        }
        if let Some(raw) = parse_define(&t) {
            out.push(MacroDef {
                name: Text::from(raw.name.as_str()),
                location: base.at(Some(d.line), Scope::File),
                params: raw.params,
                body: Text::from_bytes(raw.body),
            });
        }
    }
    out
}

enum Fail {
    Args,
    Depth,
    Size,
    Work(Exceeded),
}

fn is_p(t: Option<&PTok>, s: &str) -> bool {
    t.is_some_and(|t| t.kind == K::Punct && t.text == s)
}

fn match_close(toks: &[PTok], open: usize) -> Option<usize> {
    let mut depth = 0i64;
    for (k, t) in toks.iter().enumerate().skip(open) {
        if t.kind == K::Punct {
            match t.text.as_str() {
                "(" => depth += 1,
                ")" => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(k);
                    }
                }
                _ => {}
            }
        }
    }
    None
}

fn split_args(toks: &[PTok]) -> Vec<Vec<PTok>> {
    let mut out = vec![Vec::new()];
    let mut depth = 0i64;
    for t in toks {
        if t.kind == K::Punct {
            match t.text.as_str() {
                "(" => depth += 1,
                ")" => depth -= 1,
                "," if depth == 0 => {
                    out.push(Vec::new());
                    continue;
                }
                _ => {}
            }
        }
        if let Some(last) = out.last_mut() {
            last.push(t.clone());
        }
    }
    out
}

fn kind_of(s: &str) -> K {
    let b = s.as_bytes();
    if b.first().is_some_and(|&c| is_id_start(c)) && b.iter().all(|&c| is_id(c)) {
        K::Ident
    } else if b.first().is_some_and(|c| c.is_ascii_digit()) {
        K::Num
    } else if b.iter().any(|&c| c == b'"' || c == b'\'') {
        K::Lit
    } else {
        K::Punct
    }
}

fn stringify(a: &[PTok]) -> PTok {
    let mut s = String::from("\"");
    for (i, t) in a.iter().enumerate() {
        if i > 0 && t.sp {
            s.push(' ');
        }
        if t.kind == K::Lit {
            for ch in t.text.chars() {
                if ch == '"' || ch == '\\' {
                    s.push('\\');
                }
                s.push(ch);
            }
        } else {
            s.push_str(&t.text);
        }
    }
    s.push('"');
    PTok {
        text: s,
        kind: K::Lit,
        sp: true,
    }
}

fn is_hash(t: &PTok) -> bool {
    t.kind == K::Punct && t.text == "#"
}

fn substitute(d: &Def, params: &[String], args: &[Vec<PTok>]) -> Result<Vec<PTok>, Fail> {
    let args: Vec<Vec<PTok>> = if params.is_empty() && args.len() == 1 && args[0].is_empty() {
        Vec::new()
    } else {
        args.to_vec()
    };
    if d.variadic.is_some() {
        if args.len() < params.len() {
            return Err(Fail::Args);
        }
    } else if args.len() != params.len() {
        return Err(Fail::Args);
    }
    let mut va = Vec::new();
    for (i, a) in args.iter().enumerate().skip(params.len()) {
        if i > params.len() {
            va.push(PTok {
                text: ",".into(),
                kind: K::Punct,
                sp: false,
            });
        }
        va.extend(a.iter().cloned());
    }
    let lookup = |name: &str| -> Option<Vec<PTok>> {
        if let Some(p) = params.iter().position(|p| p == name) {
            return Some(args[p].clone());
        }
        (d.variadic.as_deref() == Some(name)).then(|| va.clone())
    };
    subst_body(&d.body, d, &lookup, !va.is_empty())
}

fn subst_body(
    body: &[PTok],
    d: &Def,
    lookup: &dyn Fn(&str) -> Option<Vec<PTok>>,
    va_nonempty: bool,
) -> Result<Vec<PTok>, Fail> {
    let mut out: Vec<PTok> = Vec::new();
    let mut paste = false;
    let mut force = false;
    let mut j = 0;
    while j < body.len() {
        let b = &body[j];
        if is_hash(b) && body.get(j + 1).is_some_and(|x| is_hash(x) && !x.sp) {
            paste = true;
            j += 2;
            continue;
        }
        let (mut seq, from_param, step) = if is_hash(b)
            && let Some(nx) = body.get(j + 1)
            && nx.kind == K::Ident
            && let Some(a) = lookup(&nx.text)
        {
            (vec![stringify(&a)], true, 2)
        } else if b.kind == K::Ident
            && b.text == "__VA_OPT__"
            && d.variadic.is_some()
            && is_p(body.get(j + 1), "(")
        {
            let close = match_close(body, j + 1).ok_or(Fail::Args)?;
            let inner = if va_nonempty {
                subst_body(&body[j + 2..close], d, lookup, va_nonempty)?
            } else {
                Vec::new()
            };
            (inner, true, close + 1 - j)
        } else if b.kind == K::Ident
            && let Some(a) = lookup(&b.text)
        {
            (a, true, 1)
        } else {
            (vec![b.clone()], false, 1)
        };
        let is_va = b.kind == K::Ident && d.variadic.as_deref() == Some(b.text.as_str());
        if (from_param || force)
            && let Some(f) = seq.first_mut()
        {
            f.sp = true;
        }
        force = from_param;
        if paste {
            paste = false;
            if seq.is_empty() {
                if is_va && is_p(out.last(), ",") {
                    out.pop();
                }
            } else if is_va && is_p(out.last(), ",") {
                out.extend(seq);
            } else if let Some(last) = out.last_mut() {
                let mut it = seq.into_iter();
                if let Some(first) = it.next() {
                    last.text.push_str(&first.text);
                    last.kind = kind_of(&last.text);
                }
                out.extend(it);
            } else {
                out.extend(seq);
            }
        } else {
            out.extend(seq);
        }
        j += step;
    }
    Ok(out)
}

pub type Charge<'c> = dyn FnMut(u64) -> Result<(), Exceeded> + 'c;

struct Ctx<'d, 't, 'c> {
    defs: &'d mut Defs<'t>,
    k: usize,
    charge: &'d mut Charge<'c>,
}

impl Ctx<'_, '_, '_> {
    fn expand(
        &mut self,
        toks: &[PTok],
        hide: &mut Vec<String>,
        depth: usize,
    ) -> Result<Vec<PTok>, Fail> {
        if depth > DEPTH {
            return Err(Fail::Depth);
        }
        let mut out = Vec::new();
        let mut force = false;
        let mut i = 0;
        while i < toks.len() {
            (self.charge)(1).map_err(Fail::Work)?;
            let t = &toks[i];
            let def = if t.kind == K::Ident && !hide.contains(&t.text) {
                self.defs.pick(&t.text, self.k)
            } else {
                None
            };
            let sub = match def {
                Some(d) => match &d.params {
                    None => Some((d.body.clone(), i + 1)),
                    Some(params) if is_p(toks.get(i + 1), "(") => {
                        let close = match_close(toks, i + 1).ok_or(Fail::Args)?;
                        let args = split_args(&toks[i + 2..close]);
                        Some((substitute(&d, params, &args)?, close + 1))
                    }
                    Some(_) => None,
                },
                None => None,
            };
            match sub {
                Some((sub, next)) => {
                    hide.push(t.text.clone());
                    let mut r = self.expand(&sub, hide, depth + 1)?;
                    hide.pop();
                    if let Some(f) = r.first_mut() {
                        f.sp = true;
                    }
                    out.extend(r);
                    force = true;
                    i = next;
                }
                None => {
                    let mut c = t.clone();
                    if force {
                        c.sp = true;
                        force = false;
                    }
                    out.push(c);
                    i += 1;
                }
            }
            if out.len() > MAX_TOKENS {
                return Err(Fail::Size);
            }
        }
        Ok(out)
    }
}

pub fn render(toks: &[PTok]) -> String {
    let mut s = String::new();
    for (i, t) in toks.iter().enumerate() {
        if i > 0 && t.sp {
            s.push(' ');
        }
        s.push_str(&t.text);
    }
    s
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Inv {
    pub start: usize,
    pub name_end: usize,
    pub end: usize,
    pub function_like: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct Seg {
    pub e: usize,
    pub o: usize,
    pub len: usize,
}

pub struct Built {
    pub text: Vec<u8>,
    pub copies: Vec<Seg>,
    pub invs: Vec<Inv>,
}

impl Built {
    pub fn to_orig(&self, e: usize) -> Option<usize> {
        let idx = self.copies.partition_point(|s| s.e <= e);
        let s = self.copies.get(idx.checked_sub(1)?)?;
        (e < s.e + s.len).then(|| s.o + e - s.e)
    }

    pub fn span_to_orig(&self, s: usize, e: usize) -> Option<(usize, usize)> {
        let a = self.to_orig(s)?;
        let b = self.to_orig(e.checked_sub(1)?)?;
        (b >= a).then_some((a, b + 1))
    }
}

fn match_close_src(src: &[u8], toks: &[STok], open: usize) -> Option<usize> {
    let mut depth = 0i64;
    for (k, t) in toks.iter().enumerate().skip(open).take(SCAN) {
        if t.kind == K::Punct {
            match src[t.s] {
                b'(' => depth += 1,
                b')' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(k);
                    }
                }
                _ => {}
            }
        }
    }
    None
}

fn name_of<'s>(src: &'s [u8], t: &STok) -> &'s str {
    std::str::from_utf8(&src[t.s..t.e]).unwrap_or("")
}

pub fn window(unread: &[bool], toks: &[STok]) -> Vec<bool> {
    let mut needed = unread.to_vec();
    let mut lines: Vec<u32> = toks.iter().map(|t| t.line).collect();
    lines.dedup();
    for a in 1..unread.len() {
        if unread[a] && !unread[a - 1] {
            let idx = lines.partition_point(|&l| (l as usize) < a);
            if idx > 0 {
                needed[lines[idx - 1] as usize] = true;
            }
        }
    }
    needed
}

pub fn max_defs(src: &[u8], toks: &[STok], needed: &[bool], defs: &mut Defs) -> usize {
    let mut max = 0;
    for t in toks {
        if t.kind != K::Ident || !needed.get(t.line as usize).copied().unwrap_or(false) {
            continue;
        }
        if let Some(list) = defs.get(name_of(src, t)) {
            max = max.max(list.len());
        }
    }
    max
}

pub fn build(
    src: &[u8],
    lx: &Lex,
    toks: &[STok],
    needed: &[bool],
    defs: &mut Defs,
    k: usize,
    charge: &mut Charge,
) -> Result<Built, Exceeded> {
    let mut text = Vec::with_capacity(src.len() + 256);
    let mut copies = Vec::new();
    let mut invs = Vec::new();
    let dir_starts: Vec<usize> = lx.dirs.iter().map(|d| d.start).collect();
    let mut pos = 0;
    let mut i = 0;
    while i < toks.len() {
        let t = toks[i];
        if t.kind != K::Ident || !needed.get(t.line as usize).copied().unwrap_or(false) {
            i += 1;
            continue;
        }
        let name = name_of(src, &t);
        let Some(d) = defs.pick(name, k) else {
            i += 1;
            continue;
        };
        charge(1)?;
        let last = if d.params.is_some() {
            if !toks
                .get(i + 1)
                .is_some_and(|n| n.kind == K::Punct && src[n.s] == b'(')
            {
                i += 1;
                continue;
            }
            match match_close_src(src, toks, i + 1) {
                Some(c) => c,
                None => {
                    i += 1;
                    continue;
                }
            }
        } else {
            i
        };
        let (s, e) = (t.s, toks[last].e);
        let ds = dir_starts.partition_point(|&x| x <= s);
        if dir_starts.get(ds).is_some_and(|&x| x < e) {
            i = last + 1;
            continue;
        }
        let ptoks: Vec<PTok> = toks[i..=last].iter().map(|t| ptok_of(src, t)).collect();
        let mut ctx = Ctx {
            defs: &mut *defs,
            k,
            charge: &mut *charge,
        };
        let r = match ctx.expand(&ptoks, &mut Vec::new(), 0) {
            Ok(r) => r,
            Err(Fail::Work(e)) => return Err(e),
            Err(_) => {
                i = last + 1;
                continue;
            }
        };
        let body = render(&r);
        if body.contains('\n') {
            i = last + 1;
            continue;
        }
        copies.push(Seg {
            e: text.len(),
            o: pos,
            len: s - pos,
        });
        text.extend_from_slice(&src[pos..s]);
        text.push(b' ');
        text.extend_from_slice(body.as_bytes());
        text.push(b' ');
        let nls = src[s..e].iter().filter(|&&b| b == b'\n').count();
        text.extend(std::iter::repeat_n(b'\n', nls));
        invs.push(Inv {
            start: s,
            name_end: t.e,
            end: e,
            function_like: d.params.is_some(),
        });
        pos = e;
        i = last + 1;
    }
    copies.push(Seg {
        e: text.len(),
        o: pos,
        len: src.len() - pos,
    });
    text.extend_from_slice(&src[pos..]);
    Ok(Built { text, copies, invs })
}

pub fn same_structure(a: &Lex, b: &Lex) -> bool {
    a.dirs.len() == b.dirs.len()
        && a.dirs
            .iter()
            .zip(&b.dirs)
            .all(|(x, y)| x.kind == y.kind && x.line == y.line)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lex::{lex, line_count};
    use boxt_core::location::{InputRef, RelPath};

    fn base() -> Location {
        Location::file(
            InputRef {
                index: 0,
                given: Text::from("/t"),
            },
            Vec::new(),
            RelPath::try_from("a.h").unwrap(),
        )
    }

    fn table(src: &str) -> MacroTable {
        let lx = lex(src.as_bytes(), true);
        MacroTable::from(definitions(src.as_bytes(), &lx, &base()))
    }

    fn exp(src: &str, all: bool) -> (String, Built) {
        let t = table(src);
        let lx = lex(src.as_bytes(), true);
        let mut defs = Defs::new(&t);
        let toks = src_tokens(src.as_bytes(), &lx);
        let needed = vec![all; line_count(src.as_bytes()) as usize + 2];
        let b = build(
            src.as_bytes(),
            &lx,
            &toks,
            &needed,
            &mut defs,
            0,
            &mut |_| Ok(()),
        )
        .unwrap();
        (String::from_utf8(b.text.clone()).unwrap(), b)
    }

    #[test]
    fn object_function_stringize_paste_variadic() {
        let src = "#define NS_BEGIN namespace x {\n#define CAT(a, b) a ## b\n#define STR(x) #x\n#define LOG(fmt, ...) printf(fmt, ## __VA_ARGS__)\nNS_BEGIN\nint CAT(foo, _t) = 1;\nconst char *s = STR(a \"b\");\nvoid f() { LOG(\"x\"); LOG(\"%\" PRId64 \"\\n\", v); }\n}\n";
        let (o, b) = exp(src, true);
        assert_eq!(line_count(o.as_bytes()), line_count(src.as_bytes()));
        assert!(o.contains(" namespace x { \n"));
        assert!(o.contains("int  foo_t  = 1;"));
        assert!(o.contains(r#""a \"b\"""#));
        assert!(o.contains(r#" printf( "x" ) "#), "{o}");
        assert!(o.contains(r#"printf( "%" "d" "\n" , v )"#), "{o}");
        let names: Vec<&str> = b.invs.iter().map(|i| &src[i.start..i.name_end]).collect();
        assert_eq!(names, vec!["NS_BEGIN", "CAT", "STR", "LOG", "LOG"]);
    }

    #[test]
    fn keeps_lines_and_maps_offsets_back() {
        let src = "#define F(a, b) g(a, b)\nint x = F(1,\n  2) + h(3);\n";
        let (o, b) = exp(src, true);
        assert_eq!(o.matches('\n').count(), src.matches('\n').count());
        let h = o.find("h(3)").unwrap();
        assert_eq!(b.to_orig(h), Some(src.find("h(3)").unwrap()));
        assert_eq!(b.to_orig(o.rfind("g(").unwrap()), None);
    }

    #[test]
    fn recursion_is_bounded_and_self_reference_stops() {
        let src = "#define A A + 1\n#define B C\n#define C B\nint x = A; int y = B;\n";
        let (o, _) = exp(src, true);
        assert!(o.contains("A + 1") && o.contains(" B "), "{o}");
    }

    #[test]
    fn window_limits_expansion_and_directives_are_untouched() {
        let src = "#define M 1\nint a = M;\nint b = M;\n";
        let t = table(src);
        let lx = lex(src.as_bytes(), false);
        let mut defs = Defs::new(&t);
        let toks = src_tokens(src.as_bytes(), &lx);
        let mut needed = vec![false; 5];
        needed[3] = true;
        let b = build(
            src.as_bytes(),
            &lx,
            &toks,
            &needed,
            &mut defs,
            0,
            &mut |_| Ok(()),
        )
        .unwrap();
        assert_eq!(
            String::from_utf8(b.text).unwrap(),
            "#define M 1\nint a = M;\nint b =  1 ;\n"
        );
    }

    #[test]
    fn several_definitions_tried_by_index() {
        let src = "#if A\n#define BEGIN namespace a {\n#else\n#define BEGIN\n#endif\nBEGIN\n";
        let t = table(src);
        let mut defs = Defs::new(&t);
        assert_eq!(defs.get("BEGIN").unwrap().len(), 2);
        let lx = lex(src.as_bytes(), true);
        let toks = src_tokens(src.as_bytes(), &lx);
        let needed = vec![true; 8];
        let b1 = build(
            src.as_bytes(),
            &lx,
            &toks,
            &needed,
            &mut defs,
            1,
            &mut |_| Ok(()),
        )
        .unwrap();
        assert!(String::from_utf8(b1.text).unwrap().ends_with("\n  \n"));
    }

    pub fn fan_out() -> String {
        let mut header = String::new();
        for d in 0..12 {
            header += &format!("#define F{d} F{n} F{n} F{n} F{n}\n", n = d + 1);
        }
        header += "#define F12\n";
        header
    }

    #[test]
    fn an_empty_fan_out_stops_at_the_work_limit() {
        let src = format!("{}int x = F0 @;\n", fan_out());
        let t = table(&src);
        let lx = lex(src.as_bytes(), false);
        let mut defs = Defs::new(&t);
        let toks = src_tokens(src.as_bytes(), &lx);
        let needed = vec![true; line_count(src.as_bytes()) as usize + 2];
        let mut spent = 0u64;
        let mut charge = |n: u64| {
            spent += n;
            if spent > 10_000 {
                return Err(Exceeded {
                    limit: boxt_core::limits::LimitName::Work,
                    value: spent,
                    max: 10_000,
                });
            }
            Ok(())
        };
        let err = build(
            src.as_bytes(),
            &lx,
            &toks,
            &needed,
            &mut defs,
            0,
            &mut charge,
        )
        .err()
        .expect("stopped by work");
        assert_eq!(err.value, 10_001);
    }

    #[test]
    fn raw_strings_in_bodies_end_at_their_delimiter() {
        let toks = tokenize(br#"f(R"x(a" // ")x", b)"#);
        let texts: Vec<&str> = toks.iter().map(|t| t.text.as_str()).collect();
        assert_eq!(texts, vec!["f", "(", r#"R"x(a" // ")x""#, ",", "b", ")"]);
        assert_eq!(
            strip_comments(br#"R"(// no)" // yes"#),
            br#"R"(// no)"  "#.to_vec()
        );
    }

    #[test]
    fn definitions_are_reported_as_written() {
        let src = "#define A(x, ...) f(x, /* c */ __VA_ARGS__) \\\n  + 1\n# define B\n#define C(a, rest...) g(rest)\n#define int long\n";
        let defs = table(src);
        let by = |n: &str| defs.iter().find(|d| d.name == Text::from(n)).unwrap();
        let a = by("A");
        assert_eq!(a.body, Text::from("f(x, /* c */ __VA_ARGS__)   + 1"));
        assert_eq!(a.location.line, Some(1));
        assert_eq!(
            a.params,
            Some(MacroParams {
                names: vec![Text::from("x")],
                variadic: Some(Variadic::Ellipsis)
            })
        );
        assert_eq!(by("B").params, None);
        assert_eq!(by("B").body, Text::from(""));
        assert_eq!(
            by("C").params.as_ref().unwrap().variadic,
            Some(Variadic::Named(Text::from("rest")))
        );
        let mut d = Defs::new(&defs);
        assert!(d.get("int").is_none());
        assert_eq!(d.get("PRId64").unwrap().len(), 1);
    }
}
