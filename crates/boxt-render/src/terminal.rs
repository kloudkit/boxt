use std::collections::BTreeMap;
use std::fmt::Write;

use anstyle::{AnsiColor, Style};
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

pub fn terminal(result: &Result, options: &TerminalOptions) -> String {
    let p = Painter {
        styled: options.styled,
    };
    let width = options.width.clamp(MIN_WIDTH, MAX_WIDTH);
    let inputs = Inputs::of(result);
    let mut out = String::new();
    let mut header = vec![Para::new().plain(&result.input_scope_text)];
    if result.host_dependent {
        header.push(Para::new().plain(labels::HOST_DEPENDENT_TEXT));
    }
    boxed(&mut out, &p, width, None, &header);
    out.push('\n');
    for target in result.targets.iter() {
        target_box(&mut out, &p, width, result, target);
        out.push('\n');
    }
    let by_id: BTreeMap<&FindingId, &ReportedFinding> =
        result.findings.iter().map(|f| (&f.id, f)).collect();
    emit(
        &mut out,
        &p,
        width,
        &Para::new().text("Findings", BOLD),
        "",
        "",
    );
    let legend = if result.groups.is_empty() {
        "none".to_owned()
    } else {
        format!(
            "{} {}  {} {}",
            mark(Confidence::Found),
            labels::confidence(Confidence::Found),
            mark(Confidence::Suspected),
            labels::confidence(Confidence::Suspected)
        )
    };
    emit(
        &mut out,
        &p,
        width,
        &Para::new().plain(&legend),
        "  ",
        "    ",
    );
    out.push('\n');
    for group in &result.groups {
        group_block(&mut out, &p, width, &inputs, &by_id, group);
        out.push('\n');
    }
    not_checked(&mut out, &p, width, &inputs, result, options.all);
    out
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
        let (bg, fg) = match tier {
            Tier::Blocked => (AnsiColor::Red, AnsiColor::BrightWhite),
            Tier::NeedsWindows | Tier::NeedsCodeChange => (AnsiColor::Yellow, AnsiColor::Black),
            Tier::ReadyWithConfig => (AnsiColor::Cyan, AnsiColor::Black),
            Tier::Ready => (AnsiColor::Green, AnsiColor::Black),
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
        (
            u64::try_from(target.unmapped.len()).unwrap_or(u64::MAX),
            "unmapped",
        ),
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

fn target_box(
    out: &mut String,
    p: &Painter,
    width: usize,
    result: &Result,
    target: &TargetVerdict,
) {
    let title = Atom::new(vec![(BOLD, target.target.as_str().to_owned())]);
    let verdict = Para::new()
        .atom(range_badge(p, target.range))
        .plain(&format!("  {}", target_counts(target)));
    let files = Para::new().plain(&format!(
        "{} files read · {} not checked · {}",
        result.counts.read,
        result.counts.not_checked,
        sm3_share(&result.sm3)
    ));
    boxed(out, p, width, Some(title), &[verdict, files]);
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
    let heading = Para::new()
        .text(behaviour.as_str(), BOLD)
        .plain(&format!("  {}", counts(group.found, group.suspected)));
    emit(out, p, width, &heading, "", "  ");
    for section in &group.rules {
        emit(
            out,
            p,
            width,
            &Para::new().plain(section.rule.as_str()),
            "  ",
            "    ",
        );
        emit(
            out,
            p,
            width,
            &Para::new().plain(&escape(&section.fix.as_str().into())),
            "    fix  ",
            "         ",
        );
        for id in &section.findings {
            if let Some(reported) = by_id.get(id) {
                finding_line(out, p, width, inputs, reported);
            }
        }
    }
}

fn finding_line(
    out: &mut String,
    p: &Painter,
    width: usize,
    inputs: &Inputs,
    reported: &ReportedFinding,
) {
    let f = &reported.finding;
    let mark = format!("    {} ", mark(f.confidence));
    let para = location(Para::new(), inputs, &f.location, true)
        .plain("  ")
        .text(f.evidence.definition.as_str(), DIM)
        .plain("  ")
        .text(reported.id.as_str(), DIM);
    emit(out, p, width, &para, &mark, "      ");
    if !f.guards.is_empty() {
        emit(
            out,
            p,
            width,
            &Para::new().value(&guards(&f.guards), PLAIN),
            "        guard ",
            "              ",
        );
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
        } => outcome_para(location(Para::new(), inputs, loc, false).glue(":"), outcome),
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
                .glue(":")
                .plain(&format!(" {text}"))
        }
        NotCheckedEntry::DynamicLoad {
            location: loc,
            symbol,
        } => location(Para::new(), inputs, loc, true)
            .glue(":")
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
