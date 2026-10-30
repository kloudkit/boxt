use std::collections::{BTreeMap, BTreeSet};

use tree_sitter::{Node, Tree};

use crate::lex::{Kind, Lex};

pub struct PassText {
    pub text: Vec<u8>,
    pub kept: Vec<bool>,
}

pub fn line_range(node: Node) -> [u32; 2] {
    let s = node.start_position();
    let e = node.end_position();
    let end = if e.column == 0 && e.row > s.row {
        e.row
    } else {
        e.row + 1
    };
    [row(s.row) + 1, row(end)]
}

fn row(r: usize) -> u32 {
    u32::try_from(r).unwrap_or(u32::MAX - 1)
}

pub fn bad_ranges(tree: &Tree) -> (Vec<[u32; 2]>, Vec<[u32; 2]>) {
    let mut error = Vec::new();
    let mut missing = Vec::new();
    let mut stack = vec![tree.root_node()];
    while let Some(node) = stack.pop() {
        if node.is_error() {
            error.push(line_range(node));
            continue;
        }
        if node.is_missing() {
            missing.push(line_range(node));
            continue;
        }
        if node.has_error() {
            let mut cursor = node.walk();
            stack.extend(node.children(&mut cursor));
        }
    }
    error.sort_unstable();
    missing.sort_unstable();
    (error, missing)
}

pub fn mark(bits: &mut [bool], r: [u32; 2], nl: u32) {
    let a = r[0].clamp(1, nl) as usize;
    let b = r[1].clamp(1, nl) as usize;
    for x in bits.iter_mut().take(b + 1).skip(a) {
        *x = true;
    }
}

pub fn to_ranges(bits: &[bool]) -> Vec<[u32; 2]> {
    let mut out = Vec::new();
    let mut start = None;
    for (l, &b) in bits.iter().enumerate() {
        let l = row(l);
        match (b, start) {
            (true, None) => start = Some(l),
            (false, Some(s)) => {
                out.push([s, l - 1]);
                start = None;
            }
            _ => {}
        }
    }
    if let Some(s) = start {
        out.push([s, row(bits.len()) - 1]);
    }
    out
}

fn blanked_dirs(lx: &Lex) -> Vec<bool> {
    let n = lx.mask.len();
    let mut blanked = vec![false; n];
    for d in &lx.dirs {
        blanked[d.start..(d.end + 1).min(n)].fill(true);
    }
    blanked
}

pub fn directive_lines(lx: &Lex, nlines: u32) -> Vec<bool> {
    lines_fully(lx, &blanked_dirs(lx), nlines)
}

fn lines_fully(lx: &Lex, blanked: &[bool], nlines: u32) -> Vec<bool> {
    let n = blanked.len();
    let mut out = vec![false; nlines as usize + 2];
    for (l, slot) in out.iter_mut().enumerate().take(nlines as usize + 1).skip(1) {
        let Some(&s) = lx.line_start.get(l - 1) else {
            continue;
        };
        let e = lx.line_start.get(l).copied().unwrap_or(n);
        *slot = s < e && blanked[s..e].iter().all(|&b| b);
    }
    out
}

pub fn uniform(lx: &Lex, k: usize) -> Vec<usize> {
    lx.groups
        .iter()
        .map(|g| k.min(g.branches.len() - 1))
        .collect()
}

pub fn kept_branches(lx: &Lex, sel: &[usize]) -> Vec<(usize, usize)> {
    let mut alive = vec![false; lx.groups.len()];
    let mut out = Vec::new();
    for (g, grp) in lx.groups.iter().enumerate() {
        alive[g] = grp.parent.is_none_or(|(pg, pj)| alive[pg] && sel[pg] == pj);
        if alive[g] {
            out.push((g, sel[g]));
        }
    }
    out
}

pub fn cover_sel(lx: &Lex, uncovered: &[(usize, usize)]) -> Option<Vec<usize>> {
    let mut assign: Vec<Option<usize>> = vec![None; lx.groups.len()];
    let mut any = false;
    for &(g, j) in uncovered {
        let mut chain = vec![(g, j)];
        let mut cur = lx.groups[g].parent;
        while let Some((pg, pj)) = cur {
            chain.push((pg, pj));
            cur = lx.groups[pg].parent;
        }
        if chain
            .iter()
            .all(|&(cg, cj)| assign[cg].is_none_or(|a| a == cj))
        {
            for (cg, cj) in chain {
                assign[cg] = Some(cj);
            }
            any = true;
        }
    }
    any.then(|| assign.into_iter().map(|a| a.unwrap_or(0)).collect())
}

pub fn pass_sel(texts: [&[u8]; 2], lx: &Lex, sel: &[usize], nlines: u32) -> (PassText, Vec<u8>) {
    let n = texts[0].len();
    let mut blanked = blanked_dirs(lx);
    for (g, grp) in lx.groups.iter().enumerate() {
        for (j, b) in grp.branches.iter().enumerate() {
            if j != sel[g] {
                blanked[b.body.0..b.body.1].fill(true);
            }
        }
    }
    let apply = |text: &[u8]| {
        let mut out = text.to_vec();
        for i in 0..n {
            if blanked[i] && out[i] != b'\n' {
                out[i] = b' ';
            }
        }
        out
    };
    let text = apply(texts[0]);
    let view = apply(texts[1]);
    let full = lines_fully(lx, &blanked, nlines);
    let kept = full
        .iter()
        .enumerate()
        .map(|(l, &f)| l >= 1 && l <= nlines as usize && !f)
        .collect();
    (PassText { text, kept }, view)
}

fn opener_lines(node: Node) -> (Vec<u32>, Option<u32>) {
    let mut lines = vec![row(node.start_position().row) + 1];
    let mut cur = node;
    while let Some(alt) = cur.child_by_field_name("alternative") {
        lines.push(row(alt.start_position().row) + 1);
        cur = alt;
    }
    let mut endif = None;
    let mut cursor = node.walk();
    for c in node.children(&mut cursor) {
        if c.kind() == "#endif" {
            endif = (!c.is_missing()).then_some(row(c.start_position().row) + 1);
        }
    }
    (lines, endif)
}

pub fn extent_mismatches(tree: &Tree, lx: &Lex, error: &[[u32; 2]], nlines: u32) -> Vec<[u32; 2]> {
    let mut lex_by_line = BTreeMap::new();
    for g in &lx.groups {
        let lines: Vec<u32> = g.branches.iter().map(|b| lx.dirs[b.dir].line).collect();
        let endif = g.endif.map(|e| lx.dirs[e].line);
        lex_by_line.insert(lines[0], (lines, endif));
    }
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    let mut stack = vec![tree.root_node()];
    while let Some(node) = stack.pop() {
        if matches!(node.kind(), "preproc_if" | "preproc_ifdef") {
            let (lines, endif) = opener_lines(node);
            let end = line_range(node)[1];
            seen.insert(lines[0]);
            match lex_by_line.get(&lines[0]) {
                Some((ll, le)) if *ll == lines && endif.is_some() && *le == endif => {}
                Some((_, le)) => out.push([lines[0], end.max(le.unwrap_or(nlines))]),
                None => out.push([lines[0], end]),
            }
        }
        let mut cursor = node.walk();
        stack.extend(node.children(&mut cursor));
    }
    for g in &lx.groups {
        let start = lx.dirs[g.branches[0].dir].line;
        if seen.contains(&start) || error.iter().any(|r| r[0] <= start && start <= r[1]) {
            continue;
        }
        let end = g.endif.map_or(nlines, |e| lx.dirs[e].line);
        out.push([start, end]);
    }
    out.sort_unstable();
    out.dedup();
    out
}

#[derive(Default, Clone)]
struct Balance {
    delta: [i64; 3],
    low: [i64; 3],
}

impl Balance {
    fn unbalanced(&self) -> bool {
        (0..3).any(|k| self.delta[k] != 0 || self.low[k] < 0)
    }

    fn count(&mut self, code: &[u8]) {
        for &b in code {
            let (k, d) = match b {
                b'(' => (0, 1),
                b'[' => (1, 1),
                b'{' => (2, 1),
                b')' => (0, -1),
                b']' => (1, -1),
                b'}' => (2, -1),
                _ => continue,
            };
            self.delta[k] += d;
            self.low[k] = self.low[k].min(self.delta[k]);
        }
    }
}

struct Open {
    start: u32,
    branches: Vec<Balance>,
    has_else: bool,
}

fn close(open: Open, end: u32, stack: &mut [Open], flagged: &mut Vec<[u32; 2]>) {
    if open.branches.iter().any(Balance::unbalanced) {
        flagged.push([open.start, end]);
    }
    if let Some(parent) = stack.last_mut().and_then(|p| p.branches.last_mut()) {
        let first = open.branches[0].delta;
        let agree =
            open.branches.iter().all(|b| b.delta == first) && (open.has_else || first == [0; 3]);
        if agree {
            for (k, d) in first.iter().enumerate() {
                parent.low[k] = parent.low[k].min(parent.delta[k] + open.branches[0].low[k]);
                parent.delta[k] += d;
            }
        }
    }
}

pub fn split_chains(lx: &Lex, nlines: u32) -> Vec<[u32; 2]> {
    let n = lx.mask.len();
    let mut stack: Vec<Open> = Vec::new();
    let mut flagged = Vec::new();
    let mut pos = 0;
    for d in &lx.dirs {
        if let Some(b) = stack.last_mut().and_then(|o| o.branches.last_mut()) {
            b.count(&lx.mask[pos.min(d.start)..d.start]);
        }
        pos = (d.end + 1).min(n);
        match d.kind {
            Kind::If | Kind::Ifdef | Kind::Ifndef => stack.push(Open {
                start: d.line,
                branches: vec![Balance::default()],
                has_else: false,
            }),
            Kind::Elif | Kind::Elifdef | Kind::Elifndef | Kind::Else => {
                if let Some(open) = stack.last_mut() {
                    open.branches.push(Balance::default());
                    open.has_else |= d.kind == Kind::Else;
                }
            }
            Kind::Endif => {
                if let Some(open) = stack.pop() {
                    close(open, d.line, &mut stack, &mut flagged);
                }
            }
            _ => {}
        }
    }
    if let Some(b) = stack.last_mut().and_then(|o| o.branches.last_mut()) {
        b.count(&lx.mask[pos.min(n)..]);
    }
    while let Some(open) = stack.pop() {
        close(open, nlines, &mut stack, &mut flagged);
    }
    flagged.sort_unstable();
    flagged
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lex::lex;

    fn parse_c(text: &[u8]) -> Tree {
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&tree_sitter_c::LANGUAGE.into())
            .unwrap();
        parser.parse(text, None).unwrap()
    }

    fn pass(src: &[u8], lx: &Lex, k: usize, nl: u32) -> PassText {
        pass_sel([src, src], lx, &uniform(lx, k), nl).0
    }

    #[test]
    fn passes_keep_branch_k_and_preserve_offsets() {
        let src = b"void t(void) {\n#if A\n  g(1,\n#elif B\n  g(2,\n#else\n  g(3,\n#endif\n  4);\n#ifdef X\n  h();\n#endif\n}\n";
        let lx = lex(src, false);
        assert_eq!(lx.max_branches(), 3);
        let p0 = pass(src, &lx, 0, 13);
        assert_eq!(p0.text.len(), src.len());
        assert_eq!(
            String::from_utf8_lossy(&p0.text),
            "void t(void) {\n     \n  g(1,\n       \n      \n     \n      \n      \n  4);\n        \n  h();\n      \n}\n"
        );
        let p2 = pass(src, &lx, 2, 13);
        assert!(String::from_utf8_lossy(&p2.text).contains("g(3,"));
        assert!(p2.kept[11] && !p2.kept[3] && p2.kept[7] && !p2.kept[2]);
        assert!(!parse_c(&p2.text).root_node().has_error());
    }

    #[test]
    fn coverage_pass_reaches_a_group_nested_in_else() {
        let src = b"#if A\na();\n#else\n#ifdef B\nb();\n#else\nc();\n#endif\n#endif\n";
        let lx = lex(src, false);
        let mut covered = BTreeSet::new();
        for k in 0..lx.max_branches() {
            covered.extend(kept_branches(&lx, &uniform(&lx, k)));
        }
        let all: Vec<(usize, usize)> = lx
            .groups
            .iter()
            .enumerate()
            .flat_map(|(g, gr)| (0..gr.branches.len()).map(move |j| (g, j)))
            .collect();
        let missing: Vec<_> = all.into_iter().filter(|b| !covered.contains(b)).collect();
        assert_eq!(missing, vec![(1, 0)]);
        let sel = cover_sel(&lx, &missing).unwrap();
        assert_eq!(sel, vec![1, 0]);
        let (p, _) = pass_sel([src, src], &lx, &sel, 9);
        assert!(String::from_utf8_lossy(&p.text).contains("b();") && p.kept[5] && !p.kept[7]);
    }

    #[test]
    fn a_runaway_chain_is_flagged_by_extent() {
        let src = b"int f(int a\n#ifdef X\n, int b) {\n#else\n) {\n#endif\n  return 0;\n}\n";
        let lx = lex(src, false);
        let tree = parse_c(src);
        let (err, _) = bad_ranges(&tree);
        let flagged = extent_mismatches(&tree, &lx, &err, 8);
        assert!(!flagged.is_empty() || !err.is_empty());
        for k in 0..lx.max_branches() {
            let p = pass(src, &lx, k, 8);
            assert!(!parse_c(&p.text).root_node().has_error(), "pass {k}");
        }
    }

    #[test]
    fn extent_mismatch_without_an_error_is_flagged() {
        let src = b"#if A\nint a;\n#else\nint b;\n#endif\n";
        let lx = lex(src, false);
        let tree = parse_c(b"int x;\n      \nint a;\n     \nint b;\n      \n");
        assert_eq!(extent_mismatches(&tree, &lx, &[], 5), vec![[1, 5]]);
        let ok = parse_c(src);
        assert!(extent_mismatches(&ok, &lx, &[], 5).is_empty());
    }

    #[test]
    fn split_statement_is_flagged() {
        let src = b"void f(void) {\n#if A\n  g(1,\n#else\n  g(2,\n#endif\n  3);\n}\n";
        assert_eq!(split_chains(&lex(src, false), 8), vec![[2, 6]]);
    }

    #[test]
    fn balanced_chain_is_not_flagged() {
        let src = b"void f(void) {\n#if A\n  x();\n#else /* ( */\n  y(\")\");\n#endif\n}\n";
        assert!(split_chains(&lex(src, false), 7).is_empty());
    }

    #[test]
    fn same_brace_in_every_branch_does_not_cascade() {
        let src = b"#ifndef H\nvoid f(void) {\n#if A\n if (a) {\n#else\n if (b) {\n#endif\n  q();\n }\n}\n#endif\n";
        assert_eq!(split_chains(&lex(src, false), 11), vec![[3, 7]]);
    }

    #[test]
    fn ranges_round_trip() {
        let mut bits = vec![false; 10];
        mark(&mut bits, [2, 3], 9);
        mark(&mut bits, [7, 20], 9);
        assert_eq!(to_ranges(&bits), vec![[2, 3], [7, 9]]);
    }
}
