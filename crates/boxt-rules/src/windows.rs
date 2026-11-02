use boxt_core::Text;
use boxt_core::fact::{Directive, Fact, GuardChain, GuardFrame};
use boxt_core::verdict::{Finding, Observation};

const MAX_NESTING: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Side {
    Positive,
    Negative,
    Neither,
}

pub(crate) struct WindowsGuards<'a> {
    macros: &'a [String],
}

pub(crate) enum Split {
    Finding(Finding),
    Branch(Observation),
}

impl<'a> WindowsGuards<'a> {
    pub(crate) fn new(macros: &'a [String]) -> Self {
        WindowsGuards { macros }
    }

    pub(crate) fn split(&self, finding: Finding, at: &Fact) -> Split {
        let Some(Some(condition)) = finding.guards.iter().find_map(|c| self.branch(c)) else {
            return Split::Finding(finding);
        };
        let api = match at {
            Fact::Call { callee, .. } => Some(callee.clone()),
            _ => None,
        };
        Split::Branch(Observation::WindowsBranch {
            location: finding.location,
            guards: finding.guards,
            condition,
            behaviour: finding.behaviour,
            api,
        })
    }

    fn branch(&self, chain: &GuardChain) -> Option<Option<Text>> {
        let code = chain.frames.len().checked_sub(1)?;
        let frames: Vec<&GuardFrame> = chain.frames.iter().chain(&chain.later).collect();
        let sides: Vec<Side> = frames.iter().map(|f| self.side(f)).collect();
        let windows_side =
            |k: usize| sides[k] == Side::Positive || sides[..k].contains(&Side::Negative);
        if !windows_side(code) {
            return None;
        }
        let alternative = frames
            .iter()
            .enumerate()
            .any(|(k, f)| k != code && f.has_token && !windows_side(k));
        if !alternative {
            return Some(None);
        }
        let text = &frames[code].text;
        if sides[code] == Side::Positive {
            return Some(Some(text.clone()));
        }
        let negative = sides[..code].iter().position(|s| *s == Side::Negative)?;
        let mut out = frames[negative].text.as_bytes().to_vec();
        out.extend_from_slice(b", ");
        out.extend_from_slice(text.as_bytes());
        Some(Some(Text::from_bytes(out)))
    }

    fn side(&self, frame: &GuardFrame) -> Side {
        let tokens = tokens(frame.text.as_bytes());
        let condition = match tokens.split_first() {
            Some((Token::Hash, rest)) => match rest.split_first() {
                Some((Token::Ident(_), condition)) => condition,
                _ => return Side::Neither,
            },
            _ => return Side::Neither,
        };
        match frame.directive {
            Directive::Ifdef | Directive::Elifdef => match condition {
                [Token::Ident(name)] if self.is_windows(name) => Side::Positive,
                _ => Side::Neither,
            },
            Directive::Ifndef | Directive::Elifndef => match condition {
                [Token::Ident(name)] if self.is_windows(name) => Side::Negative,
                _ => Side::Neither,
            },
            Directive::If | Directive::Elif => {
                if self.negation(condition) {
                    Side::Negative
                } else if self.disjunction(condition, 0) {
                    Side::Positive
                } else {
                    Side::Neither
                }
            }
            Directive::Else => Side::Neither,
        }
    }

    fn negation(&self, tokens: &[Token<'_>]) -> bool {
        let mut depth = 0usize;
        let mut start = 0;
        let negated = |conjunct: &[Token<'_>]| match conjunct {
            [Token::Not, operand @ ..] => self.term(operand, 0),
            _ => false,
        };
        for (i, t) in tokens.iter().enumerate() {
            match t {
                Token::Open => depth += 1,
                Token::Close => depth = depth.saturating_sub(1),
                Token::And if depth == 0 => {
                    if !negated(&tokens[start..i]) {
                        return false;
                    }
                    start = i + 1;
                }
                _ => {}
            }
        }
        negated(&tokens[start..])
    }

    fn disjunction(&self, tokens: &[Token<'_>], nesting: usize) -> bool {
        if nesting > MAX_NESTING {
            return false;
        }
        let mut depth = 0usize;
        let mut start = 0;
        for (i, t) in tokens.iter().enumerate() {
            match t {
                Token::Open => depth += 1,
                Token::Close => depth = depth.saturating_sub(1),
                Token::Or if depth == 0 => {
                    if !self.term(&tokens[start..i], nesting) {
                        return false;
                    }
                    start = i + 1;
                }
                _ => {}
            }
        }
        self.term(&tokens[start..], nesting)
    }

    fn term(&self, tokens: &[Token<'_>], nesting: usize) -> bool {
        match tokens {
            [Token::Open, inner @ .., Token::Close] if balanced(inner) => {
                self.disjunction(inner, nesting + 1)
            }
            _ => self.atom(tokens, nesting),
        }
    }

    fn atom(&self, tokens: &[Token<'_>], nesting: usize) -> bool {
        if nesting > MAX_NESTING {
            return false;
        }
        match tokens {
            [Token::Ident(name)] => *name != b"defined" && self.is_windows(name),
            [Token::Ident(b"defined"), Token::Ident(name)]
            | [
                Token::Ident(b"defined"),
                Token::Open,
                Token::Ident(name),
                Token::Close,
            ] => self.is_windows(name),
            _ => false,
        }
    }

    fn is_windows(&self, name: &[u8]) -> bool {
        self.macros.iter().any(|m| m.as_bytes() == name)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Token<'a> {
    Hash,
    Ident(&'a [u8]),
    Open,
    Close,
    Not,
    Or,
    And,
    Other,
}

fn balanced(tokens: &[Token<'_>]) -> bool {
    let mut depth = 0usize;
    for t in tokens {
        match t {
            Token::Open => depth += 1,
            Token::Close => match depth.checked_sub(1) {
                Some(d) => depth = d,
                None => return false,
            },
            _ => {}
        }
    }
    depth == 0
}

fn tokens(text: &[u8]) -> Vec<Token<'_>> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < text.len() {
        let c = text[i];
        if c.is_ascii_whitespace() {
            i += 1;
        } else if c.is_ascii_alphabetic() || c == b'_' || c == b'$' || c >= 0x80 {
            let start = i;
            while i < text.len()
                && (text[i].is_ascii_alphanumeric()
                    || text[i] == b'_'
                    || text[i] == b'$'
                    || text[i] >= 0x80)
            {
                i += 1;
            }
            out.push(Token::Ident(&text[start..i]));
        } else if text[i..].starts_with(b"||") {
            out.push(Token::Or);
            i += 2;
        } else if text[i..].starts_with(b"&&") {
            out.push(Token::And);
            i += 2;
        } else if c == b'!' && text.get(i + 1) != Some(&b'=') {
            out.push(Token::Not);
            i += 1;
        } else {
            out.push(match c {
                b'#' => Token::Hash,
                b'(' => Token::Open,
                b')' => Token::Close,
                _ => Token::Other,
            });
            i += 1;
        }
    }
    out
}
