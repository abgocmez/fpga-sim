//! Expression constructs against hand-computed values.

mod common;

use common::build;

/// Builds a module whose wires are the given expressions, reads them after time 0.
fn wires(decls: &str, exprs: &[(&str, &str)]) -> rtl_engine::Simulator {
    let mut body = String::new();
    for (name, e) in exprs {
        body.push_str(&format!("wire [127:0] {name} = {e};\n"));
    }
    let src = format!("module t;\n{decls}\n{body}endmodule\n");
    let mut s = build("t", &src);
    s.run_until(0).unwrap();
    s
}

#[test]
fn literals_are_sized_and_based() {
    let s = wires(
        "",
        &[
            ("a", "8'hA5"),
            ("b", "4'b1010"),
            ("c", "'d7"),
            ("d", "8'o17"),
        ],
    );
    assert_eq!(s.get("a"), Some(0xA5));
    assert_eq!(s.get("b"), Some(0b1010));
    assert_eq!(s.get("c"), Some(7));
    assert_eq!(s.get("d"), Some(0o17));
}

#[test]
fn concatenation_and_replication() {
    let s = wires("", &[("c", "{4'hA, 2'b01, 2'b11}"), ("r", "{2{4'h3}}")]);
    assert_eq!(s.get("c"), Some(0xA7));
    assert_eq!(s.get("r"), Some(0x33));
}

#[test]
fn part_and_bit_selects_including_indexed_forms() {
    let decls = "reg [7:0] v = 8'hC5;";
    let s = wires(
        decls,
        &[
            ("hi", "v[7:4]"),
            ("lo", "v[3:0]"),
            ("bit0", "v[0]"),
            ("up", "v[4 +: 4]"),
            ("down", "v[7 -: 4]"),
        ],
    );
    assert_eq!(s.get("hi"), Some(0xC));
    assert_eq!(s.get("lo"), Some(0x5));
    assert_eq!(s.get("bit0"), Some(1));
    assert_eq!(s.get("up"), Some(0xC));
    assert_eq!(s.get("down"), Some(0xC));
}

#[test]
fn ascending_declarations_map_indices_the_other_way() {
    // wire [0:7] v: v[0] is the most significant bit.
    let s = wires(
        "reg [0:7] v = 8'b1000_0001;",
        &[("first", "v[0]"), ("last", "v[7]")],
    );
    assert_eq!(s.get("first"), Some(1));
    assert_eq!(s.get("last"), Some(1));
    let s = wires("reg [0:7] v = 8'b1000_0000;", &[("last", "v[7]")]);
    assert_eq!(s.get("last"), Some(0));
}

#[test]
fn reductions_and_logical_operators() {
    let s = wires(
        "reg [3:0] a = 4'b1111; reg [7:0] b = 8'b0000_0111; reg [3:0] z = 0;",
        &[
            ("rand", "&a"),
            ("rxor", "^b"),
            ("ror", "|z"),
            ("land", "a && z"),
            ("lor", "a || z"),
            ("lnot", "!z"),
        ],
    );
    assert_eq!(s.get("rand"), Some(1));
    assert_eq!(s.get("rxor"), Some(1));
    assert_eq!(s.get("ror"), Some(0));
    assert_eq!(s.get("land"), Some(0));
    assert_eq!(s.get("lor"), Some(1));
    assert_eq!(s.get("lnot"), Some(1));
}

#[test]
fn ternary_selects_and_sizes_its_branches() {
    let s = wires("reg c = 1;", &[("y", "c ? 8'd200 : 8'd7")]);
    assert_eq!(s.get("y"), Some(200));
}

#[test]
fn context_width_keeps_the_carry_of_an_addition() {
    // The assignment to a 9-bit target makes the 8-bit addition 9 bits wide.
    let src = "module t; reg [7:0] a = 255; reg [7:0] b = 1; wire [8:0] s = a + b; endmodule";
    let mut s = build("t", src);
    s.run_until(0).unwrap();
    assert_eq!(s.get("s"), Some(256));
}

#[test]
fn comparison_operands_share_the_wider_width() {
    // Unsigned 8-bit against signed negative: the signed one is unsigned here.
    let src = "module t;
        reg [7:0] a = 8'd1;
        reg signed [7:0] b = -1;
        wire lt = a < b;
        wire mixed = $signed(a) < b;
      endmodule";
    let mut s = build("t", src);
    s.run_until(0).unwrap();
    assert_eq!(s.get("lt"), Some(1)); // 1 < 255
    assert_eq!(s.get("mixed"), Some(0)); // 1 < -1 is false
}

#[test]
fn mixed_signedness_makes_the_expression_unsigned() {
    let src = "module t;
        reg signed [7:0] s = -1;
        reg [7:0] u = 1;
        wire [15:0] y = s + u;
        wire [15:0] z = s + s;
      endmodule";
    let mut s = build("t", src);
    s.run_until(0).unwrap();
    assert_eq!(s.get("y"), Some(0x0100));
    assert_eq!(s.get("z"), Some(0xFFFE));
}

#[test]
fn arithmetic_right_shift_only_for_signed_left_operands() {
    let src = "module t;
        reg signed [3:0] s = -8;
        reg [3:0] u = 4'b1000;
        wire [3:0] sar = s >>> 1;
        wire [3:0] shr = s >> 1;
        wire [3:0] uar = u >>> 1;
      endmodule";
    let mut s = build("t", src);
    s.run_until(0).unwrap();
    assert_eq!(s.get("sar"), Some(0b1100));
    assert_eq!(s.get("shr"), Some(0b0100));
    assert_eq!(s.get("uar"), Some(0b0100));
}

#[test]
fn division_and_modulo_follow_the_sign_rules() {
    let src = "module t;
        reg signed [7:0] a = -7;
        reg signed [7:0] b = 2;
        wire signed [7:0] q = a / b;
        wire signed [7:0] r = a % b;
      endmodule";
    let mut s = build("t", src);
    s.run_until(0).unwrap();
    assert_eq!(s.get("q"), Some(0xFD)); // -3
    assert_eq!(s.get("r"), Some(0xFF)); // -1
}

#[test]
fn memories_read_and_write_words_by_index() {
    let src = "module t;
        reg [7:0] mem [0:3];
        reg [1:0] i = 2;
        wire [7:0] w = mem[i];
        initial begin mem[2] = 8'h5A; mem[0] = 8'h01; end
      endmodule";
    let mut s = build("t", src);
    s.run_until(0).unwrap();
    assert_eq!(s.get("w"), Some(0x5A));
    s.set("i", 0).unwrap();
    s.run_until(0).unwrap();
    assert_eq!(s.get("w"), Some(0x01));
    assert_eq!(s.get_word("mem", 2), Some(0x5A));
}

#[test]
fn a_memory_word_can_be_bit_selected() {
    let src = "module t;
        reg [7:0] mem [0:1];
        wire b = mem[1][3];
        initial mem[1] = 8'b0000_1000;
      endmodule";
    let mut s = build("t", src);
    s.run_until(0).unwrap();
    assert_eq!(s.get("b"), Some(1));
}

#[test]
fn constant_functions_and_clog2_in_widths() {
    let src = "module t #(parameter N = 17) ();
        localparam W = $clog2(N);
        reg [W-1:0] v = 5;
        wire [7:0] p = N * 2;
      endmodule";
    let mut s = build("t", src);
    s.run_until(0).unwrap();
    assert_eq!(s.width_of("v"), Some(5));
    assert_eq!(s.get("p"), Some(34));
}

#[test]
fn x_and_z_literals_read_as_zero() {
    let s = wires("", &[("a", "4'bx1z0"), ("b", "8'hzz")]);
    assert_eq!(s.get("a"), Some(0b0100)); // x->0, 1, z->0, 0
    assert_eq!(s.get("b"), Some(0));
}

#[test]
fn a_value_wider_than_128_bits_is_refused_by_name() {
    let src = "module t; reg [127:0] a; reg [127:0] b; wire [255:0] c = {a, b}; wire [255:0] d = c; wire [199:0] wide; endmodule";
    let d = common::diagnostics("t", src);
    assert!(d.iter().any(|m| m.contains("128")), "{d:?}");
}

#[test]
fn a_function_writing_a_memory_declared_after_it() {
    let src = "module t;
        function [7:0] mask(input [31:0] index);
            integer i;
            begin
                for (i = 0; i < 4; i = i + 1) begin
                    m[i] = 0;
                    m[i][i] = 1'b1;
                end
                mask = m[index];
            end
        endfunction
        reg [7:0] m [0:3];
        wire [7:0] w = mask(1);
      endmodule";
    let d = common::diagnostics("t", src);
    assert!(d.is_empty(), "{d:?}");
}

#[test]
fn a_bit_of_a_memory_word_can_be_assigned() {
    let src = "module t;
        reg [3:0] m [0:2];
        integer i;
        initial begin
            for (i = 0; i < 3; i = i + 1) begin
                m[i] = 0;
                m[i][i] = 1'b1;
            end
        end
      endmodule";
    let d = common::diagnostics("t", src);
    assert!(d.is_empty(), "{d:?}");
}
