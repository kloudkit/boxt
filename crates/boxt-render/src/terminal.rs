use std::fmt::Write;

use anstyle::{AnsiColor, Style};
use boxt_core::fact::GuardChain;
use boxt_core::labels;
use boxt_core::location::{Location, Scope};
use boxt_core::outcome::{Count, Details, FileOutcome, Outcome, Sm3};
use boxt_core::settings::Encoding;
use boxt_core::verdict::{NotCheckedEntry, ReportedFinding, Result, TargetVerdict, Tier};

use crate::escape::escape;

struct Painter {
    styled: bool,
}

impl Painter {
    fn paint(&self, style: Style, text: &str) -> String {
        if self.styled {
            format!("{style}{text}{style:#}")
        } else {
            text.to_owned()
        }
    }

    fn heading(&self, text: &str) -> String {
        self.paint(Style::new().bold(), text)
    }

    fn id(&self, text: &str) -> String {
        self.paint(Style::new().fg_color(Some(AnsiColor::Cyan.into())), text)
    }

    fn tier(&self, tier: Tier) -> String {
        let color = match tier {
            Tier::Blocked => AnsiColor::Red,
            Tier::NeedsWindows | Tier::NeedsCodeChange => AnsiColor::Yellow,
            Tier::ReadyWithConfig => AnsiColor::Cyan,
            Tier::Ready => AnsiColor::Green,
        };
        self.paint(
            Style::new().bold().fg_color(Some(color.into())),
            labels::tier(tier),
        )
    }
}

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
                | NotCheckedEntry::Region { location, .. }
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

    fn prefix(&self, index: u32) -> String {
        if self.multi {
            format!("{}: ", self.name(index))
        } else {
            String::new()
        }
    }
}

pub fn terminal(result: &Result, styled: bool) -> String {
    let p = Painter { styled };
    let inputs = Inputs::of(result);
    let mut out = String::new();
    out.push_str(&result.input_scope_text);
    out.push('\n');
    if result.host_dependent {
        out.push_str(labels::HOST_DEPENDENT_TEXT);
        out.push('\n');
    }
    out.push('\n');
    for target in result.targets.iter() {
        verdict_line(&mut out, &p, result, target);
    }
    out.push('\n');
    out.push_str(&p.heading("Findings"));
    out.push('\n');
    if result.findings.is_empty() {
        out.push_str("  none\n");
    }
    for finding in result.findings.iter() {
        finding_block(&mut out, &p, &inputs, finding);
    }
    out.push('\n');
    out.push_str(&p.heading("Not checked"));
    out.push('\n');
    let listed = result.targets.iter().any(|t| !t.unmapped.is_empty())
        || !result.not_checked.is_empty()
        || !result.macro_tables_cut.is_empty();
    if !listed {
        out.push_str("  none\n");
    }
    for target in result.targets.iter() {
        for behaviour in target.unmapped.iter() {
            let _ = writeln!(
                out,
                "  {}: unmapped behaviour {behaviour}",
                p.id(target.target.as_str())
            );
        }
    }
    for entry in result.not_checked.iter() {
        not_checked_line(&mut out, &inputs, entry);
    }
    for tree in result.macro_tables_cut.iter() {
        let _ = writeln!(
            out,
            "  {}: {}",
            inputs.name(*tree),
            labels::macro_table_cut()
        );
    }
    sm3_line(&mut out, &inputs, &result.sm3);
    out
}

fn verdict_line(out: &mut String, p: &Painter, result: &Result, target: &TargetVerdict) {
    let range = if target.range.found == target.range.with_suspected {
        p.tier(target.range.found)
    } else {
        format!(
            "{} … {}",
            p.tier(target.range.found),
            p.tier(target.range.with_suspected)
        )
    };
    let _ = writeln!(
        out,
        "{}  {range}  ({} found, {} suspected)  {} files read, {} not checked",
        p.id(target.target.as_str()),
        target.found,
        target.suspected,
        result.counts.read,
        result.counts.not_checked,
    );
}

fn finding_block(out: &mut String, p: &Painter, inputs: &Inputs, reported: &ReportedFinding) {
    let f = &reported.finding;
    let _ = writeln!(
        out,
        "  {}  {}  {}  {}  ({})",
        p.id(reported.id.as_str()),
        f.rule,
        f.behaviour,
        labels::confidence(f.confidence),
        f.evidence.definition,
    );
    let _ = writeln!(out, "      {}", location(inputs, &f.location, true));
    if !f.guards.is_empty() {
        let _ = writeln!(out, "      guard: {}", guards(&f.guards));
    }
    let _ = writeln!(out, "      fix: {}", escape(&f.fix.as_str().into()));
}

fn location(inputs: &Inputs, loc: &Location, with_line: bool) -> String {
    let mut parts: Vec<String> = loc.frames.iter().map(|f| escape(&f.name)).collect();
    if let Some(path) = &loc.path {
        parts.push(escape(path.as_text()));
    }
    let mut out = inputs.prefix(loc.input.index);
    out.push_str(&parts.join("!"));
    if with_line && let Some(line) = loc.line {
        let _ = write!(out, ":{line}");
    }
    match &loc.scope {
        Scope::File => {}
        Scope::Function {
            qualified_name,
            signature,
        } => {
            let _ = write!(out, "  in {}{}", escape(qualified_name), escape(signature));
        }
        Scope::Symbol(symbol) => {
            let _ = write!(out, "  symbol {}", escape(symbol));
        }
        Scope::UnitSection { section, key } => {
            let _ = write!(out, "  [{}] {}", escape(section), escape(key));
        }
        Scope::ConfigKey(key) => {
            let _ = write!(out, "  key {}", escape(key));
        }
        Scope::Script(name) => {
            let _ = write!(out, "  script {}", name.name());
        }
    }
    out
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

fn not_checked_line(out: &mut String, inputs: &Inputs, entry: &NotCheckedEntry) {
    match entry {
        NotCheckedEntry::File {
            location: loc,
            outcome,
        } => {
            let _ = writeln!(
                out,
                "  {}: {}",
                location(inputs, loc, false),
                outcome_text(outcome)
            );
        }
        NotCheckedEntry::Region {
            location: loc,
            first_line,
            last_line,
            reason,
        } => {
            let lines = if first_line == last_line {
                format!("line {first_line}")
            } else {
                format!("lines {first_line}–{last_line}")
            };
            let _ = writeln!(
                out,
                "  {} {lines}: {}",
                location(inputs, loc, false),
                labels::region_reason(*reason)
            );
        }
        NotCheckedEntry::DynamicLoad {
            location: loc,
            symbol,
        } => {
            let _ = writeln!(
                out,
                "  {}: loads code at run time through {}",
                location(inputs, loc, true),
                escape(symbol)
            );
        }
    }
}

fn outcome_text(outcome: &Outcome) -> String {
    let mut text = match &outcome.file {
        FileOutcome::NotChecked { reason, details } => {
            let mut text = labels::not_checked_reason(*reason);
            text.push_str(&details_text(details));
            text
        }
        FileOutcome::Aggregate {
            reason,
            count,
            at_least,
        } => {
            let count = match count {
                Some(n) => Count::Exact(*n),
                None => Count::AtLeast((*at_least).max(1)),
            };
            format!(
                "{}, {count} files from here on",
                labels::not_checked_reason(*reason)
            )
        }
        FileOutcome::Read => String::new(),
        FileOutcome::OutOfScope { cause } => labels::out_of_scope_cause(cause).to_owned(),
    };
    if outcome.host_dependent {
        text.push_str("; ");
        text.push_str(labels::HOST_DEPENDENT_MARK);
    }
    text
}

fn details_text(details: &Details) -> String {
    match details {
        Details::None => String::new(),
        Details::Measured { value, limit } => format!(" ({value} > {limit})"),
        Details::Encoding { declared } => format!(" (declared {})", encoding(*declared)),
        Details::Interpreter { interpreter } => format!(" ({})", escape(interpreter)),
        Details::EntryName { name } => format!(" ({})", escape(name)),
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

fn sm3_line(out: &mut String, inputs: &Inputs, sm3: &Sm3) {
    out.push_str("  Source lines not checked: ");
    match sm3.lines {
        None => out.push_str("n/a"),
        Some(lines) => {
            let at_least = if lines.is_lower_bound() {
                "at least "
            } else {
                ""
            };
            if lines.denominator == 0 {
                let _ = write!(out, "{at_least}{} of 0", lines.numerator);
            } else {
                let _ = write!(
                    out,
                    "{at_least}{} ({} of {})",
                    percent(lines.numerator, lines.denominator),
                    lines.numerator,
                    lines.denominator
                );
            }
            if lines.uncounted_files > 0 {
                let _ = write!(out, ", {} files uncounted", lines.uncounted_files);
            }
            if lines.incomplete_files > 0 {
                let _ = write!(out, ", {} files counted in part", lines.incomplete_files);
            }
        }
    }
    match sm3.files {
        None => out.push_str("; files: n/a\n"),
        Some(files) => {
            let _ = writeln!(out, "; files: {} of {}", files.numerator, files.denominator);
        }
    }
    for (label, keys) in [
        ("uncounted", &sm3.uncounted),
        ("counted in part", &sm3.incomplete),
    ] {
        for key in keys.iter() {
            let _ = writeln!(
                out,
                "    {label}: {}{}",
                inputs.prefix(key.input_index),
                escape(key.job_path.as_text())
            );
        }
    }
}
