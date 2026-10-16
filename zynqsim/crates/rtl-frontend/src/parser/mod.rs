//! Recursive-descent parser for Verilog-2001 and the synthesizable SystemVerilog
//! subset, plus the driver that preprocesses and parses files.
//!
//! Unsupported constructs are reported with [`Diagnostic`]s naming the construct;
//! the parser then skips to a point where it can continue. Every loop consumes a
//! token or stops, and each consumed token takes one unit of the step budget.

mod expr;
mod items;
mod stmt;

use crate::ast::{Item, SourceUnit};
use crate::diag::{Budget, Diagnostic, Pos};
use crate::lexer::{self, Tok, Token};
use crate::pp::{self, Macros};
use std::collections::BTreeSet;
use std::path::PathBuf;

/// Default step budget for one compilation (tokens consumed, macro expansions).
pub const DEFAULT_STEP_LIMIT: u64 = 20_000_000;

/// Options of a compilation.
#[derive(Debug, Clone)]
pub struct Options {
    /// Macro definitions in `NAME` or `NAME=VALUE` form (as on the command line).
    pub defines: Vec<String>,
    /// Directories searched by `` `include `` after the including file's directory.
    pub include_dirs: Vec<PathBuf>,
    /// Work budget for the whole compilation; exceeding it stops the passes.
    pub step_limit: u64,
}

impl Default for Options {
    fn default() -> Options {
        Options {
            defines: Vec::new(),
            include_dirs: Vec::new(),
            step_limit: DEFAULT_STEP_LIMIT,
        }
    }
}

/// The result of parsing one or more files.
#[derive(Debug, Clone, Default)]
pub struct Parsed {
    /// One unit per input file, in input order.
    pub units: Vec<SourceUnit>,
    /// All diagnostics, grouped by unit and sorted by position within a unit.
    pub diagnostics: Vec<Diagnostic>,
}

/// Parses files from disk. Macros and typedef names carry over from one file to
/// the next, as they do in a compilation.
pub fn parse_files(paths: &[PathBuf], opts: &Options) -> Parsed {
    let mut shared = Shared::new(opts);
    let mut out = Parsed::default();
    for path in paths {
        let name = path.to_string_lossy().into_owned();
        match std::fs::read(path) {
            Ok(bytes) => {
                let text = String::from_utf8_lossy(&bytes).into_owned();
                shared.parse_one(&name, &text, &mut out);
            }
            Err(e) => out.diagnostics.push(Diagnostic::new(
                &Pos::new(&name, 0, 0),
                "file",
                format!("cannot read file: {e}"),
            )),
        }
    }
    out
}

/// Parses in-memory text called `name`.
pub fn parse_text(name: &str, text: &str, opts: &Options) -> Parsed {
    let mut shared = Shared::new(opts);
    let mut out = Parsed::default();
    shared.parse_one(name, text, &mut out);
    out
}

/// State shared by the files of one compilation.
struct Shared<'o> {
    opts: &'o Options,
    macros: Macros,
    typedefs: BTreeSet<String>,
    budget: Budget,
}

impl<'o> Shared<'o> {
    fn new(opts: &'o Options) -> Shared<'o> {
        let mut macros = Macros::new();
        for spec in &opts.defines {
            let (name, m) = pp::define_from_cli(spec);
            macros.insert(name, m);
        }
        Shared {
            opts,
            macros,
            typedefs: BTreeSet::new(),
            budget: Budget::new(opts.step_limit),
        }
    }

    fn parse_one(&mut self, name: &str, text: &str, out: &mut Parsed) {
        let pre = pp::preprocess(
            name,
            text,
            &mut self.macros,
            &self.opts.include_dirs,
            &mut self.budget,
        );
        let mut diags = pre.diagnostics;
        let mut p = Parser {
            toks: pre.tokens,
            i: 0,
            diags: Vec::new(),
            budget: std::mem::replace(&mut self.budget, Budget::new(0)),
            typedefs: std::mem::take(&mut self.typedefs),
        };
        let items: Vec<Item> = p.source_unit();
        if p.budget.is_exhausted() {
            diags.push(Diagnostic::new(
                &Pos::new(name, 1, 1),
                "budget",
                "parsing stopped: step budget exhausted",
            ));
        }
        self.budget = p.budget;
        self.typedefs = p.typedefs;
        diags.extend(p.diags);
        diags.sort_by_key(|d| (d.line, d.column));
        out.diagnostics.extend(diags);
        out.units.push(SourceUnit {
            file: name.to_string(),
            items,
            timescale: pre.timescale,
            default_nettype: pre.default_nettype,
        });
    }
}

/// Keywords that cannot be used as identifiers.
const RESERVED: &[&str] = &[
    "always",
    "always_comb",
    "always_ff",
    "always_latch",
    "and",
    "assign",
    "automatic",
    "begin",
    "bit",
    "buf",
    "case",
    "casex",
    "casez",
    "default",
    "defparam",
    "disable",
    "else",
    "end",
    "endcase",
    "endfunction",
    "endgenerate",
    "endmodule",
    "endtask",
    "for",
    "forever",
    "function",
    "generate",
    "genvar",
    "if",
    "initial",
    "inout",
    "input",
    "int",
    "integer",
    "localparam",
    "logic",
    "module",
    "nand",
    "negedge",
    "nor",
    "not",
    "or",
    "output",
    "parameter",
    "posedge",
    "reg",
    "repeat",
    "return",
    "signed",
    "task",
    "time",
    "typedef",
    "unsigned",
    "wait",
    "while",
    "wire",
    "xnor",
    "xor",
];

/// Keywords that name a data type or a net type at the start of a declaration.
pub(crate) const DATA_KEYWORDS: &[&str] = &[
    "wire", "tri", "tri0", "tri1", "wand", "wor", "supply0", "supply1", "reg", "logic", "bit",
    "integer", "int", "byte", "shortint", "longint", "time", "genvar",
];

/// Drive strengths that may follow `wire`/`tri` (`tri (weak1, strong0) x`).
pub(crate) const STRENGTHS: &[&str] = &[
    "supply0", "supply1", "strong0", "strong1", "pull0", "pull1", "weak0", "weak1", "highz0",
    "highz1",
];

/// Parser state: the token stream, the cursor and what has been found so far.
pub(crate) struct Parser {
    toks: Vec<Token>,
    i: usize,
    pub(crate) diags: Vec<Diagnostic>,
    pub(crate) budget: Budget,
    pub(crate) typedefs: BTreeSet<String>,
}

impl Parser {
    pub(crate) fn tok(&self) -> &Tok {
        &self.toks[self.i].tok
    }

    pub(crate) fn tok_at(&self, k: usize) -> &Tok {
        let idx = (self.i + k).min(self.toks.len() - 1);
        &self.toks[idx].tok
    }

    pub(crate) fn pos(&self) -> Pos {
        self.toks[self.i].pos.clone()
    }

    pub(crate) fn at_end(&self) -> bool {
        matches!(self.tok(), Tok::Eof)
    }

    /// Consumes the current token (never past `Eof`).
    pub(crate) fn bump(&mut self) {
        if self.i + 1 < self.toks.len() {
            self.i += 1;
        }
        if !self.budget.take() {
            self.i = self.toks.len() - 1;
        }
    }

    pub(crate) fn is_op(&self, o: &str) -> bool {
        matches!(self.tok(), Tok::Op(x) if *x == o)
    }

    pub(crate) fn is_op_at(&self, k: usize, o: &str) -> bool {
        matches!(self.tok_at(k), Tok::Op(x) if *x == o)
    }

    pub(crate) fn is_kw(&self, k: &str) -> bool {
        matches!(self.tok(), Tok::Ident(s) if s == k)
    }

    pub(crate) fn eat_op(&mut self, o: &str) -> bool {
        if self.is_op(o) {
            self.bump();
            true
        } else {
            false
        }
    }

    pub(crate) fn eat_kw(&mut self, k: &str) -> bool {
        if self.is_kw(k) {
            self.bump();
            true
        } else {
            false
        }
    }

    /// Consumes `o` or records a syntax error (without consuming anything).
    pub(crate) fn expect_op(&mut self, o: &str) -> bool {
        if self.eat_op(o) {
            true
        } else {
            let found = self.describe();
            self.err("syntax", format!("expected `{o}`, found {found}"));
            false
        }
    }

    /// Short description of the current token for messages.
    pub(crate) fn describe(&self) -> String {
        match self.tok() {
            Tok::Eof => "end of file".to_string(),
            Tok::Ident(s) => format!("`{s}`"),
            Tok::Op(o) => format!("`{o}`"),
            other => format!("`{}`", lexer::render(other)),
        }
    }

    pub(crate) fn err(&mut self, construct: &str, msg: impl Into<String>) {
        let p = self.pos();
        self.diags.push(Diagnostic::new(&p, construct, msg));
    }

    pub(crate) fn err_at(&mut self, pos: &Pos, construct: &str, msg: impl Into<String>) {
        self.diags.push(Diagnostic::new(pos, construct, msg));
    }

    /// Takes an identifier that is not a reserved word.
    pub(crate) fn ident(&mut self) -> Option<(String, Pos)> {
        match self.tok() {
            Tok::Ident(s) if !RESERVED.contains(&s.as_str()) => {
                let s = s.clone();
                let p = self.pos();
                self.bump();
                Some((s, p))
            }
            _ => None,
        }
    }

    /// Takes an identifier or records an error.
    pub(crate) fn expect_ident(&mut self, what: &str) -> Option<(String, Pos)> {
        match self.ident() {
            Some(x) => Some(x),
            None => {
                let found = self.describe();
                self.err("syntax", format!("expected {what}, found {found}"));
                None
            }
        }
    }

    /// Skips the rest of a broken statement or declaration: up to and including
    /// the next `;` at nesting depth zero, stopping before a block-closing keyword.
    pub(crate) fn recover(&mut self) {
        let mut depth = 0i32;
        loop {
            if self.at_end() {
                return;
            }
            match self.tok() {
                Tok::Op("(" | "[" | "{") => depth += 1,
                Tok::Op(")" | "]" | "}") => depth -= 1,
                Tok::Op(";") if depth <= 0 => {
                    self.bump();
                    return;
                }
                Tok::Ident(s)
                    if depth <= 0
                        && matches!(
                            s.as_str(),
                            "endmodule"
                                | "end"
                                | "endcase"
                                | "endgenerate"
                                | "endfunction"
                                | "endtask"
                                | "endinterface"
                                | "endpackage"
                                | "endclass"
                                | "endprogram"
                                | "endgroup"
                                | "endspecify"
                        ) =>
                {
                    return;
                }
                _ => {}
            }
            self.bump();
        }
    }

    /// Skips a balanced group starting at the current `(`, `[` or `{`.
    pub(crate) fn skip_group(&mut self) {
        let mut depth = 0i32;
        loop {
            if self.at_end() {
                return;
            }
            match self.tok() {
                Tok::Op("(" | "[" | "{") => depth += 1,
                Tok::Op(")" | "]" | "}") => depth -= 1,
                _ => {}
            }
            self.bump();
            if depth <= 0 {
                return;
            }
        }
    }

    /// Skips an unsupported block up to and including the keyword `end_kw`.
    pub(crate) fn skip_block_to(&mut self, end_kw: &str) {
        while !self.at_end() {
            if self.is_kw(end_kw) {
                self.bump();
                if self.eat_op(":") {
                    let _ = self.ident();
                }
                return;
            }
            self.bump();
        }
    }

    /// Skips the rest of an unsupported construct that ends at the next `;`.
    pub(crate) fn skip_to_semi(&mut self) {
        let mut depth = 0i32;
        while !self.at_end() {
            match self.tok() {
                Tok::Op("(" | "[" | "{") => depth += 1,
                Tok::Op(")" | "]" | "}") => depth -= 1,
                Tok::Op(";") if depth <= 0 => {
                    self.bump();
                    return;
                }
                _ => {}
            }
            self.bump();
        }
    }

    /// Skips an attribute instance `(* ... *)` if one starts at the cursor.
    pub(crate) fn skip_attribute(&mut self) -> bool {
        if !(self.is_op("(") && self.is_op_at(1, "*") && !self.is_op_at(2, ")")) {
            return false;
        }
        self.bump();
        self.bump();
        while !self.at_end() && !(self.is_op("*") && self.is_op_at(1, ")")) {
            self.bump();
        }
        self.bump();
        self.bump();
        true
    }

    /// Makes sure a loop that called a parse function made progress.
    pub(crate) fn ensure_progress(&mut self, before: usize) {
        if self.i == before && !self.at_end() {
            self.bump();
        }
    }
}
