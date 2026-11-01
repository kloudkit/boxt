use boxt_render::boxt_core::Text;
use boxt_render::escape;

fn esc(s: &str) -> String {
    escape(&Text::from(s))
}

#[test]
fn c0_controls_tab_and_newline_are_visible() {
    assert_eq!(esc("a\tb\nc\rd\0e\x1b"), "a\\x09b\\x0ac\\x0dd\\x00e\\x1b");
    for b in 0u8..0x20 {
        let s = char::from(b).to_string();
        assert_eq!(esc(&s), format!("\\x{b:02x}"));
    }
}

#[test]
fn del_and_c1_controls_are_visible() {
    assert_eq!(esc("\x7f"), "\\x7f");
    assert_eq!(esc("\u{80}"), "\\u{0080}");
    assert_eq!(esc("\u{9b}"), "\\u{009b}");
    assert_eq!(esc("\u{9f}"), "\\u{009f}");
    assert_eq!(esc("\u{a0}"), "\u{a0}");
}

#[test]
fn bidi_controls_are_visible() {
    for c in [
        '\u{61c}', '\u{200e}', '\u{200f}', '\u{202a}', '\u{202b}', '\u{202c}', '\u{202d}',
        '\u{202e}', '\u{2066}', '\u{2067}', '\u{2068}', '\u{2069}',
    ] {
        assert_eq!(esc(&c.to_string()), format!("\\u{{{:04x}}}", c as u32));
    }
}

#[test]
fn zero_width_characters_are_visible() {
    for c in ['\u{200b}', '\u{200c}', '\u{200d}', '\u{2060}', '\u{feff}'] {
        assert_eq!(esc(&c.to_string()), format!("\\u{{{:04x}}}", c as u32));
    }
}

#[test]
fn undecodable_bytes_are_hex() {
    let text = Text::from_bytes(vec![b'a', 0xff, 0xc3, b'b', 0xed, 0xa0, 0x80]);
    assert_eq!(escape(&text), "a\\xff\\xc3b\\xed\\xa0\\x80");
}

#[test]
fn a_backslash_is_doubled() {
    assert_eq!(esc("C:\\x41"), "C:\\\\x41");
}

#[test]
fn everything_else_prints_as_is() {
    let s = "main.c ünï 日本 😀 drop_to(const char *, uid_t)";
    assert_eq!(esc(s), s);
}

#[test]
fn a_hostile_literal_loses_its_escape_bytes() {
    let literal = "\x1b]8;;https://example.invalid\x07click\x1b]8;;\x07 \x1b[31mred";
    let out = esc(literal);
    assert_eq!(
        out,
        "\\x1b]8;;https://example.invalid\\x07click\\x1b]8;;\\x07 \\x1b[31mred"
    );
    assert!(!out.bytes().any(|b| b == 0x1b || b == 0x07));
}

#[test]
fn format_characters_and_separators_are_visible() {
    for (c, shown) in [
        ('\u{2028}', "\\u{2028}"),
        ('\u{2029}', "\\u{2029}"),
        ('\u{ad}', "\\u{00ad}"),
        ('\u{2061}', "\\u{2061}"),
        ('\u{e0001}', "\\u{e0001}"),
        ('\u{600}', "\\u{0600}"),
        ('\u{e007f}', "\\u{e007f}"),
    ] {
        assert_eq!(esc(&c.to_string()), shown);
    }
}

#[test]
fn a_variation_selector_is_not_a_format_character() {
    assert_eq!(esc("\u{2764}\u{fe0f}"), "\u{2764}\u{fe0f}");
}
