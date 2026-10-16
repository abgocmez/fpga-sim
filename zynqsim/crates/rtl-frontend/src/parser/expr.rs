//! Expression parsing with the Verilog operator precedence table.

use super::{Parser, RESERVED};
use crate::ast::{BinOp, Expr, ExprKind, UnOp};
use crate::diag::Pos;
use crate::lexer::{Number, Tok};

/// System tasks and functions the front end knows. Others are reported.
pub(crate) const KNOWN_SYSTEM: &[&str] = &[
    "$display",
    "$write",
    "$strobe",
    "$monitor",
    "$finish",
    "$stop",
    "$fatal",
    "$error",
    "$warning",
    "$info",
    "$readmemh",
    "$readmemb",
    "$dumpfile",
    "$dumpvars",
    "$dumpflush",
    "$dumpoff",
    "$dumpon",
    "$time",
    "$stime",
    "$realtime",
    "$random",
    "$urandom",
    "$urandom_range",
    "$signed",
    "$unsigned",
    "$clog2",
    "$bits",
    "$fopen",
    "$fclose",
    "$fwrite",
    "$fdisplay",
    "$fflush",
    "$sformatf",
    "$sformat",
    "$test$plusargs",
    "$value$plusargs",
    "$feof",
    "$isunknown",
    "$countones",
    "$onehot",
    "$onehot0",
];

/// Binary operator and its precedence (higher binds tighter).
fn binop(t: &Tok) -> Option<(BinOp, u8)> {
    let Tok::Op(o) = t else { return None };
    Some(match *o {
        "||" => (BinOp::LOr, 1),
        "&&" => (BinOp::LAnd, 2),
        "|" => (BinOp::Or, 3),
        "^" => (BinOp::Xor, 4),
        "~^" | "^~" => (BinOp::Xnor, 4),
        "&" => (BinOp::And, 5),
        "==" => (BinOp::Eq, 6),
        "!=" => (BinOp::Ne, 6),
        "===" => (BinOp::CaseEq, 6),
        "!==" => (BinOp::CaseNe, 6),
        "<" => (BinOp::Lt, 7),
        "<=" => (BinOp::Le, 7),
        ">" => (BinOp::Gt, 7),
        ">=" => (BinOp::Ge, 7),
        "<<" => (BinOp::Shl, 8),
        ">>" => (BinOp::Shr, 8),
        "<<<" => (BinOp::AShl, 8),
        ">>>" => (BinOp::AShr, 8),
        "+" => (BinOp::Add, 9),
        "-" => (BinOp::Sub, 9),
        "*" => (BinOp::Mul, 10),
        "/" => (BinOp::Div, 10),
        "%" => (BinOp::Mod, 10),
        "**" => (BinOp::Pow, 11),
        _ => return None,
    })
}

/// Unary operator at the current token (reduction forms included).
fn unop(t: &Tok) -> Option<UnOp> {
    let Tok::Op(o) = t else { return None };
    Some(match *o {
        "+" => UnOp::Plus,
        "-" => UnOp::Minus,
        "!" => UnOp::LNot,
        "~" => UnOp::BitNot,
        "&" => UnOp::RedAnd,
        "~&" => UnOp::RedNand,
        "|" => UnOp::RedOr,
        "~|" => UnOp::RedNor,
        "^" => UnOp::RedXor,
        "~^" | "^~" => UnOp::RedXnor,
        _ => return None,
    })
}

pub(crate) fn num_expr(pos: Pos, value: u64) -> Expr {
    Expr {
        pos,
        kind: ExprKind::Number(Number {
            size: None,
            signed: false,
            base: 'd',
            digits: value.to_string(),
            apostrophe: false,
        }),
    }
}

impl Parser {
    /// A full expression (ternary and everything below it).
    pub(crate) fn expr(&mut self) -> Expr {
        let c = self.binary(1);
        if self.is_op("?") {
            let pos = c.pos.clone();
            self.bump();
            let t = self.expr();
            self.expect_op(":");
            let f = self.expr();
            return Expr {
                pos,
                kind: ExprKind::Ternary(Box::new(c), Box::new(t), Box::new(f)),
            };
        }
        c
    }

    /// Comma-separated expressions (at least one).
    pub(crate) fn expr_list(&mut self) -> Vec<Expr> {
        let mut v = vec![self.expr()];
        while self.eat_op(",") {
            v.push(self.expr());
        }
        v
    }

    fn binary(&mut self, min: u8) -> Expr {
        let mut lhs = self.unary();
        while let Some((op, prec)) = binop(self.tok()) {
            if prec < min {
                break;
            }
            let pos = lhs.pos.clone();
            self.bump();
            let rhs = self.binary(prec + 1);
            lhs = Expr {
                pos,
                kind: ExprKind::Binary(op, Box::new(lhs), Box::new(rhs)),
            };
        }
        lhs
    }

    fn unary(&mut self) -> Expr {
        if let Some(op) = unop(self.tok()) {
            let pos = self.pos();
            self.bump();
            let e = self.unary();
            return Expr {
                pos,
                kind: ExprKind::Unary(op, Box::new(e)),
            };
        }
        let p = self.primary();
        self.postfix_rest(p)
    }

    /// Bit and part selects following an expression.
    pub(crate) fn postfix_rest(&mut self, mut e: Expr) -> Expr {
        while self.is_op("[") {
            let pos = e.pos.clone();
            self.bump();
            let first = self.expr();
            if self.eat_op(":") {
                let lsb = self.expr();
                self.expect_op("]");
                e = Expr {
                    pos,
                    kind: ExprKind::Part(Box::new(e), Box::new(first), Box::new(lsb)),
                };
            } else if self.is_op("+:") || self.is_op("-:") {
                let up = self.is_op("+:");
                self.bump();
                let width = self.expr();
                self.expect_op("]");
                e = Expr {
                    pos,
                    kind: ExprKind::IndexedPart {
                        base: Box::new(e),
                        start: Box::new(first),
                        width: Box::new(width),
                        up,
                    },
                };
            } else {
                self.expect_op("]");
                e = Expr {
                    pos,
                    kind: ExprKind::Index(Box::new(e), Box::new(first)),
                };
            }
        }
        e
    }

    /// `( [expr {, expr}] )` for calls.
    pub(crate) fn call_args(&mut self) -> Vec<Expr> {
        self.expect_op("(");
        if self.eat_op(")") {
            return Vec::new();
        }
        let args = self.expr_list();
        self.expect_op(")");
        args
    }

    fn zero(&self, pos: Pos) -> Expr {
        num_expr(pos, 0)
    }

    pub(crate) fn primary(&mut self) -> Expr {
        let pos = self.pos();
        match self.tok().clone() {
            Tok::Num(n) => {
                self.bump();
                // Size cast: 8'(expr).
                if n.size.is_none() && n.base == 'd' && self.is_op("'") && self.is_op_at(1, "(") {
                    self.bump();
                    self.bump();
                    let e = self.expr();
                    self.expect_op(")");
                    return Expr {
                        pos,
                        kind: ExprKind::Cast(n.digits.parse::<u32>().ok(), Box::new(e)),
                    };
                }
                Expr {
                    pos,
                    kind: ExprKind::Number(n),
                }
            }
            Tok::Real(r) => {
                self.err_at(
                    &pos,
                    "real literal",
                    format!("real literal `{r}` is not supported by the native front end"),
                );
                self.bump();
                self.zero(pos)
            }
            Tok::Str(s) => {
                self.bump();
                Expr {
                    pos,
                    kind: ExprKind::Str(s),
                }
            }
            Tok::SysName(name) => {
                self.bump();
                if !super::expr::KNOWN_SYSTEM.contains(&name.as_str()) {
                    self.err_at(
                        &pos,
                        "system task",
                        format!("system task `{name}` is not supported by the native front end"),
                    );
                }
                let args = if self.is_op("(") {
                    self.call_args()
                } else {
                    Vec::new()
                };
                Expr {
                    pos,
                    kind: ExprKind::SysCall(name, args),
                }
            }
            Tok::Ident(kw)
                if (kw == "signed" || kw == "unsigned")
                    && self.is_op_at(1, "'")
                    && self.is_op_at(2, "(") =>
            {
                self.bump();
                self.bump();
                self.bump();
                let e = self.expr();
                self.expect_op(")");
                let name = if kw == "signed" {
                    "$signed"
                } else {
                    "$unsigned"
                };
                Expr {
                    pos,
                    kind: ExprKind::SysCall(name.to_string(), vec![e]),
                }
            }
            Tok::Ident(first) if !RESERVED.contains(&first.as_str()) => {
                self.bump();
                let mut name = first;
                if self.is_op("::") {
                    self.err_at(
                        &pos,
                        "package reference",
                        format!("package-scoped name after `{name}::` is not supported"),
                    );
                    self.bump();
                    if let Tok::Ident(s) = self.tok().clone() {
                        self.bump();
                        name = format!("{name}::{s}");
                    }
                }
                while self.is_op(".") {
                    let Tok::Ident(part) = self.tok_at(1).clone() else {
                        break;
                    };
                    if RESERVED.contains(&part.as_str()) {
                        break;
                    }
                    self.bump();
                    self.bump();
                    name = format!("{name}.{part}");
                }
                if self.is_op("(") {
                    let args = self.call_args();
                    return Expr {
                        pos,
                        kind: ExprKind::Call(name, args),
                    };
                }
                Expr {
                    pos,
                    kind: ExprKind::Ident(name),
                }
            }
            Tok::Op("(") => {
                self.bump();
                let e = self.expr();
                self.expect_op(")");
                e
            }
            Tok::Op("{") => {
                self.bump();
                let first = self.expr();
                if self.is_op("{") {
                    self.bump();
                    let parts = self.expr_list();
                    self.expect_op("}");
                    self.expect_op("}");
                    return Expr {
                        pos,
                        kind: ExprKind::Repl(Box::new(first), parts),
                    };
                }
                let mut parts = vec![first];
                while self.eat_op(",") {
                    parts.push(self.expr());
                }
                self.expect_op("}");
                Expr {
                    pos,
                    kind: ExprKind::Concat(parts),
                }
            }
            Tok::Op("'") => {
                if self.is_op_at(1, "{") {
                    self.err_at(
                        &pos,
                        "assignment pattern",
                        "assignment patterns ('{...}) are not supported by the native front end",
                    );
                    self.bump();
                    self.skip_group();
                    return self.zero(pos);
                }
                if self.is_op_at(1, "(") {
                    self.bump();
                    self.bump();
                    let e = self.expr();
                    self.expect_op(")");
                    return Expr {
                        pos,
                        kind: ExprKind::Cast(None, Box::new(e)),
                    };
                }
                self.err("syntax", "expected expression, found `'`");
                self.bump();
                self.zero(pos)
            }
            _ => {
                let stop = matches!(
                    self.tok(),
                    Tok::Op(";" | ")" | "]" | "}" | "," | ":") | Tok::Eof
                );
                let found = self.describe();
                self.err("syntax", format!("expected expression, found {found}"));
                if !stop {
                    self.bump();
                }
                self.zero(pos)
            }
        }
    }
}
