use std::collections::BTreeMap;
use std::fmt::Write;

use anstyle::{AnsiColor, Color, Style};
use boxt_core::fact::GuardChain;
use boxt_core::identity::FindingId;
use boxt_core::labels;
use boxt_core::location::{Location, Scope};
use boxt_core::outcome::{Count, Details, FileOutcome, NotCheckedReason, Outcome, Sm3};
use boxt_core::settings::Encoding;
use boxt_core::verdict::{
    Confidence, Group, GroupKey, LineRange, NotCheckedEntry, ReportedFinding, Result,
    TargetVerdict, Tier, TierRange,
};
use unicode_width::UnicodeWidthStr;

use crate::escape::escape;
use crate::layout::{Atom, PLAIN, Painter, Para, boxed, emit};

struct Inputs {
    given: Vec<(u32, String)>,
    multi: bool,
}

impl Inputs {
    fn of(result: &Result) -> Inputs {
        let mut given: Vec<(u32, String)> = Vec::new();
        let locations = result.findings.iter().map(|f| &f.finding.location).chain(
            result.not_checked.iter().map(|e| match e {
                NotCheckedEntry::File { location, .. }
                | NotCheckedEntry::Regions { location, .. }
                | NotCheckedEntry::DynamicLoad { location, .. } => location,
            }),
        );
        for loc in locations {
            if !given.iter().any(|(i, _)| *i == loc.input.index) {
                given.push((loc.input.index, escape(&loc.input.given)));
            }
        }
        let multi = given.len() > 1;
        Inputs { given, multi }
    }

    fn name(&self, index: u32) -> String {
        self.given
            .iter()
            .find(|(i, _)| *i == index)
            .map_or_else(|| format!("input {}", index + 1), |(_, g)| g.clone())
    }

    fn prefix(&self, para: Para, index: u32) -> Para {
        if self.multi {
            para.value(&format!("{}:", self.name(index)), PLAIN)
                .plain(" ")
        } else {
            para
        }
    }
}

pub const MIN_WIDTH: usize = 60;
pub const MAX_WIDTH: usize = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalOptions {
    pub styled: bool,
    pub width: usize,
    pub all: bool,
}

const BOLD: Style = Style::new().bold();
const DIM: Style = Style::new().dimmed();
const HEADER: Style = Style::new().dimmed().underline();
const YELLOW: Style = Style::new().fg_color(Some(Color::Ansi(AnsiColor::Yellow)));
const CYAN: Style = Style::new().fg_color(Some(Color::Ansi(AnsiColor::Cyan)));
const MAGENTA: Style = Style::new().fg_color(Some(Color::Ansi(AnsiColor::Magenta)));
const ACCENT: Style = MAGENTA.bold();
const FIX: Style = Style::new()
    .bold()
    .fg_color(Some(Color::Ansi(AnsiColor::Green)));
const TABLE_MIN_WIDTH: usize = 72;
const GUTTER: &str = "   ";
const INDENT: &str = "  ";
const MIN_SIGNATURE_COLUMN: usize = 16;
const LOCATION_COLUMN_MAX: usize = 24;
const DASH: &str = "–";

pub fn terminal(result: &Result, options: &TerminalOptions) -> String {
    let p = Painter {
        styled: options.styled,
    };
    let width = options.width.clamp(MIN_WIDTH, MAX_WIDTH);
    let inputs = Inputs::of(result);
    let mut out = String::new();
    let mut header = vec![Para::new().text(&result.input_scope_text, YELLOW)];
    if result.host_dependent {
        header.push(Para::new().text(labels::HOST_DEPENDENT_TEXT, YELLOW));
    }
    boxed(&mut out, &p, width, YELLOW, &header);
    out.push('\n');
    targets(&mut out, &p, width, result);
    out.push('\n');
    out.push_str(&p.paint(DIM, &"─".repeat(width)));
    out.push('\n');
    let heading = Para::new().text("Findings", BOLD).plain("   ");
    let heading = if result.groups.is_empty() {
        heading.plain("none")
    } else {
        heading
            .text(mark(Confidence::Found), BOLD)
            .plain(&format!(" {}   ", labels::confidence(Confidence::Found)))
            .text(mark(Confidence::Suspected), DIM)
            .plain(&format!(" {}", labels::confidence(Confidence::Suspected)))
    };
    emit(&mut out, &p, width, &heading, " ", "   ");
    out.push('\n');
    let by_id: BTreeMap<&FindingId, &ReportedFinding> =
        result.findings.iter().map(|f| (&f.id, f)).collect();
    for group in &result.groups {
        group_block(&mut out, &p, width, &inputs, &by_id, group);
    }
    not_checked(&mut out, &p, width, &inputs, result, options.all);
    out
}

fn tier_color(tier: Tier) -> AnsiColor {
    match tier {
        Tier::Blocked => AnsiColor::Red,
        Tier::NeedsWindows | Tier::NeedsCodeChange => AnsiColor::Yellow,
        Tier::ReadyWithConfig => AnsiColor::Cyan,
        Tier::Ready => AnsiColor::Green,
    }
}

fn mark(confidence: Confidence) -> &'static str {
    match confidence {
        Confidence::Found => "●",
        Confidence::Suspected => "○",
    }
}

fn badge(p: &Painter, tier: Tier) -> Vec<(Style, String)> {
    let label = labels::tier(tier);
    if p.styled {
        let bg = tier_color(tier);
        let fg = if tier == Tier::Blocked {
            AnsiColor::BrightWhite
        } else {
            AnsiColor::Black
        };
        let style = Style::new()
            .bold()
            .bg_color(Some(bg.into()))
            .fg_color(Some(fg.into()));
        vec![(style, format!(" {} ", label.to_uppercase()))]
    } else {
        vec![(PLAIN, label.to_owned())]
    }
}

fn range_badge(p: &Painter, range: TierRange) -> Atom {
    let mut parts = Vec::new();
    if !p.styled {
        parts.push((PLAIN, "[".to_owned()));
    }
    parts.extend(badge(p, range.found));
    if range.found != range.with_suspected {
        parts.push((PLAIN, if p.styled { "…" } else { " … " }.to_owned()));
        parts.extend(badge(p, range.with_suspected));
    }
    if !p.styled {
        parts.push((PLAIN, "]".to_owned()));
    }
    Atom::new(parts)
}

fn counts(found: u64, suspected: u64) -> String {
    match (found, suspected) {
        (f, 0) => format!("{f} found"),
        (0, s) => format!("{s} suspected"),
        (f, s) => format!("{f} found · {s} suspected"),
    }
}

fn target_counts(target: &TargetVerdict) -> String {
    let parts: Vec<String> = [
        (target.found, "found"),
        (target.suspected, "suspected"),
        (target.allowed, "allowed"),
        (unmapped_count(target), "unmapped"),
    ]
    .into_iter()
    .filter(|(n, _)| *n > 0)
    .map(|(n, label)| format!("{n} {label}"))
    .collect();
    if parts.is_empty() {
        "no findings".to_owned()
    } else {
        parts.join(" · ")
    }
}

fn count_cell(n: u64) -> String {
    if n == 0 {
        DASH.to_owned()
    } else {
        n.to_string()
    }
}

fn unmapped_count(target: &TargetVerdict) -> u64 {
    u64::try_from(target.unmapped.len()).unwrap_or(u64::MAX)
}

fn targets(out: &mut String, p: &Painter, width: usize, result: &Result) {
    let rows: Vec<&TargetVerdict> = result.targets.iter().collect();
    let with_unmapped = rows.iter().any(|t| !t.unmapped.is_empty());
    let mut headers = vec!["target", "verdict", "found", "suspected", "allowed"];
    if with_unmapped {
        headers.push("unmapped");
    }
    let cells: Vec<(String, Atom, Vec<String>)> = rows
        .iter()
        .map(|t| {
            let mut numbers = vec![
                count_cell(t.found),
                count_cell(t.suspected),
                count_cell(t.allowed),
            ];
            if with_unmapped {
                numbers.push(count_cell(unmapped_count(t)));
            }
            (
                t.target.as_str().to_owned(),
                range_badge(p, t.range),
                numbers,
            )
        })
        .collect();
    let mut widths: Vec<usize> = headers.iter().map(|h| h.width()).collect();
    for (target, badge, numbers) in &cells {
        widths[0] = widths[0].max(target.width());
        widths[1] = widths[1].max(badge.columns());
        for (i, n) in numbers.iter().enumerate() {
            widths[i + 2] = widths[i + 2].max(n.width());
        }
    }
    let total = INDENT.width() + widths.iter().sum::<usize>() + GUTTER.width() * (widths.len() - 1);
    if width >= TABLE_MIN_WIDTH && total <= width {
        let header: Vec<String> = headers
            .iter()
            .enumerate()
            .map(|(i, h)| aligned(h, widths[i], i >= 2))
            .collect();
        out.push_str(INDENT);
        out.push_str(&p.paint(HEADER, &header.join(GUTTER)));
        out.push('\n');
        for (t, (target, badge, numbers)) in rows.iter().zip(&cells) {
            let tint = Style::new().fg_color(Some(tier_color(t.range.with_suspected).into()));
            out.push_str(INDENT);
            out.push_str(target);
            out.push_str(&" ".repeat(widths[0] - target.width()));
            out.push_str(GUTTER);
            out.push_str(&badge.paint(p));
            out.push_str(&" ".repeat(widths[1] - badge.columns()));
            for (i, n) in numbers.iter().enumerate() {
                out.push_str(GUTTER);
                out.push_str(&" ".repeat(widths[i + 2] - n.width()));
                out.push_str(&p.paint(if n == DASH { DIM } else { tint }, n));
            }
            out.push('\n');
        }
    } else {
        for (t, (target, badge, _)) in rows.iter().zip(cells) {
            let para = Para::new()
                .plain(&target)
                .plain("  ")
                .atom(badge)
                .plain(&format!("  {}", target_counts(t)));
            emit(out, p, width, &para, INDENT, "    ");
        }
    }
    out.push('\n');
    let files = Para::new().text(
        &format!(
            "{} files read · {} not checked · {}",
            result.counts.read,
            result.counts.not_checked,
            sm3_share(&result.sm3)
        ),
        DIM,
    );
    emit(out, p, width, &files, INDENT, "    ");
}

fn aligned(text: &str, width: usize, right: bool) -> String {
    let pad = " ".repeat(width.saturating_sub(text.width()));
    if right {
        format!("{pad}{text}")
    } else {
        format!("{text}{pad}")
    }
}

fn sm3_share(sm3: &Sm3) -> String {
    match sm3.lines {
        None => "source lines not checked: n/a".to_owned(),
        Some(lines) => {
            let at_least = if lines.is_lower_bound() {
                "at least "
            } else {
                ""
            };
            if lines.denominator == 0 {
                format!(
                    "{at_least}{} of 0 source lines not checked",
                    lines.numerator
                )
            } else {
                format!(
                    "{at_least}{} of source lines not checked",
                    percent(lines.numerator, lines.denominator)
                )
            }
        }
    }
}

fn group_block(
    out: &mut String,
    p: &Painter,
    width: usize,
    inputs: &Inputs,
    by_id: &BTreeMap<&FindingId, &ReportedFinding>,
    group: &Group,
) {
    let GroupKey::Behaviour(behaviour) = &group.key;
    let name = format!("▌{behaviour}");
    let tally = counts(group.found, group.suspected);
    let used = 1 + name.width() + 2 + tally.width();
    let mut heading = Para::new().text(&name, ACCENT).plain(&format!("  {tally}"));
    let inline = match group.rules.as_slice() {
        [only] if used + 2 + only.rule.as_str().width() <= width => {
            let pad = width - used - only.rule.as_str().width();
            heading = heading
                .plain(&" ".repeat(pad))
                .text(only.rule.as_str(), DIM);
            true
        }
        _ => false,
    };
    emit(out, p, width, &heading, " ", "   ");
    let base = if inline { "   " } else { "     " };
    let place_column = group
        .rules
        .iter()
        .flat_map(|section| &section.findings)
        .filter_map(|id| by_id.get(id))
        .map(|reported| place_para(Para::new(), inputs, &reported.finding.location).columns())
        .filter(|&columns| columns <= LOCATION_COLUMN_MAX)
        .max()
        .unwrap_or(0);
    for section in &group.rules {
        if !inline {
            emit(
                out,
                p,
                width,
                &Para::new().text(section.rule.as_str(), DIM),
                "   ",
                "     ",
            );
        }
        let fix = Para::new()
            .text("fix", FIX)
            .plain("  ")
            .plain(&escape(&section.fix.as_str().into()));
        emit(out, p, width, &fix, base, &format!("{base}     "));
        out.push('\n');
        for id in &section.findings {
            if let Some(reported) = by_id.get(id) {
                finding_lines(out, p, width, inputs, base, place_column, reported);
                out.push('\n');
            }
        }
    }
}

fn place_para(para: Para, inputs: &Inputs, loc: &Location) -> Para {
    let mut parts: Vec<String> = loc.frames.iter().map(|f| escape(&f.name)).collect();
    if let Some(path) = &loc.path {
        parts.push(escape(path.as_text()));
    }
    let para = inputs
        .prefix(para, loc.input.index)
        .value(&parts.join("!"), CYAN);
    match loc.line {
        Some(line) => para.glue(&format!(":{line}"), DIM),
        None => para,
    }
}

fn parameters(signature: &str) -> Option<Vec<&str>> {
    let inner = signature.strip_prefix('(')?;
    let mut pieces = Vec::new();
    let mut depth = 0usize;
    let mut start = 0;
    let bytes = inner.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        match b {
            b'(' | b'[' | b'<' | b'{' => depth += 1,
            b')' | b']' | b'>' | b'}' => depth = depth.saturating_sub(1),
            b',' if depth == 0 && bytes.get(i + 1) == Some(&b' ') => {
                pieces.push(&inner[start..i + 2]);
                start = i + 2;
            }
            _ => {}
        }
    }
    pieces.push(&inner[start..]);
    Some(pieces)
}

fn signature_lines(pieces: &[&str], avail: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut current = String::new();
    for piece in pieces {
        let shown = piece.trim_end_matches(' ');
        if current.width() + shown.width() <= avail {
            current.push_str(piece);
            continue;
        }
        if !current.is_empty() {
            lines.push(current.trim_end_matches(' ').to_owned());
            current = String::new();
        }
        if shown.width() <= avail {
            current.push_str(piece);
        } else {
            let mut out = String::new();
            emit(
                &mut out,
                &Painter { styled: false },
                avail,
                &Para::new().value(shown, PLAIN),
                "",
                "",
            );
            lines.extend(out.lines().map(str::to_owned));
        }
    }
    if !current.is_empty() || lines.is_empty() {
        lines.push(current.trim_end_matches(' ').to_owned());
    }
    lines
}

fn finding_lines(
    out: &mut String,
    p: &Painter,
    width: usize,
    inputs: &Inputs,
    base: &str,
    place_column: usize,
    reported: &ReportedFinding,
) {
    let f = &reported.finding;
    let first = format!("{base}{} ", mark(f.confidence));
    let under = format!("{base}  ");
    let place = place_para(Para::new(), inputs, &f.location);
    let over = place.columns() > place_column;
    let pad = " ".repeat(place_column.saturating_sub(place.columns()));
    let mut placed = false;
    if let Scope::Function {
        qualified_name,
        signature,
    } = &f.location.scope
    {
        let name = escape(qualified_name);
        let signature = escape(signature);
        let column = if place_column == 0 {
            under.width()
        } else {
            first.width() + place_column + GUTTER.width()
        };
        let head = column + name.width() + 1;
        if let Some(pieces) = parameters(&signature)
            && head + MIN_SIGNATURE_COLUMN <= width
        {
            let lines = signature_lines(&pieces, width - head);
            if over {
                emit(out, p, width, &place, &first, &under);
                out.push_str(&" ".repeat(column));
            } else {
                out.push_str(&first);
                out.push_str(&place.paint(p));
                out.push_str(&pad);
                out.push_str(GUTTER);
            }
            out.push_str(&p.paint(BOLD, &name));
            out.push_str(&p.paint(DIM, "("));
            for (i, text) in lines.iter().enumerate() {
                if i > 0 {
                    out.push('\n');
                    out.push_str(&" ".repeat(head));
                }
                out.push_str(&p.paint(DIM, text));
            }
            out.push('\n');
            placed = true;
        } else {
            let (opening, rest) = signature
                .split_once(' ')
                .map_or((signature.as_str(), ""), |(a, b)| (a, b));
            let opening = if rest.is_empty() {
                opening.to_owned()
            } else {
                format!("{opening} ")
            };
            let para = place
                .clone()
                .plain(&pad)
                .plain(GUTTER)
                .value(&name, BOLD)
                .glue(&opening, DIM)
                .value(rest, DIM);
            emit(out, p, width, &para, &first, &under);
            placed = true;
        }
    }
    if !placed {
        let place = if matches!(f.location.scope, Scope::File) {
            place
        } else {
            place.plain(&pad)
        };
        let para = match &f.location.scope {
            Scope::File | Scope::Function { .. } => place,
            Scope::Symbol(symbol) => place.plain("   symbol ").value(&escape(symbol), PLAIN),
            Scope::UnitSection { section, key } => place
                .plain(GUTTER)
                .value(&format!("[{}] {}", escape(section), escape(key)), PLAIN),
            Scope::ConfigKey(key) => place.plain("   key ").value(&escape(key), PLAIN),
            Scope::Script(name) => place.plain(&format!("   script {}", name.name())),
        };
        emit(out, p, width, &para, &first, &under);
    }
    let ids = Para::new().text(&format!("{} · {}", f.evidence.definition, reported.id), DIM);
    emit(out, p, width, &ids, &under, &under);
    if !f.guards.is_empty() {
        let guard = Para::new()
            .text("guard", MAGENTA)
            .plain("  ")
            .value(&guards(&f.guards), PLAIN);
        emit(out, p, width, &guard, &under, &format!("{under}       "));
    }
}

fn not_checked(
    out: &mut String,
    p: &Painter,
    width: usize,
    inputs: &Inputs,
    result: &Result,
    all: bool,
) {
    emit(
        out,
        p,
        width,
        &Para::new().text("Not checked", BOLD),
        "",
        "",
    );
    let summarize = !all && result.no_reader > 0;
    let listed = result.targets.iter().any(|t| !t.unmapped.is_empty())
        || !result.not_checked.is_empty()
        || !result.macro_tables_cut.is_empty();
    let entry = |out: &mut String, para: Para| {
        emit(out, p, width, &para, "  ", "    ");
    };
    if !listed {
        entry(out, Para::new().plain("none"));
    }
    for target in result.targets.iter() {
        for behaviour in target.unmapped.iter() {
            entry(
                out,
                Para::new().plain(&format!(
                    "{}: unmapped behaviour {behaviour}",
                    target.target
                )),
            );
        }
    }
    if summarize {
        entry(
            out,
            Para::new().plain(&format!(
                "{}: {} {} (--all to list)",
                labels::not_checked_reason(NotCheckedReason::NoReader),
                result.no_reader,
                if result.no_reader == 1 {
                    "file"
                } else {
                    "files"
                }
            )),
        );
    }
    for e in result.not_checked.iter() {
        if summarize && e.is_no_reader() {
            continue;
        }
        entry(out, not_checked_para(inputs, e));
    }
    for tree in result.macro_tables_cut.iter() {
        entry(
            out,
            Para::new()
                .value(&format!("{}:", inputs.name(*tree)), PLAIN)
                .plain(&format!(" {}", labels::macro_table_cut())),
        );
    }
    sm3_lines(out, p, width, inputs, &result.sm3);
}

fn location(para: Para, inputs: &Inputs, loc: &Location, with_line: bool) -> Para {
    let mut parts: Vec<String> = loc.frames.iter().map(|f| escape(&f.name)).collect();
    if let Some(path) = &loc.path {
        parts.push(escape(path.as_text()));
    }
    let mut place = parts.join("!");
    if with_line && let Some(line) = loc.line {
        let _ = write!(place, ":{line}");
    }
    let para = inputs.prefix(para, loc.input.index).value(&place, PLAIN);
    match &loc.scope {
        Scope::File => para,
        Scope::Function {
            qualified_name,
            signature,
        } => para.plain("  in ").value(
            &format!("{}{}", escape(qualified_name), escape(signature)),
            PLAIN,
        ),
        Scope::Symbol(symbol) => para.plain("  symbol ").value(&escape(symbol), PLAIN),
        Scope::UnitSection { section, key } => para
            .plain("  ")
            .value(&format!("[{}] {}", escape(section), escape(key)), PLAIN),
        Scope::ConfigKey(key) => para.plain("  key ").value(&escape(key), PLAIN),
        Scope::Script(name) => para.plain(&format!("  script {}", name.name())),
    }
}

fn guards(chains: &[GuardChain]) -> String {
    chains
        .iter()
        .map(|chain| {
            chain
                .frames
                .iter()
                .map(|frame| escape(&frame.text))
                .collect::<Vec<_>>()
                .join(", ")
        })
        .collect::<Vec<_>>()
        .join(" > ")
}

fn not_checked_para(inputs: &Inputs, entry: &NotCheckedEntry) -> Para {
    match entry {
        NotCheckedEntry::File {
            location: loc,
            outcome,
        } => outcome_para(
            location(Para::new(), inputs, loc, false).glue(":", PLAIN),
            outcome,
        ),
        NotCheckedEntry::Regions {
            location: loc,
            reason,
            ranges,
        } => {
            let span = |r: &LineRange| {
                if r.first == r.last {
                    r.first.to_string()
                } else {
                    format!("{}–{}", r.first, r.last)
                }
            };
            let text = match ranges.as_slice() {
                [one] if one.first == one.last => {
                    format!("{} at line {}", labels::region_reason(*reason), one.first)
                }
                [one] => format!("{} at lines {}", labels::region_reason(*reason), span(one)),
                many => format!(
                    "{} at lines {}",
                    labels::region_reason_plural(*reason),
                    many.iter().map(span).collect::<Vec<_>>().join(", ")
                ),
            };
            location(Para::new(), inputs, loc, false)
                .glue(":", PLAIN)
                .plain(&format!(" {text}"))
        }
        NotCheckedEntry::DynamicLoad {
            location: loc,
            symbol,
        } => location(Para::new(), inputs, loc, true)
            .glue(":", PLAIN)
            .plain(" loads code at run time through ")
            .value(&escape(symbol), PLAIN),
    }
}

fn outcome_para(para: Para, outcome: &Outcome) -> Para {
    let mut para = match &outcome.file {
        FileOutcome::NotChecked { reason, details } => details_para(
            para.plain(&format!(" {}", labels::not_checked_reason(*reason))),
            details,
        ),
        FileOutcome::Aggregate {
            reason,
            count,
            at_least,
        } => {
            let count = match count {
                Some(n) => Count::Exact(*n),
                None => Count::AtLeast((*at_least).max(1)),
            };
            para.plain(&format!(
                " {}, {count} files from here on",
                labels::not_checked_reason(*reason)
            ))
        }
        FileOutcome::Read => para,
        FileOutcome::OutOfScope { cause } => {
            para.plain(&format!(" {}", labels::out_of_scope_cause(cause)))
        }
    };
    if outcome.host_dependent {
        para = para.plain(&format!("; {}", labels::HOST_DEPENDENT_MARK));
    }
    para
}

fn details_para(para: Para, details: &Details) -> Para {
    match details {
        Details::None => para,
        Details::Measured { value, limit } => para.plain(&format!(" ({value} > {limit})")),
        Details::Encoding { declared } => {
            para.plain(&format!(" (declared {})", encoding(*declared)))
        }
        Details::Interpreter { interpreter } => para
            .plain(" ")
            .value(&format!("({})", escape(interpreter)), PLAIN),
        Details::EntryName { name } => para.plain(" ").value(&format!("({})", escape(name)), PLAIN),
    }
}

fn encoding(encoding: Encoding) -> String {
    match encoding {
        Encoding::Utf8 => "UTF-8".to_owned(),
        Encoding::Utf16Le => "UTF-16LE".to_owned(),
        Encoding::Utf16Be => "UTF-16BE".to_owned(),
        Encoding::Latin1 => "Latin-1".to_owned(),
        Encoding::WindowsCodePage(n) => format!("code page {n}"),
    }
}

fn percent(numerator: u64, denominator: u64) -> String {
    let mut tenths = (u128::from(numerator) * 1000).div_ceil(u128::from(denominator));
    if numerator < denominator {
        tenths = tenths.min(999);
    }
    format!("{}.{}%", tenths / 10, tenths % 10)
}

fn sm3_lines(out: &mut String, p: &Painter, width: usize, inputs: &Inputs, sm3: &Sm3) {
    let mut text = "Source lines not checked: ".to_owned();
    match sm3.lines {
        None => text.push_str("n/a"),
        Some(lines) => {
            let at_least = if lines.is_lower_bound() {
                "at least "
            } else {
                ""
            };
            if lines.denominator == 0 {
                let _ = write!(text, "{at_least}{} of 0", lines.numerator);
            } else {
                let _ = write!(
                    text,
                    "{at_least}{} ({} of {})",
                    percent(lines.numerator, lines.denominator),
                    lines.numerator,
                    lines.denominator
                );
            }
            if lines.uncounted_files > 0 {
                let _ = write!(text, ", {} files uncounted", lines.uncounted_files);
            }
            if lines.incomplete_files > 0 {
                let _ = write!(text, ", {} files counted in part", lines.incomplete_files);
            }
        }
    }
    match sm3.files {
        None => text.push_str("; files: n/a"),
        Some(files) => {
            let _ = write!(
                text,
                "; files: {} of {}",
                files.numerator, files.denominator
            );
        }
    }
    emit(out, p, width, &Para::new().plain(&text), "  ", "    ");
    for (label, keys) in [
        ("uncounted", &sm3.uncounted),
        ("counted in part", &sm3.incomplete),
    ] {
        for key in keys.iter() {
            let para = inputs
                .prefix(Para::new().plain(&format!("{label}: ")), key.input_index)
                .value(&escape(key.job_path.as_text()), PLAIN);
            emit(out, p, width, &para, "    ", "      ");
        }
    }
}
