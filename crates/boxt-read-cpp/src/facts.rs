use boxt_core::Text;
use boxt_core::location::Scope;
use tree_sitter::{Node, Tree};

use crate::expand::{Built, K, strip_comments, tokenize};
use crate::lex::{is_ws, splice_end};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HitKind {
    Call { callee: Text, args: Vec<Text> },
    Str { value: Text },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hit {
    pub start: usize,
    pub end: usize,
    pub line: u32,
    pub kind: HitKind,
    pub scope: Scope,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Func {
    pub start: usize,
    pub end: usize,
    pub line: u32,
    pub scope: Scope,
}

pub enum View<'a> {
    Plain(&'a [u8]),
    Mapped { built: &'a Built, orig: &'a [u8] },
}

impl View<'_> {
    pub fn pos(&self, s: usize, e: usize) -> Option<(usize, usize)> {
        match self {
            View::Plain(_) => Some((s, e)),
            View::Mapped { built, .. } if s == e => built.to_orig(s).map(|a| (a, a)),
            View::Mapped { built, .. } => built.span_to_orig(s, e),
        }
    }

    pub fn text(&self, s: usize, e: usize) -> Option<&[u8]> {
        match self {
            View::Plain(t) => t.get(s..e),
            View::Mapped { orig, .. } => {
                let (a, b) = self.pos(s, e)?;
                orig.get(a..b)
            }
        }
    }

    fn node(&self, n: Node) -> Option<&[u8]> {
        self.text(n.start_byte(), n.end_byte())
    }
}

pub fn lf(text: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len());
    let mut i = 0;
    while i < text.len() {
        if text[i] == b'\r' && text.get(i + 1) == Some(&b'\n') {
            i += 1;
            continue;
        }
        out.push(text[i]);
        i += 1;
    }
    out
}

fn without_splices(text: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len());
    let mut i = 0;
    while i < text.len() {
        if let Some(j) = splice_end(text, i) {
            i = j;
            continue;
        }
        out.push(text[i]);
        i += 1;
    }
    out
}

pub fn collapse(text: &[u8]) -> String {
    let clean = strip_comments(&without_splices(text));
    String::from_utf8_lossy(&clean)
        .split_ascii_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn qualified(text: &[u8]) -> String {
    collapse(text).replace(" ::", "::").replace(":: ", "::")
}

pub fn literal_arg(text: &[u8]) -> bool {
    let toks = tokenize(&strip_comments(&without_splices(text)));
    let number = |t: &crate::expand::PTok| t.kind == K::Num && !t.text.contains('_');
    match toks.as_slice() {
        [] => false,
        [one] if one.kind == K::Num => number(one),
        [one] if one.kind == K::Ident => matches!(one.text.as_str(), "true" | "false" | "nullptr"),
        [sign, num] if sign.kind == K::Punct && matches!(sign.text.as_str(), "-" | "+") => {
            number(num)
        }
        all => all.iter().all(|t| t.kind == K::Lit),
    }
}

pub fn literal_value(text: &[u8]) -> Text {
    if !literal_arg(text) {
        return Text::default();
    }
    let a = text.iter().position(|&b| !is_ws(b)).unwrap_or(0);
    let z = text.iter().rposition(|&b| !is_ws(b)).map_or(a, |z| z + 1);
    Text::from_bytes(lf(&text[a..z]))
}

fn hex(b: u8) -> Option<u32> {
    (b as char).to_digit(16)
}

fn push_char(out: &mut Vec<u8>, value: u32, written: &[u8]) {
    match char::from_u32(value) {
        Some(c) => {
            let mut buf = [0u8; 4];
            out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
        }
        None => out.extend_from_slice(written),
    }
}

pub fn string_value(raw: &[u8]) -> Option<Vec<u8>> {
    let q = raw.iter().position(|&b| b == b'"')?;
    let prefix = &raw[..q];
    if prefix.ends_with(b"R") {
        let open = raw[q + 1..].iter().position(|&b| b == b'(')? + q + 1;
        let delim = &raw[q + 1..open];
        let mut close = vec![b')'];
        close.extend_from_slice(delim);
        close.push(b'"');
        let body = &raw[open + 1..];
        let end = body
            .windows(close.len())
            .rposition(|w| w == close.as_slice())
            .unwrap_or(body.len());
        return Some(lf(&body[..end]));
    }
    let narrow = prefix.is_empty() || prefix == b"u8";
    let last = raw.iter().rposition(|&b| b == b'"')?;
    let body = without_splices(if last > q {
        &raw[q + 1..last]
    } else {
        &raw[q + 1..]
    });
    let mut out = Vec::with_capacity(body.len());
    let mut i = 0;
    while i < body.len() {
        let c = body[i];
        if c != b'\\' || i + 1 >= body.len() {
            out.push(c);
            i += 1;
            continue;
        }
        let st = i;
        let e = body[i + 1];
        i += 2;
        match e {
            b'n' => out.push(b'\n'),
            b't' => out.push(b'\t'),
            b'r' => out.push(b'\r'),
            b'v' => out.push(0x0b),
            b'f' => out.push(0x0c),
            b'a' => out.push(0x07),
            b'b' => out.push(0x08),
            b'e' | b'E' => out.push(0x1b),
            b'0'..=b'7' => {
                let mut v = u32::from(e - b'0');
                let mut n = 1;
                while n < 3 && i < body.len() && (b'0'..=b'7').contains(&body[i]) {
                    v = v * 8 + u32::from(body[i] - b'0');
                    i += 1;
                    n += 1;
                }
                if narrow && v <= 0xFF {
                    out.push(v as u8);
                } else {
                    push_char(&mut out, v, &body[st..i]);
                }
            }
            b'x' => {
                let mut v: u32 = 0;
                let mut any = false;
                while i < body.len()
                    && let Some(d) = hex(body[i])
                {
                    v = v.saturating_mul(16).saturating_add(d);
                    i += 1;
                    any = true;
                }
                if !any {
                    out.extend_from_slice(b"\\x");
                } else if narrow && v <= 0xFF {
                    out.push(v as u8);
                } else {
                    push_char(&mut out, v, &body[st..i]);
                }
            }
            b'u' | b'U' => {
                let want = if e == b'u' { 4 } else { 8 };
                let digits: Vec<u32> = body[i..].iter().take(want).map_while(|&b| hex(b)).collect();
                if digits.len() == want {
                    i += want;
                    let v = digits.iter().fold(0, |v, d| v * 16 + d);
                    push_char(&mut out, v, &body[st..i]);
                } else {
                    out.push(b'\\');
                    out.push(e);
                }
            }
            other => out.push(other),
        }
    }
    Some(out)
}

fn function_parts(node: Node) -> (Option<Node>, Option<Node>) {
    let Some(mut d) = node.child_by_field_name("declarator") else {
        return (None, None);
    };
    loop {
        match d.kind() {
            "function_declarator" => {
                return (
                    d.child_by_field_name("declarator"),
                    d.child_by_field_name("parameters"),
                );
            }
            "pointer_declarator"
            | "reference_declarator"
            | "attributed_declarator"
            | "parenthesized_declarator" => {
                match d
                    .child_by_field_name("declarator")
                    .or_else(|| d.named_child(0))
                {
                    Some(inner) => d = inner,
                    None => return (Some(d), None),
                }
            }
            _ => return (Some(d), None),
        }
    }
}

fn declared_name(param: Node) -> Option<Node> {
    let mut d = param.child_by_field_name("declarator")?;
    loop {
        match d.kind() {
            "identifier" | "field_identifier" | "type_identifier" => return Some(d),
            "variadic_declarator" => {
                let mut cursor = d.walk();
                return d
                    .named_children(&mut cursor)
                    .find(|c| c.kind() == "identifier");
            }
            _ => d = d.child_by_field_name("declarator")?,
        }
    }
}

fn param_text(view: &View, param: Node) -> Option<String> {
    let mut cut: Vec<(usize, usize)> = Vec::new();
    if let Some(name) = declared_name(param) {
        cut.push((name.start_byte(), name.end_byte()));
    }
    if let Some(default) = param.child_by_field_name("default_value") {
        let mut cursor = param.walk();
        let eq = param
            .children(&mut cursor)
            .find(|c| c.kind() == "=")
            .map_or(default.start_byte(), |c| c.start_byte());
        cut.push((eq, default.end_byte()));
    }
    cut.sort_unstable();
    let mut text = Vec::new();
    let mut at = param.start_byte();
    for (s, e) in cut {
        if s > at {
            text.extend_from_slice(view.text(at, s)?);
        }
        at = at.max(e);
    }
    if param.end_byte() > at {
        text.extend_from_slice(view.text(at, param.end_byte())?);
    }
    Some(collapse(&text))
}

fn signature(view: &View, params: Option<Node>) -> Option<String> {
    let Some(params) = params else {
        return Some(String::new());
    };
    let mut parts = Vec::new();
    let mut cursor = params.walk();
    for child in params.children(&mut cursor) {
        match child.kind() {
            "(" | ")" | "," | "comment" => {}
            "..." | "variadic_parameter" => parts.push("...".to_string()),
            "identifier" => parts.push(collapse(view.node(child)?)),
            _ if child.is_named() => parts.push(param_text(view, child)?),
            _ => {}
        }
    }
    Some(format!("({})", parts.join(", ")))
}

fn function_scope(view: &View, node: Node, prefix: &str) -> Option<Scope> {
    let (name, params) = function_parts(node);
    let name = qualified(view.node(name?)?);
    if name.is_empty() {
        return None;
    }
    let qualified_name = if prefix.is_empty() {
        name
    } else {
        format!("{prefix}::{name}")
    };
    Some(Scope::Function {
        qualified_name: Text::from(qualified_name),
        signature: Text::from(signature(view, params)?),
    })
}

fn line_of(n: Node) -> u32 {
    u32::try_from(n.start_position().row)
        .unwrap_or(u32::MAX - 1)
        .saturating_add(1)
}

struct Item<'t> {
    node: Node<'t>,
    scope: usize,
    prefix: usize,
}

const SKIPPED: &[&str] = &[
    "preproc_def",
    "preproc_function_def",
    "preproc_call",
    "preproc_include",
    "preproc_arg",
    "preproc_params",
    "preproc_defined",
    "comment",
];

pub fn collect(
    tree: &Tree,
    view: &View,
    accept: &dyn Fn(u32) -> bool,
    hits: &mut Vec<Hit>,
    funcs: &mut Vec<Func>,
) {
    let mut scopes = vec![Scope::File];
    let mut prefixes = vec![String::new()];
    let mut stack = vec![Item {
        node: tree.root_node(),
        scope: 0,
        prefix: 0,
    }];
    while let Some(Item {
        node,
        mut scope,
        mut prefix,
    }) = stack.pop()
    {
        let kind = node.kind();
        if SKIPPED.contains(&kind) {
            continue;
        }
        let mut skip_field = None;
        match kind {
            "preproc_if" | "preproc_elif" => skip_field = node.child_by_field_name("condition"),
            "linkage_specification" => skip_field = node.child_by_field_name("value"),
            "namespace_definition" | "class_specifier" | "struct_specifier" | "union_specifier" => {
                if node.child_by_field_name("body").is_some()
                    && let Some(name) = node.child_by_field_name("name").and_then(|n| view.node(n))
                {
                    let name = qualified(name);
                    let parent = &prefixes[prefix];
                    prefixes.push(if parent.is_empty() {
                        name
                    } else {
                        format!("{parent}::{name}")
                    });
                    prefix = prefixes.len() - 1;
                }
            }
            "function_definition" => {
                let found = function_scope(view, node, &prefixes[prefix]);
                let line = line_of(node);
                if let Some(found) = found {
                    if accept(line)
                        && let Some((start, end)) = view.pos(node.start_byte(), node.end_byte())
                    {
                        funcs.push(Func {
                            start,
                            end,
                            line,
                            scope: found.clone(),
                        });
                    }
                    scopes.push(found);
                } else {
                    scopes.push(Scope::File);
                }
                scope = scopes.len() - 1;
            }
            "call_expression" => {
                let line = line_of(node);
                if accept(line)
                    && let Some(hit) = call_hit(view, node, line, &scopes[scope])
                {
                    hits.push(hit);
                }
            }
            "concatenated_string" => {
                let line = line_of(node);
                if accept(line)
                    && let Some((start, end)) = view.pos(node.start_byte(), node.end_byte())
                    && let Some(value) = joined(view, node)
                {
                    hits.push(Hit {
                        start,
                        end,
                        line,
                        kind: HitKind::Str {
                            value: Text::from_bytes(value),
                        },
                        scope: scopes[scope].clone(),
                    });
                }
                continue;
            }
            "string_literal" | "raw_string_literal" => {
                let line = line_of(node);
                if accept(line)
                    && let Some((start, end)) = view.pos(node.start_byte(), node.end_byte())
                    && let Some(value) = view.node(node).and_then(string_value)
                {
                    hits.push(Hit {
                        start,
                        end,
                        line,
                        kind: HitKind::Str {
                            value: Text::from_bytes(value),
                        },
                        scope: scopes[scope].clone(),
                    });
                }
                continue;
            }
            _ => {}
        }
        let mut cursor = node.walk();
        let children: Vec<Node> = node.children(&mut cursor).collect();
        for child in children.into_iter().rev() {
            if skip_field.is_some_and(|s| s.id() == child.id()) {
                continue;
            }
            stack.push(Item {
                node: child,
                scope,
                prefix,
            });
        }
    }
}

fn joined(view: &View, node: Node) -> Option<Vec<u8>> {
    let mut value = Vec::new();
    let mut any = false;
    let mut cursor = node.walk();
    for piece in node.children(&mut cursor) {
        if matches!(piece.kind(), "string_literal" | "raw_string_literal") {
            value.extend(view.node(piece).and_then(string_value)?);
            any = true;
        }
    }
    any.then_some(value)
}

fn call_hit(view: &View, node: Node, line: u32, scope: &Scope) -> Option<Hit> {
    let function = node.child_by_field_name("function")?;
    let (start, end) = view.pos(node.start_byte(), node.end_byte())?;
    view.pos(function.start_byte(), function.end_byte())?;
    let callee = Text::from(qualified(view.node(function)?));
    let mut args = Vec::new();
    if let Some(list) = node.child_by_field_name("arguments") {
        let mut cursor = list.walk();
        for arg in list.named_children(&mut cursor) {
            if arg.kind() == "comment" {
                continue;
            }
            args.push(view.node(arg).map(literal_value).unwrap_or_default());
        }
    }
    Some(Hit {
        start,
        end,
        line,
        kind: HitKind::Call { callee, args },
        scope: scope.clone(),
    })
}

pub fn depth(tree: &Tree) -> u64 {
    let mut cursor = tree.walk();
    let mut depth = 1u64;
    let mut deepest = 1u64;
    loop {
        if cursor.goto_first_child() {
            depth += 1;
            deepest = deepest.max(depth);
            continue;
        }
        loop {
            if cursor.goto_next_sibling() {
                break;
            }
            if !cursor.goto_parent() {
                return deepest;
            }
            depth -= 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn string_values_decode_escapes() {
        assert_eq!(string_value(br#""a\tb\n""#).unwrap(), b"a\tb\n");
        assert_eq!(string_value(br#""\101\x42\0""#).unwrap(), b"AB\0");
        assert_eq!(string_value("u8\"é\"".as_bytes()).unwrap(), "é".as_bytes());
        assert_eq!(string_value(br#"L"\x263a""#).unwrap(), "☺".as_bytes());
        assert_eq!(string_value(b"R\"x(a\r\nb)\")x\"").unwrap(), b"a\nb)\"");
        assert_eq!(string_value(b"\"a\\\r\nb\"").unwrap(), b"ab");
        assert_eq!(string_value(br#""q\"\\""#).unwrap(), br#"q"\"#);
        assert_eq!(
            string_value(br#""a\U00110000b""#).unwrap(),
            br#"a\U00110000b"#
        );
        assert_eq!(string_value(br#""\uD800""#).unwrap(), br#"\uD800"#);
        assert_eq!(string_value(br#"L"\x110000""#).unwrap(), br#"\x110000"#);
    }

    #[test]
    fn literal_arguments() {
        assert!(literal_arg(b"0"));
        assert!(literal_arg(b" -1 "));
        assert!(literal_arg(br#""a" "b""#));
        assert!(literal_arg(b"'x'"));
        assert!(!literal_arg(b"x"));
        assert!(!literal_arg(b"a + 1"));
        assert!(!literal_arg(b""));
        assert!(literal_arg(b"true") && literal_arg(b"nullptr"));
        assert!(!literal_arg(b"NULL") && !literal_arg(b"null") && !literal_arg(b"12_km"));
    }

    #[test]
    fn names_collapse_and_join_qualification() {
        assert_eq!(qualified(b"a ::\r\n  B :: f"), "a::B::f");
        assert_eq!(qualified(b"operator /* x */ =="), "operator ==");
    }
}
