use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::text::Text;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ShellCommand {
    pub line: u32,
    pub assignments: Vec<Assignment>,
    pub argv: Vec<Text>,
    pub redirections: Vec<Redirection>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Assignment {
    pub name: Text,
    pub append: bool,
    pub value: Text,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Redirection {
    pub fd: Option<u32>,
    pub op: RedirectOp,
    pub target: Text,
}

closed_enum! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub enum RedirectOp {
        Write,
        Append,
        Clobber,
        Read,
        ReadWrite,
        DupOut,
        DupIn,
        WriteBoth,
        AppendBoth,
        HereDoc,
        HereString,
    }
}

impl RedirectOp {
    pub const fn as_str(self) -> &'static str {
        match self {
            RedirectOp::Write => ">",
            RedirectOp::Append => ">>",
            RedirectOp::Clobber => ">|",
            RedirectOp::Read => "<",
            RedirectOp::ReadWrite => "<>",
            RedirectOp::DupOut => ">&",
            RedirectOp::DupIn => "<&",
            RedirectOp::WriteBoth => "&>",
            RedirectOp::AppendBoth => "&>>",
            RedirectOp::HereDoc => "<<",
            RedirectOp::HereString => "<<<",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LexError {
    pub line: u32,
    pub kind: LexErrorKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LexErrorKind {
    UnterminatedQuote,
    UnterminatedHeredoc,
    UnterminatedSubstitution,
    MissingRedirectTarget,
}

impl fmt::Display for LexError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let what = match self.kind {
            LexErrorKind::UnterminatedQuote => "unterminated quote",
            LexErrorKind::UnterminatedHeredoc => "unterminated here-document",
            LexErrorKind::UnterminatedSubstitution => "unterminated substitution",
            LexErrorKind::MissingRedirectTarget => "redirection without a target",
        };
        write!(f, "{what} starting on line {}", self.line)
    }
}

impl std::error::Error for LexError {}

pub fn lex(text: &Text) -> Result<Vec<ShellCommand>, LexError> {
    let tokens = Tokenizer::new(text.as_bytes()).run()?;
    Parser::default().run(&tokens)
}

pub const SHELL_INTERPRETERS: &[&str] = &["sh", "bash", "dash", "ash", "ksh", "mksh"];

pub fn is_shell_interpreter(interpreter: Option<&Text>) -> bool {
    let Some(interpreter) = interpreter else {
        return true;
    };
    let mut words = interpreter
        .as_bytes()
        .split(|&c| c == b' ' || c == b'\t')
        .filter(|w| !w.is_empty());
    let base = |w: &[u8]| w.rsplit(|&c| c == b'/').next().unwrap_or(w).to_vec();
    let Some(first) = words.next().map(base) else {
        return false;
    };
    let program = if first == b"env" {
        match words.find(|w| !w.starts_with(b"-")) {
            Some(w) => base(w),
            None => return false,
        }
    } else {
        first
    };
    SHELL_INTERPRETERS.iter().any(|s| s.as_bytes() == program)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    Newline,
    Semi,
    DoubleSemi,
    SemiAmp,
    DoubleSemiAmp,
    And,
    Or,
    Pipe,
    PipeAmp,
    Amp,
    LParen,
    RParen,
    Arith,
}

impl Op {
    fn as_bytes(self) -> &'static [u8] {
        match self {
            Op::Newline => b"\n",
            Op::Semi => b";",
            Op::DoubleSemi => b";;",
            Op::SemiAmp => b";&",
            Op::DoubleSemiAmp => b";;&",
            Op::And => b"&&",
            Op::Or => b"||",
            Op::Pipe => b"|",
            Op::PipeAmp => b"|&",
            Op::Amp => b"&",
            Op::LParen => b"(",
            Op::RParen => b")",
            Op::Arith => b"((",
        }
    }
}

#[derive(Debug, Clone)]
struct Word {
    bytes: Vec<u8>,
    quoted: bool,
    literal: usize,
    line: u32,
}

impl Word {
    fn is(&self, reserved: &[u8]) -> bool {
        !self.quoted && self.bytes == reserved
    }

    fn assignment(&self) -> Option<Assignment> {
        let eq = self.bytes.iter().position(|&c| c == b'=')?;
        let (name, append) = split_assignment_name(&self.bytes[..eq])?;
        (eq < self.literal).then(|| Assignment {
            name: Text::from_bytes(name.to_vec()),
            append,
            value: Text::from_bytes(self.bytes[eq + 1..].to_vec()),
        })
    }
}

#[derive(Debug, Clone)]
enum Tok {
    Word(Word),
    Op(Op),
    Redirect {
        fd: Option<u32>,
        op: RedirectOp,
        line: u32,
    },
}

const RESERVED: &[&[u8]] = &[
    b"if", b"then", b"else", b"elif", b"fi", b"do", b"done", b"while", b"until", b"!", b"{", b"}",
];

struct PendingHeredoc {
    delimiter: Vec<u8>,
    strip_tabs: bool,
    line: u32,
}

struct Tokenizer<'a> {
    b: &'a [u8],
    i: usize,
    line: u32,
    tokens: Vec<Tok>,
    heredocs: Vec<PendingHeredoc>,
    delimiter_next: Option<(bool, u32)>,
    closes: BTreeMap<usize, usize>,
}

fn error(line: u32, kind: LexErrorKind) -> LexError {
    LexError { line, kind }
}

fn split_assignment_name(before_eq: &[u8]) -> Option<(&[u8], bool)> {
    let (name, append) = match before_eq.strip_suffix(b"+") {
        Some(name) => (name, true),
        None => (before_eq, false),
    };
    let valid = name
        .first()
        .is_some_and(|&c| c.is_ascii_alphabetic() || c == b'_')
        && name.iter().all(|&c| c.is_ascii_alphanumeric() || c == b'_');
    valid.then_some((name, append))
}

fn is_assignment_prefix(bytes: &[u8]) -> bool {
    bytes
        .strip_suffix(b"=")
        .is_some_and(|before| split_assignment_name(before).is_some())
}

fn is_blank(c: u8) -> bool {
    matches!(c, b' ' | b'\t')
}

fn ends_word(c: u8) -> bool {
    is_blank(c) || matches!(c, b'\n' | b';' | b'&' | b'|' | b'(' | b')' | b'<' | b'>')
}

impl<'a> Tokenizer<'a> {
    fn new(b: &'a [u8]) -> Self {
        Tokenizer {
            b,
            i: 0,
            line: 1,
            tokens: Vec::new(),
            heredocs: Vec::new(),
            delimiter_next: None,
            closes: BTreeMap::new(),
        }
    }

    fn peek(&self, off: usize) -> Option<u8> {
        self.b.get(self.i + off).copied()
    }

    fn bump(&mut self) -> Option<u8> {
        let c = self.peek(0)?;
        self.i += 1;
        if c == b'\n' {
            self.line += 1;
        }
        Some(c)
    }

    fn skip_continuation(&mut self) -> bool {
        let len = match (self.peek(0), self.peek(1), self.peek(2)) {
            (Some(b'\\'), Some(b'\n'), _) => 2,
            (Some(b'\\'), Some(b'\r'), Some(b'\n')) => 3,
            _ => return false,
        };
        self.i += len;
        self.line += 1;
        true
    }

    fn word_end(&self, off: usize) -> bool {
        match self.peek(off) {
            None => true,
            Some(b'\r') => self.peek(off + 1) == Some(b'\n'),
            Some(c) => ends_word(c),
        }
    }

    fn at_word_end(&self) -> bool {
        match self.peek(0) {
            Some(b'<' | b'>') => self.peek(1) != Some(b'('),
            _ => self.word_end(0),
        }
    }

    fn starts_with(&self, s: &[u8]) -> bool {
        self.b[self.i..].starts_with(s)
    }

    fn op(&mut self, op: Op) {
        self.i += op.as_bytes().len();
        self.delimiter_next = None;
        self.tokens.push(Tok::Op(op));
    }

    fn run(mut self) -> Result<Vec<Tok>, LexError> {
        while let Some(c) = self.peek(0) {
            if self.skip_continuation() {
                continue;
            }
            match c {
                _ if is_blank(c) => self.i += 1,
                b'\r' if self.peek(1) == Some(b'\n') => self.i += 1,
                b'\n' => {
                    self.op(Op::Newline);
                    self.line += 1;
                    self.read_heredoc_bodies()?;
                }
                b'#' => {
                    while self.peek(0).is_some_and(|c| c != b'\n') {
                        self.i += 1;
                    }
                }
                b';' => {
                    let op = if self.starts_with(b";;&") {
                        Op::DoubleSemiAmp
                    } else if self.starts_with(b";;") {
                        Op::DoubleSemi
                    } else if self.starts_with(b";&") {
                        Op::SemiAmp
                    } else {
                        Op::Semi
                    };
                    self.op(op);
                }
                b'&' => {
                    if self.starts_with(b"&&") {
                        self.op(Op::And);
                    } else if self.starts_with(b"&>>") {
                        self.redirect(None, RedirectOp::AppendBoth, 3);
                    } else if self.starts_with(b"&>") {
                        self.redirect(None, RedirectOp::WriteBoth, 2);
                    } else {
                        self.op(Op::Amp);
                    }
                }
                b'|' => {
                    let op = match self.peek(1) {
                        Some(b'|') => Op::Or,
                        Some(b'&') => Op::PipeAmp,
                        _ => Op::Pipe,
                    };
                    self.op(op);
                }
                b'(' if self.peek(1) == Some(b'(') => {
                    let at = self.i;
                    if !self.closes.contains_key(&at) {
                        let line = self.line;
                        self.copy_paren(&mut Vec::new())?;
                        (self.i, self.line) = (at, line);
                    }
                    match (self.closes.get(&at), self.closes.get(&(at + 1))) {
                        (Some(&outer), Some(&inner)) if inner + 1 == outer => {
                            let newlines = self.b[at..=outer].iter().filter(|&&c| c == b'\n');
                            self.line += newlines.count() as u32;
                            self.i = outer + 1;
                            self.delimiter_next = None;
                            self.tokens.push(Tok::Op(Op::Arith));
                        }
                        _ => self.op(Op::LParen),
                    }
                }
                b'(' => self.op(Op::LParen),
                b')' => self.op(Op::RParen),
                b'<' | b'>' if self.peek(1) != Some(b'(') => self.operator(None),
                _ => {
                    let word = self.word()?;
                    let fd_word = !word.quoted
                        && !word.bytes.is_empty()
                        && word.bytes.iter().all(u8::is_ascii_digit);
                    if fd_word && matches!(self.peek(0), Some(b'<' | b'>')) {
                        let fd = std::str::from_utf8(&word.bytes)
                            .ok()
                            .and_then(|s| s.parse().ok());
                        self.operator(fd);
                    } else {
                        self.push_word(word);
                    }
                }
            }
        }
        if let Some(pending) = self.heredocs.first() {
            return Err(error(pending.line, LexErrorKind::UnterminatedHeredoc));
        }
        Ok(self.tokens)
    }

    fn push_word(&mut self, word: Word) {
        if let Some((strip_tabs, line)) = self.delimiter_next.take() {
            self.heredocs.push(PendingHeredoc {
                delimiter: word.bytes.clone(),
                strip_tabs,
                line,
            });
        }
        self.tokens.push(Tok::Word(word));
    }

    fn redirect(&mut self, fd: Option<u32>, op: RedirectOp, len: usize) {
        let line = self.line;
        self.i += len;
        self.delimiter_next = None;
        self.tokens.push(Tok::Redirect { fd, op, line });
    }

    fn operator(&mut self, fd: Option<u32>) {
        let line = self.line;
        let (op, len) = match (self.peek(0), self.peek(1), self.peek(2)) {
            (Some(b'>'), Some(b'>'), _) => (RedirectOp::Append, 2),
            (Some(b'>'), Some(b'|'), _) => (RedirectOp::Clobber, 2),
            (Some(b'>'), Some(b'&'), _) => (RedirectOp::DupOut, 2),
            (Some(b'>'), _, _) => (RedirectOp::Write, 1),
            (Some(b'<'), Some(b'<'), Some(b'<')) => (RedirectOp::HereString, 3),
            (Some(b'<'), Some(b'<'), Some(b'-')) => (RedirectOp::HereDoc, 3),
            (Some(b'<'), Some(b'<'), _) => (RedirectOp::HereDoc, 2),
            (Some(b'<'), Some(b'>'), _) => (RedirectOp::ReadWrite, 2),
            (Some(b'<'), Some(b'&'), _) => (RedirectOp::DupIn, 2),
            _ => (RedirectOp::Read, 1),
        };
        self.redirect(fd, op, len);
        if op == RedirectOp::HereDoc {
            self.delimiter_next = Some((len == 3, line));
        }
    }

    fn read_heredoc_bodies(&mut self) -> Result<(), LexError> {
        for pending in std::mem::take(&mut self.heredocs) {
            self.heredoc_body(&pending, None)?;
        }
        Ok(())
    }

    fn heredoc_body(
        &mut self,
        pending: &PendingHeredoc,
        mut out: Option<&mut Vec<u8>>,
    ) -> Result<(), LexError> {
        loop {
            if self.i >= self.b.len() {
                return Err(error(pending.line, LexErrorKind::UnterminatedHeredoc));
            }
            let end = self.b[self.i..]
                .iter()
                .position(|&c| c == b'\n')
                .map_or(self.b.len(), |p| self.i + p);
            let next = (end + 1).min(self.b.len());
            if let Some(out) = out.as_deref_mut() {
                out.extend_from_slice(&self.b[self.i..next]);
            }
            let mut body = &self.b[self.i..end];
            if pending.strip_tabs {
                while let [b'\t', rest @ ..] = body {
                    body = rest;
                }
            }
            let body = body.strip_suffix(b"\r").unwrap_or(body);
            let done = body == pending.delimiter.as_slice();
            if end < self.b.len() {
                self.line += 1;
            }
            self.i = next;
            if done {
                return Ok(());
            }
        }
    }

    fn copy_heredoc_operator(&mut self, out: &mut Vec<u8>) -> Option<PendingHeredoc> {
        let line = self.line;
        let strip_tabs = self.starts_with(b"<<-");
        let len = if strip_tabs { 3 } else { 2 };
        out.extend_from_slice(&self.b[self.i..self.i + len]);
        self.i += len;
        while let Some(c) = self.peek(0).filter(|&c| is_blank(c)) {
            out.push(c);
            self.i += 1;
        }
        let mut delimiter = Vec::new();
        while !self.word_end(0) {
            let Some(c) = self.peek(0) else { break };
            out.push(c);
            self.bump();
            match c {
                b'\'' | b'"' => {
                    while let Some(q) = self.bump() {
                        out.push(q);
                        if q == c {
                            break;
                        }
                        delimiter.push(q);
                    }
                }
                b'\\' => {
                    if let Some(next) = self.bump() {
                        out.push(next);
                        delimiter.push(next);
                    }
                }
                _ => delimiter.push(c),
            }
        }
        (!delimiter.is_empty()).then_some(PendingHeredoc {
            delimiter,
            strip_tabs,
            line,
        })
    }

    fn copy_heredoc_bodies(
        &mut self,
        out: &mut Vec<u8>,
        pending: &mut Vec<PendingHeredoc>,
    ) -> Result<(), LexError> {
        for heredoc in std::mem::take(pending) {
            self.heredoc_body(&heredoc, Some(out))?;
        }
        Ok(())
    }

    fn word(&mut self) -> Result<Word, LexError> {
        let mut w = Word {
            bytes: Vec::new(),
            quoted: false,
            literal: 0,
            line: self.line,
        };
        let mut literal_open = true;
        loop {
            if self.skip_continuation() {
                continue;
            }
            if self.peek(0) == Some(b'(') && literal_open && is_assignment_prefix(&w.bytes) {
                literal_open = false;
                self.copy_paren(&mut w.bytes)?;
                continue;
            }
            if self.at_word_end() {
                break;
            }
            let Some(c) = self.peek(0) else { break };
            let special = match c {
                b'\\' | b'\'' | b'"' | b'`' | b'<' | b'>' => true,
                b'$' => matches!(self.peek(1), Some(b'\'' | b'"' | b'(' | b'{')),
                _ => false,
            };
            if special {
                literal_open = false;
            }
            match c {
                b'\\' => {
                    w.quoted = true;
                    self.i += 1;
                    w.bytes.push(self.bump().unwrap_or(b'\\'));
                }
                b'\'' => {
                    w.quoted = true;
                    self.read_single(&mut w.bytes)?;
                }
                b'"' => {
                    w.quoted = true;
                    self.read_double(&mut w.bytes)?;
                }
                b'$' if self.peek(1) == Some(b'\'') => {
                    w.quoted = true;
                    self.read_ansi_c(&mut w.bytes)?;
                }
                b'$' if self.peek(1) == Some(b'"') => {
                    w.quoted = true;
                    self.i += 1;
                    self.read_double(&mut w.bytes)?;
                }
                b'$' if self.peek(1) == Some(b'(') => {
                    w.bytes.push(b'$');
                    self.i += 1;
                    self.copy_paren(&mut w.bytes)?;
                }
                b'$' if self.peek(1) == Some(b'{') => {
                    w.bytes.push(b'$');
                    self.i += 1;
                    self.copy_brace(&mut w.bytes, false)?;
                }
                b'`' => self.copy_backtick(&mut w.bytes)?,
                b'<' | b'>' => {
                    w.bytes.push(c);
                    self.i += 1;
                    self.copy_paren(&mut w.bytes)?;
                }
                _ => {
                    w.bytes.push(c);
                    self.i += 1;
                }
            }
            if literal_open {
                w.literal = w.bytes.len();
            }
        }
        Ok(w)
    }

    fn read_single(&mut self, out: &mut Vec<u8>) -> Result<(), LexError> {
        let start = self.line;
        self.i += 1;
        loop {
            match self.bump() {
                None => return Err(error(start, LexErrorKind::UnterminatedQuote)),
                Some(b'\'') => return Ok(()),
                Some(c) => out.push(c),
            }
        }
    }

    fn read_double(&mut self, out: &mut Vec<u8>) -> Result<(), LexError> {
        let start = self.line;
        self.i += 1;
        loop {
            if self.skip_continuation() {
                continue;
            }
            let Some(c) = self.peek(0) else {
                return Err(error(start, LexErrorKind::UnterminatedQuote));
            };
            match c {
                b'"' => {
                    self.i += 1;
                    return Ok(());
                }
                b'\\' => match self.peek(1) {
                    Some(next @ (b'$' | b'`' | b'"' | b'\\')) => {
                        out.push(next);
                        self.i += 2;
                    }
                    _ => {
                        out.push(b'\\');
                        self.i += 1;
                    }
                },
                b'$' if self.peek(1) == Some(b'(') => {
                    out.push(b'$');
                    self.i += 1;
                    self.copy_paren(out)?;
                }
                b'$' if self.peek(1) == Some(b'{') => {
                    out.push(b'$');
                    self.i += 1;
                    self.copy_brace(out, true)?;
                }
                b'`' => self.copy_backtick(out)?,
                _ => {
                    out.push(c);
                    self.bump();
                }
            }
        }
    }

    fn read_ansi_c(&mut self, out: &mut Vec<u8>) -> Result<(), LexError> {
        let start = self.line;
        self.i += 2;
        loop {
            let Some(c) = self.bump() else {
                return Err(error(start, LexErrorKind::UnterminatedQuote));
            };
            match c {
                b'\'' => return Ok(()),
                b'\\' => {
                    let Some(e) = self.bump() else {
                        return Err(error(start, LexErrorKind::UnterminatedQuote));
                    };
                    match e {
                        b'n' => out.push(b'\n'),
                        b't' => out.push(b'\t'),
                        b'r' => out.push(b'\r'),
                        b'a' => out.push(0x07),
                        b'b' => out.push(0x08),
                        b'e' | b'E' => out.push(0x1b),
                        b'f' => out.push(0x0c),
                        b'v' => out.push(0x0b),
                        b'\\' | b'\'' | b'"' | b'?' => out.push(e),
                        b'x' => {
                            let value = self.digits(2, 16);
                            match value {
                                Some(v) => out.push(v as u8),
                                None => out.extend_from_slice(b"\\x"),
                            }
                        }
                        b'0'..=b'7' => {
                            self.i -= 1;
                            let value = self.digits(3, 8).unwrap_or(0);
                            out.push((value & 0xff) as u8);
                        }
                        _ => {
                            out.push(b'\\');
                            out.push(e);
                        }
                    }
                }
                _ => out.push(c),
            }
        }
    }

    fn digits(&mut self, max: usize, radix: u32) -> Option<u32> {
        let mut value = None;
        for _ in 0..max {
            let Some(d) = self.peek(0).and_then(|c| char::from(c).to_digit(radix)) else {
                break;
            };
            value = Some(value.unwrap_or(0) * radix + d);
            self.i += 1;
        }
        value
    }

    fn copy_quoted_raw(&mut self, out: &mut Vec<u8>, quote: u8) -> Result<(), LexError> {
        let start = self.line;
        out.push(quote);
        self.i += 1;
        loop {
            let Some(c) = self.peek(0) else {
                return Err(error(start, LexErrorKind::UnterminatedQuote));
            };
            match c {
                b'\\' if quote == b'"' => {
                    out.push(c);
                    self.i += 1;
                    if let Some(next) = self.bump() {
                        out.push(next);
                    }
                }
                b'$' if quote == b'"' && self.peek(1) == Some(b'(') => {
                    out.push(b'$');
                    self.i += 1;
                    self.copy_paren(out)?;
                }
                b'$' if quote == b'"' && self.peek(1) == Some(b'{') => {
                    out.push(b'$');
                    self.i += 1;
                    self.copy_brace(out, true)?;
                }
                b'`' if quote == b'"' => self.copy_backtick(out)?,
                _ => {
                    out.push(c);
                    self.bump();
                    if c == quote {
                        return Ok(());
                    }
                }
            }
        }
    }

    fn copy_ansi_c_raw(&mut self, out: &mut Vec<u8>) -> Result<(), LexError> {
        let start = self.line;
        out.extend_from_slice(b"$'");
        self.i += 2;
        loop {
            let Some(c) = self.bump() else {
                return Err(error(start, LexErrorKind::UnterminatedQuote));
            };
            out.push(c);
            match c {
                b'\'' => return Ok(()),
                b'\\' => {
                    if let Some(next) = self.bump() {
                        out.push(next);
                    }
                }
                _ => {}
            }
        }
    }

    fn copy_backtick(&mut self, out: &mut Vec<u8>) -> Result<(), LexError> {
        let start = self.line;
        out.push(b'`');
        self.i += 1;
        let mut heredocs = Vec::new();
        loop {
            let Some(c) = self.peek(0) else {
                return Err(error(start, LexErrorKind::UnterminatedSubstitution));
            };
            if c == b'<' && self.starts_with(b"<<") && !self.starts_with(b"<<<") {
                heredocs.extend(self.copy_heredoc_operator(out));
                continue;
            }
            out.push(c);
            self.bump();
            match c {
                b'`' => return Ok(()),
                b'\n' => self.copy_heredoc_bodies(out, &mut heredocs)?,
                b'\\' => {
                    if let Some(next) = self.bump() {
                        out.push(next);
                    }
                }
                _ => {}
            }
        }
    }

    fn copy_brace(&mut self, out: &mut Vec<u8>, in_double: bool) -> Result<(), LexError> {
        let start = self.line;
        out.push(b'{');
        self.i += 1;
        loop {
            let Some(c) = self.peek(0) else {
                return Err(error(start, LexErrorKind::UnterminatedSubstitution));
            };
            match c {
                b'}' => {
                    out.push(c);
                    self.i += 1;
                    return Ok(());
                }
                b'\\' => {
                    out.push(c);
                    self.i += 1;
                    if let Some(next) = self.bump() {
                        out.push(next);
                    }
                }
                b'\'' if !in_double => self.copy_quoted_raw(out, b'\'')?,
                b'"' => self.copy_quoted_raw(out, b'"')?,
                b'`' => self.copy_backtick(out)?,
                b'$' if self.peek(1) == Some(b'(') => {
                    out.push(b'$');
                    self.i += 1;
                    self.copy_paren(out)?;
                }
                b'$' if self.peek(1) == Some(b'{') => {
                    out.push(b'$');
                    self.i += 1;
                    self.copy_brace(out, in_double)?;
                }
                _ => {
                    out.push(c);
                    self.bump();
                }
            }
        }
    }

    fn copy_paren(&mut self, out: &mut Vec<u8>) -> Result<(), LexError> {
        let start = self.line;
        let mut opens = vec![self.i];
        out.push(b'(');
        self.i += 1;
        let mut depth = 1u32;
        let mut prev_open = true;
        let mut arith: Vec<u32> = Vec::new();
        let mut word_start = true;
        let mut cmd_pos = true;
        let mut cases = 0u32;
        let mut case_pending = false;
        let mut pattern = false;
        let mut heredocs = Vec::new();
        loop {
            let Some(c) = self.peek(0) else {
                return Err(error(start, LexErrorKind::UnterminatedSubstitution));
            };
            let at_word_start = word_start;
            let at_cmd_pos = cmd_pos;
            let after_open = prev_open;
            prev_open = false;
            word_start = false;
            let at_end = self.word_end(0);
            if !at_end {
                cmd_pos = false;
            }
            let in_arith = arith.last().is_some_and(|&d| depth >= d);
            match c {
                b'<' if !in_arith && self.starts_with(b"<<") && !self.starts_with(b"<<<") => {
                    heredocs.extend(self.copy_heredoc_operator(out));
                }
                b'\\' => {
                    out.push(c);
                    self.i += 1;
                    if let Some(next) = self.bump() {
                        out.push(next);
                    }
                }
                b'\'' => self.copy_quoted_raw(out, b'\'')?,
                b'"' => self.copy_quoted_raw(out, b'"')?,
                b'`' => self.copy_backtick(out)?,
                b'$' if self.peek(1) == Some(b'\'') => self.copy_ansi_c_raw(out)?,
                b'$' if self.peek(1) == Some(b'(') => {
                    out.push(b'$');
                    self.i += 1;
                    self.copy_paren(out)?;
                }
                b'$' if self.peek(1) == Some(b'{') => {
                    out.push(b'$');
                    self.i += 1;
                    self.copy_brace(out, false)?;
                }
                b'#' if at_word_start => {
                    while let Some(c) = self.peek(0).filter(|&c| c != b'\n') {
                        out.push(c);
                        self.i += 1;
                    }
                }
                b'(' => {
                    if !pattern {
                        opens.push(self.i);
                        depth += 1;
                        if after_open {
                            arith.push(depth);
                        }
                        prev_open = true;
                    }
                    out.push(c);
                    self.i += 1;
                    word_start = true;
                    cmd_pos = true;
                }
                b')' => {
                    let at = self.i;
                    out.push(c);
                    self.i += 1;
                    if pattern {
                        pattern = false;
                    } else {
                        if let Some(open) = opens.pop() {
                            self.closes.insert(open, at);
                        }
                        depth -= 1;
                        while arith.last().is_some_and(|&d| d > depth) {
                            arith.pop();
                        }
                        if depth == 0 {
                            return Ok(());
                        }
                    }
                    word_start = true;
                    cmd_pos = true;
                }
                b';' => {
                    let len = if self.starts_with(b";;&") {
                        3
                    } else if self.starts_with(b";;") || self.starts_with(b";&") {
                        2
                    } else {
                        1
                    };
                    out.extend_from_slice(&self.b[self.i..self.i + len]);
                    self.i += len;
                    if len > 1 && cases > 0 {
                        pattern = true;
                    }
                    word_start = true;
                    cmd_pos = true;
                }
                _ if at_end => {
                    out.push(c);
                    self.bump();
                    word_start = true;
                    if matches!(c, b'\n' | b'&' | b'|') {
                        cmd_pos = true;
                    }
                    if c == b'\n' {
                        self.copy_heredoc_bodies(out, &mut heredocs)?;
                    }
                }
                _ if at_word_start && (c.is_ascii_alphabetic() || c == b'_') => {
                    let begin = self.i;
                    while self
                        .peek(0)
                        .is_some_and(|c| c.is_ascii_alphanumeric() || c == b'_')
                    {
                        self.i += 1;
                    }
                    let bare = &self.b[begin..self.i];
                    out.extend_from_slice(bare);
                    if self.word_end(0) {
                        match bare {
                            b"case" if at_cmd_pos && !pattern => case_pending = true,
                            b"in" if case_pending => {
                                case_pending = false;
                                cases += 1;
                                pattern = true;
                                cmd_pos = true;
                            }
                            b"esac" if at_cmd_pos && cases > 0 => {
                                cases -= 1;
                                pattern = false;
                            }
                            reserved if at_cmd_pos && RESERVED.contains(&reserved) => {
                                cmd_pos = true;
                            }
                            _ => {}
                        }
                    }
                }
                _ => {
                    out.push(c);
                    self.bump();
                }
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Normal,
    CaseSubject,
    CaseIn,
    CasePattern { started: bool },
    ForHeader { depth: u32 },
    FunctionName,
    Test,
}

#[derive(Default)]
struct Pending {
    line: Option<u32>,
    assignments: Vec<Assignment>,
    argv: Vec<Text>,
    redirections: Vec<Redirection>,
    redirect: Option<(Option<u32>, RedirectOp, u32)>,
}

impl Pending {
    fn at_start(&self) -> bool {
        self.argv.is_empty()
            && self.assignments.is_empty()
            && self.redirections.is_empty()
            && self.redirect.is_none()
    }

    fn mark(&mut self, line: u32) {
        self.line.get_or_insert(line);
    }
}

struct Parser {
    out: Vec<ShellCommand>,
    cur: Pending,
    cases: u32,
    state: State,
}

impl Default for Parser {
    fn default() -> Self {
        Parser {
            out: Vec::new(),
            cur: Pending::default(),
            cases: 0,
            state: State::Normal,
        }
    }
}

fn is_op(tok: Option<&Tok>, op: Op) -> bool {
    matches!(tok, Some(Tok::Op(o)) if *o == op)
}

impl Parser {
    fn run(mut self, tokens: &[Tok]) -> Result<Vec<ShellCommand>, LexError> {
        let mut i = 0;
        while i < tokens.len() {
            i += self.step(&tokens[i..])?;
        }
        self.end_command()?;
        Ok(self.out)
    }

    fn step(&mut self, toks: &[Tok]) -> Result<usize, LexError> {
        let tok = &toks[0];
        let used = match self.state {
            State::CaseSubject => {
                if let Tok::Word(_) = tok {
                    self.state = State::CaseIn;
                }
                1
            }
            State::CaseIn => {
                match tok {
                    Tok::Word(w) if w.is(b"in") => {
                        self.state = State::CasePattern { started: false };
                    }
                    Tok::Op(Op::Newline) => {}
                    _ => self.state = State::Normal,
                }
                1
            }
            State::CasePattern { started } => {
                match tok {
                    Tok::Word(w) if !started && w.is(b"esac") => {
                        self.cases = self.cases.saturating_sub(1);
                        self.state = State::Normal;
                    }
                    Tok::Op(Op::RParen) => self.state = State::Normal,
                    Tok::Op(Op::Newline | Op::Semi) if !started => {}
                    Tok::Op(Op::LParen) if !started => {}
                    _ => self.state = State::CasePattern { started: true },
                }
                1
            }
            State::ForHeader { depth } => {
                match tok {
                    Tok::Op(Op::LParen) => self.state = State::ForHeader { depth: depth + 1 },
                    Tok::Op(Op::RParen) => {
                        self.state = State::ForHeader {
                            depth: depth.saturating_sub(1),
                        };
                    }
                    Tok::Op(Op::Semi | Op::Newline) if depth == 0 => self.state = State::Normal,
                    Tok::Word(w) if depth == 0 && w.is(b"do") => self.state = State::Normal,
                    _ => {}
                }
                1
            }
            State::FunctionName => {
                self.state = State::Normal;
                if is_op(toks.get(1), Op::LParen) && is_op(toks.get(2), Op::RParen) {
                    3
                } else {
                    1
                }
            }
            State::Test => {
                match tok {
                    Tok::Word(w) => {
                        self.cur.argv.push(Text::from_bytes(w.bytes.clone()));
                        if w.is(b"]]") {
                            self.state = State::Normal;
                        }
                    }
                    Tok::Op(Op::Newline)
                        if self
                            .cur
                            .argv
                            .last()
                            .is_some_and(|t| matches!(t.as_bytes(), b"&&" | b"||")) => {}
                    Tok::Op(op @ (Op::Newline | Op::Semi)) => {
                        self.state = State::Normal;
                        return self.step(&[Tok::Op(*op)]);
                    }
                    Tok::Op(op) => self.cur.argv.push(Text::from(op.as_bytes())),
                    Tok::Redirect { fd, op, .. } => {
                        let mut text = fd.map(|fd| fd.to_string()).unwrap_or_default();
                        text.push_str(op.as_str());
                        self.cur.argv.push(Text::from(text));
                    }
                }
                1
            }
            State::Normal => return self.normal(toks),
        };
        Ok(used)
    }

    fn missing_target(&self) -> Result<(), LexError> {
        match self.cur.redirect {
            Some((_, _, line)) => Err(error(line, LexErrorKind::MissingRedirectTarget)),
            None => Ok(()),
        }
    }

    fn normal(&mut self, toks: &[Tok]) -> Result<usize, LexError> {
        Ok(match &toks[0] {
            Tok::Word(w) => {
                if let Some((fd, op, _)) = self.cur.redirect.take() {
                    self.cur.redirections.push(Redirection {
                        fd,
                        op,
                        target: Text::from_bytes(w.bytes.clone()),
                    });
                    return Ok(1);
                }
                if self.cur.at_start() && !w.quoted {
                    match w.bytes.as_slice() {
                        b"case" => {
                            self.cases += 1;
                            self.state = State::CaseSubject;
                            return Ok(1);
                        }
                        b"for" | b"select" => {
                            self.state = State::ForHeader { depth: 0 };
                            return Ok(1);
                        }
                        b"function" => {
                            self.state = State::FunctionName;
                            return Ok(1);
                        }
                        b"esac" => {
                            self.cases = self.cases.saturating_sub(1);
                            return Ok(1);
                        }
                        b"[[" => {
                            self.cur.mark(w.line);
                            self.cur.argv.push(Text::from("[["));
                            self.state = State::Test;
                            return Ok(1);
                        }
                        reserved if RESERVED.contains(&reserved) => return Ok(1),
                        _ => {}
                    }
                    if is_op(toks.get(1), Op::LParen) && is_op(toks.get(2), Op::RParen) {
                        return Ok(3);
                    }
                }
                self.cur.mark(w.line);
                if self.cur.argv.is_empty()
                    && let Some(assignment) = w.assignment()
                {
                    self.cur.assignments.push(assignment);
                } else {
                    self.cur.argv.push(Text::from_bytes(w.bytes.clone()));
                }
                1
            }
            Tok::Redirect { fd, op, line } => {
                self.missing_target()?;
                self.cur.mark(*line);
                self.cur.redirect = Some((*fd, *op, *line));
                1
            }
            Tok::Op(op) => {
                self.end_command()?;
                if matches!(op, Op::DoubleSemi | Op::SemiAmp | Op::DoubleSemiAmp) && self.cases > 0
                {
                    self.state = State::CasePattern { started: false };
                }
                1
            }
        })
    }

    fn end_command(&mut self) -> Result<(), LexError> {
        self.missing_target()?;
        let cur = std::mem::take(&mut self.cur);
        if cur.argv.is_empty() && cur.assignments.is_empty() && cur.redirections.is_empty() {
            return Ok(());
        }
        self.out.push(ShellCommand {
            line: cur.line.unwrap_or(1),
            assignments: cur.assignments,
            argv: cur.argv,
            redirections: cur.redirections,
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lex_str(s: &str) -> Vec<ShellCommand> {
        lex(&Text::from(s)).unwrap()
    }

    fn argv(cmd: &ShellCommand) -> Vec<&str> {
        cmd.argv.iter().map(|t| t.as_str().unwrap()).collect()
    }

    fn all_argv(s: &str) -> Vec<Vec<String>> {
        lex_str(s)
            .iter()
            .map(|c| argv(c).into_iter().map(String::from).collect())
            .collect()
    }

    fn assignments(cmd: &ShellCommand) -> Vec<(&str, &str)> {
        cmd.assignments
            .iter()
            .map(|a| (a.name.as_str().unwrap(), a.value.as_str().unwrap()))
            .collect()
    }

    fn lex_error(s: &str) -> LexError {
        lex(&Text::from(s)).unwrap_err()
    }

    #[test]
    fn useradd_with_fixed_uid() {
        let cmds = lex_str("useradd -u 501 app");
        assert_eq!(cmds.len(), 1);
        assert_eq!(argv(&cmds[0]), ["useradd", "-u", "501", "app"]);
        assert!(cmds[0].redirections.is_empty());
        assert!(cmds[0].assignments.is_empty());
        assert_eq!(cmds[0].line, 1);
    }

    #[test]
    fn single_and_double_quotes() {
        let cmds = lex_str(r#"echo 'a b' "c $d \"e\" \x" 'it'"'"'s'"#);
        assert_eq!(argv(&cmds[0]), ["echo", "a b", r#"c $d "e" \x"#, "it's"]);
    }

    #[test]
    fn line_continuations_lf_and_crlf() {
        let cmds = lex_str("useradd \\\n  -u 501 \\\n  app\nid");
        assert_eq!(all_argv("useradd \\\n  -u 501 \\\n  app\nid").len(), 2);
        assert_eq!(argv(&cmds[0]), ["useradd", "-u", "501", "app"]);
        assert_eq!(cmds[0].line, 1);
        assert_eq!(argv(&cmds[1]), ["id"]);
        assert_eq!(cmds[1].line, 4);

        let crlf = lex_str("useradd \\\r\n  -u 501 \\\r\n  app\r\nid\r\n");
        assert_eq!(crlf.len(), 2);
        assert_eq!(argv(&crlf[0]), ["useradd", "-u", "501", "app"]);
        assert_eq!(argv(&crlf[1]), ["id"]);
        assert_eq!(crlf[1].line, 4);

        assert_eq!(all_argv("user\\\nadd x"), [["useradd", "x"]]);
        assert_eq!(all_argv("user\\\r\nadd x"), [["useradd", "x"]]);
        assert_eq!(all_argv("echo \"a\\\r\nb\""), [["echo", "ab"]]);
    }

    #[test]
    fn separators() {
        assert_eq!(
            all_argv("a 1; b 2 && c 3 | d 4 || e & f |& g"),
            [
                vec!["a", "1"],
                vec!["b", "2"],
                vec!["c", "3"],
                vec!["d", "4"],
                vec!["e"],
                vec!["f"],
                vec!["g"],
            ]
        );
    }

    #[test]
    fn redirection_to_pid_file() {
        for src in ["echo $$ > /var/run/app.pid", "echo $$ >/var/run/app.pid"] {
            let cmds = lex_str(src);
            assert_eq!(cmds.len(), 1, "{src}");
            assert_eq!(argv(&cmds[0]), ["echo", "$$"]);
            assert_eq!(
                cmds[0].redirections,
                vec![Redirection {
                    fd: None,
                    op: RedirectOp::Write,
                    target: Text::from("/var/run/app.pid"),
                }]
            );
        }
    }

    #[test]
    fn v5_every_redirect_operator() {
        let src = "run 2>/dev/null >>log >|clobber <in 3<>rw 1>&2 0<&3 &>both &>>more <<<word";
        let cmds = lex_str(src);
        assert_eq!(cmds.len(), 1);
        assert_eq!(argv(&cmds[0]), ["run"]);
        let got: Vec<(Option<u32>, RedirectOp, &str)> = cmds[0]
            .redirections
            .iter()
            .map(|r| (r.fd, r.op, r.target.as_str().unwrap()))
            .collect();
        assert_eq!(
            got,
            vec![
                (Some(2), RedirectOp::Write, "/dev/null"),
                (None, RedirectOp::Append, "log"),
                (None, RedirectOp::Clobber, "clobber"),
                (None, RedirectOp::Read, "in"),
                (Some(3), RedirectOp::ReadWrite, "rw"),
                (Some(1), RedirectOp::DupOut, "2"),
                (Some(0), RedirectOp::DupIn, "3"),
                (None, RedirectOp::WriteBoth, "both"),
                (None, RedirectOp::AppendBoth, "more"),
                (None, RedirectOp::HereString, "word"),
            ]
        );
        let ops: Vec<RedirectOp> = got.iter().map(|r| r.1).collect();
        for op in RedirectOp::ALL {
            assert!(ops.contains(&op) || op == RedirectOp::HereDoc, "{op:?}");
        }

        let quoted = lex_str("echo '2'>x");
        assert_eq!(argv(&quoted[0]), ["echo", "2"]);
        assert_eq!(quoted[0].redirections[0].fd, None);
    }

    #[test]
    fn v5_here_string_does_not_hide_the_next_line() {
        assert_eq!(
            all_argv("read x <<< \"$1\"\nuseradd app\nid"),
            [vec!["read", "x"], vec!["useradd", "app"], vec!["id"]]
        );
    }

    #[test]
    fn heredoc_bodies_are_not_commands() {
        let src = "cat > /etc/app.conf <<EOF\nuseradd -u 0 root\nEOF\nchmod 600 /etc/app.conf\n\
                   cat <<-'END'\n\tnot a command\n\tEND\nid";
        let cmds = lex_str(src);
        let all: Vec<Vec<&str>> = cmds.iter().map(argv).collect();
        assert_eq!(
            all,
            vec![
                vec!["cat"],
                vec!["chmod", "600", "/etc/app.conf"],
                vec!["cat"],
                vec!["id"]
            ]
        );
        assert_eq!(cmds[0].redirections[1].op, RedirectOp::HereDoc);
        assert_eq!(cmds[0].redirections[1].target.as_str(), Some("EOF"));
        assert_eq!(cmds[1].line, 4);
        assert_eq!(cmds[3].line, 8);

        let two = lex_str("paste <<A <<\"B\"\na1\nA\nb1\nB\ntrue\r\n");
        assert_eq!(two.len(), 2);
        assert_eq!(argv(&two[1]), ["true"]);
        assert_eq!(two[1].line, 6);

        let crlf = lex_str("cat <<EOF\r\nbody\r\nEOF\r\nid\r\n");
        assert_eq!(crlf.len(), 2);
        assert_eq!(argv(&crlf[1]), ["id"]);
    }

    #[test]
    fn comments_and_reserved_words() {
        let src = "# setup\nif [ -f /x ]; then useradd -u 501 app; fi # done\n{ chown app /opt; }\n\
                   while ! id app; do sleep 1; done\nuntil true; do :; done";
        assert_eq!(
            all_argv(src),
            [
                vec!["[", "-f", "/x", "]"],
                vec!["useradd", "-u", "501", "app"],
                vec!["chown", "app", "/opt"],
                vec!["id", "app"],
                vec!["sleep", "1"],
                vec!["true"],
                vec![":"],
            ]
        );
        assert_eq!(all_argv("'if' x"), [["if", "x"]]);
        assert_eq!(
            all_argv("echo done fi a#b"),
            [["echo", "done", "fi", "a#b"]]
        );
    }

    #[test]
    fn case_arm_patterns_are_not_commands() {
        let src = "case \"$1\" in\n  start)\n    run --daemon\n    ;;\n  stop|halt) kill $PID;;\n  \
                   (restart) stop; start ;&\n  reload) hup ;;&\n  *)\n    echo usage\nesac\nafter";
        assert_eq!(
            all_argv(src),
            [
                vec!["run", "--daemon"],
                vec!["kill", "$PID"],
                vec!["stop"],
                vec!["start"],
                vec!["hup"],
                vec!["echo", "usage"],
                vec!["after"],
            ]
        );
    }

    #[test]
    fn nested_case() {
        let src = "case $a in\n x) case $b in y) inner;; esac; tail;;\n z) other;;\nesac\nlast";
        assert_eq!(
            all_argv(src),
            [vec!["inner"], vec!["tail"], vec!["other"], vec!["last"]]
        );
    }

    #[test]
    fn for_and_select_headers_are_not_commands() {
        let src = "for u in app web; do\n  useradd \"$u\"\ndone\nfor x do echo \"$x\"; done\n\
                   select opt in a b\ndo pick; done\nfor ((i = 0; i < 3; i++)); do tick; done\n\
                   for f; do rm \"$f\"; done";
        assert_eq!(
            all_argv(src),
            [
                vec!["useradd", "$u"],
                vec!["echo", "$x"],
                vec!["pick"],
                vec!["tick"],
                vec!["rm", "$f"],
            ]
        );
        assert_eq!(all_argv("(( n++ ))\nid"), [["id"]]);
    }

    #[test]
    fn function_definitions_are_not_commands() {
        let src = "setup() {\n  useradd app\n}\nstop ()\n{\n  kill 1\n}\n\
                   function start {\n  run\n}\nfunction reload() { hup; }\nsetup";
        assert_eq!(
            all_argv(src),
            [
                vec!["useradd", "app"],
                vec!["kill", "1"],
                vec!["run"],
                vec!["hup"],
                vec!["setup"],
            ]
        );
    }

    #[test]
    fn leading_assignments_are_not_argv() {
        let cmds = lex_str("FOO=1 BAR=\"a b\" useradd app X=2\nPATH=/sbin:$PATH\n\"Q=1\" cmd\n");
        assert_eq!(cmds.len(), 3);
        assert_eq!(assignments(&cmds[0]), [("FOO", "1"), ("BAR", "a b")]);
        assert_eq!(argv(&cmds[0]), ["useradd", "app", "X=2"]);
        assert_eq!(assignments(&cmds[1]), [("PATH", "/sbin:$PATH")]);
        assert!(cmds[1].argv.is_empty());
        assert!(cmds[2].assignments.is_empty());
        assert_eq!(argv(&cmds[2]), ["Q=1", "cmd"]);
        assert_eq!(
            assignments(&lex_str("V=$(id -u) run")[0]),
            [("V", "$(id -u)")]
        );
        assert_eq!(all_argv("1X=2 run"), [["1X=2", "run"]]);
        assert_eq!(all_argv("=x run"), [["=x", "run"]]);
        assert_eq!(all_argv("A\\B=1 run"), [["AB=1", "run"]]);
    }

    #[test]
    fn argv_is_empty_only_with_assignments_or_redirections() {
        let cmds = lex_str("if then fi\n> /tmp/x\nwhile read l; do :; done < list\n{ }\n!\n");
        for cmd in &cmds {
            assert!(
                !cmd.argv.is_empty() || !cmd.assignments.is_empty() || !cmd.redirections.is_empty()
            );
        }
        assert_eq!(cmds.len(), 4);
        assert!(cmds[0].argv.is_empty());
        assert_eq!(cmds[0].redirections[0].target.as_str(), Some("/tmp/x"));
        assert!(cmds[3].argv.is_empty());
        assert_eq!(cmds[3].redirections[0].target.as_str(), Some("list"));
    }

    #[test]
    fn substitutions_stay_inside_words() {
        let cmds = lex_str("echo $(id -u; echo x) ${A:-b c} `date; x` \"$(a \")\")\" $((1 + (2)))");
        assert_eq!(cmds.len(), 1);
        assert_eq!(
            argv(&cmds[0]),
            [
                "echo",
                "$(id -u; echo x)",
                "${A:-b c}",
                "`date; x`",
                "$(a \")\")",
                "$((1 + (2)))"
            ]
        );
    }

    #[test]
    fn double_quotes_keep_nested_braces_and_quotes() {
        assert_eq!(
            all_argv(r#"echo "${A:-"b c"}" "x${B#'}" "${C:-$(echo "}")}" "`echo "q"`""#),
            [[
                "echo",
                r#"${A:-"b c"}"#,
                "x${B#'}",
                r#"${C:-$(echo "}")}"#,
                r#"`echo "q"`"#
            ]]
        );
        assert_eq!(all_argv(r#"echo ${A:-'}'}"#), [["echo", "${A:-'}'}"]]);
    }

    #[test]
    fn ansi_c_quotes_decode_escapes() {
        let cmds = lex_str(r#"printf $'a\nb\tc\\d\'e\x41\x7f\0101\012' $'\q' $"t""#);
        let args: Vec<&[u8]> = cmds[0].argv.iter().map(Text::as_bytes).collect();
        assert_eq!(args[1], b"a\nb\tc\\d'eA\x7f\x081\n");
        assert_eq!(args[2], b"\\q");
        assert_eq!(args[3], b"t");
        let more = lex_str(r#"printf $'\r\"\a\b\e\E\?\f\v\xA\0'"#);
        assert_eq!(
            more[0].argv[1].as_bytes(),
            b"\r\"\x07\x08\x1b\x1b?\x0c\x0b\n\0"
        );
        let raw = lex_str(r"echo $'\xff'");
        assert_eq!(raw[0].argv[1].as_bytes(), b"\xff");
        assert!(!raw[0].argv[1].is_utf8());
    }

    #[test]
    fn comments_and_case_inside_command_substitution() {
        let src = "x=$(\n  # a comment with ) and '\n  echo hi\n)\nv=$(case $1 in a) echo A;; (b) echo B;; esac)\nnext";
        let cmds = lex_str(src);
        assert_eq!(cmds.len(), 3);
        assert_eq!(cmds[0].assignments[0].name.as_str(), Some("x"));
        assert_eq!(
            cmds[1].assignments[0].value.as_str(),
            Some("$(case $1 in a) echo A;; (b) echo B;; esac)")
        );
        assert_eq!(argv(&cmds[2]), ["next"]);
        assert_eq!(cmds[2].line, 6);
        assert_eq!(
            all_argv("echo $(echo a#b) $(echo ${#x})"),
            [["echo", "$(echo a#b)", "$(echo ${#x})"]]
        );
    }

    #[test]
    fn double_bracket_tests_are_one_command() {
        assert_eq!(
            all_argv("[[ -f $a && $b < $c || ( -n x ) ]] && run\n"),
            [
                vec![
                    "[[", "-f", "$a", "&&", "$b", "<", "$c", "||", "(", "-n", "x", ")", "]]"
                ],
                vec!["run"],
            ]
        );
    }

    #[test]
    fn unterminated_input_is_an_error() {
        let cases = [
            ("id 'open", 1, LexErrorKind::UnterminatedQuote),
            ("ok\nid \"open\nmore", 2, LexErrorKind::UnterminatedQuote),
            ("echo $'open", 1, LexErrorKind::UnterminatedQuote),
            ("a\nb\necho $(id", 3, LexErrorKind::UnterminatedSubstitution),
            ("echo ${A:-x", 1, LexErrorKind::UnterminatedSubstitution),
            ("echo `date", 1, LexErrorKind::UnterminatedSubstitution),
            ("echo \"$(id\"", 1, LexErrorKind::UnterminatedQuote),
            ("cat <<EOF\nbody\n", 1, LexErrorKind::UnterminatedHeredoc),
            ("x\ncat <<EOF", 2, LexErrorKind::UnterminatedHeredoc),
            ("(( 1 + 2", 1, LexErrorKind::UnterminatedSubstitution),
        ];
        for (src, line, kind) in cases {
            assert_eq!(lex_error(src), LexError { line, kind }, "{src:?}");
        }
    }

    #[test]
    fn raw_bytes_survive() {
        let t = Text::from_bytes(b"echo \xff\xfe".to_vec());
        let cmds = lex(&t).unwrap();
        assert_eq!(cmds[0].argv[1].as_bytes(), b"\xff\xfe");
    }

    const SYSV_INIT: &str = r#"#!/bin/sh
### BEGIN INIT INFO
# Provides:          appd
# Required-Start:    $remote_fs $syslog
# Default-Start:     2 3 4 5
### END INIT INFO

DAEMON=/usr/sbin/appd
PIDFILE=/var/run/appd.pid
[ -x "$DAEMON" ] || exit 0
. /lib/lsb/init-functions

do_start() {
	start-stop-daemon --start --quiet --pidfile $PIDFILE \
		--chuid app:app --exec $DAEMON -- $DAEMON_OPTS
	echo $! > $PIDFILE
}

case "$1" in
  start)
	log_daemon_msg "Starting" "appd"
	do_start
	;;
  stop)
	start-stop-daemon --stop --quiet --pidfile $PIDFILE
	rm -f $PIDFILE
	;;
  restart|force-reload)
	$0 stop
	$0 start
	;;
  *)
	echo "Usage: $0 {start|stop|restart}" >&2
	exit 3
	;;
esac

exit 0
"#;

    #[test]
    fn sysv_init_script() {
        let cmds = lex_str(SYSV_INIT);
        let got: Vec<(u32, Vec<&str>)> = cmds.iter().map(|c| (c.line, argv(c))).collect();
        assert_eq!(
            got,
            vec![
                (8, vec![]),
                (9, vec![]),
                (10, vec!["[", "-x", "$DAEMON", "]"]),
                (10, vec!["exit", "0"]),
                (11, vec![".", "/lib/lsb/init-functions"]),
                (
                    14,
                    vec![
                        "start-stop-daemon",
                        "--start",
                        "--quiet",
                        "--pidfile",
                        "$PIDFILE",
                        "--chuid",
                        "app:app",
                        "--exec",
                        "$DAEMON",
                        "--",
                        "$DAEMON_OPTS"
                    ]
                ),
                (16, vec!["echo", "$!"]),
                (21, vec!["log_daemon_msg", "Starting", "appd"]),
                (22, vec!["do_start"]),
                (
                    25,
                    vec![
                        "start-stop-daemon",
                        "--stop",
                        "--quiet",
                        "--pidfile",
                        "$PIDFILE"
                    ]
                ),
                (26, vec!["rm", "-f", "$PIDFILE"]),
                (29, vec!["$0", "stop"]),
                (30, vec!["$0", "start"]),
                (33, vec!["echo", "Usage: $0 {start|stop|restart}"]),
                (34, vec!["exit", "3"]),
                (38, vec!["exit", "0"]),
            ]
        );
        assert_eq!(assignments(&cmds[0]), [("DAEMON", "/usr/sbin/appd")]);
        assert_eq!(cmds[6].redirections[0].target.as_str(), Some("$PIDFILE"));
        assert_eq!(cmds[13].redirections[0].op, RedirectOp::DupOut);
    }

    const DEB_POSTINST: &str = r#"#!/bin/sh
set -e

case "$1" in
    configure|upgrade)
        if ! getent passwd app >/dev/null; then
            adduser --system --uid 501 --group --home /var/lib/app app
        fi
        chown -R app:app /var/lib/app
        cat > /etc/app/env <<EOF
APP_USER=app
rm -rf /
EOF
        ;;

    abort-upgrade|abort-remove|abort-deconfigure)
        ;;

    *)
        echo "postinst called with unknown argument \`$1'" >&2
        exit 1
        ;;
esac

#DEBHELPER#

exit 0
"#;

    #[test]
    fn deb_postinst() {
        let cmds = lex_str(DEB_POSTINST);
        let got: Vec<(u32, Vec<&str>)> = cmds.iter().map(|c| (c.line, argv(c))).collect();
        assert_eq!(
            got,
            vec![
                (2, vec!["set", "-e"]),
                (6, vec!["getent", "passwd", "app"]),
                (
                    7,
                    vec![
                        "adduser",
                        "--system",
                        "--uid",
                        "501",
                        "--group",
                        "--home",
                        "/var/lib/app",
                        "app"
                    ]
                ),
                (9, vec!["chown", "-R", "app:app", "/var/lib/app"]),
                (10, vec!["cat"]),
                (
                    20,
                    vec!["echo", "postinst called with unknown argument `$1'"]
                ),
                (21, vec!["exit", "1"]),
                (27, vec!["exit", "0"]),
            ]
        );
        assert_eq!(cmds[1].redirections[0].target.as_str(), Some("/dev/null"));
    }

    #[test]
    fn array_assignments_and_append_are_assignments() {
        let cmds = lex_str("x=(useradd -u 0 app)\narr=()\nPATH+=:/sbin run\nA+=(b 'c)')\n");
        type Row<'a> = (Vec<(&'a str, bool, &'a str)>, Vec<&'a str>);
        let got: Vec<Row> = cmds
            .iter()
            .map(|c| {
                let a = c
                    .assignments
                    .iter()
                    .map(|a| {
                        (
                            a.name.as_str().unwrap(),
                            a.append,
                            a.value.as_str().unwrap(),
                        )
                    })
                    .collect();
                (a, argv(c))
            })
            .collect();
        assert_eq!(
            got,
            vec![
                (vec![("x", false, "(useradd -u 0 app)")], vec![]),
                (vec![("arr", false, "()")], vec![]),
                (vec![("PATH", true, ":/sbin")], vec!["run"]),
                (vec![("A", true, "(b 'c)')")], vec![]),
            ]
        );
    }

    #[test]
    fn process_substitution_stays_in_its_word() {
        let cmds = lex_str("diff <(sort a) >(tee b)\nwhile read l; do :; done < <(ls -1)\n");
        assert_eq!(cmds.len(), 4);
        assert_eq!(argv(&cmds[0]), ["diff", "<(sort a)", ">(tee b)"]);
        assert!(cmds[0].redirections.is_empty());
        assert!(cmds[3].argv.is_empty());
        assert_eq!(
            cmds[3].redirections,
            vec![Redirection {
                fd: None,
                op: RedirectOp::Read,
                target: Text::from("<(ls -1)"),
            }]
        );
    }

    #[test]
    fn a_redirection_without_a_target_is_an_error() {
        for (src, line) in [
            ("cmd >", 1),
            ("a\ncmd > ; x", 2),
            ("cat <<\nx", 1),
            ("run 2>&", 1),
            ("a > > b", 1),
            ("a >\nb", 1),
        ] {
            assert_eq!(
                lex_error(src),
                LexError {
                    line,
                    kind: LexErrorKind::MissingRedirectTarget
                },
                "{src:?}"
            );
        }
    }

    #[test]
    fn heredocs_inside_substitutions_are_copied_verbatim() {
        let cmds = lex_str(
            "x=$(cat <<'EOF'\nit's (odd\nEOF\n)\ny=`cat <<-EOF\n\twon't )\n\tEOF\n`\necho $((1 << 2))\nnext",
        );
        let all: Vec<Vec<&str>> = cmds.iter().map(argv).collect();
        assert_eq!(
            all,
            vec![vec![], vec![], vec!["echo", "$((1 << 2))"], vec!["next"]]
        );
        assert_eq!(
            cmds[0].assignments[0].value.as_str(),
            Some("$(cat <<'EOF'\nit's (odd\nEOF\n)")
        );
        assert_eq!(
            cmds[1].assignments[0].value.as_str(),
            Some("`cat <<-EOF\n\twon't )\n\tEOF\n`")
        );
        assert_eq!(cmds[3].line, 10);
        assert_eq!(
            lex_error("x=$(cat <<EOF\nnever closed\n)"),
            LexError {
                line: 1,
                kind: LexErrorKind::UnterminatedHeredoc
            }
        );
    }

    #[test]
    fn case_and_in_as_arguments_inside_substitutions() {
        assert_eq!(
            all_argv("echo $(echo case in) $(printf '%s' esac)\nid"),
            [
                vec!["echo", "$(echo case in)", "$(printf '%s' esac)"],
                vec!["id"]
            ]
        );
        assert_eq!(
            all_argv("v=$(if x; then case $1 in a) y;; esac; fi)\nid")[1],
            ["id"]
        );
    }

    #[test]
    fn double_bracket_test_continues_after_and_or_newline() {
        assert_eq!(
            all_argv("[[ -f a &&\n   -r a ]] && run\n[[ x ]]\nnext"),
            [
                vec!["[[", "-f", "a", "&&", "-r", "a", "]]"],
                vec!["run"],
                vec!["[[", "x", "]]"],
                vec!["next"],
            ]
        );
    }

    #[test]
    fn a_lone_carriage_return_is_part_of_a_word() {
        assert_eq!(
            all_argv("echo a\rb c\r\nid"),
            [vec!["echo", "a\rb", "c"], vec!["id"]]
        );
    }

    #[test]
    fn double_paren_without_double_close_is_nested_subshells() {
        assert_eq!(
            all_argv("((cd /x && make) || exit 1)\n((i++))\nid"),
            [
                vec!["cd", "/x"],
                vec!["make"],
                vec!["exit", "1"],
                vec!["id"]
            ]
        );
    }

    #[test]
    fn if_elif_else_words_are_dropped() {
        assert_eq!(
            all_argv("if a; then b; elif c; then d; else useradd -u 0 x; fi"),
            [
                vec!["a"],
                vec!["b"],
                vec!["c"],
                vec!["d"],
                vec!["useradd", "-u", "0", "x"]
            ]
        );
    }

    #[test]
    fn shift_inside_arithmetic_is_not_a_heredoc() {
        assert_eq!(
            all_argv("x=$(\n  (( m = 1 << 4 ))\n  echo $m\n)\nid"),
            [vec![], vec!["id"]]
        );
        assert_eq!(all_argv("x=$((\n 1 << 2\n))\nid"), [vec![], vec!["id"]]);
        assert_eq!(
            all_argv("mask=$( (( v = 1 << 3 )); echo $v\n)\nid"),
            [vec![], vec!["id"]]
        );
    }

    #[test]
    fn crlf_inside_substitutions() {
        assert_eq!(
            all_argv("v=$(case $1 in\r\n a) echo A;;\r\n esac)\r\nid"),
            [vec![], vec!["id"]]
        );
        let cmds = lex_str("x=$(cat <<EOF\r\nbody )\r\nEOF\r\n)\r\nid");
        assert_eq!(cmds.len(), 2);
        assert_eq!(
            cmds[0].assignments[0].value.as_str(),
            Some("$(cat <<EOF\r\nbody )\r\nEOF\r\n)")
        );
        assert_eq!(argv(&cmds[1]), ["id"]);
        assert_eq!(
            all_argv("v=$(if x; then\r\n case $1 in a) y;; esac\r\nfi)\r\nid"),
            [vec![], vec!["id"]]
        );
    }

    #[test]
    fn nested_double_parens_lex_in_linear_time() {
        let n = 40_000;
        let src = "((a) ".repeat(n) + &")".repeat(n);
        assert!(src.len() >= 200_000);
        let started = std::time::Instant::now();
        let cmds = lex_str(&src);
        assert!(started.elapsed() < std::time::Duration::from_secs(2));
        assert_eq!(cmds.len(), n);
        assert!(cmds.iter().all(|c| argv(c) == ["a"]));
    }

    #[test]
    fn esac_right_after_in_or_an_arm_end_closes_the_case() {
        assert_eq!(all_argv("x=$(case x in esac)\nid"), [vec![], vec!["id"]]);
        let cmds = lex_str("v=$(case $1 in a) echo esac;; b) y;; esac)\nid");
        assert_eq!(
            cmds[0].assignments[0].value.as_str(),
            Some("$(case $1 in a) echo esac;; b) y;; esac)")
        );
        assert_eq!(argv(&cmds[1]), ["id"]);
    }

    #[test]
    fn heredoc_inside_backticks_may_hold_a_backtick() {
        let cmds = lex_str("y=`cat <<EOF\na ` b\nEOF\n`\nnext");
        assert_eq!(cmds.len(), 2);
        assert_eq!(
            cmds[0].assignments[0].value.as_str(),
            Some("`cat <<EOF\na ` b\nEOF\n`")
        );
        assert_eq!(argv(&cmds[1]), ["next"]);
    }
}
