//! Statement parsing: procedural blocks, assignments, control flow, timing controls.

use super::expr::KNOWN_SYSTEM;
use super::{DATA_KEYWORDS, Parser, RESERVED};
use crate::ast::{BinOp, CaseItem, CaseKind, Decl, Edge, Event, Expr, ExprKind, Stmt, StmtKind};
use crate::diag::Pos;
use crate::lexer::Tok;

/// Keywords that end a statement list without being statements.
const LIST_END: &[&str] = &[
    "end",
    "endmodule",
    "endcase",
    "endfunction",
    "endtask",
    "endgenerate",
];

impl Parser {
    /// Parses one statement.
    pub(crate) fn stmt(&mut self) -> Stmt {
        let pos = self.pos();
        // Attribute `(* ... *)`: skipped.
        if self.skip_attribute() {
            return self.stmt();
        }
        let kind = match self.tok().clone() {
            Tok::Op(";") => {
                self.bump();
                StmtKind::Null
            }
            Tok::Op("#") => {
                self.bump();
                let amount = self.delay_value();
                let stmt = self.stmt();
                StmtKind::Delay {
                    amount,
                    stmt: Box::new(stmt),
                }
            }
            Tok::Op("@") => self.event_control(),
            Tok::Op("{") => {
                // Concatenation target: `{a, b} = ...;`
                let lhs = self.lvalue();
                let kind = self.assignment_rest(lhs);
                if !matches!(kind, StmtKind::Null) {
                    self.expect_op(";");
                }
                kind
            }
            Tok::SysName(name) => {
                self.bump();
                if !KNOWN_SYSTEM.contains(&name.as_str()) {
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
                self.expect_op(";");
                StmtKind::SysCall { name, args }
            }
            Tok::Ident(kw) => match kw.as_str() {
                "begin" => return self.block(pos),
                "fork" => {
                    self.err(
                        "fork/join",
                        "fork/join is not supported by the native front end",
                    );
                    while !self.at_end()
                        && !(self.is_kw("join")
                            || self.is_kw("join_any")
                            || self.is_kw("join_none"))
                    {
                        self.bump();
                    }
                    self.bump();
                    StmtKind::Null
                }
                "if" => return self.if_stmt(pos),
                "case" | "casez" | "casex" => return self.case_stmt(pos),
                "unique" | "unique0" | "priority" => {
                    self.bump();
                    return self.stmt();
                }
                "for" => return self.for_stmt(pos),
                "while" => {
                    self.bump();
                    self.expect_op("(");
                    let cond = self.expr();
                    self.expect_op(")");
                    let body = self.stmt();
                    StmtKind::While {
                        cond,
                        body: Box::new(body),
                    }
                }
                "repeat" => {
                    self.bump();
                    self.expect_op("(");
                    let count = self.expr();
                    self.expect_op(")");
                    let body = self.stmt();
                    StmtKind::Repeat {
                        count,
                        body: Box::new(body),
                    }
                }
                "forever" => {
                    self.bump();
                    let body = self.stmt();
                    StmtKind::Forever {
                        body: Box::new(body),
                    }
                }
                "wait" => {
                    self.bump();
                    self.expect_op("(");
                    let cond = self.expr();
                    self.expect_op(")");
                    let stmt = self.stmt();
                    StmtKind::Wait {
                        cond,
                        stmt: Box::new(stmt),
                    }
                }
                "return" => {
                    self.bump();
                    let v = if self.is_op(";") {
                        None
                    } else {
                        Some(self.expr())
                    };
                    self.expect_op(";");
                    StmtKind::Return(v)
                }
                "break" => {
                    self.bump();
                    self.expect_op(";");
                    StmtKind::Break
                }
                "continue" => {
                    self.bump();
                    self.expect_op(";");
                    StmtKind::Continue
                }
                "disable" => {
                    self.bump();
                    let name = match self.ident() {
                        Some((n, _)) => n,
                        None => String::new(),
                    };
                    self.err_at(&pos, "disable", "disable is parsed but not synthesizable");
                    self.expect_op(";");
                    StmtKind::Disable(name)
                }
                "do" => {
                    self.err(
                        "do-while",
                        "do ... while loops are not supported by the native front end",
                    );
                    self.bump();
                    self.recover();
                    StmtKind::Null
                }
                "assert" | "assume" | "cover" | "expect" => {
                    self.err(
                        "assertion",
                        format!("`{kw}` is not supported by the native front end; skipped"),
                    );
                    self.bump();
                    if self.is_op("(") {
                        self.skip_group();
                    }
                    if self.eat_kw("else") {
                        let _ = self.stmt();
                    } else {
                        self.eat_op(";");
                    }
                    StmtKind::Null
                }
                "assign" | "deassign" | "force" | "release" => {
                    self.err(
                        "procedural assign",
                        format!("`{kw}` is not supported by the native front end"),
                    );
                    self.skip_to_semi();
                    StmtKind::Null
                }
                k if LIST_END.contains(&k) || k == "else" => {
                    let found = self.describe();
                    self.err("syntax", format!("expected statement, found {found}"));
                    StmtKind::Null
                }
                _ => {
                    if self.is_op_at(1, ":") && !RESERVED.contains(&kw.as_str()) {
                        // Statement label: `name: stmt`.
                        self.bump();
                        self.bump();
                        return self.stmt();
                    }
                    if RESERVED.contains(&kw.as_str()) || DATA_KEYWORDS.contains(&kw.as_str()) {
                        let found = self.describe();
                        self.err("syntax", format!("expected statement, found {found}"));
                        self.recover();
                        return Stmt {
                            pos,
                            kind: StmtKind::Null,
                        };
                    }
                    self.name_stmt()
                }
            },
            _ => {
                let found = self.describe();
                self.err("syntax", format!("expected statement, found {found}"));
                if !LIST_END.iter().any(|k| self.is_kw(k)) && !self.at_end() {
                    self.recover();
                }
                StmtKind::Null
            }
        };
        Stmt { pos, kind }
    }

    /// `#amount`: a parenthesised expression or a single primary.
    fn delay_value(&mut self) -> Expr {
        if self.is_op("(") {
            self.bump();
            let e = self.expr();
            self.expect_op(")");
            e
        } else {
            self.primary()
        }
    }

    fn event_control(&mut self) -> StmtKind {
        self.bump(); // @
        let events = if self.is_op("*") {
            self.bump();
            None
        } else if self.is_op("(") && self.is_op_at(1, "*") && self.is_op_at(2, ")") {
            self.bump();
            self.bump();
            self.bump();
            None
        } else if self.eat_op("(") {
            let mut evs = Vec::new();
            loop {
                let edge = if self.eat_kw("posedge") {
                    Edge::Pos
                } else if self.eat_kw("negedge") {
                    Edge::Neg
                } else {
                    Edge::Any
                };
                let expr = self.expr();
                evs.push(Event { edge, expr });
                if self.eat_kw("or") || self.eat_op(",") {
                    continue;
                }
                break;
            }
            self.expect_op(")");
            Some(evs)
        } else {
            let edge = if self.eat_kw("posedge") {
                Edge::Pos
            } else if self.eat_kw("negedge") {
                Edge::Neg
            } else {
                Edge::Any
            };
            let pos = self.pos();
            let expr = match self.ident() {
                Some((n, p)) => Expr {
                    pos: p,
                    kind: ExprKind::Ident(n),
                },
                None => {
                    self.err("syntax", "expected event expression after `@`");
                    Expr {
                        pos,
                        kind: ExprKind::Ident(String::new()),
                    }
                }
            };
            Some(vec![Event { edge, expr }])
        };
        let stmt = self.stmt();
        StmtKind::EventCtrl {
            events,
            stmt: Box::new(stmt),
        }
    }

    /// `begin [: name] {decl} {stmt} end [: name]`.
    fn block(&mut self, pos: Pos) -> Stmt {
        self.bump(); // begin
        let mut name = None;
        if self.eat_op(":") {
            name = self.ident().map(|(n, _)| n);
        }
        let mut decls: Vec<Decl> = Vec::new();
        while self.is_decl_start() {
            self.data_decl(&mut decls);
        }
        let mut stmts = Vec::new();
        loop {
            if self.at_end() || self.is_kw("end") || LIST_END.iter().any(|k| self.is_kw(k)) {
                break;
            }
            let before = self.i;
            stmts.push(self.stmt());
            self.ensure_progress(before);
        }
        if !self.eat_kw("end") {
            let found = self.describe();
            self.err_at(
                &pos,
                "syntax",
                format!("`begin` block is not closed by `end` (found {found})"),
            );
        } else if self.eat_op(":") {
            let _ = self.ident();
        }
        Stmt {
            pos,
            kind: StmtKind::Block { name, decls, stmts },
        }
    }

    fn if_stmt(&mut self, pos: Pos) -> Stmt {
        self.bump();
        self.expect_op("(");
        let cond = self.expr();
        self.expect_op(")");
        let then_stmt = self.stmt();
        let else_stmt = if self.eat_kw("else") {
            Some(Box::new(self.stmt()))
        } else {
            None
        };
        Stmt {
            pos,
            kind: StmtKind::If {
                cond,
                then_stmt: Box::new(then_stmt),
                else_stmt,
            },
        }
    }

    fn case_stmt(&mut self, pos: Pos) -> Stmt {
        let kind = match self.tok() {
            Tok::Ident(s) if s == "casez" => CaseKind::Casez,
            Tok::Ident(s) if s == "casex" => CaseKind::Casex,
            _ => CaseKind::Case,
        };
        self.bump();
        self.expect_op("(");
        let expr = self.expr();
        self.expect_op(")");
        let mut items = Vec::new();
        loop {
            if self.at_end() || self.is_kw("endcase") || LIST_END.iter().any(|k| self.is_kw(k)) {
                break;
            }
            let before = self.i;
            if self.eat_kw("default") {
                self.eat_op(":");
                let body = self.stmt();
                items.push(CaseItem {
                    labels: Vec::new(),
                    body,
                });
            } else {
                let labels = self.expr_list();
                self.expect_op(":");
                let body = self.stmt();
                items.push(CaseItem { labels, body });
            }
            self.ensure_progress(before);
        }
        if !self.eat_kw("endcase") {
            let found = self.describe();
            self.err("syntax", format!("expected `endcase`, found {found}"));
        }
        Stmt {
            pos,
            kind: StmtKind::Case { kind, expr, items },
        }
    }

    fn for_stmt(&mut self, pos: Pos) -> Stmt {
        self.bump(); // for
        self.expect_op("(");
        // A declaration in the initialiser (SystemVerilog): wrap the loop in a block.
        let mut decls = Vec::new();
        let init = if self.is_decl_start() {
            self.data_decl_one_init(&mut decls)
        } else {
            let lpos = self.pos();
            let lhs = self.lvalue();
            self.expect_op("=");
            let rhs = self.expr();
            Stmt {
                pos: lpos,
                kind: StmtKind::Assign {
                    lhs,
                    rhs,
                    nonblocking: false,
                    delay: None,
                },
            }
        };
        self.expect_op(";");
        let cond = self.expr();
        self.expect_op(";");
        let spos = self.pos();
        let step_lhs = self.lvalue();
        let step_rhs = if self.eat_op("++") {
            num_one_add(&step_lhs, BinOp::Add)
        } else if self.eat_op("--") {
            num_one_add(&step_lhs, BinOp::Sub)
        } else {
            self.expect_op("=");
            self.expr()
        };
        self.expect_op(")");
        let body = self.stmt();
        let lp = Stmt {
            pos: pos.clone(),
            kind: StmtKind::For {
                init: Box::new(init),
                cond,
                step: Box::new(Stmt {
                    pos: spos,
                    kind: StmtKind::Assign {
                        lhs: step_lhs,
                        rhs: step_rhs,
                        nonblocking: false,
                        delay: None,
                    },
                }),
                body: Box::new(body),
            },
        };
        if decls.is_empty() {
            lp
        } else {
            Stmt {
                pos,
                kind: StmtKind::Block {
                    name: None,
                    decls,
                    stmts: vec![lp],
                },
            }
        }
    }

    /// `[type] name = expr` in a for-initialiser; the declaration goes to `decls`.
    fn data_decl_one_init(&mut self, decls: &mut Vec<Decl>) -> Stmt {
        let mut tmp = Vec::new();
        // Parse the declaration without the trailing `;` (the caller expects `;`).
        let pos = self.pos();
        self.decl_no_semi(&mut tmp);
        let mut assigns = Vec::new();
        for d in tmp {
            if let Some(init) = d.init.clone() {
                assigns.push(Stmt {
                    pos: d.pos.clone(),
                    kind: StmtKind::Assign {
                        lhs: Expr {
                            pos: d.pos.clone(),
                            kind: ExprKind::Ident(d.name.clone()),
                        },
                        rhs: init,
                        nonblocking: false,
                        delay: None,
                    },
                });
            }
            decls.push(Decl { init: None, ..d });
        }
        match assigns.len() {
            1 => assigns.remove(0),
            _ => Stmt {
                pos,
                kind: StmtKind::Null,
            },
        }
    }

    /// `ident ...`: a task call or an assignment.
    fn name_stmt(&mut self) -> StmtKind {
        let Some((mut name, npos)) = self.ident() else {
            let found = self.describe();
            self.err("syntax", format!("expected statement, found {found}"));
            self.recover();
            return StmtKind::Null;
        };
        // Hierarchical target: `u_ram.ram[i] = ...`.
        while self.is_op(".") {
            let Tok::Ident(part) = self.tok_at(1).clone() else {
                break;
            };
            self.bump();
            self.bump();
            name = format!("{name}.{part}");
        }
        if self.is_op("(") || self.is_op(";") {
            let args = if self.is_op("(") {
                self.call_args()
            } else {
                Vec::new()
            };
            self.expect_op(";");
            return StmtKind::Call { name, args };
        }
        let lhs = self.postfix_rest(Expr {
            pos: npos,
            kind: ExprKind::Ident(name),
        });
        let kind = self.assignment_rest(lhs);
        if !matches!(kind, StmtKind::Null) {
            self.expect_op(";");
        }
        kind
    }

    /// Left-hand side of an assignment in a statement context.
    pub(crate) fn lvalue(&mut self) -> Expr {
        let p = self.primary();
        self.postfix_rest(p)
    }

    /// Operator and right-hand side after an assignment target. Returns `Null` on error.
    fn assignment_rest(&mut self, lhs: Expr) -> StmtKind {
        let compound = |t: &Tok| -> Option<BinOp> {
            match t {
                Tok::Op("+=") => Some(BinOp::Add),
                Tok::Op("-=") => Some(BinOp::Sub),
                Tok::Op("*=") => Some(BinOp::Mul),
                Tok::Op("/=") => Some(BinOp::Div),
                Tok::Op("%=") => Some(BinOp::Mod),
                Tok::Op("&=") => Some(BinOp::And),
                Tok::Op("|=") => Some(BinOp::Or),
                Tok::Op("^=") => Some(BinOp::Xor),
                Tok::Op("<<=") => Some(BinOp::Shl),
                Tok::Op(">>=") => Some(BinOp::Shr),
                Tok::Op("<<<=") => Some(BinOp::AShl),
                Tok::Op(">>>=") => Some(BinOp::AShr),
                _ => None,
            }
        };
        if self.is_op("=") || self.is_op("<=") {
            let nonblocking = self.is_op("<=");
            self.bump();
            let delay = if self.is_op("#") {
                self.bump();
                Some(self.delay_value())
            } else {
                None
            };
            let rhs = self.expr();
            return StmtKind::Assign {
                lhs,
                rhs,
                nonblocking,
                delay,
            };
        }
        if let Some(op) = compound(self.tok()) {
            self.bump();
            let rhs = self.expr();
            let pos = lhs.pos.clone();
            let rhs = Expr {
                pos,
                kind: ExprKind::Binary(op, Box::new(lhs.clone()), Box::new(rhs)),
            };
            return StmtKind::Assign {
                lhs,
                rhs,
                nonblocking: false,
                delay: None,
            };
        }
        if self.is_op("++") || self.is_op("--") {
            let op = if self.is_op("++") {
                BinOp::Add
            } else {
                BinOp::Sub
            };
            self.bump();
            let rhs = num_one_add(&lhs, op);
            return StmtKind::Assign {
                lhs,
                rhs,
                nonblocking: false,
                delay: None,
            };
        }
        let found = self.describe();
        self.err(
            "syntax",
            format!("expected an assignment operator, found {found}"),
        );
        self.recover();
        StmtKind::Null
    }

    /// Whether a declaration starts at the cursor (inside a block or a routine).
    pub(crate) fn is_decl_start(&self) -> bool {
        match self.tok() {
            Tok::Ident(s) => {
                DATA_KEYWORDS.contains(&s.as_str())
                    || (self.typedefs.contains(s) && matches!(self.tok_at(1), Tok::Ident(_)))
            }
            _ => false,
        }
    }
}

/// `lhs + 1` or `lhs - 1` for `++`/`--` and loop steps.
fn num_one_add(lhs: &Expr, op: BinOp) -> Expr {
    Expr {
        pos: lhs.pos.clone(),
        kind: ExprKind::Binary(
            op,
            Box::new(lhs.clone()),
            Box::new(super::expr::num_expr(lhs.pos.clone(), 1)),
        ),
    }
}
