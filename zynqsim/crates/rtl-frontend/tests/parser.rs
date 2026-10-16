//! Parser tests: one construct per test, operator precedence, recovery after
//! unsupported constructs, and the diagnostics that name them.

use rtl_frontend::ast::{
    AlwaysKind, BinOp, CaseKind, DataKind, Dir, Edge, Expr, ExprKind, GenKind, Item, Module,
    ModuleItem, RoutineKind, SourceUnit, StmtKind, TypedefKind, UnOp,
};
use rtl_frontend::lexer::{Tok, render};
use rtl_frontend::{Diagnostic, Options, parse_text};

fn parse(src: &str) -> SourceUnit {
    let p = parse_text("t.sv", src, &Options::default());
    assert!(
        p.diagnostics.is_empty(),
        "unexpected diagnostics: {:#?}",
        p.diagnostics
    );
    p.units.into_iter().next().expect("one unit")
}

fn diags(src: &str) -> Vec<Diagnostic> {
    parse_text("t.sv", src, &Options::default()).diagnostics
}

fn modules(u: &SourceUnit) -> Vec<&Module> {
    u.items
        .iter()
        .filter_map(|i| match i {
            Item::Module(m) => Some(m),
            _ => None,
        })
        .collect()
}

fn only_module(src: &str) -> Module {
    let u = parse(src);
    let ms = modules(&u);
    assert_eq!(ms.len(), 1, "one module expected");
    ms[0].clone()
}

fn bin_sym(op: BinOp) -> &'static str {
    match op {
        BinOp::Add => "+",
        BinOp::Sub => "-",
        BinOp::Mul => "*",
        BinOp::Div => "/",
        BinOp::Mod => "%",
        BinOp::Pow => "**",
        BinOp::Shl => "<<",
        BinOp::Shr => ">>",
        BinOp::AShl => "<<<",
        BinOp::AShr => ">>>",
        BinOp::Lt => "<",
        BinOp::Le => "<=",
        BinOp::Gt => ">",
        BinOp::Ge => ">=",
        BinOp::Eq => "==",
        BinOp::Ne => "!=",
        BinOp::CaseEq => "===",
        BinOp::CaseNe => "!==",
        BinOp::And => "&",
        BinOp::Or => "|",
        BinOp::Xor => "^",
        BinOp::Xnor => "~^",
        BinOp::LAnd => "&&",
        BinOp::LOr => "||",
    }
}

fn un_sym(op: UnOp) -> &'static str {
    match op {
        UnOp::Plus => "+",
        UnOp::Minus => "-",
        UnOp::LNot => "!",
        UnOp::BitNot => "~",
        UnOp::RedAnd => "&",
        UnOp::RedNand => "~&",
        UnOp::RedOr => "|",
        UnOp::RedNor => "~|",
        UnOp::RedXor => "^",
        UnOp::RedXnor => "~^",
    }
}

/// S-expression rendering of an expression, for precedence tests.
fn sx(e: &Expr) -> String {
    let list = |v: &[Expr]| v.iter().map(sx).collect::<Vec<_>>().join(" ");
    match &e.kind {
        ExprKind::Ident(n) => n.clone(),
        ExprKind::Number(n) => render(&Tok::Num(n.clone())).to_lowercase(),
        ExprKind::Str(s) => format!("\"{s}\""),
        ExprKind::Unary(op, a) => format!("({} {})", un_sym(*op), sx(a)),
        ExprKind::Binary(op, a, b) => format!("({} {} {})", bin_sym(*op), sx(a), sx(b)),
        ExprKind::Ternary(c, t, f) => format!("(? {} {} {})", sx(c), sx(t), sx(f)),
        ExprKind::Concat(v) => format!("{{{}}}", list(v)),
        ExprKind::Repl(n, v) => format!("rep({} {})", sx(n), list(v)),
        ExprKind::Index(b, i) => format!("idx({} {})", sx(b), sx(i)),
        ExprKind::Part(b, m, l) => format!("part({} {} {})", sx(b), sx(m), sx(l)),
        ExprKind::IndexedPart {
            base,
            start,
            width,
            up,
        } => format!(
            "ipart{}({} {} {})",
            if *up { "+" } else { "-" },
            sx(base),
            sx(start),
            sx(width)
        ),
        ExprKind::Call(n, a) => format!("call {n}({})", list(a)),
        ExprKind::SysCall(n, a) => format!("{n}({})", list(a)),
        ExprKind::Cast(w, a) => format!("cast({:?} {})", w, sx(a)),
    }
}

/// The right-hand side of `assign x = <e>;` in a one-module wrapper.
fn rhs(e: &str) -> String {
    let src = format!("module m; wire [31:0] x; assign x = {e}; endmodule\n");
    let m = only_module(&src);
    for it in &m.items {
        if let ModuleItem::Assign(a) = it {
            return sx(&a.rhs);
        }
    }
    panic!("no assign");
}

#[test]
fn additive_binds_looser_than_multiplicative() {
    assert_eq!(rhs("a + b * c"), "(+ a (* b c))");
    assert_eq!(rhs("a * b + c"), "(+ (* a b) c)");
}

#[test]
fn binary_operators_are_left_associative() {
    assert_eq!(rhs("a - b - c"), "(- (- a b) c)");
    assert_eq!(rhs("a ** b ** c"), "(** (** a b) c)");
}

#[test]
fn power_binds_tighter_than_multiplicative_and_unary_tighter_than_power() {
    assert_eq!(rhs("a * b ** c"), "(* a (** b c))");
    assert_eq!(rhs("-a ** b"), "(** (- a) b)");
}

#[test]
fn shift_is_below_additive_and_above_relational() {
    assert_eq!(rhs("a << b + c"), "(<< a (+ b c))");
    assert_eq!(rhs("a < b << c"), "(< a (<< b c))");
    assert_eq!(rhs("a >>> b"), "(>>> a b)");
}

#[test]
fn relational_binds_tighter_than_equality() {
    assert_eq!(rhs("a < b == c"), "(== (< a b) c)");
}

#[test]
fn bitwise_levels_are_and_xor_or() {
    assert_eq!(rhs("a & b == c"), "(& a (== b c))");
    assert_eq!(rhs("a | b ^ c & d"), "(| a (^ b (& c d)))");
    assert_eq!(rhs("a ^~ b"), "(~^ a b)");
}

#[test]
fn logical_levels_are_or_and() {
    assert_eq!(rhs("a || b && c"), "(|| a (&& b c))");
    assert_eq!(rhs("a && b == c"), "(&& a (== b c))");
}

#[test]
fn ternary_is_lowest_and_right_associative() {
    assert_eq!(rhs("c ? x : y ? z : w"), "(? c x (? y z w))");
    assert_eq!(rhs("a || b ? c : d"), "(? (|| a b) c d)");
}

#[test]
fn reductions_and_unary_operators() {
    assert_eq!(rhs("~&a"), "(~& a)");
    assert_eq!(rhs("|a + 1"), "(+ (| a) 1)");
    assert_eq!(rhs("!a && ~b"), "(&& (! a) (~ b))");
}

#[test]
fn concatenation_replication_and_selects() {
    assert_eq!(rhs("{a, {2{b}}}"), "{a rep(2 b)}");
    assert_eq!(rhs("a[7:0] + b[i +: 4]"), "(+ part(a 7 0) ipart+(b i 4))");
    assert_eq!(rhs("c[3]"), "idx(c 3)");
    assert_eq!(rhs("m[i-:2][0]"), "idx(ipart-(m i 2) 0)");
}

#[test]
fn sized_literals_and_system_functions() {
    assert_eq!(rhs("8'hff"), "8'hff");
    assert_eq!(rhs("4'b10x1"), "4'b10x1");
    assert_eq!(rhs("$clog2(N)"), "$clog2(N)");
    assert_eq!(rhs("$signed(a)"), "$signed(a)");
    assert_eq!(rhs("8'(a)"), "cast(Some(8) a)");
    assert_eq!(rhs("'(a)"), "cast(None a)");
    assert_eq!(rhs("f(a, b)"), "call f(a b)");
}

#[test]
fn real_literals_in_expressions_are_rejected_with_their_construct() {
    let d = diags("module m; wire x = 1.5; endmodule\n");
    assert_eq!(d.len(), 1, "{d:?}");
    assert_eq!(d[0].construct, "real literal");
    assert_eq!((d[0].line, d[0].column), (1, 20));
}

#[test]
fn ansi_header_with_directions_widths_and_signed() {
    let m = only_module(
        "module m(input wire clk, input signed [7:0] a, b, output reg [3:0] q, inout z); endmodule\n",
    );
    assert!(m.ansi);
    let names: Vec<(&str, Option<Dir>, bool)> = m
        .ports
        .iter()
        .map(|p| (p.name.as_str(), p.dir, p.signed))
        .collect();
    assert_eq!(
        names,
        vec![
            ("clk", Some(Dir::Input), false),
            ("a", Some(Dir::Input), true),
            ("b", Some(Dir::Input), true),
            ("q", Some(Dir::Output), false),
            ("z", Some(Dir::Inout), false),
        ]
    );
    assert!(m.ports[2].range.is_some(), "b inherits the range of a");
    assert_eq!(m.ports[3].kind, Some(DataKind::Reg));
}

#[test]
fn non_ansi_header_is_completed_by_body_declarations() {
    let m = only_module("module m(a, q);\n input [7:0] a;\n output reg q;\n endmodule\n");
    assert!(!m.ansi);
    assert_eq!(m.ports.len(), 2);
    assert_eq!(m.ports[0].dir, Some(Dir::Input));
    assert!(m.ports[0].range.is_some());
    assert_eq!(m.ports[1].dir, Some(Dir::Output));
    assert_eq!(m.ports[1].kind, Some(DataKind::Reg));
}

#[test]
fn header_and_body_parameters_and_localparams() {
    let m = only_module(
        "module m #(parameter W = 8, parameter [3:0] K = 2, localparam L = W + 1)(input x);\n\
         parameter D = 4;\n localparam E = $clog2(16);\n endmodule\n",
    );
    assert_eq!(m.params.len(), 3);
    assert!(!m.params[0].local);
    assert!(
        m.params[2].local,
        "localparam keeps its kind in a header list"
    );
    let body_params: Vec<&str> = m
        .items
        .iter()
        .filter_map(|i| match i {
            ModuleItem::Param(p) => Some(p.name.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(body_params, vec!["D", "E"]);
}

#[test]
fn declarations_packed_unpacked_init_and_kinds() {
    let m = only_module(
        "module m; reg [7:0] mem [0:15]; wire w = a & b; logic signed [3:0] s; integer i, j; \
         localparam int unsigned U = 3; endmodule\n",
    );
    let decls: Vec<&rtl_frontend::ast::Decl> = m
        .items
        .iter()
        .filter_map(|i| match i {
            ModuleItem::Decl(d) => Some(d),
            _ => None,
        })
        .collect();
    assert_eq!(decls.len(), 5);
    assert_eq!(decls[0].name, "mem");
    assert_eq!(decls[0].dims.len(), 1);
    assert!(decls[1].init.is_some());
    assert!(decls[2].signed);
    assert_eq!(decls[3].kind, DataKind::Integer);
}

#[test]
fn drive_strength_and_byte_types() {
    let m = only_module("module m; tri (weak1, strong0) p; byte b; shortint s; endmodule\n");
    let names: Vec<&str> = m
        .items
        .iter()
        .filter_map(|i| match i {
            ModuleItem::Decl(d) => Some(d.name.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(names, vec!["p", "b", "s"]);
}

#[test]
fn continuous_assign_with_delay_and_several_targets() {
    let m = only_module("module m; wire a, b; assign #2 a = x, b = ~x; endmodule\n");
    let assigns: Vec<_> = m
        .items
        .iter()
        .filter_map(|i| match i {
            ModuleItem::Assign(a) => Some(a),
            _ => None,
        })
        .collect();
    assert_eq!(assigns.len(), 2);
    assert!(assigns[0].delay.is_some(), "the delay is kept");
}

#[test]
fn always_ff_with_async_reset_and_nonblocking_assignments() {
    let m = only_module(
        "module m(input clk, rst_n, d, output reg q);\n\
         always_ff @(posedge clk or negedge rst_n) begin\n\
           if (!rst_n) q <= 1'b0; else q <= d;\n\
         end\nendmodule\n",
    );
    let ModuleItem::Always(a) = &m.items[0] else {
        panic!("always expected")
    };
    assert_eq!(a.kind, AlwaysKind::Ff);
    let StmtKind::EventCtrl {
        events: Some(ev),
        stmt,
    } = &a.body.kind
    else {
        panic!("event control")
    };
    assert_eq!(ev[0].edge, Edge::Pos);
    assert_eq!(ev[1].edge, Edge::Neg);
    let StmtKind::Block { stmts, .. } = &stmt.kind else {
        panic!("block")
    };
    let StmtKind::If {
        then_stmt,
        else_stmt,
        ..
    } = &stmts[0].kind
    else {
        panic!("if")
    };
    assert!(matches!(
        then_stmt.kind,
        StmtKind::Assign {
            nonblocking: true,
            ..
        }
    ));
    assert!(else_stmt.is_some());
}

#[test]
fn always_star_and_always_comb_and_blocking() {
    let m = only_module(
        "module m; reg y; always @* y = a; always @(*) y = b; always_comb begin y = c; end endmodule\n",
    );
    let always: Vec<_> = m
        .items
        .iter()
        .filter_map(|i| match i {
            ModuleItem::Always(a) => Some(a),
            _ => None,
        })
        .collect();
    assert_eq!(always.len(), 3);
    assert!(matches!(
        &always[0].body.kind,
        StmtKind::EventCtrl { events: None, .. }
    ));
    assert!(matches!(
        &always[1].body.kind,
        StmtKind::EventCtrl { events: None, .. }
    ));
    assert_eq!(always[2].kind, AlwaysKind::Comb);
}

#[test]
fn case_with_default_and_multiple_labels_and_casez() {
    let m = only_module(
        "module m; always @* casez (s) 2'b1?, 2'b01: y = 1; default: y = 0; endcase endmodule\n",
    );
    let ModuleItem::Always(a) = &m.items[0] else {
        panic!()
    };
    let StmtKind::EventCtrl { stmt, .. } = &a.body.kind else {
        panic!()
    };
    let StmtKind::Case { kind, items, .. } = &stmt.kind else {
        panic!("case")
    };
    assert_eq!(*kind, CaseKind::Casez);
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].labels.len(), 2);
    assert!(items[1].labels.is_empty(), "default has no labels");
}

#[test]
fn for_loop_with_increment_operator_and_named_block() {
    let m = only_module(
        "module m; integer i; initial begin : init_blk for (i = 0; i < 4; i++) begin end end endmodule\n",
    );
    let ModuleItem::Initial(init) = &m.items[1] else {
        panic!("initial")
    };
    assert!(!ModuleItem::Initial(init.clone()).is_synthesizable());
    let StmtKind::Block { name, stmts, .. } = &init.body.kind else {
        panic!("block")
    };
    assert_eq!(name.as_deref(), Some("init_blk"));
    let StmtKind::For { step, .. } = &stmts[0].kind else {
        panic!("for")
    };
    let StmtKind::Assign { rhs, .. } = &step.kind else {
        panic!()
    };
    assert_eq!(sx(rhs), "(+ i 1)", "i++ is rewritten to i = i + 1");
}

#[test]
fn for_loop_with_declaration_in_the_initialiser() {
    let m = only_module("module m; initial for (int k = 0; k < 2; k = k + 1) ; endmodule\n");
    let ModuleItem::Initial(init) = &m.items[0] else {
        panic!()
    };
    let StmtKind::Block { decls, stmts, .. } = &init.body.kind else {
        panic!("the loop is wrapped with its declaration")
    };
    assert_eq!(decls[0].name, "k");
    assert!(matches!(stmts[0].kind, StmtKind::For { .. }));
}

#[test]
fn delays_event_waits_and_system_tasks_are_flagged_in_the_tree() {
    let m = only_module(
        "module m; reg q; initial begin #5 q = 1; @(posedge q) $display(\"hi %d\", q); wait (q) $finish; end endmodule\n",
    );
    let ModuleItem::Initial(init) = &m.items[1] else {
        panic!()
    };
    let StmtKind::Block { stmts, .. } = &init.body.kind else {
        panic!()
    };
    assert!(matches!(stmts[0].kind, StmtKind::Delay { .. }));
    assert!(matches!(stmts[1].kind, StmtKind::EventCtrl { .. }));
    assert!(matches!(stmts[2].kind, StmtKind::Wait { .. }));
}

#[test]
fn unknown_system_task_is_reported() {
    let d = diags("module m; initial $my_task; endmodule\n");
    assert!(
        d.iter()
            .any(|x| x.construct == "system task" && x.message.contains("$my_task")),
        "{d:?}"
    );
}

#[test]
fn instances_with_parameters_named_positional_and_wildcard() {
    let m = only_module(
        "module m; sub #(.W(8)) u0 (.a(x), .b(), .c); sub #(16) u1 (x, y, z); sub u2 (.*); endmodule\n",
    );
    let insts: Vec<_> = m
        .items
        .iter()
        .filter_map(|i| match i {
            ModuleItem::Instance(x) => Some(x),
            _ => None,
        })
        .collect();
    assert_eq!(insts.len(), 3);
    assert_eq!(insts[0].module, "sub");
    assert_eq!(insts[0].name, "u0");
    assert_eq!(insts[0].params.len(), 1);
    assert!(insts[0].ports[1].expr.is_none(), "empty connection");
    assert_eq!(
        insts[0].ports[2].expr.as_ref().map(sx).as_deref(),
        Some("c"),
        "implicit .c connects c"
    );
    assert_eq!(insts[1].params[0].name, None);
    assert!(insts[2].wildcard);
}

#[test]
fn gate_primitives_are_instances_marked_primitive() {
    let m = only_module("module m; and g1 (y, a, b); nor (z, a, b); endmodule\n");
    let insts: Vec<_> = m
        .items
        .iter()
        .filter_map(|i| match i {
            ModuleItem::Instance(x) => Some(x),
            _ => None,
        })
        .collect();
    assert!(insts.iter().all(|i| i.primitive));
    assert_eq!(insts[0].name, "g1");
    assert_eq!(insts[1].name, "");
}

#[test]
fn generate_for_if_and_case_are_kept() {
    let m = only_module(
        "module m #(parameter N = 2)(); genvar i;\n\
         generate for (i = 0; i < N; i = i + 1) begin : g sub u(); end endgenerate\n\
         if (N > 1) begin : big sub s(); end else begin : small end\n\
         case (N) 2: sub c(); default: ; endcase\n\
         endmodule\n",
    );
    let gens: Vec<_> = m
        .items
        .iter()
        .filter_map(|i| match i {
            ModuleItem::Generate(g) => Some(g),
            _ => None,
        })
        .collect();
    assert_eq!(gens.len(), 3, "the region and the two constructs");
    let GenKind::Block(inner) = &gens[0].kind else {
        panic!("region")
    };
    let ModuleItem::Generate(g) = &inner[0] else {
        panic!("for loop inside the region")
    };
    let GenKind::For { var, body, .. } = &g.kind else {
        panic!("for loop")
    };
    assert_eq!(var, "i");
    assert!(matches!(&body[0], ModuleItem::Instance(_)));
    assert!(matches!(gens[1].kind, GenKind::If { .. }));
    assert!(matches!(gens[2].kind, GenKind::Case { .. }));
}

#[test]
fn tasks_and_functions_with_arguments_locals_and_return_type() {
    let m = only_module(
        "module m;\n\
         function automatic [7:0] f(input [7:0] a, input b);\n integer k; begin f = a; end endfunction\n\
         task t(input x); begin end endtask\n\
         endmodule\n",
    );
    let routines: Vec<_> = m
        .items
        .iter()
        .filter_map(|i| match i {
            ModuleItem::Routine(r) => Some(r),
            _ => None,
        })
        .collect();
    assert_eq!(routines.len(), 2);
    assert_eq!(routines[0].kind, RoutineKind::Function);
    assert!(routines[0].automatic);
    assert_eq!(routines[0].args.len(), 2);
    assert_eq!(routines[0].decls.len(), 1);
    assert_eq!(routines[1].kind, RoutineKind::Task);
}

#[test]
fn typedef_vector_and_enum_are_kept() {
    let u = parse(
        "typedef logic [7:0] byte_t;\ntypedef enum logic [1:0] {IDLE, RUN = 2, DONE} st_t;\nmodule m; st_t s; byte_t b; endmodule\n",
    );
    let typedefs: Vec<_> = u
        .items
        .iter()
        .filter_map(|i| match i {
            Item::Typedef(t) => Some(t),
            _ => None,
        })
        .collect();
    assert_eq!(typedefs.len(), 2);
    assert!(matches!(&typedefs[1].kind, TypedefKind::Enum(v) if v == &["IDLE", "RUN", "DONE"]));
    // Uses of typedef names are declarations, not instances.
    let m = modules(&u)[0];
    assert!(matches!(&m.items[0], ModuleItem::Decl(d) if d.kind == DataKind::Named("st_t".into())));
}

#[test]
fn struct_and_union_typedefs_are_rejected_with_a_precise_construct() {
    let d = diags("typedef struct packed { logic a; } pkt_t;\nmodule m; endmodule\n");
    assert!(
        d.iter()
            .any(|x| x.construct == "typedef struct" && x.line == 1),
        "{d:?}"
    );
    let d = diags("module m; typedef union packed { logic a; } u_t; endmodule\n");
    assert!(d.iter().any(|x| x.construct == "typedef union"), "{d:?}");
}

#[test]
fn unsupported_top_level_constructs_are_named_and_parsing_continues() {
    let src = "package p; endpackage\n\
               interface i; endinterface\n\
               class c; endclass\n\
               config cfg; endconfig\n\
               module after_all; endmodule\n";
    let p = parse_text("t.sv", src, &Options::default());
    let constructs: Vec<&str> = p.diagnostics.iter().map(|d| d.construct.as_str()).collect();
    assert_eq!(constructs, vec!["package", "interface", "class", "config"]);
    assert_eq!(
        modules(&p.units[0]).len(),
        1,
        "the module after the skipped blocks is kept"
    );
}

#[test]
fn module_level_unsupported_constructs_are_named() {
    let src = "module m;\n\
               covergroup cg; endgroup\n\
               bind top2 chk c1 (.x(y));\n\
               assert property (@(posedge clk) a);\n\
               wire ok;\n\
               endmodule\n";
    let d = diags(src);
    let c: Vec<&str> = d.iter().map(|x| x.construct.as_str()).collect();
    assert!(c.contains(&"covergroup"), "{c:?}");
    assert!(c.contains(&"bind"), "{c:?}");
    assert!(c.contains(&"assertion"), "{c:?}");
}

#[test]
fn dpi_imports_and_package_imports() {
    let d = diags("module m; import \"DPI-C\" function int f(input int a); endmodule\n");
    assert!(d.iter().any(|x| x.construct == "DPI import"), "{d:?}");
    let m = only_module("module m import pkg::*; ; import pkg::thing; endmodule\n");
    let imports: Vec<_> = m
        .items
        .iter()
        .filter_map(|i| match i {
            ModuleItem::Import(x) => Some(x),
            _ => None,
        })
        .collect();
    assert_eq!(imports.len(), 2);
    assert!(imports[0].item.is_none(), "wildcard");
    assert_eq!(imports[1].item.as_deref(), Some("thing"));
}

#[test]
fn attributes_are_skipped_wherever_they_appear() {
    let m = only_module(
        "(* keep *) module m (\n (* X_INTERFACE_INFO = \"a\" *) (* X_INTERFACE_PARAMETER = \"b\" *) input clk,\n (* mark *) output q);\n\
         (* foo *) wire w; always @* begin (* bar *) q = clk; end endmodule\n",
    );
    assert_eq!(m.ports.len(), 2);
    assert!(
        m.items
            .iter()
            .any(|i| matches!(i, ModuleItem::Decl(d) if d.name == "w"))
    );
}

#[test]
fn elaboration_tasks_are_accepted_without_a_diagnostic() {
    let m = only_module("module m; initial begin end $error(\"msg\"); endmodule\n");
    assert!(!m.items.is_empty());
}

#[test]
fn escaped_identifiers_and_hierarchical_names_in_expressions() {
    assert_eq!(
        rhs("\\weird.name + u_core.pc"),
        "(+ \\weird.name u_core.pc)"
    );
}

#[test]
fn recovery_after_a_broken_statement_keeps_the_next_module() {
    let src = "module bad; always @* begin a = ; c = 1; end endmodule\n\
               module good; wire ok; endmodule\n";
    let p = parse_text("t.sv", src, &Options::default());
    assert!(!p.diagnostics.is_empty());
    let names: Vec<&str> = modules(&p.units[0])
        .iter()
        .map(|m| m.name.as_str())
        .collect();
    assert!(names.contains(&"good"), "{names:?}");
}

#[test]
fn a_missing_endmodule_is_reported_once_at_end_of_file() {
    let d = diags("module m; wire a;\n");
    assert_eq!(d.len(), 1, "{d:?}");
    assert!(d[0].message.contains("endmodule"));
}

#[test]
fn positions_point_at_the_declaration_name() {
    let m = only_module("module m;\n  wire  foo;\nendmodule\n");
    let ModuleItem::Decl(d) = &m.items[0] else {
        panic!()
    };
    assert_eq!((d.pos.line, d.pos.col), (2, 9));
    assert_eq!(d.pos.file, "t.sv");
}
