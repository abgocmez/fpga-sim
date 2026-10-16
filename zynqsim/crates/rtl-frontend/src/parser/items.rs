//! Item parsing: modules, port and parameter lists, declarations, continuous
//! and procedural blocks, instances, routines, generate constructs and typedefs.

use super::{DATA_KEYWORDS, Parser, RESERVED, STRENGTHS};
use crate::ast::{
    Always, AlwaysKind, Conn, ContAssign, DataKind, Decl, Dir, Expr, ExprKind, GenKind, Generate,
    Import, Initial, Instance, Item, Module, ModuleItem, ParamDecl, PortDecl, Range, Routine,
    RoutineKind, Typedef, TypedefKind,
};
use crate::lexer::Tok;

/// Primitives that are instantiated like modules without a module name.
const GATES: &[&str] = &["and", "nand", "nor", "or", "xor", "xnor", "not", "buf"];

/// Unsupported top-level constructs: keyword, the keyword that closes them.
const UNIT_UNSUPPORTED: &[(&str, &str)] = &[
    ("package", "endpackage"),
    ("interface", "endinterface"),
    ("program", "endprogram"),
    ("class", "endclass"),
    ("config", "endconfig"),
    ("primitive", "endprimitive"),
    ("covergroup", "endgroup"),
    ("import", ";"),
    ("bind", ";"),
];

impl Parser {
    /// Parses a whole file's top-level items.
    pub(crate) fn source_unit(&mut self) -> Vec<Item> {
        let mut items = Vec::new();
        while !self.at_end() {
            let before = self.i;
            if self.skip_attribute() {
                // Attribute before a unit: ignored.
            } else if self.is_kw("module") || self.is_kw("macromodule") {
                if let Some(m) = self.module() {
                    items.push(Item::Module(m));
                }
            } else if self.is_kw("typedef") {
                if let Some(t) = self.typedef() {
                    items.push(Item::Typedef(t));
                }
            } else if self.eat_op(";") {
                // Stray semicolon.
            } else if let Some((kw, end)) = UNIT_UNSUPPORTED
                .iter()
                .find(|(k, _)| matches!(self.tok(), Tok::Ident(s) if s == k))
            {
                let pos = self.pos();
                self.err_at(
                    &pos,
                    kw,
                    format!("`{kw}` is not supported by the native front end; skipped"),
                );
                if *end == ";" {
                    self.skip_to_semi();
                } else {
                    self.skip_block_to(end);
                }
            } else {
                let found = self.describe();
                self.err(
                    "top-level item",
                    format!("unexpected {found} outside a module; skipped"),
                );
                self.recover();
            }
            self.ensure_progress(before);
        }
        items
    }

    /// `typedef ...;` at file or module level. Structs and unions are rejected.
    pub(crate) fn typedef(&mut self) -> Option<Typedef> {
        let pos = self.pos();
        self.bump(); // typedef
        if self.is_kw("struct") || self.is_kw("union") {
            let what = if self.is_kw("struct") {
                "typedef struct"
            } else {
                "typedef union"
            };
            self.err_at(
                &pos,
                what,
                format!("`{what}` is not supported by the native front end; skipped"),
            );
            self.bump();
            while self.is_kw("packed") || self.is_kw("signed") || self.is_kw("unsigned") {
                self.bump();
            }
            if self.is_op("{") {
                self.skip_group();
            }
            if let Some((n, _)) = self.ident() {
                self.typedefs.insert(n);
            }
            self.skip_to_semi();
            return None;
        }
        if self.eat_kw("enum") {
            if self.is_kw("logic")
                || self.is_kw("bit")
                || self.is_kw("int")
                || self.is_kw("integer")
            {
                self.bump();
            }
            if self.is_op("[") {
                let _ = self.range();
            }
            self.expect_op("{");
            let mut members = Vec::new();
            while let Some((m, _)) = self.expect_ident("enum member") {
                members.push(m);
                if self.eat_op("=") {
                    let _ = self.expr();
                }
                if !self.eat_op(",") {
                    break;
                }
            }
            self.expect_op("}");
            let (name, npos) = self.expect_ident("typedef name")?;
            self.typedefs.insert(name.clone());
            self.expect_op(";");
            return Some(Typedef {
                name,
                pos: npos,
                kind: TypedefKind::Enum(members),
            });
        }
        if let Tok::Ident(s) = self.tok().clone() {
            if s == "class" || s == "interface" {
                self.err_at(
                    &pos,
                    &s,
                    format!("typedef of `{s}` is not supported by the native front end"),
                );
                self.skip_to_semi();
                return None;
            }
        }
        self.data_kind_kw();
        let signed = self.eat_sign();
        let range = if self.is_op("[") { self.range() } else { None };
        let (name, npos) = self.expect_ident("typedef name")?;
        while self.is_op("[") {
            let _ = self.range();
        }
        self.typedefs.insert(name.clone());
        self.expect_op(";");
        Some(Typedef {
            name,
            pos: npos,
            kind: TypedefKind::Vector { range, signed },
        })
    }

    /// Optional `signed` / `unsigned`. Returns true for `signed`.
    fn eat_sign(&mut self) -> bool {
        if self.eat_kw("signed") {
            true
        } else {
            self.eat_kw("unsigned");
            false
        }
    }

    /// Consumes a data-type keyword if present and returns the kind it names.
    fn data_kind_kw(&mut self) -> Option<DataKind> {
        let Tok::Ident(s) = self.tok().clone() else {
            return None;
        };
        let k = match s.as_str() {
            "wire" | "tri" | "tri0" | "tri1" | "wand" | "wor" | "supply0" | "supply1" => {
                DataKind::Wire
            }
            "reg" => DataKind::Reg,
            "logic" => DataKind::Logic,
            "bit" => DataKind::Bit,
            "integer" => DataKind::Integer,
            "int" => DataKind::Int,
            "time" => DataKind::Time,
            "genvar" => DataKind::Genvar,
            "byte" | "shortint" | "longint" => DataKind::Named(s.clone()),
            _ => return None,
        };
        self.bump();
        Some(k)
    }

    /// `module name [#(params)] [(ports)] ; items endmodule`.
    fn module(&mut self) -> Option<Module> {
        self.bump(); // module
        let Some((name, npos)) = self.expect_ident("module name") else {
            self.skip_block_to("endmodule");
            return None;
        };
        let mut m = Module {
            name,
            pos: npos,
            params: Vec::new(),
            ports: Vec::new(),
            ansi: false,
            items: Vec::new(),
        };
        while self.is_kw("import") {
            self.module_import(&mut m.items);
        }
        if self.eat_op("#") {
            self.expect_op("(");
            if !self.is_op(")") {
                let mut local = false;
                loop {
                    self.param_entry(&mut m.params, &mut local);
                    if !self.eat_op(",") {
                        break;
                    }
                }
            }
            self.expect_op(")");
        }
        if self.is_op("(") {
            self.port_list(&mut m);
        }
        self.expect_op(";");
        loop {
            if self.at_end() || self.is_kw("endmodule") {
                break;
            }
            let before = self.i;
            self.module_item(&mut m.ports, &mut m.items);
            self.ensure_progress(before);
        }
        if !self.eat_kw("endmodule") {
            self.err("syntax", "module is not closed by `endmodule`");
        } else if self.eat_op(":") {
            let _ = self.ident();
        }
        Some(m)
    }

    /// One parameter (`parameter`/`localparam` keyword optional, inherited).
    pub(crate) fn param_entry(&mut self, out: &mut Vec<ParamDecl>, local: &mut bool) {
        if self.eat_kw("parameter") {
            *local = false;
        } else if self.eat_kw("localparam") {
            *local = true;
        }
        if let Tok::Ident(t) = self.tok().clone() {
            // `parameter pkg_type NAME = ...`: a typed parameter. Known typedefs are
            // accepted; a type from a package is not resolved and is reported.
            let is_type_name = matches!(self.tok_at(1), Tok::Ident(_))
                && !DATA_KEYWORDS.contains(&t.as_str())
                && !RESERVED.contains(&t.as_str());
            if is_type_name {
                if !self.typedefs.contains(&t) {
                    let pos = self.pos();
                    self.err_at(
                        &pos,
                        "parameter type",
                        format!("parameter of package type `{t}` is not supported by the native front end"),
                    );
                }
                self.bump();
            }
        }
        if self.is_kw("type") {
            let pos = self.pos();
            self.err_at(
                &pos,
                "parameter type",
                "type parameters are not supported by the native front end",
            );
            while !self.at_end() && !self.is_op(",") && !self.is_op(")") && !self.is_op(";") {
                self.bump();
            }
            return;
        }
        let signed = self.eat_sign();
        let _ = self.data_kind_kw();
        let _ = self.eat_sign();
        let range = if self.is_op("[") { self.range() } else { None };
        let Some((name, pos)) = self.expect_ident("parameter name") else {
            return;
        };
        let value = if self.eat_op("=") {
            Some(self.expr())
        } else {
            None
        };
        out.push(ParamDecl {
            name,
            pos,
            local: *local,
            signed,
            range,
            value,
        });
    }

    /// `( ... )` after a module name: ANSI declarations or a plain name list.
    fn port_list(&mut self, m: &mut Module) {
        self.bump(); // (
        if self.eat_op(")") {
            m.ansi = true;
            return;
        }
        self.skip_attribute();
        if self.is_kw("input") || self.is_kw("output") || self.is_kw("inout") || self.is_op("(") {
            m.ansi = true;
            self.ansi_entries(&mut m.ports);
        } else if matches!(self.tok(), Tok::Ident(_)) {
            loop {
                if let Some((name, pos)) = self.ident() {
                    m.ports.push(PortDecl {
                        name,
                        pos,
                        dir: None,
                        kind: None,
                        signed: false,
                        range: None,
                    });
                } else {
                    self.expect_ident("port name");
                    break;
                }
                if !self.eat_op(",") {
                    break;
                }
            }
        } else if self.is_op(".") {
            let pos = self.pos();
            self.err_at(
                &pos,
                "port expression",
                "named port expressions `.name(expr)` are not supported",
            );
            self.skip_group_to_close();
        } else {
            let found = self.describe();
            self.err("syntax", format!("unexpected {found} in port list"));
            self.skip_group_to_close();
            return;
        }
        self.expect_op(")");
    }

    /// Skips up to (not including) the `)` that closes the current port list.
    fn skip_group_to_close(&mut self) {
        let mut depth = 0i32;
        while !self.at_end() {
            match self.tok() {
                Tok::Op("(") => depth += 1,
                Tok::Op(")") => {
                    if depth == 0 {
                        return;
                    }
                    depth -= 1;
                }
                Tok::Op(";") => return,
                _ => {}
            }
            self.bump();
        }
    }

    /// ANSI port entries: `input wire signed [7:0] a, b, output reg q`.
    /// A name without a direction inherits the previous entry's declaration.
    pub(crate) fn ansi_entries(&mut self, out: &mut Vec<PortDecl>) {
        let mut cur_dir: Option<Dir> = None;
        let mut cur_kind: Option<DataKind> = None;
        let mut cur_signed = false;
        let mut cur_range: Option<Range> = None;
        loop {
            while self.skip_attribute() {}
            if self.is_op(")") {
                break;
            }
            if self.is_kw("input") || self.is_kw("output") || self.is_kw("inout") {
                cur_dir = Some(if self.eat_kw("input") {
                    Dir::Input
                } else if self.eat_kw("output") {
                    Dir::Output
                } else {
                    self.bump();
                    Dir::Inout
                });
                cur_kind = None;
                cur_signed = false;
                cur_range = None;
                if self.eat_kw("var") {
                    cur_kind = Some(DataKind::Logic);
                }
                if let Some(k) = self.data_kind_kw() {
                    cur_kind = Some(k);
                }
                cur_signed = self.eat_sign() || cur_signed;
                if self.is_op("[") {
                    cur_range = self.range();
                }
            }
            if self.is_op(".") {
                let pos = self.pos();
                self.err_at(
                    &pos,
                    "interface port",
                    "interface and modport ports are not supported",
                );
                self.skip_entry();
                if !self.eat_op(",") {
                    break;
                }
                continue;
            }
            let Some((name, pos)) = self.expect_ident("port name") else {
                self.skip_entry();
                if !self.eat_op(",") {
                    break;
                }
                continue;
            };
            while self.is_op("[") {
                let _ = self.range();
            }
            if self.eat_op("=") {
                let _ = self.expr();
            }
            out.push(PortDecl {
                name,
                pos,
                dir: cur_dir,
                kind: cur_kind.clone(),
                signed: cur_signed,
                range: cur_range.clone(),
            });
            if !self.eat_op(",") {
                break;
            }
        }
    }

    /// Skips one list entry up to a `,` or `)` at depth zero.
    fn skip_entry(&mut self) {
        let mut depth = 0i32;
        while !self.at_end() {
            match self.tok() {
                Tok::Op("(" | "[" | "{") => depth += 1,
                Tok::Op(")" | "]" | "}") => {
                    if depth == 0 {
                        return;
                    }
                    depth -= 1;
                }
                Tok::Op(",") if depth == 0 => return,
                _ => {}
            }
            self.bump();
        }
    }

    /// `[msb:lsb]` or `[n]`.
    pub(crate) fn range(&mut self) -> Option<Range> {
        let pos = self.pos();
        self.expect_op("[");
        let a = self.expr();
        if self.eat_op(":") {
            let b = self.expr();
            self.expect_op("]");
            return Some(Range { msb: a, lsb: b });
        }
        if self.is_op("+:") || self.is_op("-:") {
            self.err_at(
                &pos,
                "indexed range",
                "indexed ranges in declarations are not supported",
            );
            self.skip_entry();
            self.eat_op("]");
            return None;
        }
        self.expect_op("]");
        // `[n]`: n elements, stored as [n-1:0].
        let one = super::expr::num_expr(a.pos.clone(), 1);
        let msb = Expr {
            pos: a.pos.clone(),
            kind: ExprKind::Binary(crate::ast::BinOp::Sub, Box::new(a), Box::new(one)),
        };
        Some(Range {
            msb,
            lsb: super::expr::num_expr(pos, 0),
        })
    }

    /// One item of a module body (or of a generate block). Port declarations are
    /// merged into `ports`; other items are appended to `out`.
    pub(crate) fn module_item(&mut self, ports: &mut Vec<PortDecl>, out: &mut Vec<ModuleItem>) {
        let pos = self.pos();
        match self.tok().clone() {
            Tok::Op(";") => self.bump(),
            Tok::Op("(") if self.skip_attribute() => {}
            Tok::SysName(n) if matches!(n.as_str(), "$error" | "$warning" | "$info" | "$fatal") => {
                // Elaboration-time system tasks: accepted, no simulation effect.
                self.skip_to_semi();
            }
            Tok::Ident(kw) => match kw.as_str() {
                "parameter" | "localparam" => {
                    let mut local = kw == "localparam";
                    let mut list = Vec::new();
                    loop {
                        self.param_entry(&mut list, &mut local);
                        if !self.eat_op(",") {
                            break;
                        }
                    }
                    self.expect_op(";");
                    out.extend(list.into_iter().map(ModuleItem::Param));
                }
                "input" | "output" | "inout" => self.port_decl(ports),
                "typedef" => {
                    if let Some(t) = self.typedef() {
                        out.push(ModuleItem::Typedef(t));
                    }
                }
                "assign" => self.assign_items(out),
                "always" | "always_ff" | "always_comb" | "always_latch" => {
                    self.bump();
                    let akind = match kw.as_str() {
                        "always_ff" => AlwaysKind::Ff,
                        "always_comb" => AlwaysKind::Comb,
                        "always_latch" => AlwaysKind::Latch,
                        _ => AlwaysKind::Plain,
                    };
                    let body = self.stmt();
                    out.push(ModuleItem::Always(Always {
                        kind: akind,
                        body,
                        pos,
                    }));
                }
                "initial" => {
                    self.bump();
                    let body = self.stmt();
                    out.push(ModuleItem::Initial(Initial { body, pos }));
                }
                "final" => {
                    self.err_at(
                        &pos,
                        "final",
                        "final blocks are not supported by the native front end",
                    );
                    self.bump();
                    let _ = self.stmt();
                }
                "task" | "function" => {
                    if let Some(r) = self.routine() {
                        out.push(ModuleItem::Routine(r));
                    }
                }
                "generate" => {
                    self.bump();
                    let mut items = Vec::new();
                    loop {
                        if self.at_end() || self.is_kw("endgenerate") || self.is_kw("endmodule") {
                            break;
                        }
                        let before = self.i;
                        self.module_item(ports, &mut items);
                        self.ensure_progress(before);
                    }
                    if !self.eat_kw("endgenerate") {
                        self.err("syntax", "`generate` is not closed by `endgenerate`");
                    }
                    out.push(ModuleItem::Generate(Generate {
                        label: None,
                        pos,
                        kind: GenKind::Block(items),
                    }));
                }
                "for" | "if" | "case" => {
                    if let Some(g) = self.generate_construct() {
                        out.push(ModuleItem::Generate(g));
                    }
                }
                "genvar" => {
                    self.bump();
                    loop {
                        if let Some((n, p)) = self.expect_ident("genvar name") {
                            out.push(ModuleItem::Decl(Decl {
                                name: n,
                                pos: p,
                                kind: DataKind::Genvar,
                                signed: false,
                                range: None,
                                dims: Vec::new(),
                                init: None,
                            }));
                        }
                        if !self.eat_op(",") {
                            break;
                        }
                    }
                    self.expect_op(";");
                }
                "specparam" | "specify" | "defparam" => {
                    self.err_at(
                        &pos,
                        &kw,
                        format!("`{kw}` is not supported by the native front end; skipped"),
                    );
                    if kw == "specify" {
                        self.skip_block_to("endspecify");
                    } else {
                        self.skip_to_semi();
                    }
                }
                "assert" | "assume" | "cover" | "restrict" | "property" | "sequence"
                | "clocking" => {
                    self.err_at(
                        &pos,
                        "assertion",
                        format!("`{kw}` is not supported by the native front end; skipped"),
                    );
                    if kw == "property" || kw == "sequence" || kw == "clocking" {
                        self.skip_block_to(match kw.as_str() {
                            "property" => "endproperty",
                            "sequence" => "endsequence",
                            _ => "endclocking",
                        });
                    } else {
                        self.skip_to_semi();
                    }
                }
                "covergroup" | "class" | "interface" | "program" | "package" | "bind" => {
                    let end = UNIT_UNSUPPORTED
                        .iter()
                        .find(|(k, _)| *k == kw.as_str())
                        .map_or(";", |(_, e)| e);
                    let construct = kw.clone();
                    self.err_at(
                        &pos,
                        &construct,
                        format!("`{construct}` is not supported by the native front end; skipped"),
                    );
                    if end == ";" {
                        self.skip_to_semi();
                    } else {
                        self.skip_block_to(end);
                    }
                }
                "import" if matches!(self.tok_at(1), Tok::Str(_)) => {
                    self.err_at(
                        &pos,
                        "DPI import",
                        "DPI-C imports are not supported by the native front end; skipped",
                    );
                    self.skip_to_semi();
                }
                "import" => self.module_import(out),
                "event" | "real" | "realtime" => {
                    self.err_at(
                        &pos,
                        &kw,
                        format!("`{kw}` declarations are not supported by the native front end"),
                    );
                    self.skip_to_semi();
                }
                k if GATES.contains(&k) => {
                    self.bump();
                    let name = match self.ident() {
                        Some((n, _)) => n,
                        None => String::new(),
                    };
                    let ports_list = self.conns_positional();
                    self.expect_op(";");
                    out.push(ModuleItem::Instance(Instance {
                        module: kw.clone(),
                        name,
                        pos,
                        params: Vec::new(),
                        ports: ports_list,
                        wildcard: false,
                        array: None,
                        primitive: true,
                    }));
                }
                k if DATA_KEYWORDS.contains(&k)
                    || (self.typedefs.contains(k) && matches!(self.tok_at(1), Tok::Ident(_))) =>
                {
                    let mut decls = Vec::new();
                    self.data_decl(&mut decls);
                    out.extend(decls.into_iter().map(ModuleItem::Decl));
                }
                k if RESERVED.contains(&k) => {
                    let found = self.describe();
                    self.err("syntax", format!("unexpected {found} in module body"));
                    self.recover();
                }
                _ => {
                    // Module instantiation: `mod [#(...)] inst [range] (ports);`
                    let Some((module, _)) = self.ident() else {
                        let found = self.describe();
                        self.err("syntax", format!("unexpected {found} in module body"));
                        self.recover();
                        return;
                    };
                    self.instance_rest(module, out);
                }
            },
            _ => {
                let found = self.describe();
                self.err("syntax", format!("unexpected {found} in module body"));
                self.recover();
            }
        }
    }

    /// `import pkg::item;` or `import pkg::*;` inside a module. The imported names
    /// are not resolved: a wildcard import disables the undeclared-name check.
    fn module_import(&mut self, out: &mut Vec<ModuleItem>) {
        self.bump(); // import
        loop {
            let pos = self.pos();
            let Some((package, _)) = self.expect_ident("package name") else {
                self.skip_to_semi();
                return;
            };
            if !self.eat_op("::") {
                self.expect_op("::");
                self.skip_to_semi();
                return;
            }
            let item = if self.eat_op("*") {
                None
            } else {
                self.expect_ident("imported name").map(|(n, _)| n)
            };
            out.push(ModuleItem::Import(Import { package, item, pos }));
            if !self.eat_op(",") {
                break;
            }
        }
        self.expect_op(";");
    }

    /// Port declarations in the body (`input [7:0] a, b;`), merged into `ports`.
    fn port_decl(&mut self, ports: &mut Vec<PortDecl>) {
        let dir = if self.eat_kw("input") {
            Dir::Input
        } else if self.eat_kw("output") {
            Dir::Output
        } else {
            self.bump();
            Dir::Inout
        };
        let mut kind = None;
        if self.eat_kw("var") {
            kind = Some(DataKind::Logic);
        }
        if let Some(k) = self.data_kind_kw() {
            kind = Some(k);
        }
        let signed = self.eat_sign();
        let range = if self.is_op("[") { self.range() } else { None };
        loop {
            let Some((name, pos)) = self.expect_ident("port name") else {
                self.recover();
                return;
            };
            if let Some(p) = ports.iter_mut().find(|p| p.name == name) {
                p.dir = Some(dir);
                p.kind = kind.clone();
                p.signed = signed;
                p.range = range.clone();
            } else {
                ports.push(PortDecl {
                    name,
                    pos,
                    dir: Some(dir),
                    kind: kind.clone(),
                    signed,
                    range: range.clone(),
                });
            }
            if !self.eat_op(",") {
                break;
            }
        }
        self.expect_op(";");
    }

    /// `assign [strength] [#delay] lhs = rhs {, lhs = rhs};`
    fn assign_items(&mut self, out: &mut Vec<ModuleItem>) {
        let pos = self.pos();
        self.bump(); // assign
        if self.is_op("(") {
            self.skip_group();
        }
        let delay = if self.eat_op("#") {
            Some(if self.is_op("(") {
                self.bump();
                let e = self.expr();
                self.expect_op(")");
                e
            } else {
                self.primary()
            })
        } else {
            None
        };
        loop {
            let lhs = self.lvalue();
            self.expect_op("=");
            let rhs = self.expr();
            out.push(ModuleItem::Assign(ContAssign {
                lhs,
                rhs,
                delay: delay.clone(),
                pos: pos.clone(),
            }));
            if !self.eat_op(",") {
                break;
            }
        }
        self.expect_op(";");
    }

    /// Declaration with optional `;`: `kind [signed] [range] name [dims] [= init] {, ...} ;`.
    pub(crate) fn data_decl(&mut self, out: &mut Vec<Decl>) {
        self.decl_no_semi(out);
        self.expect_op(";");
    }

    /// Like [`Parser::data_decl`] but leaves the `;` to the caller.
    pub(crate) fn decl_no_semi(&mut self, out: &mut Vec<Decl>) {
        let kind = match self.data_kind_kw() {
            Some(k) => k,
            None => match self.tok().clone() {
                Tok::Ident(t) => {
                    self.bump();
                    DataKind::Named(t)
                }
                _ => DataKind::Wire,
            },
        };
        // Drive strength on a net: `tri (weak1, strong0) x;`.
        if self.is_op("(")
            && matches!(self.tok_at(1), Tok::Ident(s) if STRENGTHS.contains(&s.as_str()))
        {
            self.skip_group();
        }
        let builtin_signed = matches!(&kind, DataKind::Named(n) if matches!(n.as_str(), "byte" | "shortint" | "longint"));
        let signed = self.eat_sign() || builtin_signed;
        let range = if self.is_op("[") { self.range() } else { None };
        // Multi-dimensional packed arrays (`logic [A][B] x`) are not supported.
        if self.is_op("[") {
            let p = self.pos();
            self.err_at(
                &p,
                "packed array",
                "multi-dimensional packed arrays are not supported by the native front end",
            );
            while self.is_op("[") {
                let _ = self.range();
            }
        }
        while let Some((name, pos)) = self.expect_ident("declared name") {
            let mut dims = Vec::new();
            while self.is_op("[") {
                if let Some(r) = self.range() {
                    dims.push(r);
                }
            }
            let init = if self.eat_op("=") {
                Some(self.expr())
            } else {
                None
            };
            out.push(Decl {
                name,
                pos,
                kind: kind.clone(),
                signed,
                range: range.clone(),
                dims,
                init,
            });
            if !self.eat_op(",") {
                break;
            }
        }
    }

    /// `always`-style blocks are parsed in `module_item`; this handles
    /// `task`/`function` bodies up to their `end` keyword.
    fn routine(&mut self) -> Option<Routine> {
        let kind = if self.is_kw("task") {
            RoutineKind::Task
        } else {
            RoutineKind::Function
        };
        self.bump();
        let automatic = if self.eat_kw("automatic") {
            true
        } else {
            self.eat_kw("static");
            false
        };
        let mut ret_kind = None;
        let mut ret_range = None;
        let mut ret_signed = false;
        if kind == RoutineKind::Function {
            self.eat_kw("void");
            ret_kind = self.data_kind_kw();
            ret_signed = self.eat_sign();
            if self.is_op("[") {
                ret_range = self.range();
            }
        }
        let Some((name, pos)) = self.expect_ident("routine name") else {
            self.skip_block_to(if kind == RoutineKind::Task {
                "endtask"
            } else {
                "endfunction"
            });
            return None;
        };
        let mut args = Vec::new();
        if self.eat_op("(") {
            if !self.is_op(")") {
                self.ansi_entries(&mut args);
            }
            self.expect_op(")");
        }
        self.expect_op(";");
        let mut decls = Vec::new();
        loop {
            if self.is_kw("input") || self.is_kw("output") || self.is_kw("inout") {
                self.port_decl(&mut args);
            } else if self.is_decl_start() {
                self.data_decl(&mut decls);
            } else {
                break;
            }
        }
        let mut body = Vec::new();
        loop {
            if self.at_end()
                || self.is_kw("endfunction")
                || self.is_kw("endtask")
                || self.is_kw("endmodule")
            {
                break;
            }
            let before = self.i;
            body.push(self.stmt());
            self.ensure_progress(before);
        }
        let end = if kind == RoutineKind::Task {
            "endtask"
        } else {
            "endfunction"
        };
        if !self.eat_kw(end) {
            self.err("syntax", format!("routine is not closed by `{end}`"));
        } else if self.eat_op(":") {
            let _ = self.ident();
        }
        Some(Routine {
            kind,
            name,
            pos,
            automatic,
            ret_kind,
            ret_range,
            ret_signed,
            args,
            decls,
            body,
        })
    }

    /// Generate `for`, `if` or `case` (keyword at the cursor).
    fn generate_construct(&mut self) -> Option<Generate> {
        let pos = self.pos();
        if self.eat_kw("for") {
            self.expect_op("(");
            self.eat_kw("genvar");
            let (var, _) = self.expect_ident("loop variable")?;
            self.expect_op("=");
            let init = self.expr();
            self.expect_op(";");
            let cond = self.expr();
            self.expect_op(";");
            let (var2, p2) = self.expect_ident("loop variable")?;
            let step = if self.eat_op("++") {
                Expr {
                    pos: p2.clone(),
                    kind: ExprKind::Binary(
                        crate::ast::BinOp::Add,
                        Box::new(Expr {
                            pos: p2.clone(),
                            kind: ExprKind::Ident(var2.clone()),
                        }),
                        Box::new(super::expr::num_expr(p2.clone(), 1)),
                    ),
                }
            } else {
                self.expect_op("=");
                self.expr()
            };
            self.expect_op(")");
            let (label, body) = self.gen_body();
            return Some(Generate {
                label,
                pos,
                kind: GenKind::For {
                    var,
                    init,
                    cond,
                    step,
                    body,
                },
            });
        }
        if self.eat_kw("if") {
            self.expect_op("(");
            let cond = self.expr();
            self.expect_op(")");
            let (label, then_items) = self.gen_body();
            let else_items = if self.eat_kw("else") {
                if self.is_kw("if") {
                    let g = self.generate_construct();
                    g.map(|g| vec![ModuleItem::Generate(g)])
                } else {
                    Some(self.gen_body().1)
                }
            } else {
                None
            };
            return Some(Generate {
                label,
                pos,
                kind: GenKind::If {
                    cond,
                    then_items,
                    else_items,
                },
            });
        }
        if self.eat_kw("case") {
            self.expect_op("(");
            let expr = self.expr();
            self.expect_op(")");
            let mut items = Vec::new();
            loop {
                if self.at_end() || self.is_kw("endcase") || self.is_kw("endmodule") {
                    break;
                }
                let before = self.i;
                if self.eat_kw("default") {
                    self.eat_op(":");
                    items.push((Vec::new(), self.gen_body().1));
                } else {
                    let labels = self.expr_list_for_case();
                    self.expect_op(":");
                    items.push((labels, self.gen_body().1));
                }
                self.ensure_progress(before);
            }
            if !self.eat_kw("endcase") {
                self.err("syntax", "generate case is not closed by `endcase`");
            }
            return Some(Generate {
                label: None,
                pos,
                kind: GenKind::Case { expr, items },
            });
        }
        None
    }

    fn expr_list_for_case(&mut self) -> Vec<Expr> {
        self.expr_list()
    }

    /// Body of a generate construct: `begin [: label] items end`, or one item.
    fn gen_body(&mut self) -> (Option<String>, Vec<ModuleItem>) {
        let mut ports = Vec::new();
        if self.eat_kw("begin") {
            let mut label = None;
            if self.eat_op(":") {
                label = self.ident().map(|(n, _)| n);
            }
            let mut items = Vec::new();
            loop {
                if self.at_end() || self.is_kw("end") || self.is_kw("endmodule") {
                    break;
                }
                let before = self.i;
                self.module_item(&mut ports, &mut items);
                self.ensure_progress(before);
            }
            if !self.eat_kw("end") {
                self.err("syntax", "generate `begin` is not closed by `end`");
            } else if self.eat_op(":") {
                let _ = self.ident();
            }
            return (label, items);
        }
        let mut items = Vec::new();
        self.module_item(&mut ports, &mut items);
        (None, items)
    }

    /// Instance after its module name: parameter overrides, name, ports.
    fn instance_rest(&mut self, module: String, out: &mut Vec<ModuleItem>) {
        let mut params = Vec::new();
        let mut wildcard = false;
        if self.eat_op("#") {
            let (p, _) = self.conns();
            params = p;
        }
        loop {
            let Some((name, npos)) = self.expect_ident("instance name") else {
                self.recover();
                return;
            };
            let array = if self.is_op("[") { self.range() } else { None };
            let (ports, wc) = if self.is_op("(") {
                self.conns()
            } else {
                let found = self.describe();
                self.err(
                    "syntax",
                    format!("expected `(` for instance `{name}`, found {found}"),
                );
                self.recover();
                return;
            };
            wildcard |= wc;
            out.push(ModuleItem::Instance(Instance {
                module: module.clone(),
                name,
                pos: npos,
                params: params.clone(),
                ports,
                wildcard,
                array,
                primitive: false,
            }));
            if !self.eat_op(",") {
                break;
            }
            wildcard = false;
        }
        self.expect_op(";");
    }

    /// `( .name(expr), .*, expr, ... )`. Returns the connections and `.*`.
    fn conns(&mut self) -> (Vec<Conn>, bool) {
        let mut out = Vec::new();
        let mut wildcard = false;
        self.expect_op("(");
        if self.eat_op(")") {
            return (out, false);
        }
        loop {
            let pos = self.pos();
            if self.is_op(".") && self.is_op_at(1, "*") {
                self.bump();
                self.bump();
                wildcard = true;
            } else if self.eat_op(".") {
                let Some((name, _)) = self.expect_ident("port name") else {
                    self.recover();
                    return (out, wildcard);
                };
                let expr = if self.eat_op("(") {
                    if self.eat_op(")") {
                        None
                    } else {
                        let e = self.expr();
                        self.expect_op(")");
                        Some(e)
                    }
                } else {
                    // Implicit connection `.name`.
                    Some(Expr {
                        pos: pos.clone(),
                        kind: ExprKind::Ident(name.clone()),
                    })
                };
                out.push(Conn {
                    name: Some(name),
                    expr,
                    pos,
                });
            } else if self.is_op(",") || self.is_op(")") {
                out.push(Conn {
                    name: None,
                    expr: None,
                    pos,
                });
            } else {
                let e = self.expr();
                out.push(Conn {
                    name: None,
                    expr: Some(e),
                    pos,
                });
            }
            if !self.eat_op(",") {
                break;
            }
        }
        self.expect_op(")");
        (out, wildcard)
    }

    /// Positional connections of a gate primitive: `( a, b, c )`.
    fn conns_positional(&mut self) -> Vec<Conn> {
        self.conns().0
    }
}
