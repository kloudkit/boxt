//! C and C++ source reader: tree-sitter, guard structure and encodings.

mod expand;
mod facts;
mod idioms;
mod lex;
mod passes;

use std::collections::BTreeSet;
use std::ffi::c_void;
use std::ops::ControlFlow;

use boxt_core::Text;
use boxt_core::compile_db::extension_class;
use boxt_core::decode::decode;
use boxt_core::fact::{Fact, FileKind, GuardChain};
use boxt_core::location::{Location, Scope};
use boxt_core::message::{MacroDef, MacroTable, Region, RegionReason};
use boxt_core::outcome::NotCheckedReason;
use boxt_core::read::{Exceeded, FactSink, NotRead, ReadCtx, ReadResult, Reader};
use boxt_core::settings::Encoding;
use tree_sitter::{Language, ParseOptions, Parser, Tree};

use crate::expand::{ATTEMPTS, Built, Defs};
use crate::facts::{Func, Hit, HitKind, View};
use crate::idioms::Pseudo;
use crate::lex::{Kind, Lex, is_ws};
use crate::passes::PassText;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Grammar {
    C,
    Cpp,
}

impl Grammar {
    pub fn for_file_name(file_name: &[u8]) -> Option<Grammar> {
        extension_class(file_name)?;
        Some(Grammar::tried_for(Some(file_name))[0])
    }

    pub fn tried_for(file_name: Option<&[u8]>) -> &'static [Grammar] {
        let ext = file_name.and_then(|name| {
            let dot = name.iter().rposition(|&c| c == b'.')?;
            Some(&name[dot + 1..])
        });
        match ext {
            Some(b"c") => &[Grammar::C],
            Some(e) if e.eq_ignore_ascii_case(b"h") => &[Grammar::C, Grammar::Cpp],
            Some(_) if file_name.and_then(extension_class).is_some() => &[Grammar::Cpp],
            _ => &[Grammar::C, Grammar::Cpp],
        }
    }

    fn language(self) -> Language {
        match self {
            Grammar::C => tree_sitter_c::LANGUAGE.into(),
            Grammar::Cpp => tree_sitter_cpp::LANGUAGE.into(),
        }
    }
}

pub type CMalloc = unsafe extern "C" fn(usize) -> *mut c_void;
pub type CCalloc = unsafe extern "C" fn(usize, usize) -> *mut c_void;
pub type CRealloc = unsafe extern "C" fn(*mut c_void, usize) -> *mut c_void;
pub type CFree = unsafe extern "C" fn(*mut c_void);

/// # Safety
///
/// Call before any other tree-sitter call, from one thread, with four functions of one
/// allocator family that never return null for a non-zero size.
pub unsafe fn set_c_allocator(malloc: CMalloc, calloc: CCalloc, realloc: CRealloc, free: CFree) {
    unsafe {
        tree_sitter::set_allocator(Some(tree_sitter::Allocator {
            malloc,
            calloc,
            realloc,
            free,
        }));
    }
}

enum Stop {
    Limit(Exceeded),
    Parser,
}

impl From<Exceeded> for Stop {
    fn from(e: Exceeded) -> Self {
        Stop::Limit(e)
    }
}

impl From<Stop> for NotRead {
    fn from(stop: Stop) -> Self {
        match stop {
            Stop::Limit(e) => NotRead::from(e),
            Stop::Parser => NotRead::new(NotCheckedReason::Unparseable),
        }
    }
}

struct Engine<'c, 'm> {
    ctx: &'c mut ReadCtx<'m>,
    parsers: Vec<(Grammar, Parser)>,
}

impl Engine<'_, '_> {
    fn parse(&mut self, grammar: Grammar, text: &[u8]) -> Result<Tree, Stop> {
        self.ctx.work(1)?;
        let at = match self.parsers.iter().position(|(g, _)| *g == grammar) {
            Some(at) => at,
            None => {
                let mut parser = Parser::new();
                parser
                    .set_language(&grammar.language())
                    .map_err(|_| Stop::Parser)?;
                self.parsers.push((grammar, parser));
                self.parsers.len() - 1
            }
        };
        let ctx = &mut *self.ctx;
        let parser = &mut self.parsers[at].1;
        let mut exceeded = None;
        let mut progress = |_: &tree_sitter::ParseState| match ctx.work(1) {
            Ok(()) => ControlFlow::Continue(()),
            Err(e) => {
                exceeded = Some(e);
                ControlFlow::Break(())
            }
        };
        let mut read = |i: usize, _| &text[i.min(text.len())..];
        let tree = parser.parse_with_options(
            &mut read,
            None,
            Some(ParseOptions::new().progress_callback(&mut progress)),
        );
        if let Some(e) = exceeded {
            parser.reset();
            return Err(Stop::Limit(e));
        }
        let tree = tree.ok_or(Stop::Parser)?;
        self.ctx.syntax_depth(facts::depth(&tree))?;
        Ok(tree)
    }

    fn work(&mut self, steps: u64) -> Result<(), Stop> {
        Ok(self.ctx.work(steps)?)
    }

    fn buffer(&mut self, bytes: usize) -> Result<(), Stop> {
        Ok(self.ctx.buffer(bytes as u64)?)
    }

    fn release(&mut self, bytes: usize) {
        self.ctx.release(bytes as u64);
    }
}

struct Analysis {
    lx: Lex,
    unread: Vec<bool>,
    hits: Vec<Hit>,
    funcs: Vec<Func>,
    pseudo: Vec<Pseudo>,
}

enum Src<'a> {
    Original,
    Expanded { built: &'a Built, orig: &'a [u8] },
}

fn overlaps(a: [u32; 2], b: [u32; 2]) -> bool {
    a[0] <= b[1] && b[0] <= a[1]
}

fn analyse(eng: &mut Engine, g: Grammar, text: &[u8], src: &Src) -> Result<Analysis, Stop> {
    let nl = lex::line_count(text).max(1);
    let lx = lex::lex(text, g == Grammar::Cpp);
    let (rewritten, pseudo) = idioms::rewrite(text, &lx);
    eng.buffer(rewritten.len())?;
    let tree = eng.parse(g, &rewritten)?;
    let (error, missing) = passes::bad_ranges(&tree);
    let silent: Vec<[u32; 2]> = passes::split_chains(&lx, nl)
        .into_iter()
        .filter(|&r| !error.iter().chain(missing.iter()).any(|&b| overlaps(r, b)))
        .collect();
    let t4 = passes::extent_mismatches(&tree, &lx, &error, nl);
    let damaged = !error.is_empty() || !missing.is_empty() || !silent.is_empty() || !t4.is_empty();
    let mut unread = vec![false; nl as usize + 2];
    let mut hits = Vec::new();
    let mut funcs = Vec::new();
    let mapped = |built, orig| View::Mapped { built, orig };
    if damaged {
        let want = lx.max_branches();
        let dirl = passes::directive_lines(&lx, nl);
        let mut kept_any = vec![false; nl as usize + 2];
        let mut ok_any = vec![false; nl as usize + 2];
        let mut covered = BTreeSet::new();
        let nonempty: Vec<(usize, usize)> = lx
            .groups
            .iter()
            .enumerate()
            .flat_map(|(g, gr)| {
                gr.branches
                    .iter()
                    .enumerate()
                    .filter(|(_, b)| b.body.0 < b.body.1)
                    .map(move |(j, _)| (g, j))
            })
            .collect();
        let mut k = 0;
        loop {
            let sel = if k < want {
                passes::uniform(&lx, k)
            } else {
                let uncovered: Vec<(usize, usize)> = nonempty
                    .iter()
                    .copied()
                    .filter(|b| !covered.contains(b))
                    .collect();
                match passes::cover_sel(&lx, &uncovered) {
                    Some(sel) => sel,
                    None => break,
                }
            };
            k += 1;
            covered.extend(passes::kept_branches(&lx, &sel));
            let second = match src {
                Src::Original => text,
                Src::Expanded { .. } => &rewritten,
            };
            let held = 2 * text.len();
            eng.buffer(held)?;
            let (pt, blank_view): (PassText, Vec<u8>) =
                passes::pass_sel([&rewritten, second], &lx, &sel, nl);
            let tree = eng.parse(g, &pt.text)?;
            let (e, m) = passes::bad_ranges(&tree);
            let mut errline = vec![false; nl as usize + 2];
            for r in e.iter().chain(m.iter()) {
                passes::mark(&mut errline, *r, nl);
            }
            for l in 1..=nl as usize {
                if pt.kept[l] {
                    kept_any[l] = true;
                    if !errline[l] {
                        ok_any[l] = true;
                    }
                }
            }
            let accept = |line: u32| {
                let l = line as usize;
                pt.kept.get(l).copied().unwrap_or(false) && !errline.get(l).copied().unwrap_or(true)
            };
            let view = match src {
                Src::Original => View::Plain(&blank_view),
                Src::Expanded { built, orig } => mapped(built, orig),
            };
            facts::collect(&tree, &view, &accept, &mut hits, &mut funcs);
            eng.release(held);
        }
        for l in 1..=nl as usize {
            unread[l] = if kept_any[l] { !ok_any[l] } else { !dirl[l] };
        }
    } else {
        let view = match src {
            Src::Original => View::Plain(text),
            Src::Expanded { built, orig } => mapped(built, orig),
        };
        facts::collect(&tree, &view, &|_| true, &mut hits, &mut funcs);
    }
    eng.release(rewritten.len());
    Ok(Analysis {
        lx,
        unread,
        hits,
        funcs,
        pseudo,
    })
}

struct FileRead {
    lx: Lex,
    unread: Vec<bool>,
    hits: Vec<Hit>,
    funcs: Vec<Func>,
    calls: Vec<(usize, usize, usize)>,
}

fn count(bits: &[bool]) -> usize {
    bits.iter().filter(|&&b| b).count()
}

fn analyse_any(
    eng: &mut Engine,
    g: Grammar,
    src: &[u8],
    table: Option<&MacroTable>,
) -> Result<FileRead, Stop> {
    let a0 = analyse(eng, g, src, &Src::Original)?;
    let mut merged = a0.unread.clone();
    let mut hits = a0.hits;
    let mut funcs = a0.funcs;
    let mut calls: Vec<(usize, usize, usize, bool)> = a0
        .pseudo
        .iter()
        .filter(|p| p.rule.is_invocation())
        .map(|p| (p.start, p.name_end, p.end, true))
        .collect();
    if let Some(table) = table
        && count(&a0.unread) > 0
    {
        let mut defs = Defs::new(table);
        let toks = expand::src_tokens(src, &a0.lx);
        let needed = expand::window(&a0.unread, &toks);
        let maxd = expand::max_defs(src, &toks, &needed, &mut defs);
        let mut seen: Vec<Vec<u8>> = Vec::new();
        let mut held = 0usize;
        for k in 0..maxd.min(ATTEMPTS) {
            eng.work(1)?;
            let built = {
                let ctx = &mut *eng.ctx;
                let mut charge = |n: u64| ctx.work(n);
                expand::build(src, &a0.lx, &toks, &needed, &mut defs, k, &mut charge)?
            };
            if built.invs.is_empty() || seen.contains(&built.text) {
                continue;
            }
            eng.buffer(2 * built.text.len())?;
            held += built.text.len();
            seen.push(built.text.clone());
            let a = analyse(
                eng,
                g,
                &built.text,
                &Src::Expanded {
                    built: &built,
                    orig: src,
                },
            )?;
            eng.release(built.text.len());
            if !expand::same_structure(&a0.lx, &a.lx) || a.unread.len() != merged.len() {
                continue;
            }
            for (m, &u) in merged.iter_mut().zip(&a.unread) {
                *m = *m && u;
            }
            let gained = |line: u32| {
                let l = line as usize;
                a0.unread.get(l).copied().unwrap_or(false)
                    && !a.unread.get(l).copied().unwrap_or(true)
            };
            hits.extend(a.hits.into_iter().filter(|h| gained(h.line)));
            funcs.extend(a.funcs.into_iter().filter(|f| gained(f.line)));
            for p in a.pseudo.iter().filter(|p| p.rule.is_invocation()) {
                if let Some((s, e)) = built.span_to_orig(p.start, p.end)
                    && let Some((_, ne)) = built.span_to_orig(p.start, p.name_end)
                    && gained(a0.lx.line_of(s))
                {
                    calls.push((s, ne, e, true));
                }
            }
            for inv in built.invs.iter().filter(|i| i.function_like) {
                calls.push((inv.start, inv.name_end, inv.end, false));
            }
        }
        eng.release(held);
    }
    let lx = a0.lx;
    let calls = calls
        .into_iter()
        .filter(|&(s, ..)| !merged.get(lx.line_of(s) as usize).copied().unwrap_or(true))
        .map(|(s, ne, e, _)| (s, ne, e))
        .collect();
    Ok(FileRead {
        lx,
        unread: merged,
        hits,
        funcs,
        calls,
    })
}

fn invocation_args(src: &[u8], open: usize, end: usize) -> Vec<Text> {
    let inner = &src[open..end];
    let Some(a) = inner.iter().position(|&b| b == b'(') else {
        return Vec::new();
    };
    let Some(z) = inner.iter().rposition(|&b| b == b')') else {
        return Vec::new();
    };
    if z <= a {
        return Vec::new();
    }
    let body = &inner[a + 1..z];
    let lx = lex::lex(body, true);
    let mut parts = Vec::new();
    let mut depth = 0i64;
    let mut from = 0;
    for (i, &b) in lx.mask.iter().enumerate() {
        match b {
            b'(' => depth += 1,
            b')' => depth -= 1,
            b',' if depth == 0 => {
                parts.push(&body[from..i]);
                from = i + 1;
            }
            _ => {}
        }
    }
    parts.push(&body[from..]);
    if parts.len() == 1 && parts[0].iter().all(|&b| is_ws(b)) {
        return Vec::new();
    }
    parts.into_iter().map(facts::literal_value).collect()
}

struct GuardSweep<'l> {
    lx: &'l Lex,
    branches: Vec<(usize, usize, usize, usize)>,
    next: usize,
    open: Vec<(usize, usize)>,
    chains: Vec<Option<GuardChain>>,
}

impl<'l> GuardSweep<'l> {
    fn new(lx: &'l Lex) -> Self {
        let mut branches = Vec::new();
        for (g, group) in lx.groups.iter().enumerate() {
            for (j, b) in group.branches.iter().enumerate() {
                branches.push((b.body.0, b.body.1, g, j));
            }
        }
        branches.sort_unstable();
        let n = branches.len();
        GuardSweep {
            lx,
            branches,
            next: 0,
            open: Vec::new(),
            chains: vec![None; n],
        }
    }

    fn at(&mut self, p: usize) -> Vec<GuardChain> {
        while let Some(&(start, end, ..)) = self.branches.get(self.next)
            && start <= p
        {
            while self.open.last().is_some_and(|&(e, _)| e <= start) {
                self.open.pop();
            }
            self.open.push((end, self.next));
            self.next += 1;
        }
        while self.open.last().is_some_and(|&(e, _)| e <= p) {
            self.open.pop();
        }
        let mut out = Vec::with_capacity(self.open.len());
        for &(_, i) in &self.open {
            let (_, _, g, j) = self.branches[i];
            out.push(
                self.chains[i]
                    .get_or_insert_with(|| self.lx.chain(g, j))
                    .clone(),
            );
        }
        out
    }
}

struct ScopeSweep {
    funcs: Vec<Func>,
    next: usize,
    open: Vec<usize>,
}

impl ScopeSweep {
    fn new(mut funcs: Vec<Func>) -> Self {
        funcs.sort_by(|a, b| a.start.cmp(&b.start).then(b.end.cmp(&a.end)));
        funcs.dedup_by(|a, b| a.start == b.start && a.end == b.end);
        ScopeSweep {
            funcs,
            next: 0,
            open: Vec::new(),
        }
    }

    fn at(&mut self, p: usize) -> Scope {
        while let Some(f) = self.funcs.get(self.next)
            && f.start <= p
        {
            let start = f.start;
            while self
                .open
                .last()
                .is_some_and(|&i| self.funcs[i].end <= start)
            {
                self.open.pop();
            }
            self.open.push(self.next);
            self.next += 1;
        }
        while self.open.last().is_some_and(|&i| self.funcs[i].end <= p) {
            self.open.pop();
        }
        self.open
            .last()
            .map_or(Scope::File, |&i| self.funcs[i].scope.clone())
    }
}

fn include_target(src: &[u8], lx: &Lex, at: usize, end: usize) -> Option<(Text, bool)> {
    let mask = &lx.mask;
    let mut i = at + 1;
    while i < end && is_ws(mask[i]) {
        i += 1;
    }
    while i < end && lex::is_ident(mask[i]) {
        i += 1;
    }
    while i < end && is_ws(mask[i]) {
        i += 1;
    }
    let (close, system) = match mask.get(i)? {
        b'<' => (b'>', true),
        b'"' => (b'"', false),
        _ => return None,
    };
    let from = i + 1;
    let to = from + mask[from..end].iter().position(|&b| b == close)?;
    if system {
        let target: Vec<u8> = (from..to)
            .filter(|&k| lx.cls[k] != lex::COMMENT && lex::splice_end(src, k).is_none())
            .map(|k| src[k])
            .collect();
        return Some((Text::from_bytes(target), true));
    }
    Some((Text::from(&src[from..to]), false))
}

fn code_lines(lx: &Lex, nl: u32) -> Vec<bool> {
    let mut out = vec![false; nl as usize + 2];
    let n = lx.mask.len();
    let mut in_dir = vec![false; n];
    for d in &lx.dirs {
        in_dir[d.start..(d.end + 1).min(n)].fill(true);
    }
    for (i, &b) in lx.mask.iter().enumerate() {
        if !in_dir[i] && !is_ws(b) {
            out[lx.line_of(i) as usize] = true;
        }
    }
    out
}

struct Ordered {
    pos: usize,
    rank: u8,
    end: usize,
    scoped: bool,
    own: Option<(usize, usize)>,
    fact: Fact,
}

fn parts_mut(fact: &mut Fact) -> Option<(&mut Location, &mut Vec<GuardChain>)> {
    match fact {
        Fact::Call {
            location, guards, ..
        }
        | Fact::Include {
            location, guards, ..
        }
        | Fact::StringLiteral {
            location, guards, ..
        }
        | Fact::GuardRegion {
            location, guards, ..
        } => Some((location, guards)),
        _ => None,
    }
}

fn text_bytes(t: &Text) -> u64 {
    t.as_bytes().len() as u64
}

fn fact_bytes(fact: &Fact) -> u64 {
    let guards: u64 = fact
        .guards()
        .iter()
        .flat_map(|c| c.frames.iter())
        .map(|f| 64 + text_bytes(&f.text) + f.identifiers.iter().map(text_bytes).sum::<u64>())
        .sum();
    let values = match fact {
        Fact::Call { callee, args, .. } => {
            text_bytes(callee) + args.iter().map(|a| 24 + text_bytes(a)).sum::<u64>()
        }
        Fact::StringLiteral { value, .. } => text_bytes(value),
        Fact::Include { target, .. } => text_bytes(target),
        _ => 0,
    };
    let scope = match &fact.location().scope {
        Scope::Function {
            qualified_name,
            signature,
        } => text_bytes(qualified_name) + text_bytes(signature),
        _ => 0,
    };
    std::mem::size_of::<Fact>() as u64 + guards + values + scope
}

struct TextRead {
    facts: Vec<Fact>,
    regions: Vec<Region>,
    held: u64,
}

fn read_text(
    text: &[u8],
    file_name: Option<&[u8]>,
    ctx: &mut ReadCtx<'_>,
) -> Result<TextRead, NotRead> {
    let nl_real = lex::line_count(text);
    let table = ctx.macros();
    let base = ctx.location.clone();
    let mut eng = Engine {
        ctx,
        parsers: Vec::new(),
    };
    let mut best: Option<FileRead> = None;
    for &g in Grammar::tried_for(file_name) {
        let read = analyse_any(&mut eng, g, text, table)?;
        let bad = count(&read.unread);
        let better = best.as_ref().is_none_or(|b| bad < count(&b.unread));
        if better {
            best = Some(read);
        }
        if bad == 0 {
            break;
        }
    }
    let Some(read) = best else {
        return Err(NotRead::new(NotCheckedReason::Unparseable));
    };
    let nl = lex::line_count(text).max(1);
    let code = code_lines(&read.lx, nl);
    let readable = (1..=nl as usize).any(|l| code[l] && !read.unread[l]);
    let any_code = (1..=nl as usize).any(|l| code[l]);
    if any_code && !readable {
        return Err(NotRead::new(NotCheckedReason::Unparseable));
    }
    let lx = &read.lx;
    let at = |line: u32, scope: Scope| base.at(Some(line), scope);
    let mut out: Vec<Ordered> = Vec::new();
    let mut seen = BTreeSet::new();
    for hit in read.hits {
        let rank = match hit.kind {
            HitKind::Call { .. } => 1,
            HitKind::Str { .. } => 2,
        };
        if !seen.insert((hit.start, rank, hit.end)) {
            continue;
        }
        let location = at(lx.line_of(hit.start), hit.scope);
        let fact = match hit.kind {
            HitKind::Call { callee, args } => Fact::Call {
                file_kind: FileKind::Source,
                location,
                callee,
                args,
                guards: Vec::new(),
            },
            HitKind::Str { value } => Fact::StringLiteral {
                file_kind: FileKind::Source,
                location,
                value,
                guards: Vec::new(),
            },
        };
        out.push(Ordered {
            pos: hit.start,
            rank,
            end: hit.end,
            scoped: false,
            own: None,
            fact,
        });
    }
    for (s, ne, e) in read.calls {
        if !seen.insert((s, 1, e)) {
            continue;
        }
        let callee = Text::from(facts::qualified(&text[s..ne]));
        let args = if e > ne {
            invocation_args(text, ne, e)
        } else {
            Vec::new()
        };
        out.push(Ordered {
            pos: s,
            rank: 1,
            end: e,
            scoped: true,
            own: None,
            fact: Fact::Call {
                file_kind: FileKind::Source,
                location: at(lx.line_of(s), Scope::File),
                callee,
                args,
                guards: Vec::new(),
            },
        });
    }
    for d in lx.dirs.iter().filter(|d| d.kind == Kind::Include) {
        let Some((target, system)) = include_target(text, lx, d.hash, d.end.min(text.len())) else {
            continue;
        };
        out.push(Ordered {
            pos: d.hash,
            rank: 0,
            end: d.end,
            scoped: true,
            own: None,
            fact: Fact::Include {
                file_kind: FileKind::Source,
                location: at(d.line, Scope::File),
                target,
                system,
                guards: Vec::new(),
            },
        });
    }
    for (g, group) in lx.groups.iter().enumerate() {
        for (j, branch) in group.branches.iter().enumerate() {
            let d = &lx.dirs[branch.dir];
            out.push(Ordered {
                pos: d.hash,
                rank: 0,
                end: d.end,
                scoped: true,
                own: Some((g, j)),
                fact: Fact::GuardRegion {
                    file_kind: FileKind::Source,
                    location: at(d.line, Scope::File),
                    region: branch.region,
                    guards: Vec::new(),
                    end_line: lx.branch_end_line(g, j, nl),
                },
            });
        }
    }
    out.sort_by_key(|o| (o.pos, o.rank, o.end));
    let mut guard_sweep = GuardSweep::new(lx);
    let mut scope_sweep = ScopeSweep::new(read.funcs);
    let mut facts = Vec::with_capacity(out.len());
    let mut held = 0u64;
    for mut o in out {
        let guards = guard_sweep.at(o.pos);
        let scope = scope_sweep.at(o.pos);
        if let Some((location, slot)) = parts_mut(&mut o.fact) {
            *slot = guards;
            if let Some((g, j)) = o.own {
                slot.push(lx.chain(g, j));
            }
            if o.scoped {
                location.scope = scope;
            }
        }
        let bytes = fact_bytes(&o.fact);
        eng.ctx.buffer(bytes)?;
        held += bytes;
        facts.push(o.fact);
    }
    let regions = passes::to_ranges(&read.unread[..=(nl as usize).min(read.unread.len() - 1)])
        .into_iter()
        .filter_map(|[first_line, last_line]| {
            Region {
                first_line,
                last_line,
                reason: RegionReason::Unparseable,
            }
            .fit_within(u64::from(nl_real))
        })
        .collect();
    Ok(TextRead {
        facts,
        regions,
        held,
    })
}

fn working_text(decoded: String) -> Vec<u8> {
    let mut text = decoded.into_bytes();
    if text.starts_with("\u{FEFF}".as_bytes()) {
        text[..3].fill(b' ');
    }
    text
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CppReader;

impl Reader for CppReader {
    fn file_kind(&self) -> FileKind {
        FileKind::Source
    }

    fn read(&self, bytes: &[u8], ctx: &mut ReadCtx<'_>, sink: &mut dyn FactSink) -> ReadResult {
        let decoded = decode(bytes, ctx.encoding).map_err(NotRead::new)?;
        let text = working_text(decoded.text);
        let file_name = ctx.location.path.as_ref().map(|p| p.file_name().to_vec());
        let read = read_text(&text, file_name.as_deref(), ctx)?;
        for fact in read.facts {
            sink.fact(fact)?;
        }
        ctx.release(read.held);
        for region in read.regions {
            sink.region(region)?;
        }
        Ok(())
    }
}

const READABLE_SHARE_PERCENT: u64 = 90;

fn has_function(tree: &Tree) -> bool {
    let mut stack = vec![tree.root_node()];
    while let Some(node) = stack.pop() {
        if node.kind() == "function_definition" && !node.has_error() {
            return true;
        }
        let mut cursor = node.walk();
        stack.extend(node.children(&mut cursor));
    }
    false
}

fn readable_share_met(tree: &Tree, nl: u32) -> bool {
    let mut bad = vec![false; nl as usize + 2];
    let (error, missing) = passes::bad_ranges(tree);
    for r in error.iter().chain(missing.iter()) {
        passes::mark(&mut bad, *r, nl);
    }
    let unreadable = count(&bad) as u64;
    let readable = u64::from(nl).saturating_sub(unreadable);
    readable.saturating_mul(100) >= u64::from(nl).saturating_mul(READABLE_SHARE_PERCENT)
}

/// The router's content check. `listed` is whether the file name has a listed C/C++
/// extension: such a file is C/C++ unless its decoded text holds a NUL, and one that fails
/// decoding is `Err` with the FR-10 reason. An extensionless file needs most lines readable
/// in one parse and a function definition, `#include` or `#define`.
pub fn identify(bytes: &[u8], ctx: &mut ReadCtx<'_>, listed: bool) -> Result<bool, NotRead> {
    let decoded = match decode(bytes, ctx.encoding) {
        Ok(decoded) => decoded,
        Err(reason) if listed => return Err(NotRead::new(reason)),
        Err(_) => return Ok(false),
    };
    if decoded.text.contains('\0') {
        return Ok(false);
    }
    if listed {
        return Ok(true);
    }
    let text = working_text(decoded.text);
    let file_name = ctx.location.path.as_ref().map(|p| p.file_name().to_vec());
    let grammars = Grammar::tried_for(file_name.as_deref());
    let lx = lex::lex(&text, grammars[0] == Grammar::Cpp);
    let mut eng = Engine {
        ctx,
        parsers: Vec::new(),
    };
    let nl = lex::line_count(&text);
    if nl == 0 {
        return Ok(false);
    }
    let directive = lx
        .dirs
        .iter()
        .any(|d| matches!(d.kind, Kind::Include | Kind::Define));
    for &g in grammars {
        let tree = eng.parse(g, &text).map_err(NotRead::from)?;
        if readable_share_met(&tree, nl) && (directive || has_function(&tree)) {
            return Ok(true);
        }
    }
    Ok(false)
}

pub fn definitions(
    bytes: &[u8],
    declared: Option<Encoding>,
    base: &Location,
) -> Result<Vec<MacroDef>, NotCheckedReason> {
    let text = working_text(decode(bytes, declared)?.text);
    let file_name = base.path.as_ref().map(|p| p.file_name());
    let cpp = Grammar::tried_for(file_name) != [Grammar::C];
    let lx = lex::lex(&text, cpp);
    Ok(expand::definitions(&text, &lx, base))
}

#[cfg(test)]
mod tests;
