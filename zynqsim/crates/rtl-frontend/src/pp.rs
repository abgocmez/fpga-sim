//! Preprocessor: `define` (object-like and function-like macros), `ifdef`/`ifndef`/
//! `elsif`/`else`/`endif`, `include` (relative to the including file, then the
//! include directories), `timescale`, `default_nettype`, and the directives that are
//! accepted and ignored. Positions of macro expansions point at the use site.

use crate::diag::{Budget, Diagnostic, Pos};
use crate::lexer::{self, Tok, Token};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Maximum nesting of macro expansions and includes.
const MAX_DEPTH: u32 = 64;
/// Maximum nesting of includes.
const MAX_INCLUDE_DEPTH: u32 = 32;

/// A macro definition.
#[derive(Debug, Clone, PartialEq)]
pub struct Macro {
    /// Formal parameter names for a function-like macro, `None` for an object-like one.
    pub params: Option<Vec<String>>,
    /// Default value of each formal parameter (`name = value`), if given.
    pub defaults: Vec<Option<Vec<Token>>>,
    /// Replacement tokens (their positions are the definition's).
    pub body: Vec<Token>,
}

/// Macro table shared by all the files of one compilation.
pub type Macros = BTreeMap<String, Macro>;

/// Result of preprocessing one top-level file (with everything it includes).
#[derive(Debug, Clone, Default)]
pub struct Output {
    /// Surviving tokens, ending with [`Tok::Eof`].
    pub tokens: Vec<Token>,
    /// Problems found while preprocessing.
    pub diagnostics: Vec<Diagnostic>,
    /// Argument of the last `` `timescale `` in the file, if any.
    pub timescale: Option<String>,
    /// Argument of the last `` `default_nettype `` in the file, if any.
    pub default_nettype: Option<String>,
}

#[derive(Debug, Clone, Copy)]
struct Cond {
    /// This branch is the one being compiled.
    active: bool,
    /// Some branch of this `ifdef` chain was already taken.
    taken: bool,
    /// The enclosing region is active (an inactive parent keeps every branch off).
    parent_active: bool,
    /// An `else` was seen (a further `else` or `elsif` is an error).
    else_seen: bool,
}

/// Preprocesses `text`, read from a file called `file`.
pub fn preprocess(
    file: &str,
    text: &str,
    macros: &mut Macros,
    include_dirs: &[PathBuf],
    budget: &mut Budget,
) -> Output {
    let mut pp = Pp {
        macros,
        include_dirs,
        budget,
        out: Vec::new(),
        diags: Vec::new(),
        conds: Vec::new(),
        dirs: Vec::new(),
        includes: Vec::new(),
        timescale: None,
        default_nettype: None,
    };
    pp.process_file(file, text);
    let eof_pos = Pos::new(file, 1, 1);
    pp.out.push(Token {
        tok: Tok::Eof,
        pos: eof_pos,
        space_before: false,
        newline_before: false,
        expansion: None,
    });
    if !pp.conds.is_empty() {
        pp.diags.push(Diagnostic::new(
            &Pos::new(file, 1, 1),
            "conditional",
            "`ifdef/`ifndef without a matching `endif at end of file",
        ));
    }
    Output {
        tokens: pp.out,
        diagnostics: pp.diags,
        timescale: pp.timescale,
        default_nettype: pp.default_nettype,
    }
}

struct Pp<'a> {
    macros: &'a mut Macros,
    include_dirs: &'a [PathBuf],
    budget: &'a mut Budget,
    out: Vec<Token>,
    diags: Vec<Diagnostic>,
    conds: Vec<Cond>,
    /// Directory of each file being processed (innermost last).
    dirs: Vec<PathBuf>,
    /// Files being included (cycle detection).
    includes: Vec<String>,
    timescale: Option<String>,
    default_nettype: Option<String>,
}

fn is_op(t: &Token, s: &str) -> bool {
    matches!(&t.tok, Tok::Op(o) if *o == s)
}

impl Pp<'_> {
    fn active(&self) -> bool {
        self.conds.last().is_none_or(|c| c.active)
    }

    fn error(&mut self, pos: &Pos, construct: &str, msg: impl Into<String>) {
        self.diags.push(Diagnostic::new(pos, construct, msg));
    }

    /// Processes the tokens of a file (already lexed by `lexer::lex`).
    fn process_file(&mut self, name: &str, text: &str) {
        let toks = lexer::lex(name, text);
        let dir = Path::new(name)
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        self.dirs.push(dir);
        self.run(&toks, 0);
        self.dirs.pop();
    }

    /// Walks a token list, executing directives and expanding macros.
    fn run(&mut self, toks: &[Token], depth: u32) {
        let mut i = 0;
        while i < toks.len() {
            if !self.budget.take() {
                let p = toks[i].pos.clone();
                self.error(&p, "budget", "preprocessing stopped: step budget exhausted");
                return;
            }
            let t = &toks[i];
            match &t.tok {
                Tok::Eof => break,
                Tok::Directive(name) => {
                    let name = name.clone();
                    i = self.directive(&name, toks, i, depth);
                }
                Tok::Error(e) => {
                    if self.active() {
                        let msg = e.clone();
                        self.error(&t.pos, "lexical", msg);
                    }
                    i += 1;
                }
                _ => {
                    if self.active() {
                        self.out.push(t.clone());
                    }
                    i += 1;
                }
            }
        }
    }

    /// Index after a directive that takes one macro name: the name is skipped, nothing
    /// else. Conditional directives must not swallow the rest of an expansion.
    fn after_name(toks: &[Token], i: usize) -> usize {
        match toks.get(i + 1).map(|t| &t.tok) {
            Some(Tok::Ident(_)) => i + 2,
            _ => i + 1,
        }
    }

    /// Index of the first token after the directive's line (the rest of the line).
    fn line_end(toks: &[Token], i: usize) -> usize {
        let mut j = i + 1;
        while j < toks.len() && !toks[j].newline_before && !matches!(toks[j].tok, Tok::Eof) {
            j += 1;
        }
        j
    }

    fn directive(&mut self, name: &str, toks: &[Token], i: usize, depth: u32) -> usize {
        let pos = toks[i].pos.clone();
        match name {
            "ifdef" | "ifndef" => {
                let parent = self.active();
                let defined = match toks.get(i + 1).map(|t| &t.tok) {
                    Some(Tok::Ident(n)) => self.macros.contains_key(n),
                    _ => {
                        self.error(&pos, "conditional", format!("`{name} needs a macro name"));
                        false
                    }
                };
                let cond = if name == "ifdef" { defined } else { !defined };
                let active = parent && cond;
                self.conds.push(Cond {
                    active,
                    taken: active,
                    parent_active: parent,
                    else_seen: false,
                });
                return Self::after_name(toks, i);
            }
            "elsif" => {
                let Some(top) = self.conds.last().copied() else {
                    self.error(&pos, "conditional", "`elsif without `ifdef");
                    return Self::line_end(toks, i);
                };
                if top.else_seen {
                    self.error(&pos, "conditional", "`elsif after `else");
                }
                let defined = match toks.get(i + 1).map(|t| &t.tok) {
                    Some(Tok::Ident(n)) => self.macros.contains_key(n),
                    _ => false,
                };
                let active = top.parent_active && !top.taken && defined;
                if let Some(c) = self.conds.last_mut() {
                    c.active = active;
                    c.taken |= active;
                }
                return Self::after_name(toks, i);
            }
            "else" => {
                match self.conds.last().copied() {
                    None => self.error(&pos, "conditional", "`else without `ifdef"),
                    Some(c) => {
                        if c.else_seen {
                            self.error(&pos, "conditional", "second `else in the same `ifdef");
                        }
                        if let Some(top) = self.conds.last_mut() {
                            top.active = c.parent_active && !c.taken;
                            top.taken = true;
                            top.else_seen = true;
                        }
                    }
                }
                return i + 1;
            }
            "endif" => {
                if self.conds.pop().is_none() {
                    self.error(&pos, "conditional", "`endif without `ifdef");
                }
                return i + 1;
            }
            _ => {}
        }
        if !self.active() {
            return i + 1;
        }
        match name {
            "define" => self.define(toks, i),
            "undef" => {
                if let Some(Tok::Ident(n)) = toks.get(i + 1).map(|t| &t.tok) {
                    self.macros.remove(n);
                } else {
                    self.error(&pos, "directive", "`undef needs a macro name");
                }
                Self::after_name(toks, i)
            }
            "include" => {
                let next = if matches!(toks.get(i + 1).map(|t| &t.tok), Some(Tok::Str(_))) {
                    i + 2
                } else {
                    i + 1
                };
                match toks.get(i + 1).map(|t| t.tok.clone()) {
                    Some(Tok::Str(path)) => self.include(&path, &pos, depth),
                    _ => self.error(&pos, "include", "`include needs a quoted file name"),
                }
                next
            }
            "timescale" => {
                let end = Self::line_end(toks, i);
                let text: String = toks[i + 1..end]
                    .iter()
                    .map(|t| lexer::render(&t.tok))
                    .collect();
                self.timescale = Some(text);
                end
            }
            "default_nettype" => {
                if let Some(Tok::Ident(n)) = toks.get(i + 1).map(|t| &t.tok) {
                    self.default_nettype = Some(n.clone());
                }
                Self::after_name(toks, i)
            }
            "resetall"
            | "celldefine"
            | "endcelldefine"
            | "unconnected_drive"
            | "nounconnected_drive"
            | "begin_keywords"
            | "end_keywords"
            | "line"
            | "pragma" => Self::line_end(toks, i),
            "__FILE__" => {
                let file = pos.file.clone();
                self.out.push(Token {
                    tok: Tok::Str(file),
                    pos,
                    space_before: toks[i].space_before,
                    newline_before: false,
                    expansion: None,
                });
                i + 1
            }
            "__LINE__" => {
                let line = pos.line;
                self.out.push(Token {
                    tok: Tok::Num(crate::lexer::Number {
                        size: None,
                        signed: false,
                        base: 'd',
                        digits: line.to_string(),
                        apostrophe: false,
                    }),
                    pos,
                    space_before: toks[i].space_before,
                    newline_before: false,
                    expansion: None,
                });
                i + 1
            }
            _ => self.expand(name, toks, i, depth),
        }
    }

    fn define(&mut self, toks: &[Token], i: usize) -> usize {
        let pos = toks[i].pos.clone();
        let end = Self::line_end(toks, i);
        let Some(Tok::Ident(name)) = toks.get(i + 1).map(|t| t.tok.clone()) else {
            self.error(&pos, "directive", "`define needs a macro name");
            return end;
        };
        let mut j = i + 2;
        let mut params = None;
        let mut defaults = Vec::new();
        if j < end && is_op(&toks[j], "(") && !toks[j].space_before {
            let mut names = Vec::new();
            j += 1;
            loop {
                if let (true, Some(Tok::Ident(p))) = (j < end, toks.get(j).map(|t| &t.tok)) {
                    names.push(p.clone());
                    j += 1;
                    // Optional default: `name = tokens` up to `,` or `)` at depth zero.
                    let mut dflt = None;
                    if j < end && is_op(&toks[j], "=") {
                        j += 1;
                        let mut nest = 0i32;
                        let mut v = Vec::new();
                        while j < end {
                            let t = &toks[j];
                            if nest == 0 && (is_op(t, ",") || is_op(t, ")")) {
                                break;
                            }
                            if matches!(t.tok, Tok::Op("(" | "[" | "{")) {
                                nest += 1;
                            } else if matches!(t.tok, Tok::Op(")" | "]" | "}")) {
                                nest -= 1;
                            }
                            v.push(t.clone());
                            j += 1;
                        }
                        dflt = Some(v);
                    }
                    defaults.push(dflt);
                }
                if j < end && is_op(&toks[j], ",") {
                    j += 1;
                    continue;
                }
                if j < end && is_op(&toks[j], ")") {
                    j += 1;
                    break;
                }
                self.error(
                    &pos,
                    "macro",
                    format!("malformed parameter list of macro `{name}`"),
                );
                return end;
            }
            params = Some(names);
        }
        let body: Vec<Token> = toks[j.min(end)..end].to_vec();
        self.macros.insert(
            name,
            Macro {
                params,
                defaults,
                body,
            },
        );
        end
    }

    fn include(&mut self, path: &str, pos: &Pos, depth: u32) {
        if depth > MAX_INCLUDE_DEPTH || self.includes.len() as u32 > MAX_INCLUDE_DEPTH {
            self.error(
                pos,
                "include",
                format!("include nesting too deep at `{path}`"),
            );
            return;
        }
        let mut candidates = Vec::new();
        if let Some(d) = self.dirs.last() {
            candidates.push(d.join(path));
        }
        for d in self.include_dirs {
            candidates.push(d.join(path));
        }
        candidates.push(PathBuf::from(path));
        let Some(found) = candidates.into_iter().find(|p| p.is_file()) else {
            self.error(
                pos,
                "include",
                format!("cannot find included file `{path}`"),
            );
            return;
        };
        let shown = found.to_string_lossy().into_owned();
        if self.includes.contains(&shown) {
            self.error(pos, "include", format!("recursive include of `{path}`"));
            return;
        }
        match std::fs::read(&found) {
            Ok(bytes) => {
                let text = String::from_utf8_lossy(&bytes).into_owned();
                self.includes.push(shown.clone());
                self.process_file(&shown, &text);
                self.includes.pop();
            }
            Err(e) => self.error(pos, "include", format!("cannot read `{path}`: {e}")),
        }
    }

    /// Expands the macro `name` used at `toks[i]`. Returns the index after the use.
    fn expand(&mut self, name: &str, toks: &[Token], i: usize, depth: u32) -> usize {
        let use_pos = toks[i].pos.clone();
        let Some(mac) = self.macros.get(name).cloned() else {
            self.error(&use_pos, "macro", format!("undefined macro `{name}"));
            return i + 1;
        };
        if depth >= MAX_DEPTH {
            self.error(
                &use_pos,
                "macro",
                format!("macro expansion of `{name} is too deep (recursive?)"),
            );
            return i + 1;
        }
        let mut next = i + 1;
        let mut out: Vec<Token> = Vec::new();
        let mk = |t: &Token| Token {
            tok: t.tok.clone(),
            pos: use_pos.clone(),
            space_before: t.space_before,
            newline_before: false,
            expansion: Some(name.to_string()),
        };
        if let Some(params) = mac.params.clone() {
            if !toks.get(next).is_some_and(|t| is_op(t, "(")) {
                self.error(
                    &use_pos,
                    "macro",
                    format!("macro `{name} needs an argument list"),
                );
                return next;
            }
            next += 1;
            let mut args: Vec<Vec<Token>> = vec![Vec::new()];
            let mut nest = 0i32;
            let mut closed = false;
            while next < toks.len() {
                let t = &toks[next];
                if matches!(t.tok, Tok::Eof) {
                    break;
                }
                next += 1;
                if nest == 0 && is_op(t, ")") {
                    closed = true;
                    break;
                }
                if nest == 0 && is_op(t, ",") {
                    args.push(Vec::new());
                    continue;
                }
                if matches!(t.tok, Tok::Op("(" | "[" | "{")) {
                    nest += 1;
                } else if matches!(t.tok, Tok::Op(")" | "]" | "}")) {
                    nest -= 1;
                }
                args.last_mut().expect("at least one").push(t.clone());
            }
            if !closed {
                self.error(
                    &use_pos,
                    "macro",
                    format!("unterminated argument list of `{name}"),
                );
                return next;
            }
            if params.is_empty() && args.len() == 1 && args[0].is_empty() {
                args.clear();
            }
            if args.len() > params.len() {
                self.error(
                    &use_pos,
                    "macro",
                    format!(
                        "`{name} takes {} arguments, got {}",
                        params.len(),
                        args.len()
                    ),
                );
                return next;
            }
            // Missing or empty arguments take the default value, if there is one.
            let mut resolved: Vec<Vec<Token>> = Vec::new();
            for (k, formal) in params.iter().enumerate() {
                let given = args.get(k).filter(|a| !a.is_empty());
                match (given, mac.defaults.get(k).cloned().flatten()) {
                    (Some(a), _) => resolved.push(a.clone()),
                    (None, Some(d)) => resolved.push(d),
                    (None, None) => {
                        self.error(
                            &use_pos,
                            "macro",
                            format!("`{name} needs a value for argument `{formal}"),
                        );
                        return next;
                    }
                }
            }
            let body = &mac.body;
            let mut b = 0;
            while b < body.len() {
                let t = &body[b];
                // Stringification: `"arg`" becomes one string literal.
                if matches!(t.tok, Tok::Op("`\"")) {
                    if let (
                        Some(Token {
                            tok: Tok::Ident(p), ..
                        }),
                        Some(close),
                    ) = (body.get(b + 1), body.get(b + 2))
                    {
                        if matches!(close.tok, Tok::Op("`\"")) && params.contains(p) {
                            let k = params.iter().position(|x| x == p).expect("contains");
                            let text: Vec<String> =
                                resolved[k].iter().map(|a| lexer::render(&a.tok)).collect();
                            out.push(Token {
                                tok: Tok::Str(text.join(" ")),
                                pos: use_pos.clone(),
                                space_before: t.space_before,
                                newline_before: false,
                                expansion: Some(name.to_string()),
                            });
                            b += 3;
                            continue;
                        }
                    }
                }
                match &t.tok {
                    Tok::Ident(p) if params.contains(p) => {
                        let k = params.iter().position(|x| x == p).expect("contains");
                        for a in &resolved[k] {
                            out.push(Token {
                                tok: a.tok.clone(),
                                pos: use_pos.clone(),
                                space_before: a.space_before,
                                newline_before: false,
                                expansion: Some(name.to_string()),
                            });
                        }
                    }
                    _ => out.push(mk(t)),
                }
                b += 1;
            }
        } else {
            out.extend(mac.body.iter().map(mk));
        }
        if !self.budget.take() {
            self.error(
                &use_pos,
                "budget",
                "macro expansion stopped: step budget exhausted",
            );
            return next;
        }
        if out.len() > 100_000 {
            self.error(
                &use_pos,
                "macro",
                format!("expansion of `{name} is too large"),
            );
            return next;
        }
        // Textual pasting: `W'd4 with `W` = 32 is the text 32'd4.
        let joined = match (out.last(), toks.get(next)) {
            (
                Some(Token {
                    tok: Tok::Num(n1), ..
                }),
                Some(Token {
                    tok: Tok::Num(n2),
                    space_before: false,
                    ..
                }),
            ) if n2.apostrophe
                && n1.size.is_none()
                && n1.base == 'd'
                && !n1.apostrophe
                && n1.digits.bytes().all(|c| c.is_ascii_digit()) =>
            {
                Some(crate::lexer::Number {
                    size: n1.digits.parse().ok(),
                    signed: n2.signed,
                    base: n2.base,
                    digits: n2.digits.clone(),
                    apostrophe: true,
                })
            }
            _ => None,
        };
        if let (Some(num), Some(last)) = (joined, out.last_mut()) {
            last.tok = Tok::Num(num);
            next += 1;
        }
        self.run(&out, depth + 1);
        next
    }
}

/// Parses a `-D NAME[=VALUE]` definition into a macro table entry.
pub fn define_from_cli(spec: &str) -> (String, Macro) {
    let (name, value) = match spec.split_once('=') {
        Some((n, v)) => (n.trim().to_string(), v.to_string()),
        None => (spec.trim().to_string(), "1".to_string()),
    };
    let body: Vec<Token> = lexer::lex("<command-line>", &value)
        .into_iter()
        .filter(|t| !matches!(t.tok, Tok::Eof))
        .collect();
    (
        name,
        Macro {
            params: None,
            defaults: Vec::new(),
            body,
        },
    )
}
