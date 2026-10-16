//! Elaboration (no simulation): parameter resolution with overrides, generate
//! unrolling, the module instance tree, declared widths, and checks for undefined
//! modules, ports, functions and identifiers.
//!
//! The result is a [`Design`]. Hierarchical names are dotted paths built from the
//! top module; generate iterations appear as `label[i]`.

use crate::ast::{
    CaseItem, DataKind, Decl, Expr, ExprKind, GenKind, Generate, Instance, Item, Module,
    ModuleItem, Range, Routine, Stmt, StmtKind, TypedefKind,
};
use crate::consteval::{Ctx, Val};
use crate::diag::{Diagnostic, Pos};
use crate::parser::{Options, Parsed, parse_files};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;

/// Deepest module instantiation chain (guards against recursive instantiation).
const MAX_DEPTH: u32 = 64;
/// Largest number of iterations of one generate loop.
const MAX_GEN_ITER: u64 = 10_000;

/// Result of elaborating a design.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct Design {
    /// Name of the top module that was asked for.
    pub top: Option<String>,
    /// Every module of the sources, with its ports and default parameters.
    pub modules: Vec<ModuleInfo>,
    /// The instance tree below the top module (absent if elaboration could not start).
    pub hierarchy: Option<InstanceNode>,
    /// Parse and elaboration diagnostics.
    pub diagnostics: Vec<Diagnostic>,
}

/// Summary of a module declaration.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct ModuleInfo {
    /// Module name.
    pub name: String,
    /// File that declares it.
    pub file: String,
    /// Parameters with their default values (`None` when not constant).
    pub params: Vec<ParamInfo>,
    /// Ports in header order.
    pub ports: Vec<PortInfo>,
}

/// A parameter and its value.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct ParamInfo {
    /// Parameter name.
    pub name: String,
    /// Value in decimal, `None` when it is not a constant.
    pub value: Option<String>,
}

/// A port of an elaborated module.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct PortInfo {
    /// Port name.
    pub name: String,
    /// `input`, `output` or `inout`; `None` if the direction was never declared.
    pub dir: Option<String>,
    /// Width in bits, `None` when the range is not constant.
    pub width: Option<u32>,
    /// Declared signed.
    pub signed: bool,
}

/// A declared net or variable of an elaborated module.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct NetInfo {
    /// Declared name.
    pub name: String,
    /// `wire`, `reg`, `logic`, `integer`, ... or the name of a typedef.
    pub kind: String,
    /// Packed width in bits, `None` when not constant.
    pub width: Option<u32>,
    /// Declared signed.
    pub signed: bool,
    /// Number of words of an unpacked array (`None` for a scalar or vector).
    pub depth: Option<u64>,
}

/// One instance in the hierarchy.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct InstanceNode {
    /// Hierarchical path (`top`, `top.u_core`, `top.gen[2].u_alu`).
    pub path: String,
    /// Module name.
    pub module: String,
    /// Resolved parameters of this instance.
    pub params: Vec<ParamInfo>,
    /// Ports with their widths in this instance.
    pub ports: Vec<PortInfo>,
    /// Declared nets and variables.
    pub nets: Vec<NetInfo>,
    /// Child instances, in elaboration order.
    pub children: Vec<InstanceNode>,
}

/// Elaborates `top` from the given source files.
pub fn elaborate(
    top: &str,
    sources: &[PathBuf],
    defines: &[String],
    include_dirs: &[PathBuf],
) -> Design {
    let opts = Options {
        defines: defines.to_vec(),
        include_dirs: include_dirs.to_vec(),
        ..Options::default()
    };
    let parsed = parse_files(sources, &opts);
    elaborate_parsed(top, &parsed)
}

/// Summary of every module in parsed sources, without elaborating a hierarchy.
pub fn summarize_parsed(parsed: &Parsed) -> Design {
    let mut el = Elab::new(parsed);
    let names: Vec<String> = el.modules.keys().cloned().collect();
    let modules = names.iter().map(|n| el.module_info(n)).collect();
    Design {
        top: None,
        modules,
        hierarchy: None,
        diagnostics: el.diags,
    }
}

/// Elaborates `top` from already parsed units.
pub fn elaborate_parsed(top: &str, parsed: &Parsed) -> Design {
    let mut el = Elab::new(parsed);
    let names: Vec<String> = el.modules.keys().cloned().collect();
    let mut modules = Vec::new();
    for n in &names {
        modules.push(el.module_info(n));
    }
    let hierarchy = if el.modules.contains_key(top) {
        el.elab_instance(top, top, &[], 0)
    } else {
        let p = Pos::new("<elaboration>", 0, 0);
        el.diags.push(Diagnostic::new(
            &p,
            "elaboration",
            format!("top module `{top}` is not defined"),
        ));
        None
    };
    Design {
        top: Some(top.to_string()),
        modules,
        hierarchy,
        diagnostics: el.diags,
    }
}

/// Names visible in a scope, with their values where known.
struct Scope {
    vals: HashMap<String, Val>,
    frames: Vec<HashSet<String>>,
    funcs: BTreeMap<String, Routine>,
    nets: Vec<NetInfo>,
    /// A wildcard package import makes the set of visible names unknown.
    wildcard: bool,
}

impl Scope {
    fn new(funcs: BTreeMap<String, Routine>) -> Scope {
        Scope {
            vals: HashMap::new(),
            frames: vec![HashSet::new()],
            funcs,
            nets: Vec::new(),
            wildcard: false,
        }
    }

    fn declare(&mut self, name: &str) {
        if let Some(f) = self.frames.last_mut() {
            f.insert(name.to_string());
        }
    }

    fn declared(&self, n: &str) -> bool {
        self.wildcard
            || n.contains('.')
            || n.contains("::")
            || self.vals.contains_key(n)
            || self.frames.iter().any(|f| f.contains(n))
    }

    fn ctx(&self) -> Ctx<'_> {
        Ctx::new(self.vals.clone(), &self.funcs)
    }

    fn eval(&self, e: &Expr) -> Result<Val, String> {
        self.ctx().eval(e)
    }

    fn width(&self, r: &Range) -> Result<u32, String> {
        self.ctx().range_width(r)
    }
}

/// What a typedef denotes, as far as elaboration needs it.
struct TypedefInfo {
    width: Option<u32>,
    members: Vec<String>,
}

struct Elab<'p> {
    modules: BTreeMap<String, (&'p Module, String)>,
    typedefs: BTreeMap<String, TypedefInfo>,
    /// Enum members of every typedef in the sources: constants visible everywhere.
    globals: HashSet<String>,
    diags: Vec<Diagnostic>,
}

impl<'p> Elab<'p> {
    fn new(parsed: &'p Parsed) -> Elab<'p> {
        let mut modules: BTreeMap<String, (&Module, String)> = BTreeMap::new();
        let mut diags: Vec<Diagnostic> = parsed.diagnostics.clone();
        let mut typedefs = BTreeMap::new();
        let mut globals = HashSet::new();
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
                            modules.insert(m.name.clone(), (m, unit.file.clone()));
                        }
                    }
                    Item::Typedef(t) => {
                        let info = typedef_info(&t.kind);
                        globals.extend(info.members.iter().cloned());
                        typedefs.insert(t.name.clone(), info);
                    }
                }
            }
        }
        Elab {
            modules,
            typedefs,
            globals,
            diags,
        }
    }

    fn diag(&mut self, pos: &Pos, construct: &str, msg: impl Into<String>) {
        self.diags.push(Diagnostic::new(pos, construct, msg));
    }

    /// Summary of a module with its default parameters (diagnostics are dropped).
    fn module_info(&mut self, name: &str) -> ModuleInfo {
        let (m, file) = {
            let (m, f) = self.modules[name].clone();
            (m, f)
        };
        let saved = self.diags.len();
        let mut scope = Scope::new(routines_of(m));
        self.bind_params(m, &[], &mut scope);
        let params = m
            .params
            .iter()
            .map(|p| ParamInfo {
                name: p.name.clone(),
                value: scope.vals.get(&p.name).map(|v| v.as_i128().to_string()),
            })
            .collect();
        let ports = self.ports_of(m, &scope);
        self.diags.truncate(saved);
        ModuleInfo {
            name: name.to_string(),
            file,
            params,
            ports,
        }
    }

    /// Binds parameters in declaration order. `overrides` are `(name, value, pos)`;
    /// unnamed ones apply to the header parameters by position.
    fn bind_params(
        &mut self,
        m: &Module,
        overrides: &[(Option<String>, Val, Pos)],
        scope: &mut Scope,
    ) {
        let mut positional = overrides.iter().filter(|(n, _, _)| n.is_none());
        let named: HashMap<&str, Val> = overrides
            .iter()
            .filter_map(|(n, v, _)| n.as_deref().map(|n| (n, *v)))
            .collect();
        let mut header: Vec<&crate::ast::ParamDecl> = m.params.iter().collect();
        for it in &m.items {
            if let ModuleItem::Param(p) = it {
                header.push(p);
            }
        }
        for p in header {
            let value = if !p.local && named.contains_key(p.name.as_str()) {
                Some(named[p.name.as_str()])
            } else if !p.local && m.params.iter().any(|h| h.name == p.name) {
                let idx = m.params.iter().position(|h| h.name == p.name).unwrap_or(0);
                let _ = idx;
                positional.next().map(|(_, v, _)| *v)
            } else {
                None
            };
            let value = match (value, &p.value) {
                (Some(v), _) => Ok(v),
                (None, Some(e)) => scope.eval(e),
                (None, None) => Err("parameter has no value".to_string()),
            };
            match value {
                Ok(mut v) => {
                    if let Some(r) = &p.range {
                        if let Ok(w) = scope.width(r) {
                            v = Val::new(v.bits, w, p.signed);
                        }
                    }
                    scope.vals.insert(p.name.clone(), v);
                    scope.declare(&p.name);
                }
                Err(msg) => {
                    let pos = p.pos.clone();
                    self.diag(
                        &pos,
                        "parameter",
                        format!("parameter `{}` is not constant: {msg}", p.name),
                    );
                    scope.declare(&p.name);
                }
            }
        }
    }

    fn ports_of(&mut self, m: &Module, scope: &Scope) -> Vec<PortInfo> {
        m.ports
            .iter()
            .map(|p| PortInfo {
                name: p.name.clone(),
                dir: p.dir.map(|d| d.keyword().to_string()),
                width: width_of(scope, p.kind.as_ref(), p.range.as_ref(), &self.typedefs),
                signed: p.signed,
            })
            .collect()
    }

    /// Elaborates one instance of `mname` at hierarchical `path`.
    fn elab_instance(
        &mut self,
        mname: &str,
        path: &str,
        overrides: &[(Option<String>, Val, Pos)],
        depth: u32,
    ) -> Option<InstanceNode> {
        let (m, _) = self.modules.get(mname)?.clone();
        if depth > MAX_DEPTH {
            let p = Pos::new(path, 0, 0);
            self.diag(
                &p,
                "instance",
                format!("instantiation of `{mname}` is nested too deeply (recursive?)"),
            );
            return None;
        }
        let mut scope = Scope::new(routines_of(m));
        self.bind_params(m, overrides, &mut scope);
        for p in &m.ports {
            scope.declare(&p.name);
        }
        let params = m
            .params
            .iter()
            .map(|p| ParamInfo {
                name: p.name.clone(),
                value: scope.vals.get(&p.name).map(|v| v.as_i128().to_string()),
            })
            .collect();
        let ports = self.ports_of(m, &scope);
        // Names declared at module level are visible everywhere in the module.
        predeclare(&m.items, &mut scope);
        let mut children = Vec::new();
        let prefix = format!("{path}.");
        self.walk(&m.items, &mut scope, &prefix, &mut children, depth, true);
        Some(InstanceNode {
            path: path.to_string(),
            module: mname.to_string(),
            params,
            ports,
            nets: scope.nets,
            children,
        })
    }

    fn walk(
        &mut self,
        items: &[ModuleItem],
        scope: &mut Scope,
        prefix: &str,
        children: &mut Vec<InstanceNode>,
        depth: u32,
        top: bool,
    ) {
        for item in items {
            match item {
                ModuleItem::Param(p) => {
                    if !top {
                        let pos = p.pos.clone();
                        match p.value.as_ref().map(|e| scope.eval(e)) {
                            Some(Ok(v)) => {
                                scope.vals.insert(p.name.clone(), v);
                            }
                            Some(Err(msg)) => self.diag(&pos, "parameter", msg),
                            None => {}
                        }
                        scope.declare(&p.name);
                    }
                }
                ModuleItem::Decl(d) => self.decl(d, scope),
                ModuleItem::Typedef(t) => {
                    if let TypedefKind::Enum(members) = &t.kind {
                        for mbr in members {
                            scope.declare(mbr);
                        }
                    }
                    scope.declare(&t.name);
                }
                ModuleItem::Assign(a) => {
                    self.check_expr(&a.lhs, scope);
                    self.check_expr(&a.rhs, scope);
                }
                ModuleItem::Always(a) => self.check_stmt(&a.body, scope),
                ModuleItem::Initial(i) => self.check_stmt(&i.body, scope),
                ModuleItem::Instance(inst) => {
                    self.instance_item(inst, scope, prefix, children, depth)
                }
                ModuleItem::Routine(r) => self.check_routine(r, scope),
                ModuleItem::Generate(g) => self.generate(g, scope, prefix, children, depth),
                ModuleItem::Import(imp) => match &imp.item {
                    Some(n) => scope.declare(n),
                    None => scope.wildcard = true,
                },
            }
        }
    }

    fn decl(&mut self, d: &Decl, scope: &mut Scope) {
        scope.declare(&d.name);
        if let Some(r) = &d.range {
            self.check_expr(&r.msb, scope);
            self.check_expr(&r.lsb, scope);
        }
        if let Some(init) = &d.init {
            self.check_expr(init, scope);
        }
        let width = width_of(scope, Some(&d.kind), d.range.as_ref(), &self.typedefs);
        let depth = if d.dims.is_empty() {
            None
        } else {
            let mut n: u64 = 1;
            for r in &d.dims {
                match scope.ctx().range_width(r) {
                    Ok(w) => n = n.saturating_mul(w as u64),
                    Err(_) => n = 0,
                }
            }
            Some(n)
        };
        scope.nets.push(NetInfo {
            name: d.name.clone(),
            kind: kind_name(&d.kind),
            width,
            signed: d.signed,
            depth,
        });
    }

    fn generate(
        &mut self,
        g: &Generate,
        scope: &mut Scope,
        prefix: &str,
        children: &mut Vec<InstanceNode>,
        depth: u32,
    ) {
        let label_prefix = |label: &Option<String>| match label {
            Some(l) => format!("{prefix}{l}."),
            None => prefix.to_string(),
        };
        match &g.kind {
            GenKind::Block(items) => {
                scope.frames.push(HashSet::new());
                let p = label_prefix(&g.label);
                predeclare(items, scope);
                self.walk(items, scope, &p, children, depth, false);
                scope.frames.pop();
            }
            GenKind::For {
                var,
                init,
                cond,
                step,
                body,
            } => {
                let saved = scope.vals.get(var).copied();
                let mut cur = match scope.eval(init) {
                    Ok(v) => v,
                    Err(msg) => {
                        self.diag(
                            &g.pos,
                            "generate for",
                            format!("loop start is not constant: {msg}"),
                        );
                        return;
                    }
                };
                let mut count: u64 = 0;
                loop {
                    scope.vals.insert(var.clone(), cur);
                    match scope.eval(cond) {
                        Ok(c) if !c.truthy() => break,
                        Ok(_) => {}
                        Err(msg) => {
                            self.diag(
                                &g.pos,
                                "generate for",
                                format!("loop condition is not constant: {msg}"),
                            );
                            break;
                        }
                    }
                    count += 1;
                    if count > MAX_GEN_ITER {
                        self.diag(
                            &g.pos,
                            "generate for",
                            format!("loop exceeds {MAX_GEN_ITER} iterations"),
                        );
                        break;
                    }
                    let base = g.label.clone().unwrap_or_else(|| "genblk".to_string());
                    let p = format!("{prefix}{base}[{}].", cur.as_i128());
                    scope.frames.push(HashSet::new());
                    predeclare(body, scope);
                    self.walk(body, scope, &p, children, depth, false);
                    scope.frames.pop();
                    cur = match scope.eval(step) {
                        Ok(v) => v,
                        Err(msg) => {
                            self.diag(
                                &g.pos,
                                "generate for",
                                format!("loop step is not constant: {msg}"),
                            );
                            break;
                        }
                    };
                }
                match saved {
                    Some(v) => {
                        scope.vals.insert(var.clone(), v);
                    }
                    None => {
                        scope.vals.remove(var);
                    }
                }
            }
            GenKind::If {
                cond,
                then_items,
                else_items,
            } => {
                let taken = match scope.eval(cond) {
                    Ok(c) => {
                        if c.truthy() {
                            Some(then_items)
                        } else {
                            else_items.as_ref()
                        }
                    }
                    Err(msg) => {
                        self.diag(
                            &g.pos,
                            "generate if",
                            format!("condition is not constant: {msg}"),
                        );
                        None
                    }
                };
                if let Some(items) = taken {
                    scope.frames.push(HashSet::new());
                    let p = label_prefix(&g.label);
                    predeclare(items, scope);
                    self.walk(items, scope, &p, children, depth, false);
                    scope.frames.pop();
                }
            }
            GenKind::Case { expr, items } => {
                let sel = match scope.eval(expr) {
                    Ok(v) => v,
                    Err(msg) => {
                        self.diag(
                            &g.pos,
                            "generate case",
                            format!("selector is not constant: {msg}"),
                        );
                        return;
                    }
                };
                let chosen = self.pick_case(sel, items, scope);
                if let Some(body) = chosen {
                    scope.frames.push(HashSet::new());
                    let p = label_prefix(&g.label);
                    predeclare(body, scope);
                    self.walk(body, scope, &p, children, depth, false);
                    scope.frames.pop();
                }
            }
        }
    }

    fn pick_case<'i>(
        &mut self,
        sel: Val,
        items: &'i [(Vec<Expr>, Vec<ModuleItem>)],
        scope: &Scope,
    ) -> Option<&'i Vec<ModuleItem>> {
        let mut default = None;
        for (labels, body) in items {
            if labels.is_empty() {
                default = Some(body);
                continue;
            }
            for l in labels {
                match scope.eval(l) {
                    Ok(v) => {
                        if v.bits == sel.bits {
                            return Some(body);
                        }
                    }
                    Err(msg) => {
                        let p = l.pos.clone();
                        self.diag(&p, "generate case", format!("label is not constant: {msg}"));
                    }
                }
            }
        }
        default
    }

    fn instance_item(
        &mut self,
        inst: &Instance,
        scope: &mut Scope,
        prefix: &str,
        children: &mut Vec<InstanceNode>,
        depth: u32,
    ) {
        // Port expressions may name implicit nets; only the other expressions are checked.
        if inst.primitive {
            for c in &inst.ports {
                if let Some(e) = &c.expr {
                    self.check_expr(e, scope);
                }
            }
            return;
        }
        if inst.array.is_some() {
            self.diag(
                &inst.pos,
                "instance array",
                format!("instance array `{}` is not elaborated", inst.name),
            );
            return;
        }
        let Some((child, _)) = self.modules.get(&inst.module).cloned() else {
            self.diag(
                &inst.pos,
                "instance",
                format!(
                    "module `{}` is not defined (instance `{}`)",
                    inst.module, inst.name
                ),
            );
            return;
        };
        let mut overrides = Vec::new();
        for (i, c) in inst.params.iter().enumerate() {
            let Some(e) = &c.expr else { continue };
            match scope.eval(e) {
                Ok(v) => {
                    let name = c.name.clone();
                    let _ = i;
                    overrides.push((name, v, c.pos.clone()));
                }
                Err(msg) => {
                    let p = c.pos.clone();
                    self.diag(
                        &p,
                        "parameter override",
                        format!("value is not constant: {msg}"),
                    );
                }
            }
        }
        for c in &inst.ports {
            if let Some(n) = &c.name {
                if !child.ports.iter().any(|p| &p.name == n) {
                    let p = c.pos.clone();
                    self.diag(
                        &p,
                        "port connection",
                        format!("module `{}` has no port `{n}`", inst.module),
                    );
                }
            }
        }
        if inst.ports.iter().filter(|c| c.name.is_none()).count() > child.ports.len() {
            self.diag(
                &inst.pos,
                "port connection",
                format!("too many positional connections for `{}`", inst.module),
            );
        }
        let path = format!("{prefix}{}", inst.name);
        // Expressions of the connections are checked in the parent scope; undeclared
        // names there are implicit nets, so only the expression shape is checked.
        for c in &inst.ports {
            if let Some(e) = &c.expr {
                self.check_port_expr(e, scope);
            }
        }
        if let Some(ci) = self.elab_instance(&inst.module, &path, &overrides, depth + 1) {
            children.push(ci);
        }
    }

    fn check_routine(&mut self, r: &Routine, scope: &mut Scope) {
        scope.frames.push(HashSet::new());
        scope.declare(&r.name);
        for a in &r.args {
            scope.declare(&a.name);
        }
        for d in &r.decls {
            scope.declare(&d.name);
        }
        for s in &r.body {
            self.check_stmt(s, scope);
        }
        scope.frames.pop();
    }

    fn check_stmt(&mut self, s: &Stmt, scope: &mut Scope) {
        match &s.kind {
            StmtKind::Null | StmtKind::Break | StmtKind::Continue | StmtKind::Disable(_) => {}
            StmtKind::Block { decls, stmts, .. } => {
                scope.frames.push(HashSet::new());
                for d in decls {
                    scope.declare(&d.name);
                }
                for st in stmts {
                    self.check_stmt(st, scope);
                }
                scope.frames.pop();
            }
            StmtKind::Assign {
                lhs, rhs, delay, ..
            } => {
                self.check_expr(lhs, scope);
                self.check_expr(rhs, scope);
                if let Some(d) = delay {
                    self.check_expr(d, scope);
                }
            }
            StmtKind::If {
                cond,
                then_stmt,
                else_stmt,
            } => {
                self.check_expr(cond, scope);
                self.check_stmt(then_stmt, scope);
                if let Some(e) = else_stmt {
                    self.check_stmt(e, scope);
                }
            }
            StmtKind::Case { expr, items, .. } => {
                self.check_expr(expr, scope);
                for CaseItem { labels, body } in items {
                    for l in labels {
                        self.check_expr(l, scope);
                    }
                    self.check_stmt(body, scope);
                }
            }
            StmtKind::For {
                init,
                cond,
                step,
                body,
            } => {
                self.check_stmt(init, scope);
                self.check_expr(cond, scope);
                self.check_stmt(step, scope);
                self.check_stmt(body, scope);
            }
            StmtKind::While { cond, body } => {
                self.check_expr(cond, scope);
                self.check_stmt(body, scope);
            }
            StmtKind::Repeat { count, body } => {
                self.check_expr(count, scope);
                self.check_stmt(body, scope);
            }
            StmtKind::Forever { body } => self.check_stmt(body, scope),
            StmtKind::Delay { amount, stmt } => {
                self.check_expr(amount, scope);
                self.check_stmt(stmt, scope);
            }
            StmtKind::EventCtrl { events, stmt } => {
                if let Some(evs) = events {
                    for e in evs {
                        self.check_expr(&e.expr, scope);
                    }
                }
                self.check_stmt(stmt, scope);
            }
            StmtKind::Wait { cond, stmt } => {
                self.check_expr(cond, scope);
                self.check_stmt(stmt, scope);
            }
            StmtKind::SysCall { args, .. } => {
                for a in args {
                    self.check_expr(a, scope);
                }
            }
            StmtKind::Call { name, args } => {
                if !scope.funcs.contains_key(name) {
                    let p = s.pos.clone();
                    self.diag(
                        &p,
                        "task call",
                        format!("task `{name}` is not declared in this module"),
                    );
                }
                for a in args {
                    self.check_expr(a, scope);
                }
            }
            StmtKind::Return(v) => {
                if let Some(e) = v {
                    self.check_expr(e, scope);
                }
            }
        }
    }

    /// Reports identifiers that are not declared in `scope`.
    fn check_expr(&mut self, e: &Expr, scope: &Scope) {
        match &e.kind {
            ExprKind::Ident(n) => {
                if !scope.declared(n) && !self.globals.contains(n) {
                    let p = e.pos.clone();
                    self.diag(&p, "identifier", format!("`{n}` is not declared"));
                }
            }
            ExprKind::Call(name, args) => {
                if !scope.funcs.contains_key(name) {
                    let p = e.pos.clone();
                    self.diag(
                        &p,
                        "function call",
                        format!("function `{name}` is not declared in this module"),
                    );
                }
                for a in args {
                    self.check_expr(a, scope);
                }
            }
            _ => self.for_each_child(e, |el, c| el.check_expr(c, scope)),
        }
    }

    /// Checks an instance port expression. A bare identifier is an implicit net and
    /// is accepted; anything more complex must name declared identifiers.
    fn check_port_expr(&mut self, e: &Expr, scope: &Scope) {
        if !matches!(e.kind, ExprKind::Ident(_)) {
            self.check_expr(e, scope);
        }
    }

    fn for_each_child(&mut self, e: &Expr, mut f: impl FnMut(&mut Self, &Expr)) {
        match &e.kind {
            ExprKind::Ident(_) | ExprKind::Number(_) | ExprKind::Str(_) => {}
            ExprKind::Unary(_, a) | ExprKind::Cast(_, a) => f(self, a),
            ExprKind::Binary(_, a, b) | ExprKind::Index(a, b) => {
                f(self, a);
                f(self, b);
            }
            ExprKind::Ternary(a, b, c) | ExprKind::Part(a, b, c) => {
                f(self, a);
                f(self, b);
                f(self, c);
            }
            ExprKind::Concat(v) | ExprKind::Call(_, v) | ExprKind::SysCall(_, v) => {
                for x in v {
                    f(self, x);
                }
            }
            ExprKind::Repl(n, v) => {
                f(self, n);
                for x in v {
                    f(self, x);
                }
            }
            ExprKind::IndexedPart {
                base, start, width, ..
            } => {
                f(self, base);
                f(self, start);
                f(self, width);
            }
        }
    }
}

/// Declares the names of a list of items in the current frame, so that they can be
/// used before their declaration.
fn predeclare(items: &[ModuleItem], scope: &mut Scope) {
    for it in items {
        match it {
            ModuleItem::Decl(d) => scope.declare(&d.name),
            ModuleItem::Param(p) => scope.declare(&p.name),
            ModuleItem::Typedef(t) => {
                scope.declare(&t.name);
                if let TypedefKind::Enum(m) = &t.kind {
                    for x in m {
                        scope.declare(x);
                    }
                }
            }
            ModuleItem::Routine(r) => scope.declare(&r.name),
            ModuleItem::Import(imp) => {
                if let Some(n) = &imp.item {
                    scope.declare(n);
                }
            }
            _ => {}
        }
    }
}

fn routines_of(m: &Module) -> BTreeMap<String, Routine> {
    let mut out = BTreeMap::new();
    for it in &m.items {
        if let ModuleItem::Routine(r) = it {
            out.insert(r.name.clone(), r.clone());
        }
    }
    out
}

fn typedef_info(kind: &TypedefKind) -> TypedefInfo {
    match kind {
        TypedefKind::Vector { range, .. } => TypedefInfo {
            width: range
                .as_ref()
                .and_then(|r| {
                    Ctx::new(HashMap::new(), &BTreeMap::new())
                        .range_width(r)
                        .ok()
                })
                .or(Some(1)),
            members: Vec::new(),
        },
        TypedefKind::Enum(m) => TypedefInfo {
            width: None,
            members: m.clone(),
        },
    }
}

fn kind_name(k: &DataKind) -> String {
    match k {
        DataKind::Wire => "wire",
        DataKind::Reg => "reg",
        DataKind::Logic => "logic",
        DataKind::Bit => "bit",
        DataKind::Integer => "integer",
        DataKind::Int => "int",
        DataKind::Time => "time",
        DataKind::Genvar => "genvar",
        DataKind::Named(n) => return n.clone(),
    }
    .to_string()
}

/// Width of a declaration: its packed range, or the natural width of its kind.
fn width_of(
    scope: &Scope,
    kind: Option<&DataKind>,
    range: Option<&Range>,
    typedefs: &BTreeMap<String, TypedefInfo>,
) -> Option<u32> {
    if let Some(r) = range {
        return scope.width(r).ok();
    }
    match kind {
        Some(DataKind::Integer) | Some(DataKind::Int) => Some(32),
        Some(DataKind::Time) => Some(64),
        Some(DataKind::Named(n)) => match n.as_str() {
            "byte" => Some(8),
            "shortint" => Some(16),
            "longint" => Some(64),
            _ => typedefs.get(n).and_then(|t| t.width),
        },
        _ => Some(1),
    }
}
