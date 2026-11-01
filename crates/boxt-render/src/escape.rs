use std::fmt::Write;

use boxt_core::Text;

pub fn escape(text: &Text) -> String {
    let mut out = String::with_capacity(text.as_bytes().len());
    for chunk in text.as_bytes().utf8_chunks() {
        for c in chunk.valid().chars() {
            push_char(&mut out, c);
        }
        for &b in chunk.invalid() {
            let _ = write!(out, "\\x{b:02x}");
        }
    }
    out
}

fn push_char(out: &mut String, c: char) {
    match c {
        '\\' => out.push_str("\\\\"),
        '\0'..='\x1f' | '\x7f' => {
            let _ = write!(out, "\\x{:02x}", c as u32);
        }
        c if is_hidden(c) => {
            let _ = write!(out, "\\u{{{:04x}}}", c as u32);
        }
        c => out.push(c),
    }
}

// C1 controls, the Unicode 15.1 format characters (Cf), and U+2028, U+2029 (Zl, Zp).
const HIDDEN: &[(char, char)] = &[
    ('\u{80}', '\u{9f}'),
    ('\u{ad}', '\u{ad}'),
    ('\u{600}', '\u{605}'),
    ('\u{61c}', '\u{61c}'),
    ('\u{6dd}', '\u{6dd}'),
    ('\u{70f}', '\u{70f}'),
    ('\u{890}', '\u{891}'),
    ('\u{8e2}', '\u{8e2}'),
    ('\u{180e}', '\u{180e}'),
    ('\u{200b}', '\u{200f}'),
    ('\u{2028}', '\u{202e}'),
    ('\u{2060}', '\u{2064}'),
    ('\u{2066}', '\u{206f}'),
    ('\u{feff}', '\u{feff}'),
    ('\u{fff9}', '\u{fffb}'),
    ('\u{110bd}', '\u{110bd}'),
    ('\u{110cd}', '\u{110cd}'),
    ('\u{13430}', '\u{1343f}'),
    ('\u{1bca0}', '\u{1bca3}'),
    ('\u{1d173}', '\u{1d17a}'),
    ('\u{e0001}', '\u{e0001}'),
    ('\u{e0020}', '\u{e007f}'),
];

fn is_hidden(c: char) -> bool {
    HIDDEN
        .iter()
        .any(|&(first, last)| (first..=last).contains(&c))
}
