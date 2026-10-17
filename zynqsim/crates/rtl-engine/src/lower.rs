//! Lowering of expressions and statements to the flat IR.
//!
//! Sizing follows the Verilog rules: an expression is lowered at a context width and
//! signedness. Context-determined operators pass the context down to their operands,
//! self-determined operands (comparison operands are the exception: they share one
//! width) are sized by [`Elab::width_of`] and [`Elab::sign_of`]. Each lowered node has
//! exactly the width it was asked for; leaves are extended with the context signedness.

use crate::elab::{Elab, Scope, Sym};
use crate::ir::{Arg, Edge, Ex, K, LPart, LVal, Op, SigId, Sys, Trigger};
use rtl_frontend::Pos;
use rtl_frontend::ast::{
    BinOp, CaseKind, DataKind, Expr, ExprKind, Routine, RoutineKind, Stmt, StmtKind, UnOp,
};
use rtl_frontend::consteval::{Ctx, Val};
use rtl_frontend::lexer::Number;
use std::collections::HashSet;

/// Code under construction for one process, function or inlined task.
pub(crate) struct Body {
    /// Instructions.
    pub code: Vec<Op>,
    /// Open loops: (break jumps, continue jumps).
    loops: Vec<(Vec<usize>, Vec<usize>)>,
    /// Return jumps to patch to the end of the body.
    pub rets: Vec<usize>,
    /// Function return variable: (signal, width, signed).
    pub ret: Option<(SigId, u32, bool)>,
    /// Scope name for `%m`.
    pub scope_name: String,
    /// Picoseconds per time unit.
    pub unit_ps: u64,
}

impl Body {
    /// An empty body.
    pub(crate) fn new(scope_name: &str, unit_ps: u64) -> Body {
        Body {
            code: Vec::new(),
            loops: Vec::new(),
            rets: Vec::new(),
            ret: None,
            scope_name: scope_name.to_string(),
            unit_ps,
        }
    }
}

/// Sets the target of a jump instruction.
fn patch(code: &mut [Op], at: usize, to: u32) {
    match &mut code[at] {
        Op::Jump(t) => *t = to,
        Op::JumpIf { to: t, .. } | Op::JumpIfNot { to: t, .. } => *t = to,
        _ => {}
    }
}

/// Resizes a lowered node to `w` bits: zero- or sign-extended (with `s`) or truncated.
pub(crate) fn resize(raw: Ex, w: u32, s: bool) -> Ex {
    if raw.w == w {
        raw
    } else if raw.w < w {
        Ex {
            w,
            s,
            k: K::Ext(Box::new(raw)),
        }
    } else {
        Ex {
            w,
            s,
            k: K::Trunc(Box::new(raw)),
        }
    }
}

/// Width and signedness of a number literal.
fn number_shape(n: &Number) -> (u32, bool) {
    match n.size {
        Some(w) => (w.clamp(1, 128), n.signed),
        None => (32, n.base == 'd'),
    }
}

/// Whether a literal is an unsized fill (`'0`, `'1`, `'x`, `'z`).
fn is_fill(n: &Number) -> bool {
    n.base == '\''
}

/// The value of a literal with x, z and ? digits read as 0.
fn number_value(n: &Number) -> u128 {
    if is_fill(n) {
        return if n.digits == "1" { u128::MAX } else { 0 };
    }
    let radix = match n.base {
        'b' => 2,
        'o' => 8,
        'h' => 16,
        _ => 10,
    };
    let mut v: u128 = 0;
    for c in n.digits.bytes() {
        let d = (c as char).to_digit(radix).unwrap_or(0) as u128;
        v = v.wrapping_mul(radix as u128).wrapping_add(d);
    }
    v
}

/// Value and care mask of a casez/casex label (wildcard digits are don't-care bits).
pub(crate) fn wildcard_label(n: &Number, cw: u32) -> (u128, u128) {
    let bpd: u32 = match n.base {
        'b' => 1,
        'o' => 3,
        'h' => 4,
        _ => 0,
    };
    if bpd == 0 || is_fill(n) {
        let v = if is_fill(n) && n.digits == "1" {
            u128::MAX
        } else {
            number_value(n)
        };
        let care = if is_fill(n) && matches!(n.digits.as_str(), "x" | "z") {
            0
        } else {
            u128::MAX
        };
        return (v & care & mask_w(cw), care & mask_w(cw));
    }
    let (mut v, mut care) = (0u128, 0u128);
    for c in n.digits.bytes() {
        let wild = matches!(c, b'x' | b'z' | b'?');
        let d = (c as char).to_digit(1 << bpd).unwrap_or(0) as u128;
        let all = (1u128 << bpd) - 1;
        v = (v << bpd) | if wild { 0 } else { d };
        care = (care << bpd) | if wild { 0 } else { all };
    }
    let lw = match n.size {
        Some(s) => s,
        None => n.digits.len() as u32 * bpd,
    };
    let lw = lw.clamp(1, 128);
    v &= mask_w(lw);
    care &= mask_w(lw);
    if cw > lw {
        // Zero-extended label bits are real zeros, so they must match.
        let ext = mask_w(cw) & !mask_w(lw);
        care |= ext;
    }
    (v & care & mask_w(cw), care & mask_w(cw))
}

fn mask_w(w: u32) -> u128 {
    if w >= 128 {
        u128::MAX
    } else {
        (1u128 << w) - 1
    }
}

/// Names referenced by an expression or statement, and the routines it calls.
#[derive(Default)]
struct Refs {
    names: Vec<String>,
    calls: Vec<String>,
}

fn refs_expr(e: &Expr, r: &mut Refs) {
    match &e.kind {
        ExprKind::Ident(n) => r.names.push(n.clone()),
        ExprKind::Number(_) | ExprKind::Str(_) => {}
        ExprKind::Unary(_, a) | ExprKind::Cast(_, a) => refs_expr(a, r),
        ExprKind::Binary(_, a, b) | ExprKind::Index(a, b) => {
            refs_expr(a, r);
            refs_expr(b, r);
        }
        ExprKind::Ternary(a, b, c) | ExprKind::Part(a, b, c) => {
            refs_expr(a, r);
            refs_expr(b, r);
            refs_expr(c, r);
        }
        ExprKind::IndexedPart {
            base, start, width, ..
        } => {
            refs_expr(base, r);
            refs_expr(start, r);
            refs_expr(width, r);
        }
        ExprKind::Concat(xs) => xs.iter().for_each(|x| refs_expr(x, r)),
        ExprKind::Repl(n, xs) => {
            refs_expr(n, r);
            xs.iter().for_each(|x| refs_expr(x, r));
        }
        ExprKind::Call(name, args) => {
            r.calls.push(name.clone());
            args.iter().for_each(|x| refs_expr(x, r));
        }
        ExprKind::SysCall(_, args) => args.iter().for_each(|x| refs_expr(x, r)),
    }
}

fn refs_stmt(s: &Stmt, r: &mut Refs) {
    match &s.kind {
        StmtKind::Null | StmtKind::Break | StmtKind::Continue | StmtKind::Disable(_) => {}
        StmtKind::Block { stmts, decls, .. } => {
            for d in decls {
                if let Some(i) = &d.init {
                    refs_expr(i, r);
                }
            }
            stmts.iter().for_each(|x| refs_stmt(x, r));
        }
        StmtKind::Assign {
            lhs, rhs, delay, ..
        } => {
            refs_expr(lhs, r);
            refs_expr(rhs, r);
            if let Some(d) = delay {
                refs_expr(d, r);
            }
        }
        StmtKind::If {
            cond,
            then_stmt,
            else_stmt,
        } => {
            refs_expr(cond, r);
            refs_stmt(then_stmt, r);
            if let Some(e) = else_stmt {
                refs_stmt(e, r);
            }
        }
        StmtKind::Case { expr, items, .. } => {
            refs_expr(expr, r);
            for it in items {
                it.labels.iter().for_each(|l| refs_expr(l, r));
                refs_stmt(&it.body, r);
            }
        }
        StmtKind::For {
            init,
            cond,
            step,
            body,
        } => {
            refs_stmt(init, r);
            refs_expr(cond, r);
            refs_stmt(step, r);
            refs_stmt(body, r);
        }
        StmtKind::While { cond, body } => {
            refs_expr(cond, r);
            refs_stmt(body, r);
        }
        StmtKind::Repeat { count, body } => {
            refs_expr(count, r);
            refs_stmt(body, r);
        }
        StmtKind::Forever { body } => refs_stmt(body, r),
        StmtKind::Delay { amount, stmt } => {
            refs_expr(amount, r);
            refs_stmt(stmt, r);
        }
        StmtKind::EventCtrl { events, stmt } => {
            if let Some(ev) = events {
                ev.iter().for_each(|e| refs_expr(&e.expr, r));
            }
            refs_stmt(stmt, r);
        }
        StmtKind::Wait { cond, stmt } => {
            refs_expr(cond, r);
            refs_stmt(stmt, r);
        }
        StmtKind::SysCall { args, .. } | StmtKind::Call { args, .. } => {
            args.iter().for_each(|x| refs_expr(x, r));
            if let StmtKind::Call { name, .. } = &s.kind {
                r.calls.push(name.clone());
            }
        }
        StmtKind::Return(e) => {
            if let Some(e) = e {
                refs_expr(e, r);
            }
        }
    }
}

impl<'p> Elab<'p> {
    /// Self-determined width of an expression.
    pub(crate) fn width_of(&mut self, e: &Expr, sc: &Scope) -> u32 {
        match &e.kind {
            ExprKind::Ident(n) => {
                if let Some(v) = sc.vals.get(n) {
                    v.width
                } else {
                    sc.syms.get(n).map_or(1, |s| s.width)
                }
            }
            ExprKind::Number(n) => {
                if is_fill(n) {
                    1
                } else {
                    number_shape(n).0
                }
            }
            ExprKind::Str(s) => (s.len() as u32 * 8).clamp(8, 128),
            ExprKind::Unary(op, a) => match op {
                UnOp::Plus | UnOp::Minus | UnOp::BitNot => self.width_of(a, sc),
                _ => 1,
            },
            ExprKind::Binary(op, a, b) => match op {
                BinOp::Add
                | BinOp::Sub
                | BinOp::Mul
                | BinOp::Div
                | BinOp::Mod
                | BinOp::And
                | BinOp::Or
                | BinOp::Xor
                | BinOp::Xnor => self.width_of(a, sc).max(self.width_of(b, sc)),
                BinOp::Pow | BinOp::Shl | BinOp::Shr | BinOp::AShl | BinOp::AShr => {
                    self.width_of(a, sc)
                }
                _ => 1,
            },
            ExprKind::Ternary(_, t, f) => self.width_of(t, sc).max(self.width_of(f, sc)),
            ExprKind::Concat(xs) => xs.iter().map(|x| self.width_of(x, sc)).sum::<u32>(),
            ExprKind::Repl(n, xs) => {
                let k = self.const_u64(n, sc).unwrap_or(1) as u32;
                let inner: u32 = xs.iter().map(|x| self.width_of(x, sc)).sum();
                k * inner
            }
            ExprKind::Index(base, _) => match &base.kind {
                ExprKind::Ident(n) => match sc.syms.get(n) {
                    Some(s) if s.depth > 0 => s.width,
                    _ => 1,
                },
                _ => 1,
            },
            ExprKind::Part(_, m, l) => {
                let a = self.const_i64(m, sc).unwrap_or(0);
                let b = self.const_i64(l, sc).unwrap_or(0);
                ((a - b).abs() + 1) as u32
            }
            ExprKind::IndexedPart { width, .. } => self.const_u64(width, sc).unwrap_or(1) as u32,
            ExprKind::Call(name, _) => self.routine_ret(name, sc).map_or(1, |r| r.0),
            ExprKind::SysCall(name, args) => match name.as_str() {
                "$signed" | "$unsigned" => args.first().map_or(32, |a| self.width_of(a, sc)),
                "$time" => 64,
                _ => 32,
            },
            ExprKind::Cast(size, x) => size.unwrap_or_else(|| self.width_of(x, sc)),
        }
    }

    /// Self-determined signedness of an expression.
    pub(crate) fn sign_of(&mut self, e: &Expr, sc: &Scope) -> bool {
        match &e.kind {
            ExprKind::Ident(n) => {
                if let Some(v) = sc.vals.get(n) {
                    v.signed
                } else {
                    sc.syms.get(n).is_some_and(|s| s.signed)
                }
            }
            ExprKind::Number(n) => {
                if is_fill(n) {
                    false
                } else {
                    number_shape(n).1
                }
            }
            ExprKind::Str(_) => false,
            ExprKind::Unary(UnOp::Plus | UnOp::Minus | UnOp::BitNot, a) => self.sign_of(a, sc),
            ExprKind::Unary(..) => false,
            ExprKind::Binary(op, a, b) => match op {
                BinOp::Add
                | BinOp::Sub
                | BinOp::Mul
                | BinOp::Div
                | BinOp::Mod
                | BinOp::And
                | BinOp::Or
                | BinOp::Xor
                | BinOp::Xnor => self.sign_of(a, sc) && self.sign_of(b, sc),
                BinOp::Pow | BinOp::Shl | BinOp::Shr | BinOp::AShl | BinOp::AShr => {
                    self.sign_of(a, sc)
                }
                _ => false,
            },
            ExprKind::Ternary(_, t, f) => self.sign_of(t, sc) && self.sign_of(f, sc),
            ExprKind::Index(base, _) => match &base.kind {
                ExprKind::Ident(n) => sc.syms.get(n).is_some_and(|s| s.depth > 0 && s.signed),
                _ => false,
            },
            ExprKind::Call(name, _) => self.routine_ret(name, sc).is_some_and(|r| r.1),
            ExprKind::SysCall(name, _) => name == "$signed",
            ExprKind::Cast(_, x) => self.sign_of(x, sc),
            _ => false,
        }
    }

    /// Width and signedness of a routine's return value.
    fn routine_ret(&mut self, name: &str, sc: &Scope) -> Option<(u32, bool)> {
        let r = sc.module.routines.get(name)?;
        if r.kind != RoutineKind::Function {
            return None;
        }
        let (w, s, _, _) = self.shape(r.ret_range.as_ref(), r.ret_kind.as_ref(), r.ret_signed, sc);
        Some((w, s))
    }

    fn const_i64(&mut self, e: &Expr, sc: &Scope) -> Option<i64> {
        let mut ctx = Ctx::new(sc.vals.clone(), &sc.module.routines);
        ctx.eval(e).ok().map(|v| v.as_i128() as i64)
    }

    fn const_u64(&mut self, e: &Expr, sc: &Scope) -> Option<u64> {
        self.const_i64(e, sc).map(|v| v.max(0) as u64)
    }

    /// Lowers an expression at width `w` with context signedness `s`.
    pub(crate) fn lower(&mut self, e: &Expr, w: u32, s: bool, sc: &Scope) -> Option<Ex> {
        match &e.kind {
            ExprKind::Number(n) => {
                if is_fill(n) {
                    let v = if n.digits == "1" { mask_w(w) } else { 0 };
                    return Some(Ex {
                        w,
                        s,
                        k: K::Const(v),
                    });
                }
                let (lw, ls) = number_shape(n);
                let raw = Ex {
                    w: lw,
                    s: ls,
                    k: K::Const(number_value(n) & mask_w(lw)),
                };
                Some(resize(raw, w, s))
            }
            ExprKind::Ident(n) => {
                if let Some(v) = sc.vals.get(n).copied() {
                    let raw = Ex {
                        w: v.width,
                        s: v.signed,
                        k: K::Const(v.bits),
                    };
                    return Some(resize(raw, w, s));
                }
                let Some(sym) = sc.syms.get(n).copied() else {
                    self.diag(&e.pos, "identifier", format!("`{n}` is not declared"));
                    return None;
                };
                if sym.depth > 0 {
                    self.diag(
                        &e.pos,
                        "memory",
                        format!("memory `{n}` used without an index"),
                    );
                    return None;
                }
                let raw = Ex {
                    w: sym.width,
                    s: sym.signed,
                    k: K::Sig(sym.id),
                };
                Some(resize(raw, w, s))
            }
            ExprKind::Str(text) => {
                // A string literal is its bytes, first character most significant.
                let bytes = text.as_bytes();
                let lw = (bytes.len() as u32 * 8).max(8);
                if lw > 128 {
                    self.diag(
                        &e.pos,
                        "string",
                        "a string literal longer than 16 characters is not supported",
                    );
                    return None;
                }
                let v = bytes.iter().fold(0u128, |acc, b| (acc << 8) | *b as u128);
                Some(resize(
                    Ex {
                        w: lw,
                        s: false,
                        k: K::Const(v),
                    },
                    w,
                    s,
                ))
            }
            ExprKind::Unary(op, a) => match op {
                UnOp::Plus => self.lower(a, w, s, sc),
                UnOp::Minus | UnOp::BitNot => {
                    let x = self.lower(a, w, s, sc)?;
                    Some(Ex {
                        w,
                        s,
                        k: K::Un(*op, Box::new(x)),
                    })
                }
                _ => {
                    let aw = self.width_of(a, sc);
                    let asg = self.sign_of(a, sc);
                    let x = self.lower(a, aw, asg, sc)?;
                    Some(resize(
                        Ex {
                            w: 1,
                            s: false,
                            k: K::Un(*op, Box::new(x)),
                        },
                        w,
                        s,
                    ))
                }
            },
            ExprKind::Binary(op, a, b) => self.binary(*op, a, b, w, s, sc),
            ExprKind::Ternary(c, t, f) => {
                let cw = self.width_of(c, sc);
                let cs = self.sign_of(c, sc);
                let cond = self.lower(c, cw, cs, sc)?;
                let t2 = self.lower(t, w, s, sc)?;
                let f2 = self.lower(f, w, s, sc)?;
                Some(Ex {
                    w,
                    s,
                    k: K::Tern(Box::new(cond), Box::new(t2), Box::new(f2)),
                })
            }
            ExprKind::Concat(parts) => {
                let mut xs = Vec::new();
                for p in parts {
                    // Zero-width parts (a zero replication) contribute no bits.
                    if self.width_of(p, sc) == 0 {
                        continue;
                    }
                    let pw = self.width_of(p, sc);
                    let ps = self.sign_of(p, sc);
                    xs.push(self.lower(p, pw, ps, sc)?);
                }
                self.concat_node(xs, w, s, &e.pos)
            }
            ExprKind::Repl(count, parts) => {
                let k = self.const_u64(count, sc).unwrap_or(0);
                let mut xs = Vec::new();
                for p in parts {
                    let pw = self.width_of(p, sc);
                    let ps = self.sign_of(p, sc);
                    xs.push(self.lower(p, pw, ps, sc)?);
                }
                let mut all = Vec::new();
                for _ in 0..k {
                    all.extend(xs.iter().cloned());
                }
                if all.is_empty() {
                    self.diag(
                        &e.pos,
                        "replication",
                        "a zero-width replication stands alone; it is only allowed inside a wider concatenation",
                    );
                    return None;
                }
                self.concat_node(all, w, s, &e.pos)
            }
            ExprKind::Index(base, idx) => self.index_expr(base, idx, w, s, sc),
            ExprKind::Part(base, m, l) => {
                let a = self.const_i64(m, sc);
                let b = self.const_i64(l, sc);
                let (Some(a), Some(b)) = (a, b) else {
                    self.diag(&e.pos, "part select", "part-select bounds must be constant");
                    return None;
                };
                let (lo, width) = (a.min(b), (a - b).unsigned_abs() as u32 + 1);
                let (bw, bs) = self.base_shape(base, sc);
                let bx = self.self_lower(base, sc)?;
                let _ = (bw, bs);
                let (off, flip) = self.base_map(base, sc);
                let raw = Ex {
                    w: width,
                    s: false,
                    k: K::Slice {
                        base: Box::new(bx),
                        start: Box::new(Ex {
                            w: 32,
                            s: true,
                            k: K::Const(lo as u128),
                        }),
                        off,
                        flip,
                        up: true,
                    },
                };
                Some(resize(raw, w, s))
            }
            ExprKind::IndexedPart {
                base,
                start,
                width,
                up,
            } => {
                let Some(wd) = self.const_u64(width, sc) else {
                    self.diag(
                        &e.pos,
                        "part select",
                        "indexed part-select width must be constant",
                    );
                    return None;
                };
                let sw = self.width_of(start, sc);
                let ss = self.sign_of(start, sc);
                let st = self.lower(start, sw, ss, sc)?;
                let bx = self.self_lower(base, sc)?;
                let (off, flip) = self.base_map(base, sc);
                let raw = Ex {
                    w: wd.clamp(1, 128) as u32,
                    s: false,
                    k: K::Slice {
                        base: Box::new(bx),
                        start: Box::new(st),
                        off,
                        flip,
                        up: *up,
                    },
                };
                Some(resize(raw, w, s))
            }
            ExprKind::Call(name, args) => {
                let fid = match self.func_index(name, sc) {
                    Some(f) => f,
                    None => {
                        self.diag(
                            &e.pos,
                            "function",
                            format!("function `{name}` is not defined here (or recursive)"),
                        );
                        return None;
                    }
                };
                let formals = self.funcs[fid as usize].formals.clone();
                let ret = self.funcs[fid as usize].ret;
                if formals.len() != args.len() {
                    self.diag(
                        &e.pos,
                        "function",
                        format!("`{name}` takes {} arguments", formals.len()),
                    );
                    return None;
                }
                let mut xs = Vec::new();
                for (a, f) in args.iter().zip(&formals) {
                    let fw = self.signals[*f as usize].width;
                    xs.push(self.assign_expr(a, fw, sc)?);
                }
                let rw = self.signals[ret as usize].width;
                let rs = self.signals[ret as usize].signed;
                let raw = Ex {
                    w: rw,
                    s: rs,
                    k: K::Call { fid, args: xs },
                };
                Some(resize(raw, w, s))
            }
            ExprKind::SysCall(name, args) => match name.as_str() {
                "$signed" | "$unsigned" => {
                    let a = args.first()?;
                    let aw = self.width_of(a, sc);
                    let raw = self.self_lower(a, sc)?;
                    let _ = aw;
                    Some(resize(raw, w, s))
                }
                "$time" => Some(resize(
                    Ex {
                        w: 64,
                        s: false,
                        k: K::Time(sc.module.unit_ps),
                    },
                    w,
                    s,
                )),
                "$clog2" => {
                    let a = args.first()?;
                    let v = self.konst_val(a, sc)?;
                    let c = rtl_frontend::consteval::clog2(v.as_i128());
                    Some(resize(
                        Ex {
                            w: 32,
                            s: true,
                            k: K::Const(c as u128),
                        },
                        w,
                        s,
                    ))
                }
                "$bits" => {
                    let a = args.first()?;
                    let bw = self.width_of(a, sc);
                    Some(resize(
                        Ex {
                            w: 32,
                            s: true,
                            k: K::Const(bw as u128),
                        },
                        w,
                        s,
                    ))
                }
                _ => {
                    self.diag(
                        &e.pos,
                        "system function",
                        format!("system function `{name}` is not supported by the engine"),
                    );
                    None
                }
            },
            ExprKind::Cast(size, x) => {
                let n = size.unwrap_or_else(|| self.width_of(x, sc));
                let xs = self.sign_of(x, sc);
                let inner = self.lower(x, n, xs, sc)?;
                Some(resize(inner, w, s))
            }
        }
    }

    /// Lowers an expression at its own width (self-determined).
    pub(crate) fn self_lower(&mut self, e: &Expr, sc: &Scope) -> Option<Ex> {
        let w = self.width_of(e, sc);
        let s = self.sign_of(e, sc);
        self.lower(e, w, s, sc)
    }

    /// Lowers an expression as the value assigned to a `tw`-bit target.
    pub(crate) fn assign_expr(&mut self, e: &Expr, tw: u32, sc: &Scope) -> Option<Ex> {
        let ew = self.width_of(e, sc);
        let es = self.sign_of(e, sc);
        let w = ew.max(tw);
        let raw = self.lower(e, w, es, sc)?;
        Some(resize(raw, tw, es))
    }

    /// Public-to-the-crate form of [`Elab::self_lower`] used by the elaborator.
    pub(crate) fn self_expr(&mut self, e: &Expr, sc: &Scope) -> Option<Ex> {
        self.self_lower(e, sc)
    }

    /// Like [`Elab::assign_expr`] with the target's width.
    pub(crate) fn resize_to(&mut self, raw: Ex, tw: u32, s: bool) -> Ex {
        resize(raw, tw, s)
    }

    fn konst_val(&mut self, e: &Expr, sc: &Scope) -> Option<Val> {
        let mut ctx = Ctx::new(sc.vals.clone(), &sc.module.routines);
        match ctx.eval(e) {
            Ok(v) => Some(v),
            Err(msg) => {
                self.diag(&e.pos, "constant", msg);
                None
            }
        }
    }

    fn concat_node(&mut self, xs: Vec<Ex>, w: u32, s: bool, pos: &Pos) -> Option<Ex> {
        let total: u32 = xs.iter().map(|x| x.w).sum();
        if total == 0 {
            self.diag(
                pos,
                "replication",
                "a concatenation of zero-width parts has no value",
            );
            return None;
        }
        if total > 128 {
            self.diag(
                pos,
                "width",
                format!(
                    "a {total}-bit concatenation is wider than the 128-bit limit of the engine"
                ),
            );
            return None;
        }
        Some(resize(
            Ex {
                w: total,
                s: false,
                k: K::Concat(xs),
            },
            w,
            s,
        ))
    }

    /// Width and signedness of a base expression (identifier or memory word).
    fn base_shape(&mut self, base: &Expr, sc: &Scope) -> (u32, bool) {
        (self.width_of(base, sc), self.sign_of(base, sc))
    }

    /// Declared offset and flip of the packed range a select's base refers to.
    fn base_map(&mut self, base: &Expr, sc: &Scope) -> (i64, bool) {
        let root = match &base.kind {
            ExprKind::Ident(n) => Some(n),
            ExprKind::Index(b, _) => match &b.kind {
                ExprKind::Ident(n) => Some(n),
                _ => None,
            },
            _ => None,
        };
        match root.and_then(|n| sc.syms.get(n)) {
            Some(s) => (s.lsb, s.msb < s.lsb),
            None => (0, false),
        }
    }

    fn index_expr(&mut self, base: &Expr, idx: &Expr, w: u32, s: bool, sc: &Scope) -> Option<Ex> {
        let iw = self.width_of(idx, sc);
        let is = self.sign_of(idx, sc);
        if let ExprKind::Ident(n) = &base.kind
            && let Some(sym) = sc.syms.get(n).copied()
            && sym.depth > 0
        {
            let i = self.lower(idx, iw, is, sc)?;
            let raw = Ex {
                w: sym.width,
                s: sym.signed,
                k: K::Word {
                    sig: sym.id,
                    idx: Box::new(i),
                    off: self.signals[sym.id as usize].mem_lo,
                },
            };
            return Some(resize(raw, w, s));
        }
        let bx = self.self_lower(base, sc)?;
        let (off, flip) = self.base_map(base, sc);
        let i = self.lower(idx, iw, is, sc)?;
        let raw = Ex {
            w: 1,
            s: false,
            k: K::Slice {
                base: Box::new(bx),
                start: Box::new(i),
                off,
                flip,
                up: true,
            },
        };
        Some(resize(raw, w, s))
    }

    fn binary(&mut self, op: BinOp, a: &Expr, b: &Expr, w: u32, s: bool, sc: &Scope) -> Option<Ex> {
        let wa = self.width_of(a, sc);
        let wb = self.width_of(b, sc);
        let sa = self.sign_of(a, sc);
        let sb = self.sign_of(b, sc);
        match op {
            BinOp::Add
            | BinOp::Sub
            | BinOp::Mul
            | BinOp::Div
            | BinOp::Mod
            | BinOp::And
            | BinOp::Or
            | BinOp::Xor
            | BinOp::Xnor => {
                let x = self.lower(a, w, s, sc)?;
                let y = self.lower(b, w, s, sc)?;
                Some(Ex {
                    w,
                    s,
                    k: K::Bin {
                        op,
                        a: Box::new(x),
                        b: Box::new(y),
                        os: s,
                    },
                })
            }
            BinOp::Pow | BinOp::Shl | BinOp::Shr | BinOp::AShl | BinOp::AShr => {
                let x = self.lower(a, w, s, sc)?;
                let y = self.lower(b, wb, false, sc)?;
                let _ = (wa, sb);
                Some(Ex {
                    w,
                    s,
                    k: K::Bin {
                        op,
                        a: Box::new(x),
                        b: Box::new(y),
                        os: s,
                    },
                })
            }
            BinOp::Lt
            | BinOp::Le
            | BinOp::Gt
            | BinOp::Ge
            | BinOp::Eq
            | BinOp::Ne
            | BinOp::CaseEq
            | BinOp::CaseNe => {
                let cw = wa.max(wb);
                let cs = sa && sb;
                let x = self.lower(a, cw, cs, sc)?;
                let y = self.lower(b, cw, cs, sc)?;
                Some(resize(
                    Ex {
                        w: 1,
                        s: false,
                        k: K::Bin {
                            op,
                            a: Box::new(x),
                            b: Box::new(y),
                            os: cs,
                        },
                    },
                    w,
                    s,
                ))
            }
            BinOp::LAnd | BinOp::LOr => {
                let x = self.lower(a, wa, sa, sc)?;
                let y = self.lower(b, wb, sb, sc)?;
                Some(resize(
                    Ex {
                        w: 1,
                        s: false,
                        k: K::Bin {
                            op,
                            a: Box::new(x),
                            b: Box::new(y),
                            os: false,
                        },
                    },
                    w,
                    s,
                ))
            }
        }
    }

    /// Target of an assignment. Parts are MSB first.
    pub(crate) fn lval(&mut self, e: &Expr, sc: &Scope) -> Option<LVal> {
        match &e.kind {
            ExprKind::Ident(n) => {
                let sym = self.sym_or_diag(n, e, sc)?;
                if sym.depth > 0 {
                    self.diag(
                        &e.pos,
                        "memory",
                        format!("memory `{n}` assigned without an index"),
                    );
                    return None;
                }
                Some(LVal {
                    w: sym.width,
                    parts: vec![LPart {
                        sig: sym.id,
                        word: None,
                        word_off: 0,
                        sel: None,
                        w: sym.width,
                    }],
                })
            }
            ExprKind::Index(base, idx) => {
                let (sym, word) = self.part_base(base, sc)?;
                let iw = self.width_of(idx, sc);
                let is = self.sign_of(idx, sc);
                let i = self.lower(idx, iw, is, sc)?;
                if let Some(wi) = word {
                    // A bit of a memory word: `mem[a][b] = v`.
                    let off = self.signals[sym.id as usize].mem_lo;
                    let flip = sym.msb < sym.lsb;
                    return Some(LVal {
                        w: 1,
                        parts: vec![LPart {
                            sig: sym.id,
                            word: Some(wi),
                            word_off: off,
                            sel: Some((i, sym.lsb, flip, true)),
                            w: 1,
                        }],
                    });
                }
                if sym.depth > 0 {
                    let off = self.signals[sym.id as usize].mem_lo;
                    return Some(LVal {
                        w: sym.width,
                        parts: vec![LPart {
                            sig: sym.id,
                            word: Some(i),
                            word_off: off,
                            sel: None,
                            w: sym.width,
                        }],
                    });
                }
                let flip = sym.msb < sym.lsb;
                Some(LVal {
                    w: 1,
                    parts: vec![LPart {
                        sig: sym.id,
                        word: None,
                        word_off: 0,
                        sel: Some((i, sym.lsb, flip, true)),
                        w: 1,
                    }],
                })
            }
            ExprKind::Part(base, m, l) => {
                let (sym, word) = self.part_base(base, sc)?;
                let a = self.const_i64(m, sc)?;
                let b = self.const_i64(l, sc)?;
                let width = (a - b).unsigned_abs() as u32 + 1;
                let start = Ex {
                    w: 32,
                    s: true,
                    k: K::Const(a.min(b) as u128),
                };
                let off = self.signals[sym.id as usize].mem_lo;
                Some(part_target(&sym, word, off, start, width, true))
            }
            ExprKind::IndexedPart {
                base,
                start,
                width,
                up,
            } => {
                let (sym, word) = self.part_base(base, sc)?;
                let wd = self.const_u64(width, sc)? as u32;
                let sw = self.width_of(start, sc);
                let ss = self.sign_of(start, sc);
                let st = self.lower(start, sw, ss, sc)?;
                let off = self.signals[sym.id as usize].mem_lo;
                Some(part_target(&sym, word, off, st, wd, *up))
            }
            ExprKind::Concat(parts) => {
                let mut out = Vec::new();
                let mut w = 0;
                for p in parts {
                    let l = self.lval(p, sc)?;
                    w += l.w;
                    out.extend(l.parts);
                }
                Some(LVal { w, parts: out })
            }
            _ => {
                self.diag(
                    &e.pos,
                    "assignment target",
                    "this expression cannot be assigned to",
                );
                None
            }
        }
    }

    /// The signal a part-select target refers to, and the memory word index if the
    /// base is a word (`mem[i][7:0] <= ...`).
    fn part_base(&mut self, base: &Expr, sc: &Scope) -> Option<(Sym, Option<Ex>)> {
        match &base.kind {
            ExprKind::Ident(n) => Some((self.sym_or_diag(n, base, sc)?, None)),
            ExprKind::Index(b, idx) => {
                let ExprKind::Ident(n) = &b.kind else {
                    self.diag(
                        &base.pos,
                        "assignment target",
                        "only a signal or memory word can be a part-select target",
                    );
                    return None;
                };
                let sym = self.sym_or_diag(n, b, sc)?;
                if sym.depth == 0 {
                    self.diag(
                        &base.pos,
                        "assignment target",
                        format!("`{n}` is not a memory"),
                    );
                    return None;
                }
                let iw = self.width_of(idx, sc);
                let is = self.sign_of(idx, sc);
                let i = self.lower(idx, iw, is, sc)?;
                Some((sym, Some(i)))
            }
            _ => {
                self.diag(
                    &base.pos,
                    "assignment target",
                    "only a signal can be a part-select target",
                );
                None
            }
        }
    }

    fn sym_or_diag(&mut self, n: &str, e: &Expr, sc: &Scope) -> Option<Sym> {
        match sc.syms.get(n).copied() {
            Some(s) => Some(s),
            None => {
                self.diag(
                    &e.pos,
                    "identifier",
                    format!("`{n}` is not a declared signal"),
                );
                None
            }
        }
    }

    /// Signals an expression reads, as triggers (any change).
    pub(crate) fn triggers_of_expr(&mut self, e: &Expr, sc: &Scope) -> Vec<Trigger> {
        let mut r = Refs::default();
        refs_expr(e, &mut r);
        self.triggers_of_refs(r, sc)
    }

    /// Signals a statement reads (including called routines), as triggers.
    pub(crate) fn triggers_of_stmt(&mut self, s: &Stmt, sc: &Scope) -> Vec<Trigger> {
        let mut r = Refs::default();
        refs_stmt(s, &mut r);
        self.triggers_of_refs(r, sc)
    }

    /// Signals used by the index expressions of an assignment target.
    pub(crate) fn index_triggers(&mut self, e: &Expr, sc: &Scope) -> Vec<Trigger> {
        let mut r = Refs::default();
        fn targets(e: &Expr, r: &mut Refs) {
            match &e.kind {
                ExprKind::Index(_, i) => refs_expr(i, r),
                ExprKind::IndexedPart { start, .. } => refs_expr(start, r),
                ExprKind::Concat(xs) => xs.iter().for_each(|x| targets(x, r)),
                _ => {}
            }
        }
        targets(e, &mut r);
        self.triggers_of_refs(r, sc)
    }

    fn triggers_of_refs(&mut self, r: Refs, sc: &Scope) -> Vec<Trigger> {
        let mut seen: HashSet<SigId> = HashSet::new();
        let mut out = Vec::new();
        let mut names = r.names;
        let mut visited: HashSet<String> = HashSet::new();
        let mut calls = r.calls;
        while let Some(c) = calls.pop() {
            if !visited.insert(c.clone()) {
                continue;
            }
            if let Some(rt) = sc.module.routines.get(&c).cloned() {
                let mut inner = Refs::default();
                let locals: HashSet<String> = rt
                    .args
                    .iter()
                    .map(|a| a.name.clone())
                    .chain(rt.decls.iter().map(|d| d.name.clone()))
                    .collect();
                for st in &rt.body {
                    refs_stmt(st, &mut inner);
                }
                names.extend(inner.names.into_iter().filter(|n| !locals.contains(n)));
                calls.extend(inner.calls);
            }
        }
        for n in names {
            if sc.vals.contains_key(&n) {
                continue;
            }
            if let Some(sym) = sc.syms.get(&n) {
                if seen.insert(sym.id) {
                    out.push(Trigger {
                        sig: sym.id,
                        bit: None,
                        edge: Edge::Any,
                    });
                }
            }
        }
        out
    }

    /// Lowers a statement into `b`.
    pub(crate) fn stmt(&mut self, b: &mut Body, s: &Stmt, sc: &Scope) {
        match &s.kind {
            StmtKind::Null => {}
            StmtKind::Block { decls, stmts, name } => {
                let sc2 = self.enter_block(b, decls, name.as_deref(), sc);
                for st in stmts {
                    self.stmt(b, st, &sc2);
                }
            }
            StmtKind::Assign {
                lhs,
                rhs,
                nonblocking,
                delay,
            } => {
                let Some(l) = self.lval(lhs, sc) else { return };
                let Some(r) = self.assign_expr(rhs, l.w, sc) else {
                    return;
                };
                if let Some(d) = delay {
                    // Intra-assignment delay: the value is taken now, stored after the delay.
                    let tmp = self.temp(b, "delay", l.w, sc);
                    b.code.push(Op::Assign {
                        lhs: tmp_lval(tmp, l.w),
                        rhs: r,
                        nb: false,
                    });
                    if let Some(amount) = self.self_expr(d, sc) {
                        b.code.push(Op::Delay {
                            amount,
                            unit_ps: b.unit_ps,
                        });
                    }
                    let src = Ex {
                        w: l.w,
                        s: false,
                        k: K::Sig(tmp),
                    };
                    b.code.push(Op::Assign {
                        lhs: l,
                        rhs: src,
                        nb: *nonblocking,
                    });
                    return;
                }
                b.code.push(Op::Assign {
                    lhs: l,
                    rhs: r,
                    nb: *nonblocking,
                });
            }
            StmtKind::If {
                cond,
                then_stmt,
                else_stmt,
            } => {
                let Some(c) = self.self_expr(cond, sc) else {
                    return;
                };
                let jf = b.code.len();
                b.code.push(Op::JumpIfNot { cond: c, to: 0 });
                self.stmt(b, then_stmt, sc);
                match else_stmt {
                    Some(e) => {
                        let jend = b.code.len();
                        b.code.push(Op::Jump(0));
                        let els = b.code.len() as u32;
                        patch(&mut b.code, jf, els);
                        self.stmt(b, e, sc);
                        let end = b.code.len() as u32;
                        patch(&mut b.code, jend, end);
                    }
                    None => {
                        let end = b.code.len() as u32;
                        patch(&mut b.code, jf, end);
                    }
                }
            }
            StmtKind::Case { kind, expr, items } => self.case(b, *kind, expr, items, sc),
            StmtKind::For {
                init,
                cond,
                step,
                body,
            } => {
                let sc2 = match &init.kind {
                    StmtKind::Block { decls, stmts, name } if !decls.is_empty() => {
                        let s2 = self.enter_block(b, decls, name.as_deref(), sc);
                        for st in stmts {
                            self.stmt(b, st, &s2);
                        }
                        s2
                    }
                    _ => {
                        self.stmt(b, init, sc);
                        sc.clone()
                    }
                };
                let top = b.code.len() as u32;
                let Some(c) = self.self_expr(cond, &sc2) else {
                    return;
                };
                let jf = b.code.len();
                b.code.push(Op::JumpIfNot { cond: c, to: 0 });
                b.loops.push((Vec::new(), Vec::new()));
                self.stmt(b, body, &sc2);
                let cont = b.code.len() as u32;
                self.stmt(b, step, &sc2);
                b.code.push(Op::Jump(top));
                self.close_loop(b, jf, cont);
            }
            StmtKind::While { cond, body } => {
                let top = b.code.len() as u32;
                let Some(c) = self.self_expr(cond, sc) else {
                    return;
                };
                let jf = b.code.len();
                b.code.push(Op::JumpIfNot { cond: c, to: 0 });
                b.loops.push((Vec::new(), Vec::new()));
                self.stmt(b, body, sc);
                b.code.push(Op::Jump(top));
                self.close_loop(b, jf, top);
            }
            StmtKind::Repeat { count, body } => {
                let tmp = self.temp(b, "repeat", 32, sc);
                let Some(n) = self.assign_expr(count, 32, sc) else {
                    return;
                };
                b.code.push(Op::Assign {
                    lhs: tmp_lval(tmp, 32),
                    rhs: n,
                    nb: false,
                });
                let top = b.code.len() as u32;
                let jf = b.code.len();
                b.code.push(Op::JumpIfNot {
                    cond: Ex {
                        w: 32,
                        s: false,
                        k: K::Sig(tmp),
                    },
                    to: 0,
                });
                b.loops.push((Vec::new(), Vec::new()));
                self.stmt(b, body, sc);
                let cont = b.code.len() as u32;
                let dec = Ex {
                    w: 32,
                    s: false,
                    k: K::Bin {
                        op: BinOp::Sub,
                        a: Box::new(Ex {
                            w: 32,
                            s: false,
                            k: K::Sig(tmp),
                        }),
                        b: Box::new(Ex {
                            w: 32,
                            s: false,
                            k: K::Const(1),
                        }),
                        os: false,
                    },
                };
                b.code.push(Op::Assign {
                    lhs: tmp_lval(tmp, 32),
                    rhs: dec,
                    nb: false,
                });
                b.code.push(Op::Jump(top));
                self.close_loop(b, jf, cont);
            }
            StmtKind::Forever { body } => {
                let top = b.code.len() as u32;
                b.loops.push((Vec::new(), Vec::new()));
                self.stmt(b, body, sc);
                b.code.push(Op::Jump(top));
                self.close_loop(b, usize::MAX, top);
            }
            StmtKind::Delay { amount, stmt } => {
                if let Some(a) = self.self_expr(amount, sc) {
                    b.code.push(Op::Delay {
                        amount: a,
                        unit_ps: b.unit_ps,
                    });
                }
                self.stmt(b, stmt, sc);
            }
            StmtKind::EventCtrl { events, stmt } => {
                let trig = match events {
                    Some(list) => {
                        let mut t = Vec::new();
                        for ev in list {
                            let edge = match ev.edge {
                                rtl_frontend::ast::Edge::Any => Edge::Any,
                                rtl_frontend::ast::Edge::Pos => Edge::Pos,
                                rtl_frontend::ast::Edge::Neg => Edge::Neg,
                            };
                            t.extend(self.event_triggers(&ev.expr, edge, sc));
                        }
                        t
                    }
                    None => self.triggers_of_stmt(stmt, sc),
                };
                b.code.push(Op::Wait(trig));
                self.stmt(b, stmt, sc);
            }
            StmtKind::Wait { cond, stmt } => {
                let check = b.code.len() as u32;
                let Some(c) = self.self_expr(cond, sc) else {
                    return;
                };
                let jt = b.code.len();
                b.code.push(Op::JumpIf { cond: c, to: 0 });
                let trig = self.triggers_of_expr(cond, sc);
                b.code.push(Op::Wait(trig));
                b.code.push(Op::Jump(check));
                let ok = b.code.len() as u32;
                patch(&mut b.code, jt, ok);
                self.stmt(b, stmt, sc);
            }
            StmtKind::SysCall { name, args } => self.sys_task(b, name, args, &s.pos, sc),
            StmtKind::Call { name, args } => self.call_task(b, name, args, &s.pos, sc),
            StmtKind::Return(e) => {
                if let (Some((sig, w, sg)), Some(e)) = (b.ret, e) {
                    let rhs = self.assign_expr(e, w, sc);
                    if let Some(rhs) = rhs {
                        b.code.push(Op::Assign {
                            lhs: tmp_lval(sig, w),
                            rhs,
                            nb: false,
                        });
                    }
                    let _ = sg;
                }
                b.rets.push(b.code.len());
                b.code.push(Op::Jump(0));
            }
            StmtKind::Break => {
                let at = b.code.len();
                b.code.push(Op::Jump(0));
                match b.loops.last_mut() {
                    Some(l) => l.0.push(at),
                    None => self.diag(&s.pos, "break", "`break` outside a loop"),
                }
            }
            StmtKind::Continue => {
                let at = b.code.len();
                b.code.push(Op::Jump(0));
                match b.loops.last_mut() {
                    Some(l) => l.1.push(at),
                    None => self.diag(&s.pos, "continue", "`continue` outside a loop"),
                }
            }
            StmtKind::Disable(n) => {
                self.diag(
                    &s.pos,
                    "disable",
                    format!("`disable {n}` is not supported by the engine"),
                );
            }
        }
    }

    /// Ends a loop: patches the exit jump (if any), `break`s and `continue`s.
    fn close_loop(&mut self, b: &mut Body, exit_jump: usize, cont: u32) {
        let end = b.code.len() as u32;
        if exit_jump != usize::MAX {
            patch(&mut b.code, exit_jump, end);
        }
        let (breaks, conts) = b.loops.pop().unwrap_or_default();
        for at in breaks {
            patch(&mut b.code, at, end);
        }
        for at in conts {
            patch(&mut b.code, at, cont);
        }
    }

    /// Allocates a local declaration (a block, function or task local), including an
    /// unpacked memory dimension.
    pub(crate) fn alloc_local(
        &mut self,
        name: &str,
        d: &rtl_frontend::ast::Decl,
        sc: &Scope,
    ) -> Sym {
        let (w, s, msb, lsb) = self.shape(d.range.as_ref(), Some(&d.kind), d.signed, sc);
        let (depth, mem_lo) = match d.dims.first() {
            None => (0, 0),
            Some(r) => {
                let m = self
                    .konst(&r.msb, sc)
                    .map(|v| v.as_i128() as i64)
                    .unwrap_or(0);
                let l = self
                    .konst(&r.lsb, sc)
                    .map(|v| v.as_i128() as i64)
                    .unwrap_or(0);
                (((m - l).abs() + 1).min(1 << 20) as u32, m.min(l))
            }
        };
        let id = self.alloc(name, w, s, depth, mem_lo, msb, lsb, true);
        Sym {
            id,
            width: w,
            signed: s,
            depth,
            msb,
            lsb,
        }
    }

    /// A hidden 'temporary' variable of a process.
    fn temp(&mut self, _b: &mut Body, what: &str, w: u32, sc: &Scope) -> SigId {
        let name = format!("{}{}#{}", sc.prefix, what, self.signals.len());
        self.alloc(&name, w, false, 0, 0, w as i64 - 1, 0, true)
    }

    /// Declares a block's locals (hidden signals) and runs their initialisers.
    fn enter_block(
        &mut self,
        b: &mut Body,
        decls: &[rtl_frontend::ast::Decl],
        name: Option<&str>,
        sc: &Scope,
    ) -> Scope {
        let mut sc2 = sc.clone();
        let prefix = match name {
            Some(n) => format!("{}{}.", sc.prefix, n),
            None => sc.prefix.clone(),
        };
        sc2.prefix = prefix.clone();
        for d in decls {
            if d.kind == DataKind::Genvar {
                continue;
            }
            let sym = self.alloc_local(&format!("{prefix}{}", d.name), d, &sc2);
            sc2.syms.insert(d.name.clone(), sym);
        }
        for d in decls {
            if let (Some(init), Some(sym)) = (&d.init, sc2.syms.get(&d.name).copied()) {
                if let Some(rhs) = self.assign_expr(init, sym.width, &sc2) {
                    b.code.push(Op::Assign {
                        lhs: tmp_lval(sym.id, sym.width),
                        rhs,
                        nb: false,
                    });
                }
            }
        }
        sc2
    }

    fn case(
        &mut self,
        b: &mut Body,
        kind: CaseKind,
        expr: &Expr,
        items: &[rtl_frontend::ast::CaseItem],
        sc: &Scope,
    ) {
        let mut cw = self.width_of(expr, sc);
        let mut cs = self.sign_of(expr, sc);
        for it in items {
            for l in &it.labels {
                cw = cw.max(self.width_of(l, sc));
                cs = cs && self.sign_of(l, sc);
            }
        }
        let Some(sel) = self.lower(expr, cw, cs, sc) else {
            return;
        };
        // Dispatch: one JumpIf per label, then a jump to the default (or the end).
        let mut dispatch: Vec<(usize, usize)> = Vec::new(); // (jump index, item index)
        let mut default_item: Option<usize> = None;
        for (i, it) in items.iter().enumerate() {
            if it.labels.is_empty() {
                default_item = Some(i);
                continue;
            }
            for l in &it.labels {
                let m = match (&l.kind, kind) {
                    (ExprKind::Number(n), CaseKind::Casez | CaseKind::Casex) => {
                        let (v, care) = wildcard_label(n, cw);
                        Some(Ex {
                            w: 1,
                            s: false,
                            k: K::MaskEq {
                                a: Box::new(sel.clone()),
                                b: Box::new(Ex {
                                    w: cw,
                                    s: false,
                                    k: K::Const(v),
                                }),
                                mask: care,
                            },
                        })
                    }
                    _ => self.lower(l, cw, cs, sc).map(|lab| Ex {
                        w: 1,
                        s: false,
                        k: K::MaskEq {
                            a: Box::new(sel.clone()),
                            b: Box::new(lab),
                            mask: mask_w(cw),
                        },
                    }),
                };
                let Some(m) = m else { return };
                dispatch.push((b.code.len(), i));
                b.code.push(Op::JumpIf { cond: m, to: 0 });
            }
        }
        let to_default = b.code.len();
        b.code.push(Op::Jump(0));
        let mut body_at = vec![0u32; items.len()];
        let mut end_jumps = Vec::new();
        for (i, it) in items.iter().enumerate() {
            body_at[i] = b.code.len() as u32;
            self.stmt(b, &it.body, sc);
            end_jumps.push(b.code.len());
            b.code.push(Op::Jump(0));
        }
        let end = b.code.len() as u32;
        for (at, item) in dispatch {
            patch(&mut b.code, at, body_at[item]);
        }
        let dflt = match default_item {
            Some(i) => body_at[i],
            None => end,
        };
        patch(&mut b.code, to_default, dflt);
        for at in end_jumps {
            patch(&mut b.code, at, end);
        }
    }

    fn sys_task(&mut self, b: &mut Body, name: &str, args: &[Expr], pos: &Pos, sc: &Scope) {
        let scope = b.scope_name.clone();
        let display = |this: &mut Self, args: &[Expr], newline: bool| -> Option<Op> {
            let mut out = Vec::new();
            for a in args {
                if let ExprKind::Str(s) = &a.kind {
                    out.push(Arg::Str(s.clone()));
                } else {
                    let sg = this.sign_of(a, sc);
                    let e = this.self_expr(a, sc)?;
                    out.push(Arg::Val(e, sg));
                }
            }
            Some(Op::Sys(Sys::Display {
                args: out,
                newline,
                scope: scope.clone(),
            }))
        };
        match name {
            "$display" | "$strobe" | "$monitor" | "$error" | "$warning" | "$info" => {
                if let Some(op) = display(self, args, true) {
                    b.code.push(op);
                }
            }
            "$write" => {
                if let Some(op) = display(self, args, false) {
                    b.code.push(op);
                }
            }
            "$finish" | "$stop" => b.code.push(Op::Sys(Sys::Finish)),
            "$fatal" => {
                let rest = if args.len() > 1 { &args[1..] } else { &[] };
                if let Some(op) = display(self, rest, true) {
                    b.code.push(op);
                }
                b.code.push(Op::Sys(Sys::Finish));
            }
            "$readmemh" | "$readmemb" => {
                let file = match args.first().map(|a| &a.kind) {
                    Some(ExprKind::Str(s)) => s.clone(),
                    _ => {
                        self.diag(pos, "readmem", "the file name must be a string literal");
                        return;
                    }
                };
                let Some(ExprKind::Ident(mem)) = args.get(1).map(|a| &a.kind) else {
                    self.diag(pos, "readmem", "the memory must be a plain name");
                    return;
                };
                let Some(sym) = sc.syms.get(mem).copied() else {
                    self.diag(pos, "readmem", format!("`{mem}` is not declared"));
                    return;
                };
                if sym.depth == 0 {
                    self.diag(pos, "readmem", format!("`{mem}` is not a memory"));
                    return;
                }
                b.code.push(Op::Sys(Sys::ReadMem {
                    sig: sym.id,
                    file,
                    hex: name == "$readmemh",
                }));
            }
            _ => self.diag(
                pos,
                "system task",
                format!("system task `{name}` is not supported by the engine"),
            ),
        }
    }

    /// Inlines a task call. Its formals and locals are static hidden signals.
    fn call_task(&mut self, b: &mut Body, name: &str, args: &[Expr], pos: &Pos, sc: &Scope) {
        let Some(r) = sc.module.routines.get(name).cloned() else {
            self.diag(pos, "task", format!("task `{name}` is not defined"));
            return;
        };
        if r.kind != RoutineKind::Task {
            self.diag(
                pos,
                "task",
                format!("`{name}` is a function; call it in an expression"),
            );
            return;
        }
        if self.task_stack.iter().any(|t| t == name) {
            self.diag(
                pos,
                "task",
                format!("task `{name}` calls itself (recursion is not supported)"),
            );
            return;
        }
        if r.args.len() != args.len() {
            self.diag(
                pos,
                "task",
                format!("`{name}` takes {} arguments", r.args.len()),
            );
            return;
        }
        let mut tsc = sc.clone();
        tsc.syms = sc.module.syms.clone();
        tsc.vals = sc.module.vals.clone();
        tsc.prefix = format!("{}{}.", sc.prefix, name);
        let mut formals: Vec<(SigId, u32, bool)> = Vec::new();
        for a in &r.args {
            let (w, s, msb, lsb) = self.shape(a.range.as_ref(), a.kind.as_ref(), a.signed, &tsc);
            let id = self.alloc(
                &format!("{}{}", tsc.prefix, a.name),
                w,
                s,
                0,
                0,
                msb,
                lsb,
                true,
            );
            tsc.syms.insert(
                a.name.clone(),
                Sym {
                    id,
                    width: w,
                    signed: s,
                    depth: 0,
                    msb,
                    lsb,
                },
            );
            formals.push((id, w, s));
        }
        for d in &r.decls {
            let sym = self.alloc_local(&format!("{}{}", tsc.prefix, d.name), d, &tsc);
            tsc.syms.insert(d.name.clone(), sym);
        }
        // Copy the actuals in.
        let mut outs: Vec<(usize, SigId, u32)> = Vec::new();
        for (i, (a, (fid, fw, _))) in args.iter().zip(&formals).enumerate() {
            let dir_out = r.args[i].dir == Some(rtl_frontend::ast::Dir::Output)
                || r.args[i].dir == Some(rtl_frontend::ast::Dir::Inout);
            let dir_in = r.args[i].dir != Some(rtl_frontend::ast::Dir::Output);
            if dir_in {
                if let Some(rhs) = self.assign_expr(a, *fw, sc) {
                    b.code.push(Op::Assign {
                        lhs: tmp_lval(*fid, *fw),
                        rhs,
                        nb: false,
                    });
                }
            }
            if dir_out {
                outs.push((i, *fid, *fw));
            }
        }
        for d in &r.decls {
            if let (Some(init), Some(sym)) = (&d.init, tsc.syms.get(&d.name).copied())
                && let Some(rhs) = self.assign_expr(init, sym.width, &tsc)
            {
                b.code.push(Op::Assign {
                    lhs: tmp_lval(sym.id, sym.width),
                    rhs,
                    nb: false,
                });
            }
        }
        self.task_stack.push(name.to_string());
        let saved_rets = std::mem::take(&mut b.rets);
        let saved_ret = b.ret.take();
        for st in &r.body {
            self.stmt(b, st, &tsc);
        }
        let end = b.code.len() as u32;
        for at in std::mem::replace(&mut b.rets, saved_rets) {
            patch(&mut b.code, at, end);
        }
        b.ret = saved_ret;
        self.task_stack.pop();
        // Copy the outputs back.
        for (i, fid, fw) in outs {
            if let Some(l) = self.lval(&args[i], sc) {
                b.code.push(Op::Assign {
                    lhs: l.clone(),
                    rhs: Ex {
                        w: fw,
                        s: false,
                        k: K::Sig(fid),
                    },
                    nb: false,
                });
            }
        }
    }

    /// Lowers a function into a [`crate::ir::Func`].
    pub(crate) fn lower_function(&mut self, r: &Routine, caller: &Scope) -> crate::ir::Func {
        let module = caller.module.clone();
        let mut sc = Scope {
            vals: module.vals.clone(),
            syms: module.syms.clone(),
            module: module.clone(),
            prefix: format!("{}.", r.name),
        };
        let mut formals = Vec::new();
        for a in &r.args {
            let (w, s, msb, lsb) = self.shape(a.range.as_ref(), a.kind.as_ref(), a.signed, &sc);
            let id = self.alloc(
                &format!("{}.{}", r.name, a.name),
                w,
                s,
                0,
                0,
                msb,
                lsb,
                true,
            );
            sc.syms.insert(
                a.name.clone(),
                Sym {
                    id,
                    width: w,
                    signed: s,
                    depth: 0,
                    msb,
                    lsb,
                },
            );
            formals.push(id);
        }
        let (rw, rs, rmsb, rlsb) =
            self.shape(r.ret_range.as_ref(), r.ret_kind.as_ref(), r.ret_signed, &sc);
        let ret = self.alloc(&r.name, rw, rs, 0, 0, rmsb, rlsb, true);
        // Inside the function, its name is the return variable.
        sc.syms.insert(
            r.name.clone(),
            Sym {
                id: ret,
                width: rw,
                signed: rs,
                depth: 0,
                msb: rmsb,
                lsb: rlsb,
            },
        );
        for d in &r.decls {
            let sym = self.alloc_local(&format!("{}.{}", r.name, d.name), d, &sc);
            sc.syms.insert(d.name.clone(), sym);
        }
        let mut b = Body::new(&format!("{}{}", caller.prefix, r.name), module.unit_ps);
        b.ret = Some((ret, rw, rs));
        for d in &r.decls {
            if let (Some(init), Some(sym)) = (&d.init, sc.syms.get(&d.name).copied())
                && let Some(rhs) = self.assign_expr(init, sym.width, &sc)
            {
                b.code.push(Op::Assign {
                    lhs: tmp_lval(sym.id, sym.width),
                    rhs,
                    nb: false,
                });
            }
        }
        for st in &r.body {
            self.stmt(&mut b, st, &sc);
        }
        let end = b.code.len() as u32;
        for at in std::mem::take(&mut b.rets) {
            patch(&mut b.code, at, end);
        }
        b.code.push(Op::Halt);
        crate::ir::Func {
            name: r.name.clone(),
            formals,
            ret,
            code: b.code,
        }
    }
}

/// A part-select target of a signal or of a memory word.
fn part_target(
    sym: &Sym,
    word: Option<Ex>,
    word_off: i64,
    start: Ex,
    width: u32,
    up: bool,
) -> LVal {
    LVal {
        w: width,
        parts: vec![LPart {
            sig: sym.id,
            word,
            word_off,
            sel: Some((start, sym.lsb, sym.msb < sym.lsb, up)),
            w: width,
        }],
    }
}

/// An assignment target that is a whole signal.
pub(crate) fn tmp_lval(sig: SigId, w: u32) -> LVal {
    LVal {
        w,
        parts: vec![LPart {
            sig,
            word: None,
            word_off: 0,
            sel: None,
            w,
        }],
    }
}
