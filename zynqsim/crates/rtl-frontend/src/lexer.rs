//! Lexer for Verilog-2001 and the common synthesizable SystemVerilog tokens.
//!
//! The lexer never fails: a bad character or an unterminated string becomes a
//! [`Tok::Error`] token, which the preprocessor or the parser reports.

use crate::diag::Pos;

/// The value of a numeric literal as written.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct Number {
    /// Declared size in bits (`8` in `8'hFF`), `None` when unsized.
    pub size: Option<u32>,
    /// `s` marker present (`4'sb1010`).
    pub signed: bool,
    /// `b`, `o`, `d`, `h` for based and plain decimal literals (plain decimal is `d`).
    /// `'` marks an unsized fill literal (`'0`, `'1`, `'x`, `'z`).
    pub base: char,
    /// Lower-case digits without underscores; may contain `x`, `z` and `?`.
    pub digits: String,
    /// Written with an apostrophe (`8'd4`, `'d4`); a plain `4` is `false`.
    pub apostrophe: bool,
}

/// The kind of a token.
#[derive(Debug, Clone, PartialEq)]
pub enum Tok {
    /// Identifier or keyword. Escaped identifiers keep their leading backslash.
    Ident(String),
    /// System name including the `$`, for example `$display`.
    SysName(String),
    /// Compiler directive or macro use, without the backtick.
    Directive(String),
    /// Integer literal.
    Num(Number),
    /// Real literal (text as written). The parser rejects these in expressions.
    Real(String),
    /// String literal contents (escapes resolved for the common ones).
    Str(String),
    /// Operator or punctuation.
    Op(&'static str),
    /// A lexical error; the text says what was wrong.
    Error(String),
    /// End of input.
    Eof,
}

/// A token with its position and the whitespace that preceded it.
#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    /// The token kind and payload.
    pub tok: Tok,
    /// Where the token starts (for macro expansions: the use site).
    pub pos: Pos,
    /// Whitespace or a comment preceded the token.
    pub space_before: bool,
    /// A real line break (not a `\`-continuation) preceded the token.
    pub newline_before: bool,
    /// Name of the macro whose expansion produced this token, if any.
    pub expansion: Option<String>,
}

/// Operators, longest first so that the first match is the longest match.
const OPS: &[&str] = &[
    "<<<=", ">>>=", "===", "!==", "<<<", ">>>", "<<=", ">>=", "|->", "|=>", "**", "==", "!=", "<=",
    ">=", "&&", "||", "<<", ">>", "++", "--", "+=", "-=", "*=", "/=", "%=", "&=", "|=", "^=", "~&",
    "~|", "~^", "^~", "::", "->", "=>", "+:", "-:", "##", "+", "-", "*", "/", "%", "&", "|", "^",
    "~", "!", "<", ">", "=", "?", ":", ",", ";", ".", "(", ")", "[", "]", "{", "}", "@", "#", "'",
    "`\"",
];

/// Tokenises one file's text. Positions use `file` as the file name.
pub fn lex(file: &str, src: &str) -> Vec<Token> {
    let mut lx = Lexer {
        b: src.as_bytes(),
        i: 0,
        line: 1,
        line_start: 0,
        file,
        out: Vec::new(),
    };
    lx.run();
    lx.out
}

struct Lexer<'a> {
    b: &'a [u8],
    i: usize,
    line: u32,
    line_start: usize,
    file: &'a str,
    out: Vec<Token>,
}

fn is_ident_start(c: u8) -> bool {
    c.is_ascii_alphabetic() || c == b'_'
}

fn is_ident_char(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_' || c == b'$'
}

fn valid_digit(base: char, c: u8) -> bool {
    let c = c.to_ascii_lowercase();
    match base {
        'b' => matches!(c, b'0' | b'1' | b'x' | b'z' | b'?' | b'_'),
        'o' => matches!(c, b'0'..=b'7' | b'x' | b'z' | b'?' | b'_'),
        'd' => matches!(c, b'0'..=b'9' | b'x' | b'z' | b'?' | b'_'),
        'h' => matches!(c, b'0'..=b'9' | b'a'..=b'f' | b'x' | b'z' | b'?' | b'_'),
        _ => false,
    }
}

impl Lexer<'_> {
    fn peek(&self, k: usize) -> u8 {
        self.b.get(self.i + k).copied().unwrap_or(0)
    }

    fn pos_at(&self, i: usize) -> Pos {
        Pos::new(
            self.file,
            self.line,
            (i.saturating_sub(self.line_start) + 1) as u32,
        )
    }

    fn newline(&mut self) {
        // `self.i` points at the `\n`.
        self.i += 1;
        self.line += 1;
        self.line_start = self.i;
    }

    /// Skips blanks, comments and continuations. Returns (space, newline seen).
    fn skip_ws(&mut self) -> (bool, bool, Option<Token>) {
        let mut space = false;
        let mut nl = false;
        loop {
            let c = self.peek(0);
            match c {
                b' ' | b'\t' | b'\r' | 0x0c => {
                    space = true;
                    self.i += 1;
                }
                b'\n' => {
                    space = true;
                    nl = true;
                    self.newline();
                }
                b'\\' if self.peek(1) == b'\n' => {
                    // A continuation is not a line break for macro bodies.
                    space = true;
                    self.i += 1;
                    self.newline();
                }
                b'\\' if self.peek(1) == b'\r' && self.peek(2) == b'\n' => {
                    space = true;
                    self.i += 2;
                    self.newline();
                }
                b'/' if self.peek(1) == b'/' => {
                    space = true;
                    while self.i < self.b.len() && self.b[self.i] != b'\n' {
                        self.i += 1;
                    }
                }
                b'/' if self.peek(1) == b'*' => {
                    space = true;
                    let start = self.pos_at(self.i);
                    self.i += 2;
                    loop {
                        if self.i >= self.b.len() {
                            let p = start.clone();
                            let t = Token {
                                tok: Tok::Error("unterminated block comment".into()),
                                pos: p,
                                space_before: space,
                                newline_before: nl,
                                expansion: None,
                            };
                            return (space, nl, Some(t));
                        }
                        if self.b[self.i] == b'*' && self.peek(1) == b'/' {
                            self.i += 2;
                            break;
                        }
                        if self.b[self.i] == b'\n' {
                            nl = true;
                            self.newline();
                        } else {
                            self.i += 1;
                        }
                    }
                }
                _ => return (space, nl, None),
            }
        }
    }

    fn push(&mut self, tok: Tok, pos: Pos, space: bool, nl: bool) {
        self.out.push(Token {
            tok,
            pos,
            space_before: space,
            newline_before: nl,
            expansion: None,
        });
    }

    fn run(&mut self) {
        loop {
            let (space, nl, err) = self.skip_ws();
            if let Some(t) = err {
                self.out.push(t);
                continue;
            }
            let pos = self.pos_at(self.i);
            if self.i >= self.b.len() {
                self.push(Tok::Eof, pos, space, nl);
                return;
            }
            let c = self.b[self.i];
            let tok_start = self.i;
            let tok = if is_ident_start(c) {
                let s = self.i;
                while self.i < self.b.len() && is_ident_char(self.b[self.i]) {
                    self.i += 1;
                }
                Tok::Ident(String::from_utf8_lossy(&self.b[s..self.i]).into_owned())
            } else if c.is_ascii_digit() || (c == b'\'' && self.fill_or_based_follows()) {
                self.number()
            } else if c == b'"' {
                self.string()
            } else if c == b'$' && is_ident_char(self.peek(1)) && self.peek(1) != b'$' {
                let s = self.i;
                self.i += 1;
                while self.i < self.b.len() && is_ident_char(self.b[self.i]) {
                    self.i += 1;
                }
                Tok::SysName(String::from_utf8_lossy(&self.b[s..self.i]).into_owned())
            } else if c == b'`' && is_ident_start(self.peek(1)) {
                let s = self.i + 1;
                self.i += 1;
                while self.i < self.b.len() && is_ident_char(self.b[self.i]) {
                    self.i += 1;
                }
                Tok::Directive(String::from_utf8_lossy(&self.b[s..self.i]).into_owned())
            } else if c == b'\\' {
                let s = self.i;
                self.i += 1;
                while self.i < self.b.len() && !self.b[self.i].is_ascii_whitespace() {
                    self.i += 1;
                }
                if self.i == s + 1 {
                    Tok::Error("empty escaped identifier".into())
                } else {
                    Tok::Ident(String::from_utf8_lossy(&self.b[s..self.i]).into_owned())
                }
            } else if let Some(op) = OPS
                .iter()
                .find(|op| self.b[self.i..].starts_with(op.as_bytes()))
            {
                self.i += op.len();
                Tok::Op(op)
            } else {
                let shown = if c.is_ascii_graphic() {
                    (c as char).to_string()
                } else {
                    format!("byte 0x{c:02x}")
                };
                self.i += 1;
                Tok::Error(format!("unexpected character {shown}"))
            };
            if self.i == tok_start {
                // Every token consumes input; this guard makes that true even for a bad path.
                self.i += 1;
            }
            self.push(tok, pos, space, nl);
        }
    }

    /// `'` starts a based literal (`'h`, `'sb`) or an unsized fill (`'0`).
    fn fill_or_based_follows(&self) -> bool {
        let c = self.peek(1).to_ascii_lowercase();
        if matches!(c, b'b' | b'o' | b'd' | b'h') {
            return true;
        }
        if c == b's' {
            return matches!(self.peek(2).to_ascii_lowercase(), b'b' | b'o' | b'd' | b'h');
        }
        matches!(c, b'0' | b'1' | b'x' | b'z')
    }

    fn number(&mut self) -> Tok {
        let s = self.i;
        if self.b[self.i] == b'\'' {
            // Unsized: based part with no size, or a fill literal.
            if matches!(self.peek(1).to_ascii_lowercase(), b'0' | b'1' | b'x' | b'z')
                && !is_ident_char(self.peek(2))
            {
                let d = (self.peek(1) as char).to_ascii_lowercase();
                self.i += 2;
                return Tok::Num(Number {
                    size: None,
                    signed: false,
                    base: '\'',
                    digits: d.to_string(),
                    apostrophe: true,
                });
            }
            return self.based(None);
        }
        while self.i < self.b.len() && (self.b[self.i].is_ascii_digit() || self.b[self.i] == b'_') {
            self.i += 1;
        }
        // Real literal: digits '.' digits [exp] or digits exp.
        let frac = self.peek(0) == b'.' && self.peek(1).is_ascii_digit();
        let exp_at = if frac { self.i + 1 } else { self.i };
        let exp = matches!(self.b.get(exp_at), Some(b'e' | b'E'))
            && (self.b.get(exp_at + 1).is_some_and(|c| c.is_ascii_digit())
                || (matches!(self.b.get(exp_at + 1), Some(b'+' | b'-'))
                    && self.b.get(exp_at + 2).is_some_and(|c| c.is_ascii_digit())));
        if frac || exp {
            if frac {
                self.i += 1;
                while self.i < self.b.len() && self.b[self.i].is_ascii_digit() {
                    self.i += 1;
                }
            }
            if matches!(self.peek(0), b'e' | b'E') {
                self.i += 1;
                if matches!(self.peek(0), b'+' | b'-') {
                    self.i += 1;
                }
                while self.i < self.b.len() && self.b[self.i].is_ascii_digit() {
                    self.i += 1;
                }
            }
            return Tok::Real(String::from_utf8_lossy(&self.b[s..self.i]).into_owned());
        }
        let dec: String = String::from_utf8_lossy(&self.b[s..self.i])
            .chars()
            .filter(|c| *c != '_')
            .collect();
        if self.peek(0) == b'\'' && self.fill_or_based_follows() {
            let size = dec.parse::<u32>().ok();
            return self.based(size);
        }
        Tok::Num(Number {
            size: None,
            signed: false,
            base: 'd',
            digits: dec,
            apostrophe: false,
        })
    }

    /// Lexes `'[s]<base><digits>` with the size already read (or `None`).
    fn based(&mut self, size: Option<u32>) -> Tok {
        self.i += 1; // the apostrophe
        let mut signed = false;
        if matches!(self.peek(0), b's' | b'S') {
            signed = true;
            self.i += 1;
        }
        let base = self.peek(0).to_ascii_lowercase() as char;
        if !matches!(base, 'b' | 'o' | 'd' | 'h') {
            // The apostrophe (and `s`) are consumed: rewinding here would loop forever.
            return Tok::Error("apostrophe not followed by a base or a fill value".into());
        }
        self.i += 1;
        while matches!(self.peek(0), b' ' | b'\t') {
            self.i += 1;
        }
        let ds = self.i;
        while self.i < self.b.len()
            && (self.b[self.i].is_ascii_alphanumeric() || matches!(self.b[self.i], b'_' | b'?'))
        {
            self.i += 1;
        }
        let raw = String::from_utf8_lossy(&self.b[ds..self.i]).into_owned();
        if raw.is_empty() || !raw.bytes().all(|c| valid_digit(base, c)) {
            return Tok::Error(format!("invalid digits for base '{base}': `{raw}`"));
        }
        let digits: String = raw
            .chars()
            .filter(|c| *c != '_')
            .map(|c| c.to_ascii_lowercase())
            .collect();
        Tok::Num(Number {
            size,
            signed,
            base,
            digits,
            apostrophe: true,
        })
    }

    fn string(&mut self) -> Tok {
        let start_pos = self.i;
        self.i += 1;
        let mut s = String::new();
        loop {
            if self.i >= self.b.len() || self.b[self.i] == b'\n' {
                let _ = start_pos;
                return Tok::Error("unterminated string literal".into());
            }
            let c = self.b[self.i];
            if c == b'"' {
                self.i += 1;
                return Tok::Str(s);
            }
            if c == b'\\' && self.i + 1 < self.b.len() {
                let e = self.b[self.i + 1];
                self.i += 2;
                s.push(match e {
                    b'n' => '\n',
                    b't' => '\t',
                    b'\\' => '\\',
                    b'"' => '"',
                    other => other as char,
                });
                continue;
            }
            s.push(c as char);
            self.i += 1;
        }
    }
}

/// Renders a token back to source-like text (used for `timescale` and messages).
pub fn render(tok: &Tok) -> String {
    match tok {
        Tok::Ident(s) | Tok::SysName(s) | Tok::Real(s) => s.clone(),
        Tok::Directive(s) => format!("`{s}"),
        Tok::Num(n) => match (n.size, n.base) {
            (_, '\'') => format!("'{}", n.digits),
            (Some(sz), b) => format!("{sz}'{}{b}{}", if n.signed { "s" } else { "" }, n.digits),
            (None, 'd') => n.digits.clone(),
            (None, b) => format!("'{b}{}", n.digits),
        },
        Tok::Str(s) => format!("\"{s}\""),
        Tok::Op(o) => (*o).to_string(),
        Tok::Error(e) => format!("<error: {e}>"),
        Tok::Eof => String::new(),
    }
}
