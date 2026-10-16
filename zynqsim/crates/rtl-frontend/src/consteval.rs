//! Constant expression evaluation for parameters, ranges and generate conditions.
//!
//! Values are unsigned or signed integers of up to 128 bits with the Verilog
//! sizing and signedness rules for the arithmetic, bitwise, shift, comparison and
//! logical operators, reductions, concatenation and replication, selects, `$clog2`,
//! `$signed`/`$unsigned`, and calls of constant functions (a small interpreter for
//! the statement forms that appear in them). `x`, `z` and reals are errors.

use crate::ast::{
    BinOp, CaseKind, DataKind, Expr, ExprKind, Range, Routine, RoutineKind, Stmt, StmtKind, UnOp,
};
use crate::lexer::Number;
use std::collections::{BTreeMap, HashMap};

/// Largest width a constant value may have.
pub const MAX_WIDTH: u32 = 128;
/// Work limit of one evaluation (statements executed in constant functions).
const STEP_LIMIT: u64 = 1_000_000;

/// Result of an evaluation: a value or a message.
pub type EvalResult<T> = Result<T, String>;

/// A constant value: `width` bits of `bits`, interpreted as signed if `signed`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Val {
    /// The bits (upper bits above `width` are zero).
    pub bits: u128,
    /// Width in bits, 1 to 128.
    pub width: u32,
    /// Signed interpretation.
    pub signed: bool,
}

/// Mask of the low `w` bits.
pub fn mask(w: u32) -> u128 {
    if w >= 128 {
        u128::MAX
    } else {
        (1u128 << w) - 1
    }
}

impl Val {
    /// A value truncated to `width` bits.
    pub fn new(bits: u128, width: u32, signed: bool) -> Val {
        let width = width.clamp(1, MAX_WIDTH);
        Val {
            bits: bits & mask(width),
            width,
            signed,
        }
    }

    /// A 32-bit signed integer (the type of an unsized decimal).
    pub fn int(v: i64) -> Val {
        Val::new(v as u128, 32, true)
    }

    /// The numeric value, sign-extended when the value is signed.
    pub fn as_i128(self) -> i128 {
        if self.signed && self.width < 128 && (self.bits >> (self.width - 1)) & 1 == 1 {
            (self.bits | !mask(self.width)) as i128
        } else {
            self.bits as i128
        }
    }

    /// Whether any bit is set.
    pub fn truthy(self) -> bool {
        self.bits != 0
    }

    /// The value as `u64`, if it fits and is not negative.
    pub fn to_u64(self) -> Option<u64> {
        let v = self.as_i128();
        if v < 0 { None } else { u64::try_from(v).ok() }
    }
}

/// Resizes `v` to `w` bits. Sign-extends when `sign` is true, zero-extends otherwise.
fn resize(v: Val, w: u32, sign: bool) -> Val {
    if sign {
        Val::new(v.as_i128() as u128, w, true)
    } else {
        Val::new(v.bits, w, false)
    }
}

/// Evaluation context: constant names, constant functions and the step counter.
pub struct Ctx<'a> {
    /// Values of parameters, genvars and enum members, and of function locals.
    pub vars: HashMap<String, Val>,
    funcs: &'a BTreeMap<String, Routine>,
    steps: u64,
}

/// Non-local control flow inside a constant function.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Flow {
    Normal,
    Break,
    Continue,
    Return,
}

impl<'a> Ctx<'a> {
    /// A context with the given names and routines.
    pub fn new(vars: HashMap<String, Val>, funcs: &'a BTreeMap<String, Routine>) -> Ctx<'a> {
        Ctx {
            vars,
            funcs,
            steps: 0,
        }
    }

    /// Evaluates a constant expression.
    pub fn eval(&mut self, e: &Expr) -> EvalResult<Val> {
        match &e.kind {
            ExprKind::Ident(n) => self
                .vars
                .get(n)
                .copied()
                .ok_or_else(|| format!("`{n}` is not a constant in this context")),
            ExprKind::Number(n) => number_val(n),
            ExprKind::Str(_) => Err("string literals are not constant values here".into()),
            ExprKind::Unary(op, a) => {
                let v = self.eval(a)?;
                Ok(unary(*op, v))
            }
            ExprKind::Binary(op, a, b) => {
                let l = self.eval(a)?;
                let r = self.eval(b)?;
                binary(*op, l, r)
            }
            ExprKind::Ternary(c, t, f) => {
                let cv = self.eval(c)?;
                let tv = self.eval(t)?;
                let fv = self.eval(f)?;
                let w = tv.width.max(fv.width);
                let s = tv.signed && fv.signed;
                let chosen = if cv.truthy() { tv } else { fv };
                Ok(resize(chosen, w, s))
            }
            ExprKind::Concat(parts) => {
                let mut acc = Val::new(0, 1, false);
                let mut first = true;
                for p in parts {
                    let v = self.eval(p)?;
                    acc = if first {
                        Val::new(v.bits, v.width, false)
                    } else {
                        concat2(acc, v)?
                    };
                    first = false;
                }
                if first {
                    return Err("empty concatenation".into());
                }
                Ok(acc)
            }
            ExprKind::Repl(count, parts) => {
                let n = self
                    .eval(count)?
                    .to_u64()
                    .ok_or("negative or unknown replication count")?;
                let mut unit = Val::new(0, 1, false);
                let mut first = true;
                for p in parts {
                    let v = self.eval(p)?;
                    unit = if first {
                        Val::new(v.bits, v.width, false)
                    } else {
                        concat2(unit, v)?
                    };
                    first = false;
                }
                if first || n == 0 {
                    return Err("empty or zero replication is not a value".into());
                }
                let mut acc = unit;
                for _ in 1..n {
                    acc = concat2(acc, unit)?;
                }
                Ok(acc)
            }
            ExprKind::Index(base, idx) => {
                let b = self.eval(base)?;
                let i = self.eval(idx)?;
                let i = i.as_i128();
                if i < 0 || i >= b.width as i128 {
                    return Ok(Val::new(0, 1, false));
                }
                Ok(Val::new(b.bits >> i, 1, false))
            }
            ExprKind::Part(base, msb, lsb) => {
                let b = self.eval(base)?;
                let hi = self.eval(msb)?.as_i128();
                let lo = self.eval(lsb)?.as_i128();
                part(b, hi.max(lo), hi.min(lo))
            }
            ExprKind::IndexedPart {
                base,
                start,
                width,
                up,
            } => {
                let b = self.eval(base)?;
                let s = self.eval(start)?.as_i128();
                let w = self.eval(width)?.as_i128();
                if w <= 0 {
                    return Err("indexed part select with a non-positive width".into());
                }
                let (lo, hi) = if *up { (s, s + w - 1) } else { (s - w + 1, s) };
                part(b, hi, lo)
            }
            ExprKind::Call(name, args) => {
                let mut vals = Vec::new();
                for a in args {
                    vals.push(self.eval(a)?);
                }
                self.call(name, vals)
            }
            ExprKind::SysCall(name, args) => {
                let mut vals = Vec::new();
                for a in args {
                    vals.push(self.eval(a)?);
                }
                match (name.as_str(), vals.as_slice()) {
                    ("$clog2", [v]) => Ok(Val::int(clog2(v.as_i128()) as i64)),
                    ("$signed", [v]) => Ok(Val { signed: true, ..*v }),
                    ("$unsigned", [v]) => Ok(Val {
                        signed: false,
                        ..*v
                    }),
                    ("$bits", [v]) => Ok(Val::int(v.width as i64)),
                    _ => Err(format!("system function `{name}` is not constant")),
                }
            }
            ExprKind::Cast(size, inner) => {
                let v = self.eval(inner)?;
                Ok(match size {
                    Some(w) => resize(v, *w, v.signed),
                    None => v,
                })
            }
        }
    }

    /// Calls a constant function.
    fn call(&mut self, name: &str, args: Vec<Val>) -> EvalResult<Val> {
        let funcs = self.funcs;
        let Some(r) = funcs.get(name) else {
            return Err(format!("`{name}` is not a constant function"));
        };
        if r.kind == RoutineKind::Task {
            return Err(format!("task `{name}` has no value"));
        }
        if args.len() != r.args.len() {
            return Err(format!(
                "`{name}` takes {} arguments, got {}",
                r.args.len(),
                args.len()
            ));
        }
        let saved = self.vars.clone();
        let result = (|| -> EvalResult<Val> {
            for (decl, v) in r.args.iter().zip(args) {
                let (w, s) = self.shape(decl.range.as_ref(), decl.signed, decl.kind.as_ref())?;
                self.vars.insert(decl.name.clone(), resize(v, w, s));
            }
            let (rw, rs) = self.shape(r.ret_range.as_ref(), r.ret_signed, r.ret_kind.as_ref())?;
            self.vars.insert(name.to_string(), Val::new(0, rw, rs));
            for d in &r.decls {
                if !d.dims.is_empty() {
                    return Err(format!(
                        "unpacked arrays in constant function `{name}` are not supported"
                    ));
                }
                let (w, s) = self.shape(d.range.as_ref(), d.signed, Some(&d.kind))?;
                let init = match &d.init {
                    Some(e) => self.eval(e)?.bits,
                    None => 0,
                };
                self.vars.insert(d.name.clone(), Val::new(init, w, s));
            }
            for s in &r.body {
                if self.exec(s, name)? == Flow::Return {
                    break;
                }
            }
            Ok(self
                .vars
                .get(name)
                .copied()
                .unwrap_or(Val::new(0, 1, false)))
        })();
        self.vars = saved;
        result
    }

    /// Width and signedness of a declaration: its packed range, else the natural
    /// width of its kind (`integer` 32 bits signed, `time` 64 bits, otherwise 1 bit).
    fn shape(
        &mut self,
        range: Option<&Range>,
        signed: bool,
        kind: Option<&DataKind>,
    ) -> EvalResult<(u32, bool)> {
        match (range, kind) {
            (Some(r), _) => Ok((self.range_width(r)?, signed)),
            (None, Some(DataKind::Integer | DataKind::Int)) => Ok((32, true)),
            (None, Some(DataKind::Time)) => Ok((64, false)),
            (None, _) => Ok((1, signed)),
        }
    }

    /// Width of `[msb:lsb]`.
    pub fn range_width(&mut self, r: &Range) -> EvalResult<u32> {
        let m = self.eval(&r.msb)?.as_i128();
        let l = self.eval(&r.lsb)?.as_i128();
        let w = (m - l).abs() + 1;
        if w > MAX_WIDTH as i128 {
            return Err(format!("range of {w} bits exceeds {MAX_WIDTH}"));
        }
        Ok(w as u32)
    }

    fn tick(&mut self) -> EvalResult<()> {
        self.steps += 1;
        if self.steps > STEP_LIMIT {
            return Err("constant evaluation exceeded its step limit (infinite loop?)".into());
        }
        Ok(())
    }

    fn exec(&mut self, s: &Stmt, ret: &str) -> EvalResult<Flow> {
        self.tick()?;
        match &s.kind {
            StmtKind::Null => Ok(Flow::Normal),
            StmtKind::Block { decls, stmts, .. } => {
                for d in decls {
                    let (w, sg) = self.shape(d.range.as_ref(), d.signed, Some(&d.kind))?;
                    self.vars.insert(d.name.clone(), Val::new(0, w, sg));
                }
                for st in stmts {
                    match self.exec(st, ret)? {
                        Flow::Normal => {}
                        other => return Ok(other),
                    }
                }
                Ok(Flow::Normal)
            }
            StmtKind::Assign {
                lhs,
                rhs,
                nonblocking,
                ..
            } => {
                if *nonblocking {
                    return Err("non-blocking assignment in a constant function".into());
                }
                let ExprKind::Ident(name) = &lhs.kind else {
                    return Err(
                        "only plain variables may be assigned in a constant function".into(),
                    );
                };
                let v = self.eval(rhs)?;
                let old = self.vars.get(name).copied().ok_or_else(|| {
                    format!("assignment to undeclared `{name}` in a constant function")
                })?;
                self.vars
                    .insert(name.clone(), resize(v, old.width, old.signed));
                Ok(Flow::Normal)
            }
            StmtKind::If {
                cond,
                then_stmt,
                else_stmt,
            } => {
                if self.eval(cond)?.truthy() {
                    self.exec(then_stmt, ret)
                } else if let Some(e) = else_stmt {
                    self.exec(e, ret)
                } else {
                    Ok(Flow::Normal)
                }
            }
            StmtKind::Case { kind, expr, items } => {
                let sel = self.eval(expr)?;
                let mut default = None;
                for item in items {
                    if item.labels.is_empty() {
                        default = Some(&item.body);
                        continue;
                    }
                    for label in &item.labels {
                        let lv = self.eval(label)?;
                        let w = sel.width.max(lv.width);
                        let (a, b) = (resize(sel, w, false), resize(lv, w, false));
                        let hit = match kind {
                            CaseKind::Case | CaseKind::Casez | CaseKind::Casex => a.bits == b.bits,
                        };
                        if hit {
                            return self.exec(&item.body, ret);
                        }
                    }
                }
                match default {
                    Some(b) => self.exec(b, ret),
                    None => Ok(Flow::Normal),
                }
            }
            StmtKind::For {
                init,
                cond,
                step,
                body,
            } => {
                self.exec(init, ret)?;
                loop {
                    if !self.eval(cond)?.truthy() {
                        break;
                    }
                    match self.exec(body, ret)? {
                        Flow::Break => break,
                        Flow::Return => return Ok(Flow::Return),
                        _ => {}
                    }
                    self.exec(step, ret)?;
                    self.tick()?;
                }
                Ok(Flow::Normal)
            }
            StmtKind::While { cond, body } => {
                while self.eval(cond)?.truthy() {
                    match self.exec(body, ret)? {
                        Flow::Break => break,
                        Flow::Return => return Ok(Flow::Return),
                        _ => {}
                    }
                    self.tick()?;
                }
                Ok(Flow::Normal)
            }
            StmtKind::Repeat { count, body } => {
                let n = self.eval(count)?.to_u64().unwrap_or(0);
                for _ in 0..n {
                    match self.exec(body, ret)? {
                        Flow::Break => break,
                        Flow::Return => return Ok(Flow::Return),
                        _ => {}
                    }
                    self.tick()?;
                }
                Ok(Flow::Normal)
            }
            StmtKind::Return(v) => {
                if let Some(e) = v {
                    let val = self.eval(e)?;
                    let old = self.vars.get(ret).copied().unwrap_or(Val::new(0, 1, false));
                    self.vars
                        .insert(ret.to_string(), resize(val, old.width, old.signed));
                }
                Ok(Flow::Return)
            }
            StmtKind::Break => Ok(Flow::Break),
            StmtKind::Continue => Ok(Flow::Continue),
            _ => Err("this statement is not allowed in a constant function".into()),
        }
    }
}

/// Value of an integer literal.
fn number_val(n: &Number) -> EvalResult<Val> {
    if n.base == '\'' {
        return match n.digits.as_str() {
            "0" => Ok(Val::new(0, 32, false)),
            "1" => Ok(Val::new(u128::MAX, 32, false)),
            _ => Err("x or z in a constant expression".into()),
        };
    }
    if n.digits.bytes().any(|c| matches!(c, b'x' | b'z' | b'?')) {
        return Err("x or z in a constant expression".into());
    }
    let radix = match n.base {
        'b' => 2,
        'o' => 8,
        'h' => 16,
        _ => 10,
    };
    let mut v: u128 = 0;
    for c in n.digits.bytes() {
        let d = (c as char)
            .to_digit(radix)
            .ok_or("invalid digit in literal")?;
        v = v
            .checked_mul(radix as u128)
            .and_then(|x| x.checked_add(d as u128))
            .ok_or("literal wider than 128 bits")?;
    }
    let (width, signed) = match n.size {
        Some(w) => {
            if w == 0 || w > MAX_WIDTH {
                return Err(format!("literal width {w} is outside 1..=128"));
            }
            (w, n.signed)
        }
        None => (32, n.base == 'd'),
    };
    Ok(Val::new(v, width, signed))
}

fn unary(op: UnOp, v: Val) -> Val {
    let w = v.width;
    match op {
        UnOp::Plus => v,
        UnOp::Minus => Val::new((!v.bits).wrapping_add(1), w, v.signed),
        UnOp::BitNot => Val::new(!v.bits, w, v.signed),
        UnOp::LNot => Val::new(u128::from(!v.truthy()), 1, false),
        UnOp::RedAnd => Val::new(u128::from(v.bits == mask(w)), 1, false),
        UnOp::RedNand => Val::new(u128::from(v.bits != mask(w)), 1, false),
        UnOp::RedOr => Val::new(u128::from(v.truthy()), 1, false),
        UnOp::RedNor => Val::new(u128::from(!v.truthy()), 1, false),
        UnOp::RedXor => Val::new(u128::from(v.bits.count_ones() % 2), 1, false),
        UnOp::RedXnor => Val::new(u128::from(v.bits.count_ones() % 2 == 0), 1, false),
    }
}

fn binary(op: BinOp, l: Val, r: Val) -> EvalResult<Val> {
    let w = l.width.max(r.width);
    let both_signed = l.signed && r.signed;
    let arith = |f: fn(u128, u128) -> u128| -> Val {
        Val::new(
            f(
                resize(l, w, both_signed).bits,
                resize(r, w, both_signed).bits,
            ),
            w,
            both_signed,
        )
    };
    let cmp = |ord: std::cmp::Ordering| -> Val {
        let a = resize(l, w, both_signed);
        let b = resize(r, w, both_signed);
        let c = if both_signed {
            a.as_i128().cmp(&b.as_i128())
        } else {
            a.bits.cmp(&b.bits)
        };
        Val::new(u128::from(c == ord), 1, false)
    };
    Ok(match op {
        BinOp::Add => arith(|a, b| a.wrapping_add(b)),
        BinOp::Sub => arith(|a, b| a.wrapping_sub(b)),
        BinOp::Mul => arith(|a, b| a.wrapping_mul(b)),
        BinOp::Div | BinOp::Mod => {
            let a = resize(l, w, both_signed);
            let b = resize(r, w, both_signed);
            if b.bits == 0 {
                return Err("division by zero in a constant expression".into());
            }
            if both_signed {
                let (x, y) = (a.as_i128(), b.as_i128());
                let v = if op == BinOp::Div {
                    x.wrapping_div(y)
                } else {
                    x.wrapping_rem(y)
                };
                Val::new(v as u128, w, true)
            } else {
                let v = if op == BinOp::Div {
                    a.bits / b.bits
                } else {
                    a.bits % b.bits
                };
                Val::new(v, w, false)
            }
        }
        BinOp::Pow => {
            let e = r.as_i128();
            if e < 0 {
                return Err("negative exponent in a constant expression".into());
            }
            let base = Val::new(l.bits, l.width, l.signed);
            let mut acc: u128 = 1;
            for _ in 0..e.min(256) {
                acc = acc.wrapping_mul(base.bits);
            }
            Val::new(acc, l.width, l.signed)
        }
        BinOp::And => arith(|a, b| a & b),
        BinOp::Or => arith(|a, b| a | b),
        BinOp::Xor => arith(|a, b| a ^ b),
        BinOp::Xnor => arith(|a, b| !(a ^ b)),
        BinOp::Shl | BinOp::AShl => shift(l, r, false),
        BinOp::Shr => shift(l, r, true),
        BinOp::AShr => {
            if l.signed {
                let amt = r.bits.min(128) as u32;
                let x = l.as_i128();
                let sh = if amt >= 128 {
                    if x < 0 { -1 } else { 0 }
                } else {
                    x >> amt
                };
                Val::new(sh as u128, l.width, true)
            } else {
                shift(l, r, true)
            }
        }
        BinOp::Lt => cmp(std::cmp::Ordering::Less),
        BinOp::Gt => cmp(std::cmp::Ordering::Greater),
        BinOp::Eq | BinOp::CaseEq => cmp(std::cmp::Ordering::Equal),
        BinOp::Ne | BinOp::CaseNe => Val::new(
            u128::from(!matches!(cmp(std::cmp::Ordering::Equal).bits, 1)),
            1,
            false,
        ),
        BinOp::Le => Val::new(
            u128::from(!matches!(cmp(std::cmp::Ordering::Greater).bits, 1)),
            1,
            false,
        ),
        BinOp::Ge => Val::new(
            u128::from(!matches!(cmp(std::cmp::Ordering::Less).bits, 1)),
            1,
            false,
        ),
        BinOp::LAnd => Val::new(u128::from(l.truthy() && r.truthy()), 1, false),
        BinOp::LOr => Val::new(u128::from(l.truthy() || r.truthy()), 1, false),
    })
}

fn shift(l: Val, r: Val, right: bool) -> Val {
    let amt = if r.bits > 1024 { 1024 } else { r.bits as u32 };
    let bits = if amt >= 128 {
        0
    } else if right {
        l.bits >> amt
    } else {
        l.bits << amt
    };
    Val::new(bits, l.width, l.signed)
}

fn concat2(hi: Val, lo: Val) -> EvalResult<Val> {
    let w = hi.width + lo.width;
    if w > MAX_WIDTH {
        return Err(format!("concatenation of {w} bits exceeds {MAX_WIDTH}"));
    }
    Ok(Val::new((hi.bits << lo.width) | lo.bits, w, false))
}

fn part(b: Val, hi: i128, lo: i128) -> EvalResult<Val> {
    if lo < 0 || hi < lo {
        return Err("part select with invalid bounds".into());
    }
    let w = (hi - lo + 1) as u32;
    let bits = if lo >= 128 { 0 } else { b.bits >> lo };
    Ok(Val::new(bits, w, false))
}

/// `$clog2(x)`: the ceiling of log2 of `x` (0 for 0 and 1).
pub fn clog2(x: i128) -> u32 {
    if x <= 1 {
        return 0;
    }
    let v = (x - 1) as u128;
    128 - v.leading_zeros()
}

