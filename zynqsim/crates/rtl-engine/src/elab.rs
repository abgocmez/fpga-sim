//! Flattening elaboration: parameters, declarations, instances, generate blocks and
//! the processes of each module instance. Expressions and statements are lowered by
//! [`crate::lower`].

use crate::ir::{Edge, Func, Op, Proc, ProcKind, SigId, Signal, Trigger};
use rtl_frontend::ast::{
    Always, AlwaysKind, Conn, ContAssign, DataKind, Decl, Dir, Expr, ExprKind, GenKind, Generate,
    Instance, Item, Module, ModuleItem, Range, Routine, RoutineKind, TypedefKind,
};
use rtl_frontend::consteval::{Ctx, Val};
use rtl_frontend::parser::Parsed;
use rtl_frontend::{Diagnostic, Pos};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::rc::Rc;

/// Deepest instance nesting accepted.
pub(crate) const MAX_DEPTH: u32 = 64;
/// Most iterations of one generate loop.
pub(crate) const MAX_GEN_ITER: u64 = 10_000;

/// A declared name: its signal and the shape the lowering needs.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Sym {
    /// The signal.
    pub id: SigId,
    /// Element width.
    pub width: u32,
    /// Declared signed.
    pub signed: bool,
    /// Words of a memory (0 for a scalar or vector).
    pub depth: u32,
    /// Declared `msb` (or `width - 1`).
    pub msb: i64,
    /// Declared `lsb` (or 0).
    pub lsb: i64,
}

/// What a module's routines and top-level names look like, shared by its scopes.
pub(crate) struct ModScope {
    /// Module name.
    pub name: String,
    /// Picoseconds per time unit of the module.
    pub unit_ps: u64,
    /// Functions and tasks of the module.
    pub routines: BTreeMap<String, Routine>,
    /// Module-level constants.
    pub vals: HashMap<String, Val>,
    /// Module-level signals.
    pub syms: HashMap<String, Sym>,
    /// Instance serial (keys the function cache).
    pub serial: usize,
}

/// Names visible at one point of the elaboration.
#[derive(Clone)]
pub(crate) struct Scope {
    /// Constants: parameters, genvars, enum members.
    pub vals: HashMap<String, Val>,
    /// Signals.
    pub syms: HashMap<String, Sym>,
    /// The enclosing module.
    pub module: Rc<ModScope>,
    /// Hierarchical prefix for new names (`""` at the top, `u.` or `g[2].` below).
    pub prefix: String,
}

/// A port of an elaborated instance, as the parent connects it.
#[derive(Debug, Clone)]
pub(crate) struct PortSig {
    pub name: String,
    pub id: SigId,
    pub width: u32,
    pub signed: bool,
    pub dir: Option<Dir>,
}

/// The elaborator.
pub(crate) struct Elab<'p> {
    pub modules: BTreeMap<String, (&'p Module, u64, String)>,
    /// Typedefs: width, signedness and whether they are enums.
    pub typedefs: HashMap<String, (u32, bool, bool)>,
    pub signals: Vec<Signal>,
    pub store_len: usize,
    pub procs: Vec<Proc>,
    pub init_procs: Vec<Proc>,
    pub funcs: Vec<Func>,
    pub diags: Vec<Diagnostic>,
    pub serial: usize,
    pub fn_ids: HashMap<(usize, String), u32>,
    pub fn_busy: HashSet<(usize, String)>,
    pub task_stack: Vec<String>,
}

impl<'p> Elab<'p> {
    pub(crate) fn new(parsed: &'p Parsed, units: HashMap<String, (u64, String)>) -> Elab<'p> {
        let mut modules = BTreeMap::new();
        let mut typedefs = HashMap::new();
        let mut diags = Vec::new();
        for unit in &parsed.units {
            for item in &unit.items {
                match item {
                    Item::Module(m) => {
                        if modules.contains_key(&m.name) {
                            diags.push(Diagnostic::new(
                                &m.pos,
                                "duplicate module",
                                format!("module `{}` is declared more than once", m.name),
                            ));
                        } else {
                            let (ps, file) = units
                                .get(&m.name)
                                .cloned()
                                .unwrap_or((1000, unit.file.clone()));
                            modules.insert(m.name.clone(), (m, ps, file));
                        }
                    }
                    Item::Typedef(t) => {
                        let entry = match &t.kind {
                            TypedefKind::Vector { range, signed } => {
                                let w = match range {
                                    Some(r) => const_width(r).unwrap_or(1),
                                    None => 1,
                                };
                                (w, *signed, false)
                            }
                            TypedefKind::Enum(_) => (32, true, true),
                        };
                        typedefs.insert(t.name.clone(), entry);
                    }
                }
            }
        }
        Elab {
            modules,
            typedefs,
            signals: Vec::new(),
            store_len: 0,
            procs: Vec::new(),
            init_procs: Vec::new(),
            funcs: Vec::new(),
            diags,
            serial: 0,
            fn_ids: HashMap::new(),
            fn_busy: HashSet::new(),
            task_stack: Vec::new(),
        }
    }

    pub(crate) fn diag(&mut self, pos: &Pos, construct: &str, msg: impl Into<String>) {
        self.diags.push(Diagnostic::new(pos, construct, msg));
    }

    /// Elaborates the top module. Returns `false` when it is not defined.
    ///
    /// The scope label of an instance is its hierarchical prefix, or the module name at
    /// the top (what `%m` prints).
    pub(crate) fn top(&mut self, name: &str) -> bool {
        if !self.modules.contains_key(name) {
            return false;
        }
        let _ = self.instantiate(name, "", &[], 0);
        true
    }

    /// Elaborates one instance of module `mname` whose names start with `prefix`.
    /// `overrides` are the parameter overrides (named or positional, already evaluated).
    pub(crate) fn instantiate(
        &mut self,
        mname: &str,
        prefix: &str,
        overrides: &[(Option<String>, Val)],
        depth: u32,
    ) -> Option<Vec<PortSig>> {
        let (m, unit_ps, _) = {
            let (m, ps, f) = self.modules.get(mname)?;
            (*m, *ps, f.clone())
        };
        if depth > MAX_DEPTH {
            let pos = Pos::new(prefix, 0, 0);
            self.diag(
                &pos,
                "instance",
                format!("instantiation of `{mname}` is nested too deeply (recursive?)"),
            );
            return None;
        }
        self.serial += 1;
        let mut routines = BTreeMap::new();
        for it in &m.items {
            if let ModuleItem::Routine(r) = it {
                routines.insert(r.name.clone(), r.clone());
            }
        }
        let mut scope = Scope {
            vals: HashMap::new(),
            syms: HashMap::new(),
            module: Rc::new(ModScope {
                name: mname.to_string(),
                unit_ps,
                routines: BTreeMap::new(),
                vals: HashMap::new(),
                syms: HashMap::new(),
                serial: self.serial,
            }),
            prefix: prefix.to_string(),
        };
        // Header parameters, then body parameters, in order.
        self.bind_params(m, overrides, &mut scope, &routines);
        // Ports become signals first (their names may be redeclared in the body).
        let mut ports: Vec<PortSig> = Vec::new();
        for p in &m.ports {
            let (width, signed, msb, lsb) =
                self.shape(p.range.as_ref(), p.kind.as_ref(), p.signed, &scope);
            self.check_width(&p.name, width, &p.pos);
            let id = self.alloc(
                &format!("{prefix}{}", p.name),
                width,
                signed,
                0,
                0,
                msb,
                lsb,
                false,
            );
            scope.syms.insert(
                p.name.clone(),
                Sym {
                    id,
                    width,
                    signed,
                    depth: 0,
                    msb,
                    lsb,
                },
            );
            ports.push(PortSig {
                name: p.name.clone(),
                id,
                width,
                signed,
                dir: p.dir,
            });
        }
        // Body declarations (a redeclared port keeps its signal).
        let port_names: HashSet<String> = m.ports.iter().map(|p| p.name.clone()).collect();
        let mut body_items: Vec<&ModuleItem> = Vec::new();
        for it in &m.items {
            if let ModuleItem::Decl(d) = it
                && port_names.contains(&d.name)
            {
                if let Some(sym) = scope.syms.get(&d.name).copied() {
                    if d.range.is_some() {
                        let (w, s, msb, lsb) =
                            self.shape(d.range.as_ref(), Some(&d.kind), d.signed, &scope);
                        if let Some(p) = ports.iter_mut().find(|p| p.name == d.name) {
                            p.width = w;
                            p.signed = s;
                        }
                        self.signals[sym.id as usize].width = w;
                        self.signals[sym.id as usize].signed = s;
                        self.signals[sym.id as usize].msb_fix(msb, lsb);
                        scope.syms.insert(
                            d.name.clone(),
                            Sym {
                                width: w,
                                signed: s,
                                msb,
                                lsb,
                                ..sym
                            },
                        );
                    }
                }
                continue;
            }
            body_items.push(it);
        }
        let owned: Vec<ModuleItem> = body_items.into_iter().cloned().collect();
        self.declare_items(&owned, &mut scope, depth);
        // Module-level scope for function bodies: everything declared so far.
        scope.module = Rc::new(ModScope {
            name: mname.to_string(),
            unit_ps,
            routines,
            vals: scope.vals.clone(),
            syms: scope.syms.clone(),
            serial: self.serial,
        });
        self.lower_items(&owned, &mut scope, depth);
        Some(ports)
    }

    /// Binds parameters (header then body) in order. Overrides apply to header and
    /// body parameters that are not `localparam`, named ones by name, others by position.
    fn bind_params(
        &mut self,
        m: &Module,
        overrides: &[(Option<String>, Val)],
        scope: &mut Scope,
        routines: &BTreeMap<String, Routine>,
    ) {
        let mut positional = overrides
            .iter()
            .filter(|(n, _)| n.is_none())
            .map(|(_, v)| *v);
        let named: HashMap<&str, Val> = overrides
            .iter()
            .filter_map(|(n, v)| n.as_deref().map(|n| (n, *v)))
            .collect();
        let mut all: Vec<&rtl_frontend::ast::ParamDecl> = m.params.iter().collect();
        for it in &m.items {
            if let ModuleItem::Param(p) = it {
                all.push(p);
            }
        }
        let _ = routines;
        for p in all {
            let over = if p.local {
                None
            } else if let Some(v) = named.get(p.name.as_str()) {
                Some(*v)
            } else if m.params.iter().any(|h| h.name == p.name) {
                positional.next()
            } else {
                None
            };
            let value = match over {
                Some(v) => Some(v),
                None => match &p.value {
                    Some(e) => self.konst(e, scope),
                    None => None,
                },
            };
            match value {
                Some(mut v) => {
                    if let Some(r) = &p.range {
                        if let Some(w) = self.konst_width(r, scope) {
                            v = Val::new(v.bits, w, p.signed);
                        }
                    } else if p.signed {
                        v = Val::new(v.bits, v.width, true);
                    }
                    scope.vals.insert(p.name.clone(), v);
                }
                None => {
                    let pos = p.pos.clone();
                    self.diag(
                        &pos,
                        "parameter",
                        format!("parameter `{}` is not a constant", p.name),
                    );
                }
            }
        }
    }

    /// Evaluates a constant expression in a scope.
    pub(crate) fn konst(&mut self, e: &Expr, scope: &Scope) -> Option<Val> {
        let e = &desugar_strings(e);
        let routines = &scope.module.routines;
        let mut ctx = Ctx::new(scope.vals.clone(), routines);
        match ctx.eval(e) {
            Ok(v) => Some(v),
            Err(msg) => {
                self.diag(&e.pos, "constant", msg);
                None
            }
        }
    }

    /// Width of a range in a scope, unclamped (`None` when the bounds are not constant).
    pub(crate) fn konst_width(&mut self, r: &Range, scope: &Scope) -> Option<u32> {
        let m = self.konst(&r.msb, scope)?.as_i128();
        let l = self.konst(&r.lsb, scope)?.as_i128();
        Some(((m - l).abs() + 1).min(1 << 20) as u32)
    }

    /// Reports a declaration wider than the engine's 128-bit limit, naming it.
    pub(crate) fn check_width(&mut self, name: &str, width: u32, pos: &Pos) {
        if width > 128 {
            self.diag(
                pos,
                "width",
                format!("signal `{name}` is {width} bits wide; the engine's limit is 128"),
            );
        }
    }

    /// Width, signedness and declared `msb`/`lsb` of a declaration.
    pub(crate) fn shape(
        &mut self,
        range: Option<&Range>,
        kind: Option<&DataKind>,
        signed: bool,
        scope: &Scope,
    ) -> (u32, bool, i64, i64) {
        if let Some(r) = range {
            let m = self
                .konst(&r.msb, scope)
                .map(|v| v.as_i128() as i64)
                .unwrap_or(0);
            let l = self
                .konst(&r.lsb, scope)
                .map(|v| v.as_i128() as i64)
                .unwrap_or(0);
            let w = self.konst_width(r, scope).unwrap_or(1);
            let base_signed = signed
                || match kind {
                    Some(DataKind::Named(t)) => self.typedefs.get(t).is_some_and(|x| x.1),
                    _ => false,
                };
            return (w, base_signed, m, l);
        }
        match kind {
            Some(DataKind::Integer | DataKind::Int) => (32, true, 31, 0),
            Some(DataKind::Time) => (64, false, 63, 0),
            Some(DataKind::Named(t)) => {
                let (w, s, _) = self.typedefs.get(t).copied().unwrap_or((1, false, false));
                (w, s || signed, w as i64 - 1, 0)
            }
            _ => (1, signed, 0, 0),
        }
    }

    /// Allocates a signal of the flat design.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn alloc(
        &mut self,
        name: &str,
        width: u32,
        signed: bool,
        depth: u32,
        mem_lo: i64,
        msb: i64,
        lsb: i64,
        hidden: bool,
    ) -> SigId {
        let width = width.clamp(1, 128);
        let base = self.store_len;
        self.store_len += depth.max(1) as usize;
        self.signals.push(Signal {
            name: name.to_string(),
            width,
            signed,
            depth,
            base,
            mem_lo,
            flip: msb < lsb,
            off: lsb,
            hidden,
        });
        (self.signals.len() - 1) as SigId
    }

    /// Adds a process (initialisers go first, see [`crate::Design::procs`]).
    pub(crate) fn add_proc(&mut self, kind: ProcKind, scope: &str, code: Vec<Op>) {
        let p = Proc {
            kind,
            scope: scope.to_string(),
            code,
        };
        if kind == ProcKind::Init {
            self.init_procs.push(p);
        } else {
            self.procs.push(p);
        }
    }

    /// Pass one over an item list: parameters, typedef constants and declarations.
    pub(crate) fn declare_items(&mut self, items: &[ModuleItem], scope: &mut Scope, depth: u32) {
        let _ = depth;
        for it in items {
            match it {
                ModuleItem::Param(p) => {
                    if let Some(v) = p.value.as_ref().and_then(|e| self.konst(e, scope)) {
                        scope.vals.insert(p.name.clone(), v);
                    }
                }
                ModuleItem::Typedef(t) => {
                    if let TypedefKind::Enum(members) = &t.kind {
                        for (i, m) in members.iter().enumerate() {
                            scope.vals.insert(m.clone(), Val::new(i as u128, 32, true));
                        }
                    }
                }
                ModuleItem::Decl(d) => self.declare_decl(d, scope),
                _ => {}
            }
        }
    }

    fn declare_decl(&mut self, d: &Decl, scope: &mut Scope) {
        if d.kind == DataKind::Genvar {
            return;
        }
        let (width, signed, msb, lsb) =
            self.shape(d.range.as_ref(), Some(&d.kind), d.signed, scope);
        self.check_width(&d.name, width, &d.pos);
        let (depth, mem_lo) = if d.dims.is_empty() {
            (0, 0)
        } else {
            let r = &d.dims[0];
            if d.dims.len() > 1 {
                let pos = d.pos.clone();
                self.diag(
                    &pos,
                    "memory",
                    "multi-dimensional memories are not supported",
                );
            }
            let m = self
                .konst(&r.msb, scope)
                .map(|v| v.as_i128() as i64)
                .unwrap_or(0);
            let l = self
                .konst(&r.lsb, scope)
                .map(|v| v.as_i128() as i64)
                .unwrap_or(0);
            let n = (m - l).abs() + 1;
            if n > 1 << 20 {
                let pos = d.pos.clone();
                self.diag(
                    &pos,
                    "memory",
                    format!("memory `{}` has {n} words (over 2^20)", d.name),
                );
                (1, 0)
            } else {
                (n as u32, m.min(l))
            }
        };
        let full = format!("{}{}", scope.prefix, d.name);
        let id = self.alloc(&full, width, signed, depth, mem_lo, msb, lsb, false);
        scope.syms.insert(
            d.name.clone(),
            Sym {
                id,
                width,
                signed,
                depth,
                msb,
                lsb,
            },
        );
    }

    /// Pass two over an item list: assignments, processes, instances, generates.
    pub(crate) fn lower_items(&mut self, items: &[ModuleItem], scope: &mut Scope, depth: u32) {
        for it in items {
            match it {
                ModuleItem::Param(_) | ModuleItem::Typedef(_) | ModuleItem::Routine(_) => {}
                ModuleItem::Decl(d) => self.decl_init(d, scope),
                ModuleItem::Assign(a) => self.cont_assign(a, scope),
                ModuleItem::Always(a) => self.always(a, scope),
                ModuleItem::Initial(i) => self.initial(&i.body, scope),
                ModuleItem::Instance(inst) => self.instance(inst, scope, depth),
                ModuleItem::Generate(g) => self.generate(g, scope, depth),
                ModuleItem::Import(imp) => {
                    if imp.item.is_none() {
                        let pos = imp.pos.clone();
                        self.diag(
                            &pos,
                            "import",
                            format!(
                                "wildcard import of `{}` is not resolved by the engine",
                                imp.package
                            ),
                        );
                    }
                }
            }
        }
    }

    /// `reg x = v;` (initial value at time 0) and `wire x = e;` (a continuous assign).
    fn decl_init(&mut self, d: &Decl, scope: &Scope) {
        let Some(init) = &d.init else { return };
        let Some(sym) = scope.syms.get(&d.name).copied() else {
            return;
        };
        let scope_name = scope_label(scope);
        let mut b = crate::lower::Body::new(&scope_name, scope.module.unit_ps);
        let lhs = crate::ir::LVal {
            w: sym.width,
            parts: vec![crate::ir::LPart {
                sig: sym.id,
                word: None,
                word_off: 0,
                sel: None,
                w: sym.width,
            }],
        };
        match &d.kind {
            DataKind::Wire => {
                if let Some(rhs) = self.assign_expr(init, sym.width, scope) {
                    let trig = self.triggers_of_expr(init, scope);
                    b.code.push(Op::Wait(trig));
                    b.code.push(Op::Assign {
                        lhs,
                        rhs,
                        nb: false,
                    });
                    b.code.push(Op::Jump(0));
                    self.add_proc(ProcKind::Assign, &scope_name, b.code);
                }
            }
            _ => {
                if let Some(rhs) = self.assign_expr(init, sym.width, scope) {
                    b.code.push(Op::Assign {
                        lhs,
                        rhs,
                        nb: false,
                    });
                    b.code.push(Op::Halt);
                    self.add_proc(ProcKind::Init, &scope_name, b.code);
                }
            }
        }
    }

    fn cont_assign(&mut self, a: &ContAssign, scope: &Scope) {
        let scope_name = scope_label(scope);
        let mut b = crate::lower::Body::new(&scope_name, scope.module.unit_ps);
        let Some(lhs) = self.lval(&a.lhs, scope) else {
            return;
        };
        let Some(rhs) = self.assign_expr(&a.rhs, lhs.w, scope) else {
            return;
        };
        let mut trig = self.triggers_of_expr(&a.rhs, scope);
        trig.extend(self.index_triggers(&a.lhs, scope));
        b.code.push(Op::Wait(trig));
        if let Some(d) = &a.delay
            && let Some(amount) = self.self_expr(d, scope)
        {
            b.code.push(Op::Delay {
                amount,
                unit_ps: scope.module.unit_ps,
            });
        }
        b.code.push(Op::Assign {
            lhs,
            rhs,
            nb: false,
        });
        b.code.push(Op::Jump(0));
        self.add_proc(ProcKind::Assign, &scope_name, b.code);
    }

    fn always(&mut self, a: &Always, scope: &Scope) {
        let scope_name = scope_label(scope);
        let mut b = crate::lower::Body::new(&scope_name, scope.module.unit_ps);
        match a.kind {
            AlwaysKind::Comb | AlwaysKind::Latch => {
                let trig = self.triggers_of_stmt(&a.body, scope);
                b.code.push(Op::Wait(trig));
                self.stmt(&mut b, &a.body, scope);
                b.code.push(Op::Jump(0));
                self.add_proc(ProcKind::Comb, &scope_name, b.code);
            }
            AlwaysKind::Plain | AlwaysKind::Ff => {
                self.stmt(&mut b, &a.body, scope);
                b.code.push(Op::Jump(0));
                self.add_proc(ProcKind::Always, &scope_name, b.code);
            }
        }
    }

    fn initial(&mut self, body: &rtl_frontend::ast::Stmt, scope: &Scope) {
        let scope_name = scope_label(scope);
        let mut b = crate::lower::Body::new(&scope_name, scope.module.unit_ps);
        self.stmt(&mut b, body, scope);
        b.code.push(Op::Halt);
        self.add_proc(ProcKind::Initial, &scope_name, b.code);
    }

    fn generate(&mut self, g: &Generate, scope: &mut Scope, depth: u32) {
        let label = |prefix: &str, label: &Option<String>| match label {
            Some(l) => format!("{prefix}{l}."),
            None => prefix.to_string(),
        };
        match &g.kind {
            GenKind::Block(items) => {
                let mut sc = scope.clone();
                sc.prefix = label(&scope.prefix, &g.label);
                self.declare_items(items, &mut sc, depth);
                self.lower_items(items, &mut sc, depth);
            }
            GenKind::For {
                var,
                init,
                cond,
                step,
                body,
            } => {
                let Some(start) = self.konst(init, scope) else {
                    return;
                };
                let mut value = start;
                let mut n = 0u64;
                loop {
                    let mut sc = scope.clone();
                    sc.vals.insert(var.clone(), value);
                    let Some(c) = self.konst(cond, &sc) else {
                        return;
                    };
                    if !c.truthy() {
                        break;
                    }
                    n += 1;
                    if n > MAX_GEN_ITER {
                        self.diag(
                            &cond.pos,
                            "generate",
                            "generate loop exceeds 10000 iterations",
                        );
                        return;
                    }
                    sc.prefix = match &g.label {
                        Some(l) => format!("{}{}[{}].", scope.prefix, l, value.as_i128()),
                        None => scope.prefix.clone(),
                    };
                    self.declare_items(body, &mut sc, depth);
                    self.lower_items(body, &mut sc, depth);
                    let mut next = sc.clone();
                    next.vals.insert(var.clone(), value);
                    let Some(v) = self.konst(step, &next) else {
                        return;
                    };
                    value = v;
                }
            }
            GenKind::If {
                cond,
                then_items,
                else_items,
            } => {
                let Some(c) = self.konst(cond, scope) else {
                    return;
                };
                let items = if c.truthy() {
                    Some(then_items.clone())
                } else {
                    else_items.clone()
                };
                if let Some(items) = items {
                    let mut sc = scope.clone();
                    sc.prefix = label(&scope.prefix, &g.label);
                    self.declare_items(&items, &mut sc, depth);
                    self.lower_items(&items, &mut sc, depth);
                }
            }
            GenKind::Case { expr, items } => {
                let Some(sel) = self.konst(expr, scope) else {
                    return;
                };
                let mut chosen: Option<Vec<ModuleItem>> = None;
                let mut default: Option<Vec<ModuleItem>> = None;
                'outer: for (labels, body) in items {
                    if labels.is_empty() {
                        default = Some(body.clone());
                        continue;
                    }
                    for l in labels {
                        if let Some(v) = self.konst(l, scope)
                            && v.as_i128() == sel.as_i128()
                        {
                            chosen = Some(body.clone());
                            break 'outer;
                        }
                    }
                }
                if let Some(items) = chosen.or(default) {
                    let mut sc = scope.clone();
                    sc.prefix = label(&scope.prefix, &g.label);
                    self.declare_items(&items, &mut sc, depth);
                    self.lower_items(&items, &mut sc, depth);
                }
            }
        }
    }

    /// Instantiates a module or a gate primitive below `scope`, and wires its ports.
    fn instance(&mut self, inst: &Instance, scope: &mut Scope, depth: u32) {
        if inst.array.is_some() {
            self.diag(
                &inst.pos,
                "instance array",
                "instance arrays are not elaborated by the engine",
            );
            return;
        }
        if inst.primitive {
            self.gate(inst, scope);
            return;
        }
        if !self.modules.contains_key(&inst.module) {
            self.diag(
                &inst.pos,
                "undefined module",
                format!("module `{}` is not defined", inst.module),
            );
            return;
        }
        let mut overrides: Vec<(Option<String>, Val)> = Vec::new();
        for c in &inst.params {
            if let Some(e) = &c.expr
                && let Some(v) = self.konst(e, scope)
            {
                overrides.push((c.name.clone(), v));
            }
        }
        let child_prefix = format!("{}{}.", scope.prefix, inst.name);
        let Some(ports) = self.instantiate(&inst.module, &child_prefix, &overrides, depth + 1)
        else {
            return;
        };
        let parent_scope = scope_label(scope);
        for (i, p) in ports.iter().enumerate() {
            let conn: Option<&Conn> = inst
                .ports
                .iter()
                .find(|c| c.name.as_deref() == Some(p.name.as_str()))
                .or_else(|| {
                    if inst.ports.iter().any(|c| c.name.is_some()) {
                        None
                    } else {
                        inst.ports.get(i)
                    }
                });
            let expr: Option<Expr> = match conn {
                Some(c) => c.expr.clone(),
                None if inst.wildcard => Some(Expr {
                    pos: inst.pos.clone(),
                    kind: ExprKind::Ident(p.name.clone()),
                }),
                None => None,
            };
            let Some(e) = expr else { continue };
            self.wire_port(p, &e, scope, &parent_scope);
        }
    }

    /// Connects one port: an input is assigned from the parent expression, an output
    /// assigns the parent target from the child's port.
    fn wire_port(&mut self, p: &PortSig, e: &Expr, scope: &Scope, parent: &str) {
        match p.dir {
            Some(Dir::Output) => {
                let Some(lhs) = self.lval(e, scope) else {
                    return;
                };
                let rhs = crate::ir::Ex {
                    w: p.width,
                    s: p.signed,
                    k: crate::ir::K::Sig(p.id),
                };
                let rhs = self.resize_to(rhs, lhs.w, p.signed);
                let mut b = crate::lower::Body::new(parent, scope.module.unit_ps);
                b.code.push(Op::Wait(vec![Trigger {
                    sig: p.id,
                    bit: None,
                    edge: Edge::Any,
                }]));
                b.code.push(Op::Assign {
                    lhs,
                    rhs,
                    nb: false,
                });
                b.code.push(Op::Jump(0));
                self.add_proc(ProcKind::Assign, parent, b.code);
            }
            Some(Dir::Inout) => {
                self.diag(
                    &e.pos,
                    "inout",
                    "inout ports are not supported by the engine",
                );
            }
            _ => {
                let Some(rhs) = self.assign_expr(e, p.width, scope) else {
                    return;
                };
                let lhs = crate::ir::LVal {
                    w: p.width,
                    parts: vec![crate::ir::LPart {
                        sig: p.id,
                        word: None,
                        word_off: 0,
                        sel: None,
                        w: p.width,
                    }],
                };
                let mut b = crate::lower::Body::new(parent, scope.module.unit_ps);
                let trig = self.triggers_of_expr(e, scope);
                b.code.push(Op::Wait(trig));
                b.code.push(Op::Assign {
                    lhs,
                    rhs,
                    nb: false,
                });
                b.code.push(Op::Jump(0));
                self.add_proc(ProcKind::Assign, parent, b.code);
            }
        }
    }

    /// `and`, `or`, `nand`, `nor`, `xor`, `xnor`, `not`, `buf`: first port is the output.
    fn gate(&mut self, inst: &Instance, scope: &Scope) {
        let ports: Vec<&Expr> = inst.ports.iter().filter_map(|c| c.expr.as_ref()).collect();
        if ports.len() < 2 {
            self.diag(
                &inst.pos,
                "gate",
                "a gate needs an output and at least one input",
            );
            return;
        }
        let out = ports[0];
        let Some(lhs) = self.lval(out, scope) else {
            return;
        };
        let w = lhs.w;
        let kind = inst.module.as_str();
        let inputs: Vec<crate::ir::Ex> = ports[1..]
            .iter()
            .filter_map(|e| self.assign_expr(e, w, scope))
            .collect();
        if inputs.len() != ports.len() - 1 {
            return;
        }
        use rtl_frontend::ast::BinOp;
        let rhs = match kind {
            "buf" => inputs[0].clone(),
            "not" => crate::ir::Ex {
                w,
                s: false,
                k: crate::ir::K::Un(rtl_frontend::ast::UnOp::BitNot, Box::new(inputs[0].clone())),
            },
            "and" | "nand" | "or" | "nor" | "xor" | "xnor" => {
                let op = match kind {
                    "and" | "nand" => BinOp::And,
                    "or" | "nor" => BinOp::Or,
                    _ => BinOp::Xor,
                };
                let mut acc = inputs[0].clone();
                for x in &inputs[1..] {
                    acc = crate::ir::Ex {
                        w,
                        s: false,
                        k: crate::ir::K::Bin {
                            op,
                            a: Box::new(acc),
                            b: Box::new(x.clone()),
                            os: false,
                        },
                    };
                }
                if kind == "nand" || kind == "nor" || kind == "xnor" {
                    crate::ir::Ex {
                        w,
                        s: false,
                        k: crate::ir::K::Un(rtl_frontend::ast::UnOp::BitNot, Box::new(acc)),
                    }
                } else {
                    acc
                }
            }
            _ => {
                self.diag(
                    &inst.pos,
                    "primitive",
                    format!("gate `{kind}` is not supported"),
                );
                return;
            }
        };
        let scope_name = scope_label(scope);
        let mut b = crate::lower::Body::new(&scope_name, scope.module.unit_ps);
        let mut trig = Vec::new();
        for e in &ports[1..] {
            trig.extend(self.triggers_of_expr(e, scope));
        }
        b.code.push(Op::Wait(trig));
        b.code.push(Op::Assign {
            lhs,
            rhs,
            nb: false,
        });
        b.code.push(Op::Jump(0));
        self.add_proc(ProcKind::Assign, &scope_name, b.code);
    }

    /// Funcs, for lowering: allocates the function id, lowering its body on first use.
    pub(crate) fn func_index(&mut self, name: &str, scope: &Scope) -> Option<u32> {
        let key = (scope.module.serial, name.to_string());
        // A function still being lowered is being called from its own body: recursion.
        if self.fn_busy.contains(&key) {
            return None;
        }
        if let Some(id) = self.fn_ids.get(&key) {
            return Some(*id);
        }
        let r = scope.module.routines.get(name)?.clone();
        if r.kind != RoutineKind::Function {
            return None;
        }
        self.fn_busy.insert(key.clone());
        let fid = self.funcs.len() as u32;
        self.funcs.push(Func {
            name: name.to_string(),
            formals: Vec::new(),
            ret: 0,
            code: Vec::new(),
        });
        self.fn_ids.insert(key.clone(), fid);
        let f = self.lower_function(&r, scope);
        self.funcs[fid as usize] = f;
        self.fn_busy.remove(&key);
        Some(fid)
    }

    /// Triggers of an event expression (`posedge x`, `x`, `x[3]`): see [`crate::lower`].
    pub(crate) fn event_triggers(&mut self, e: &Expr, edge: Edge, scope: &Scope) -> Vec<Trigger> {
        if edge != Edge::Any {
            if let Some((sig, bit)) = self.bit_of(e, scope) {
                return vec![Trigger {
                    sig,
                    bit: Some(bit),
                    edge,
                }];
            }
            if let ExprKind::Ident(n) = &e.kind
                && let Some(sym) = scope.syms.get(n)
            {
                return vec![Trigger {
                    sig: sym.id,
                    bit: Some(0),
                    edge,
                }];
            }
            self.diag(
                &e.pos,
                "event",
                "edge events are supported on a signal or a constant bit only",
            );
            return Vec::new();
        }
        self.triggers_of_expr(e, scope)
    }

    /// Helper: the signal and constant bit of `x` or `x[k]`.
    fn bit_of(&mut self, e: &Expr, scope: &Scope) -> Option<(SigId, u32)> {
        if let ExprKind::Index(base, idx) = &e.kind
            && let ExprKind::Ident(n) = &base.kind
            && let Some(sym) = scope.syms.get(n).copied()
            && sym.depth == 0
        {
            let v = self.konst(idx, scope)?;
            let i = v.as_i128() as i64;
            let pos = if sym.msb < sym.lsb {
                sym.lsb - i
            } else {
                i - sym.lsb
            };
            if pos >= 0 && (pos as u32) < sym.width {
                return Some((sym.id, pos as u32));
            }
        }
        None
    }
}

impl Signal {
    /// Records the declared bounds (kept for the bit mapping of later passes).
    fn msb_fix(&mut self, msb: i64, lsb: i64) {
        self.flip = msb < lsb;
        self.off = lsb;
    }
}

/// The hierarchical name of a scope: its prefix, or the module name at the top.
pub(crate) fn scope_label(scope: &Scope) -> String {
    let p = scope.prefix.trim_end_matches('.');
    if p.is_empty() {
        scope.module.name.clone()
    } else {
        p.to_string()
    }
}

/// A copy of a constant expression in which every string literal is a hexadecimal
/// literal of its bytes, so that the constant evaluator (which takes numbers only)
/// can compare strings, as in `if (MODE == "FAST")`.
pub(crate) fn desugar_strings(e: &Expr) -> Expr {
    let kind = match &e.kind {
        ExprKind::Str(text) => {
            let bytes = text.as_bytes();
            let w = (bytes.len() as u32 * 8).clamp(8, 128);
            let digits: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
            ExprKind::Number(rtl_frontend::lexer::Number {
                size: Some(w),
                signed: false,
                base: 'h',
                digits,
                apostrophe: true,
            })
        }
        ExprKind::Unary(op, a) => ExprKind::Unary(*op, Box::new(desugar_strings(a))),
        ExprKind::Binary(op, a, b) => ExprKind::Binary(
            *op,
            Box::new(desugar_strings(a)),
            Box::new(desugar_strings(b)),
        ),
        ExprKind::Ternary(a, b, c) => ExprKind::Ternary(
            Box::new(desugar_strings(a)),
            Box::new(desugar_strings(b)),
            Box::new(desugar_strings(c)),
        ),
        ExprKind::Concat(xs) => ExprKind::Concat(xs.iter().map(desugar_strings).collect()),
        ExprKind::Repl(n, xs) => ExprKind::Repl(
            Box::new(desugar_strings(n)),
            xs.iter().map(desugar_strings).collect(),
        ),
        ExprKind::Call(n, xs) => {
            ExprKind::Call(n.clone(), xs.iter().map(desugar_strings).collect())
        }
        other => other.clone(),
    };
    Expr {
        pos: e.pos.clone(),
        kind,
    }
}

/// Width of a constant range without any scope (typedef widths).
fn const_width(r: &Range) -> Option<u32> {
    let scope: BTreeMap<String, Routine> = BTreeMap::new();
    let mut ctx = Ctx::new(HashMap::new(), &scope);
    let w = ctx.range_width(r).ok()?;
    Some(w)
}
