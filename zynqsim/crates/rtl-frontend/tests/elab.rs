//! Constant evaluation and elaboration tests: operator widths and signedness,
//! `$clog2`, constant functions, parameter overrides, generate unrolling, widths,
//! and the checks for undefined modules, ports and identifiers.

use rtl_frontend::ast::{Item, ModuleItem};
use rtl_frontend::consteval::{Ctx, Val, clog2};
use rtl_frontend::{Design, InstanceNode, Options, elaborate_parsed, parse_text};
use std::collections::{BTreeMap, HashMap};

/// Evaluates a constant expression written as the default of a parameter.
fn konst(expr: &str) -> Result<Val, String> {
    konst_with(expr, "")
}

/// Like [`konst`], with extra items (typedefs and routines) in the module.
fn konst_with(expr: &str, items: &str) -> Result<Val, String> {
    let src = format!("module m #(parameter P = {expr}) (); {items} endmodule\n");
    let p = parse_text("k.sv", &src, &Options::default());
    assert!(p.diagnostics.is_empty(), "{:?}", p.diagnostics);
    let Item::Module(m) = &p.units[0].items[0] else {
        panic!("module")
    };
    let mut funcs = BTreeMap::new();
    for it in &m.items {
        if let ModuleItem::Routine(r) = it {
            funcs.insert(r.name.clone(), r.clone());
        }
    }
    let value = m.params[0].value.clone().expect("value");
    Ctx::new(HashMap::new(), &funcs).eval(&value)
}

fn v(bits: u128, width: u32, signed: bool) -> Val {
    Val::new(bits, width, signed)
}

#[test]
fn arithmetic_wraps_at_the_result_width() {
    // Sized operands: 8-bit result truncates.
    assert_eq!(konst("8'd200 + 8'd100").unwrap(), v(44, 8, false));
    // Unsized decimals are 32-bit signed.
    assert_eq!(konst("7 / 2").unwrap(), v(3, 32, true));
    assert_eq!(
        konst("-7 / 2").unwrap().as_i128(),
        -3,
        "signed division truncates toward zero"
    );
    assert_eq!(konst("-7 % 3").unwrap().as_i128(), -1);
}

#[test]
fn mixed_signedness_makes_the_operation_unsigned() {
    // 4'sb1111 is -1; compared with an unsigned 4-bit value it is 15.
    assert_eq!(
        konst("4'sb1111 < 4'd0").unwrap().bits,
        0,
        "-1 < 0 is false only when signed"
    );
    assert_eq!(
        konst("4'sb1111 == 4'd15").unwrap().bits,
        1,
        "mixed compare is unsigned"
    );
    assert_eq!(konst("-4'sd1 < 4'sd0").unwrap().bits, 1, "signed compare");
}

#[test]
fn shifts_keep_the_left_operand_width_and_signedness() {
    assert_eq!(
        konst("8'd1 << 9").unwrap(),
        v(0, 8, false),
        "shift past the width clears the value"
    );
    assert_eq!(konst("8'b1000_0000 >> 7").unwrap(), v(1, 8, false));
    assert_eq!(
        konst("-8'sd16 >>> 2").unwrap().as_i128(),
        -4,
        "arithmetic shift of a signed value"
    );
    assert_eq!(
        konst("8'hf0 >>> 4").unwrap().bits,
        0x0f,
        "logical shift of an unsigned value"
    );
}

#[test]
fn bitwise_and_reduction_operators() {
    assert_eq!(konst("4'b1100 & 4'b1010").unwrap(), v(0b1000, 4, false));
    assert_eq!(konst("4'b1100 ^ 4'b1010").unwrap(), v(0b0110, 4, false));
    assert_eq!(konst("~4'b1100").unwrap(), v(0b0011, 4, false));
    assert_eq!(konst("&4'b1111").unwrap(), v(1, 1, false));
    assert_eq!(konst("~&4'b1111").unwrap(), v(0, 1, false));
    assert_eq!(konst("^4'b0111").unwrap(), v(1, 1, false));
    assert_eq!(konst("~^4'b0111").unwrap(), v(0, 1, false));
}

#[test]
fn concatenation_replication_and_selects() {
    assert_eq!(konst("{4'hA, 4'h5}").unwrap(), v(0xA5, 8, false));
    assert_eq!(konst("{3{2'b01}}").unwrap(), v(0b010101, 6, false));
    assert_eq!(konst("8'hA5[7:4]").unwrap(), v(0xA, 4, false));
    assert_eq!(konst("8'hA5[0]").unwrap(), v(1, 1, false));
    assert_eq!(konst("8'hA5[4 +: 4]").unwrap(), v(0xA, 4, false));
    assert_eq!(konst("8'hA5[7 -: 4]").unwrap(), v(0xA, 4, false));
}

#[test]
fn logical_and_ternary_and_power() {
    assert_eq!(konst("(3 > 2) && (1 == 0)").unwrap(), v(0, 1, false));
    assert_eq!(konst("(3 > 2) || (1 == 0)").unwrap(), v(1, 1, false));
    assert_eq!(konst("1 ? 5 : 9").unwrap().as_i128(), 5);
    assert_eq!(konst("2 ** 10").unwrap().as_i128(), 1024);
    assert!(konst("4 / 0").unwrap_err().contains("division by zero"));
    assert!(
        konst("2 ** (0 - 1)")
            .unwrap_err()
            .contains("negative exponent")
    );
}

#[test]
fn x_and_z_and_unknown_names_are_errors_in_constants() {
    assert!(konst("4'b1x00").unwrap_err().contains("x or z"));
    assert!(
        konst("NOPE + 1")
            .unwrap_err()
            .contains("`NOPE` is not a constant")
    );
}

#[test]
fn clog2_values() {
    assert_eq!(clog2(0), 0);
    assert_eq!(clog2(1), 0);
    assert_eq!(clog2(2), 1);
    assert_eq!(clog2(3), 2);
    assert_eq!(clog2(16), 4);
    assert_eq!(clog2(17), 5);
    assert_eq!(konst("$clog2(1000)").unwrap().as_i128(), 10);
}

#[test]
fn constant_functions_with_loops_and_returns() {
    let items = "function integer sum_to(input integer n); integer k; begin \
                 sum_to = 0; for (k = 1; k <= n; k = k + 1) sum_to = sum_to + k; end endfunction";
    assert_eq!(konst_with("sum_to(10)", items).unwrap().as_i128(), 55);
    let items = "function integer pick(input integer a); begin if (a > 2) return 7; else return 1; end endfunction";
    assert_eq!(konst_with("pick(3)", items).unwrap().as_i128(), 7);
    assert_eq!(konst_with("pick(1)", items).unwrap().as_i128(), 1);
}

#[test]
fn runaway_constant_functions_stop_at_the_step_limit() {
    let items =
        "function integer spin(input integer a); begin while (1) a = a + 1; end endfunction";
    let e = konst_with("spin(0)", items).unwrap_err();
    assert!(e.contains("step limit"), "{e}");
}

fn elab(top: &str, src: &str) -> Design {
    let p = parse_text("e.sv", src, &Options::default());
    elaborate_parsed(top, &p)
}

fn kinds(d: &Design) -> Vec<String> {
    d.diagnostics
        .iter()
        .map(|x| format!("{}: {}", x.construct, x.message))
        .collect()
}

fn find<'a>(n: &'a InstanceNode, path: &str) -> Option<&'a InstanceNode> {
    if n.path == path {
        return Some(n);
    }
    n.children.iter().find_map(|c| find(c, path))
}

const CHILD: &str = "module sub #(parameter W = 4)(input [W-1:0] a, output y);\n\
                     reg [7:0] mem [0:15];\n\
                     endmodule\n";

#[test]
fn parameter_override_named_and_positional() {
    let src = format!(
        "{CHILD}module top; wire [15:0] a; wire y1, y2;\n\
         sub #(.W(16)) u_named (.a(a), .y(y1));\n\
         sub #(8) u_pos (.a(a[7:0]), .y(y2));\n\
         endmodule\n"
    );
    let d = elab("top", &src);
    assert!(d.diagnostics.is_empty(), "{:?}", kinds(&d));
    let top = d.hierarchy.as_ref().expect("hierarchy");
    let named = find(top, "top.u_named").expect("named instance");
    assert_eq!(named.params[0].value.as_deref(), Some("16"));
    assert_eq!(
        named.ports[0].width,
        Some(16),
        "port width follows the override"
    );
    let pos = find(top, "top.u_pos").expect("positional instance");
    assert_eq!(pos.ports[0].width, Some(8));
    assert_eq!(named.nets[0].depth, Some(16), "unpacked array depth");
    assert_eq!(named.nets[0].width, Some(8));
}

#[test]
fn generate_for_unrolls_into_indexed_instances() {
    let src = format!(
        "{CHILD}module top; genvar i;\n\
         generate for (i = 0; i < 3; i = i + 1) begin : g\n\
           sub #(.W(i + 1)) u (.a(), .y());\n\
         end endgenerate\n\
         endmodule\n"
    );
    let d = elab("top", &src);
    assert!(d.diagnostics.is_empty(), "{:?}", kinds(&d));
    let top = d.hierarchy.as_ref().expect("hierarchy");
    let names: Vec<&str> = top.children.iter().map(|c| c.path.as_str()).collect();
    assert_eq!(names, vec!["top.g[0].u", "top.g[1].u", "top.g[2].u"]);
    assert_eq!(top.children[2].params[0].value.as_deref(), Some("3"));
}

#[test]
fn generate_if_and_case_choose_one_branch() {
    let src = format!(
        "{CHILD}module top #(parameter MODE = 2, parameter USE = 1);\n\
         if (USE) begin : yes sub a1 (.a(), .y()); end else begin : no sub a2 (.a(), .y()); end\n\
         case (MODE) 1: sub c1 (.a(), .y()); 2: sub c2 (.a(), .y()); default: sub c3 (.a(), .y()); endcase\n\
         endmodule\n"
    );
    let d = elab("top", &src);
    assert!(d.diagnostics.is_empty(), "{:?}", kinds(&d));
    let top = d.hierarchy.as_ref().expect("hierarchy");
    let names: Vec<&str> = top.children.iter().map(|c| c.path.as_str()).collect();
    assert_eq!(names, vec!["top.yes.a1", "top.c2"]);
}

#[test]
fn clog2_parameter_is_resolved() {
    let src = "module top #(parameter DEPTH = 16, parameter AW = $clog2(DEPTH)) (input [AW-1:0] addr);\n\
               endmodule\n";
    let d = elab("top", src);
    assert!(d.diagnostics.is_empty(), "{:?}", kinds(&d));
    let info = &d.modules[0];
    assert_eq!(info.params[1].value.as_deref(), Some("4"));
    assert_eq!(info.ports[0].width, Some(4));
}

#[test]
fn undefined_module_is_reported_at_the_instance() {
    let d = elab("top", "module top; nothere u0 (); endmodule\n");
    assert!(
        d.diagnostics
            .iter()
            .any(|x| x.construct == "instance" && x.message.contains("nothere")),
        "{:?}",
        kinds(&d)
    );
    assert_eq!(d.diagnostics[0].line, 1);
}

#[test]
fn undeclared_identifier_is_reported() {
    let d = elab(
        "top",
        "module top; wire a; assign a = b & 1'b1; endmodule\n",
    );
    assert!(
        d.diagnostics
            .iter()
            .any(|x| x.construct == "identifier" && x.message.contains("`b`")),
        "{:?}",
        kinds(&d)
    );
}

#[test]
fn implicit_nets_in_port_connections_are_accepted() {
    let src = format!("{CHILD}module top; sub u (.a(free), .y(out)); endmodule\n");
    let d = elab("top", &src);
    assert!(d.diagnostics.is_empty(), "{:?}", kinds(&d));
}

#[test]
fn unknown_named_port_and_too_many_positional_connections() {
    let src = format!("{CHILD}module top; wire a, y; sub u (.a(a), .zz(y)); endmodule\n");
    let d = elab("top", &src);
    assert!(
        d.diagnostics
            .iter()
            .any(|x| x.message.contains("no port `zz`")),
        "{:?}",
        kinds(&d)
    );
    let src = format!("{CHILD}module top; wire a, y; sub u (a, y, y); endmodule\n");
    let d = elab("top", &src);
    assert!(
        d.diagnostics
            .iter()
            .any(|x| x.message.contains("too many positional")),
        "{:?}",
        kinds(&d)
    );
}

#[test]
fn recursive_instantiation_is_stopped() {
    let d = elab("top", "module top; top u (); endmodule\n");
    assert!(
        d.diagnostics
            .iter()
            .any(|x| x.message.contains("nested too deeply")),
        "{:?}",
        kinds(&d)
    );
}

#[test]
fn unknown_top_module_is_reported() {
    let d = elab("missing", "module top; endmodule\n");
    assert!(d.hierarchy.is_none());
    assert!(
        d.diagnostics
            .iter()
            .any(|x| x.message.contains("top module `missing`")),
        "{:?}",
        kinds(&d)
    );
}

#[test]
fn duplicate_module_names_are_reported() {
    let d = elab("a", "module a; endmodule\nmodule a; endmodule\n");
    assert!(
        d.diagnostics
            .iter()
            .any(|x| x.construct == "duplicate module"),
        "{:?}",
        kinds(&d)
    );
}

#[test]
fn non_constant_generate_bound_is_reported_not_unrolled() {
    let src = "module top; wire [3:0] n; genvar i;\n\
               generate for (i = 0; i < n; i = i + 1) begin : g end endgenerate\n\
               endmodule\n";
    let d = elab("top", src);
    assert!(
        d.diagnostics.iter().any(|x| x.construct == "generate for"),
        "{:?}",
        kinds(&d)
    );
}

#[test]
fn widths_of_ports_and_declared_nets() {
    let src = "module top(input [7:0] a, output signed [15:0] b, input c);\n\
               integer n; logic [3:0] v; reg [1:0] m [0:3][0:1];\n\
               endmodule\n";
    let d = elab("top", src);
    assert!(d.diagnostics.is_empty(), "{:?}", kinds(&d));
    let top = d.hierarchy.as_ref().expect("hierarchy");
    let widths: Vec<Option<u32>> = top.ports.iter().map(|p| p.width).collect();
    assert_eq!(widths, vec![Some(8), Some(16), Some(1)]);
    assert_eq!(top.ports[0].dir.as_deref(), Some("input"));
    assert!(top.ports[1].signed);
    let n = top.nets.iter().find(|x| x.name == "n").expect("integer");
    assert_eq!(n.width, Some(32));
    let m = top.nets.iter().find(|x| x.name == "m").expect("memory");
    assert_eq!(m.depth, Some(8));
    assert_eq!(m.width, Some(2));
}

#[test]
fn parameter_overrides_do_not_touch_localparams() {
    let src = "module sub #(parameter W = 2) (); localparam L = W * 2; endmodule\n\
               module top; sub #(.W(5)) u (); endmodule\n";
    let d = elab("top", src);
    let top = d.hierarchy.as_ref().expect("hierarchy");
    assert_eq!(top.children[0].params[0].value.as_deref(), Some("5"));
    let info = d.modules.iter().find(|m| m.name == "sub").expect("sub");
    assert_eq!(info.params[0].value.as_deref(), Some("2"));
}

#[test]
fn module_summaries_list_every_module_with_its_ports() {
    let src = format!("{CHILD}module top(input clk); endmodule\n");
    let d = elab("top", &src);
    let names: Vec<&str> = d.modules.iter().map(|m| m.name.as_str()).collect();
    assert_eq!(names, vec!["sub", "top"]);
    let sub = &d.modules[0];
    assert_eq!(sub.ports.len(), 2);
    assert_eq!(sub.ports[0].width, Some(4), "default W = 4");
}

#[test]
fn enum_members_are_visible_in_the_module_and_across_units() {
    let src = "typedef enum logic [1:0] {A0, B0} e_t;\n\
               module top; wire x; assign x = (A0 == B0); endmodule\n";
    let d = elab("top", src);
    assert!(d.diagnostics.is_empty(), "{:?}", kinds(&d));
}
