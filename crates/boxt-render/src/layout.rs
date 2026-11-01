use anstyle::Style;
use textwrap::core::Fragment;
use textwrap::wrap_algorithms::{Penalties, wrap_first_fit, wrap_optimal_fit};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

pub(crate) const PLAIN: Style = Style::new();

pub(crate) struct Painter {
    pub(crate) styled: bool,
}

impl Painter {
    pub(crate) fn paint(&self, style: Style, text: &str) -> String {
        if self.styled && style != PLAIN && !text.is_empty() {
            format!("{style}{text}{style:#}")
        } else {
            text.to_owned()
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Atom {
    parts: Vec<(Style, String)>,
    width: usize,
    gap: usize,
}

impl Atom {
    pub(crate) fn new(parts: Vec<(Style, String)>) -> Atom {
        let width = parts.iter().map(|(_, text)| text.width()).sum();
        Atom {
            parts,
            width,
            gap: 0,
        }
    }

    fn split(&self, max: usize) -> Vec<Atom> {
        let mut pieces = Vec::new();
        let mut parts: Vec<(Style, String)> = Vec::new();
        let mut width = 0;
        for (style, text) in &self.parts {
            let mut run = String::new();
            for c in text.chars() {
                let w = c.width().unwrap_or(0);
                if width + w > max && width > 0 {
                    if !run.is_empty() {
                        parts.push((*style, std::mem::take(&mut run)));
                    }
                    pieces.push(Atom::new(std::mem::take(&mut parts)));
                    width = 0;
                }
                run.push(c);
                width += w;
            }
            if !run.is_empty() {
                parts.push((*style, run));
            }
        }
        if !parts.is_empty() {
            pieces.push(Atom::new(parts));
        }
        if let Some(last) = pieces.last_mut() {
            last.gap = self.gap;
        }
        pieces
    }
}

impl Fragment for Atom {
    fn width(&self) -> f64 {
        self.width as f64
    }

    fn whitespace_width(&self) -> f64 {
        self.gap as f64
    }

    fn penalty_width(&self) -> f64 {
        0.0
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct Para {
    atoms: Vec<Atom>,
}

impl Para {
    pub(crate) fn new() -> Para {
        Para::default()
    }

    pub(crate) fn text(mut self, text: &str, style: Style) -> Para {
        for (i, word) in text.split(' ').enumerate() {
            if i > 0
                && let Some(last) = self.atoms.last_mut()
            {
                last.gap += 1;
            }
            if !word.is_empty() {
                self.atoms.push(Atom::new(vec![(style, word.to_owned())]));
            }
        }
        self
    }

    pub(crate) fn value(mut self, text: &str, style: Style) -> Para {
        for piece in text.split_inclusive(' ') {
            self.atoms.push(Atom::new(vec![(style, piece.to_owned())]));
        }
        self
    }

    pub(crate) fn glue(mut self, text: &str) -> Para {
        match self.atoms.last_mut() {
            Some(last) if last.gap == 0 => {
                last.width += text.width();
                last.parts.push((PLAIN, text.to_owned()));
            }
            _ => self.atoms.push(Atom::new(vec![(PLAIN, text.to_owned())])),
        }
        self
    }

    pub(crate) fn plain(self, text: &str) -> Para {
        self.text(text, PLAIN)
    }

    pub(crate) fn atom(mut self, atom: Atom) -> Para {
        self.atoms.push(atom);
        self
    }

    fn wrap(&self, first: usize, rest: usize) -> Vec<Vec<Atom>> {
        let max = first.min(rest).max(1);
        let atoms: Vec<Atom> = self
            .atoms
            .iter()
            .flat_map(|a| {
                if a.width > max {
                    a.split(max)
                } else {
                    vec![a.clone()]
                }
            })
            .collect();
        if atoms.is_empty() {
            return vec![Vec::new()];
        }
        let limits = [first.max(1), rest.max(1)];
        let widths = limits.map(|w| w as f64);
        let fits = |lines: &Vec<&[Atom]>| {
            lines.iter().enumerate().all(|(i, atoms)| {
                let used: usize = atoms.iter().map(|a| a.width).sum::<usize>()
                    + atoms[..atoms.len().saturating_sub(1)]
                        .iter()
                        .map(|a| a.gap)
                        .sum::<usize>();
                used <= limits[i.min(1)]
            })
        };
        let lines = wrap_optimal_fit(&atoms, &widths, &Penalties::new())
            .ok()
            .filter(fits)
            .unwrap_or_else(|| wrap_first_fit(&atoms, &widths));
        lines.into_iter().map(<[Atom]>::to_vec).collect()
    }
}

fn line(p: &Painter, atoms: &[Atom]) -> (String, usize) {
    let mut out = String::new();
    let mut width = 0;
    for (i, atom) in atoms.iter().enumerate() {
        for (style, text) in &atom.parts {
            out.push_str(&p.paint(*style, text));
        }
        width += atom.width;
        if i + 1 < atoms.len() {
            out.push_str(&" ".repeat(atom.gap));
            width += atom.gap;
        }
    }
    (out, width)
}

pub(crate) fn emit(
    out: &mut String,
    p: &Painter,
    width: usize,
    para: &Para,
    initial: &str,
    hanging: &str,
) {
    let first = width.saturating_sub(initial.width());
    let rest = width.saturating_sub(hanging.width());
    for (i, atoms) in para.wrap(first, rest).iter().enumerate() {
        out.push_str(if i == 0 { initial } else { hanging });
        out.push_str(&line(p, atoms).0);
        out.push('\n');
    }
}

pub(crate) fn boxed(
    out: &mut String,
    p: &Painter,
    width: usize,
    title: Option<Atom>,
    paras: &[Para],
) {
    let inner = width.saturating_sub(4);
    let mut heading = None;
    match title {
        Some(title) if title.width + 5 <= width => {
            let (text, w) = line(p, std::slice::from_ref(&title));
            out.push_str("┌ ");
            out.push_str(&text);
            out.push(' ');
            out.push_str(&"─".repeat(width - 4 - w));
            out.push_str("┐\n");
        }
        other => {
            out.push('┌');
            out.push_str(&"─".repeat(width.saturating_sub(2)));
            out.push_str("┐\n");
            heading = other.map(|t| Para::new().atom(t));
        }
    }
    for para in heading.iter().chain(paras) {
        for atoms in para.wrap(inner, inner) {
            let (text, w) = line(p, &atoms);
            out.push_str("│ ");
            out.push_str(&text);
            out.push_str(&" ".repeat(inner.saturating_sub(w)));
            out.push_str(" │\n");
        }
    }
    out.push('└');
    out.push_str(&"─".repeat(width.saturating_sub(2)));
    out.push_str("┘\n");
}
