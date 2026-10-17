//! Four-state semantics (0, 1, X, Z) of the engine.
//!
//! The operator tables below are written here from IEEE 1364-2005 (clause 5) as small
//! functions over the characters `0 1 x z`, independent of the engine's code. The
//! engine's results are compared with them for every operand combination.

mod common;

use common::build;
use rtl_engine::{Simulator, Value4};

// ---------------------------------------------------------------------------
// Reference tables (IEEE 1364-2005, 5.1). Z acts as X in every operator.
// ---------------------------------------------------------------------------

fn unknown(c: char) -> bool {
    c == 'x' || c == 'z'
}

/// `&`: 0 dominates; 1 & 1 = 1; otherwise X.
fn ref_and(a: char, b: char) -> char {
    if a == '0' || b == '0' {
        '0'
    } else if a == '1' && b == '1' {
        '1'
    } else {
        'x'
    }
}

/// `|`: 1 dominates; 0 | 0 = 0; otherwise X.
fn ref_or(a: char, b: char) -> char {
    if a == '1' || b == '1' {
        '1'
    } else if a == '0' && b == '0' {
        '0'
    } else {
        'x'
    }
}

/// `^`: X if either is unknown, else the exclusive or.
fn ref_xor(a: char, b: char) -> char {
    if unknown(a) || unknown(b) {
        'x'
    } else if a == b {
        '0'
    } else {
        '1'
    }
}

/// `~` on one bit.
fn ref_not(a: char) -> char {
    match a {
        '0' => '1',
        '1' => '0',
        _ => 'x',
    }
}

/// `==` on one bit: X when either side is unknown.
fn ref_eq(a: char, b: char) -> char {
    if unknown(a) || unknown(b) {
        'x'
    } else if a == b {
        '1'
    } else {
        '0'
    }
}

/// `===` on one bit: exact, Z and X included.
fn ref_case_eq(a: char, b: char) -> char {
    if a == b { '1' } else { '0' }
}

/// `?:` with a 1-bit condition: a known condition selects; an unknown one keeps a bit
/// where both branches agree on a known value, and gives X otherwise.
fn ref_cond(c: char, t: char, f: char) -> char {
    match c {
        '1' => t,
        '0' => f,
        _ if t == f && (t == '0' || t == '1') => t,
        _ if t == f && t == 'z' => 'z',
        _ => 'x',
    }
}

/// Resolution of the drivers of a wire (1364-2005, 4.8): Z does not drive, equal
/// strong values keep, a conflict or an X gives X, nothing driving gives Z.
fn ref_resolve(drivers: &[char]) -> char {
    let driving: Vec<char> = drivers.iter().copied().filter(|c| *c != 'z').collect();
    if driving.is_empty() {
        'z'
    } else if driving.iter().all(|c| *c == driving[0]) && driving[0] != 'x' {
        driving[0]
    } else {
        'x'
    }
}

fn lit(c: char) -> String {
    format!("1'b{c}")
}

/// Evaluates each expression into a 32-bit wire `r<i>` and returns the values.
fn eval(exprs: &[String]) -> Vec<Value4> {
    let mut body = String::new();
    for (i, e) in exprs.iter().enumerate() {
        body.push_str(&format!("wire [31:0] r{i} = {e};\n"));
    }
    let mut s = build("t", &format!("module t;\n{body}endmodule"));
    s.run_until(0).unwrap();
    (0..exprs.len())
        .map(|i| s.get4(&format!("r{i}")).expect("result wire"))
        .collect()
}

/// The low bit of each result, as a character.
fn low(v: &[Value4]) -> Vec<char> {
    v.iter().map(|x| x.bit(0).ch()).collect()
}

const STATES: [char; 4] = ['0', '1', 'x', 'z'];

// ---------------------------------------------------------------------------
// Operator tables
// ---------------------------------------------------------------------------

#[test]
fn bitwise_operators_match_the_ieee_tables_for_every_pair() {
    type Table = fn(char, char) -> char;
    let ops: [(&str, Table); 4] = [
        ("&", ref_and),
        ("|", ref_or),
        ("^", ref_xor),
        ("~^", |a, b| ref_not(ref_xor(a, b))),
    ];
    for (op, table) in ops {
        let mut exprs = Vec::new();
        let mut want = Vec::new();
        for a in STATES {
            for b in STATES {
                exprs.push(format!("{} {op} {}", lit(a), lit(b)));
                want.push(table(a, b));
            }
        }
        assert_eq!(low(&eval(&exprs)), want, "operator {op}");
    }
}

#[test]
fn unary_and_logical_operators_match_the_ieee_tables() {
    let mut exprs = Vec::new();
    let mut want = Vec::new();
    for a in STATES {
        exprs.push(format!("~{}", lit(a)));
        want.push(ref_not(a));
        // `!` on one bit: X when unknown.
        exprs.push(format!("!{}", lit(a)));
        want.push(ref_not(a));
    }
    for a in STATES {
        for b in STATES {
            // `&&`: a known 0 operand gives 0; both known 1 gives 1; otherwise X.
            exprs.push(format!("{} && {}", lit(a), lit(b)));
            want.push(ref_and(a, b));
            // `||`: a known 1 operand gives 1; both known 0 gives 0; otherwise X.
            exprs.push(format!("{} || {}", lit(a), lit(b)));
            want.push(ref_or(a, b));
        }
    }
    assert_eq!(low(&eval(&exprs)), want);
}

#[test]
fn logical_operators_short_circuit_the_unknown() {
    // x && 0 = 0, x || 1 = 1, x && 1 = x, x || 0 = x, and vectors use their truth value.
    let v = eval(&[
        "1'bx && 1'b0".into(),
        "1'bx || 1'b1".into(),
        "1'bx && 1'b1".into(),
        "1'bx || 1'b0".into(),
        "2'b10 && 2'bx0".into(),
        "2'b0x || 2'b00".into(),
    ]);
    // `2'b10 && 2'bx0`: the first is true, the second is unknown, so the result is x.
    assert_eq!(low(&v), vec!['0', '1', 'x', 'x', 'x', 'x']);
}

#[test]
fn reduction_operators_match_the_folded_tables() {
    // Two-bit vectors of every state pair; the reference folds the bit tables.
    let mut exprs = Vec::new();
    let mut want = Vec::new();
    for hi in STATES {
        for lo in STATES {
            let vec = format!("2'b{hi}{lo}");
            exprs.push(format!("&{vec}"));
            want.push(ref_and(hi, lo));
            exprs.push(format!("~&{vec}"));
            want.push(ref_not(ref_and(hi, lo)));
            exprs.push(format!("|{vec}"));
            want.push(ref_or(hi, lo));
            exprs.push(format!("~|{vec}"));
            want.push(ref_not(ref_or(hi, lo)));
            exprs.push(format!("^{vec}"));
            want.push(ref_xor(hi, lo));
            exprs.push(format!("~^{vec}"));
            want.push(ref_not(ref_xor(hi, lo)));
        }
    }
    assert_eq!(low(&eval(&exprs)), want);
}

#[test]
fn equality_is_x_on_unknown_and_case_equality_is_exact() {
    let mut exprs = Vec::new();
    let mut want = Vec::new();
    for a in STATES {
        for b in STATES {
            exprs.push(format!("{} == {}", lit(a), lit(b)));
            want.push(ref_eq(a, b));
            exprs.push(format!("{} != {}", lit(a), lit(b)));
            want.push(ref_not(ref_eq(a, b)));
            exprs.push(format!("{} === {}", lit(a), lit(b)));
            want.push(ref_case_eq(a, b));
            exprs.push(format!("{} !== {}", lit(a), lit(b)));
            want.push(ref_not(ref_case_eq(a, b)));
        }
    }
    assert_eq!(low(&eval(&exprs)), want);
    // Vectors: any unknown bit makes `==` unknown, even when a known bit differs.
    let v = eval(&[
        "4'b1x00 == 4'b0100".into(),
        "4'b10x0 === 4'b10x0".into(),
        "4'b10x0 === 4'b1000".into(),
        "4'b1z00 !== 4'b1x00".into(),
    ]);
    assert_eq!(low(&v), vec!['x', '1', '0', '1']);
}

#[test]
fn relational_operators_are_x_on_unknown() {
    let v = eval(&[
        "2'b01 < 2'b10".into(),
        "2'b10 < 2'b01".into(),
        "2'bx1 < 2'b10".into(),
        "1'bz >= 1'b0".into(),
    ]);
    assert_eq!(low(&v), vec!['1', '0', 'x', 'x']);
}

#[test]
fn arithmetic_is_all_x_on_any_unknown_bit_and_on_division_by_zero() {
    let v = eval(&[
        "4'b10x0 + 4'b0001".into(),
        "4'd9 - 4'bz".into(),
        "4'd3 * 4'bx".into(),
        "4'd9 / 4'd0".into(),
        "4'd9 % 4'd0".into(),
        "-4'bx001".into(),
        "4'd9 / 4'd2".into(),
    ]);
    for (i, r) in v.iter().enumerate().take(6) {
        for b in 0..4 {
            assert_eq!(r.bit(b).ch(), 'x', "expression {i}, bit {b}");
        }
    }
    assert_eq!(v[6].to_bin_string(4), "0100", "9 / 2 is 4");
}

#[test]
fn shifts_with_unknown_amounts_and_bits() {
    let v = eval(&[
        "4'b1010 << 2'bx0".into(),
        "4'b1010 >> 1'bz".into(),
        "4'b10x0 >> 1".into(),
        "4'b1010 << 1".into(),
        "4'sb1x00 >>> 1".into(),
        "4'sb1000 >>> 1".into(),
        "4'b0110 >>> 1".into(),
    ]);
    assert_eq!(v[0].to_bin_string(4), "xxxx");
    assert_eq!(v[1].to_bin_string(4), "xxxx");
    assert_eq!(v[2].to_bin_string(4), "010x");
    assert_eq!(v[3].to_bin_string(4), "0100");
    assert_eq!(v[4].to_bin_string(4), "11x0");
    assert_eq!(v[5].to_bin_string(4), "1100");
    assert_eq!(v[6].to_bin_string(4), "0011");
}

#[test]
fn conditional_with_unknown_condition_merges_the_branches() {
    let v = eval(&[
        "1'bx ? 4'b1100 : 4'b1010".into(),
        "1'bz ? 1'b1 : 1'b1".into(),
        "1'bx ? 1'b0 : 1'b1".into(),
        "1'b1 ? 4'bz1x0 : 4'b0000".into(),
    ]);
    assert_eq!(v[0].to_bin_string(4), "1xx0");
    assert_eq!(v[1].bit(0).ch(), '1');
    assert_eq!(v[2].bit(0).ch(), 'x');
    assert_eq!(v[3].to_bin_string(4), "z1x0");
}

#[test]
fn part_selects_and_concatenation_keep_z_and_x() {
    let v = eval(&["{2'bz1, 2'bx0}".into(), "{2{2'b1z}}".into()]);
    assert_eq!(v[0].to_bin_string(4), "z1x0");
    assert_eq!(v[1].to_bin_string(4), "1z1z");
    let src = "module t;
        wire [7:0] w = 8'b1z0x_1100;
        wire b3 = w[3];
        wire b6 = w[6];
        wire [1:0] p = w[5:4];
        wire outside = w[9];
        wire [2:0] idx_x_read = w[1'bx +: 3];
        endmodule";
    let mut s = build("t", src);
    s.run_until(0).unwrap();
    // w = 1 z 0 x 1 1 0 0 (bit 7 down to bit 0).
    assert_eq!(s.get4("b3").unwrap().bit(0).ch(), '1');
    assert_eq!(s.get4("b6").unwrap().bit(0).ch(), 'z');
    assert_eq!(s.get4("p").unwrap().to_bin_string(2), "0x");
    assert_eq!(
        s.get4("outside").unwrap().bit(0).ch(),
        'x',
        "out of range reads x"
    );
    assert_eq!(
        s.get4("idx_x_read").unwrap().to_bin_string(3),
        "xxx",
        "x index reads x"
    );
}

#[test]
fn literals_with_x_and_z_digits() {
    let v = eval(&[
        "8'hxF".into(),
        "4'bz0x1".into(),
        "'x".into(),
        "8'bz".into(),
        "4'b10?1".into(),
        "8'h0z".into(),
    ]);
    assert_eq!(v[0].to_bin_string(8), "xxxx1111");
    assert_eq!(v[1].to_bin_string(4), "z0x1");
    assert_eq!(v[2].to_bin_string(8), "xxxxxxxx");
    assert_eq!(v[3].to_bin_string(8), "zzzzzzzz");
    assert_eq!(v[4].to_bin_string(4), "10z1", "? is z in a literal");
    assert_eq!(v[5].to_bin_string(8), "0000zzzz");
}

// ---------------------------------------------------------------------------
// Nets, drivers and resolution
// ---------------------------------------------------------------------------

#[test]
fn two_drivers_of_a_tristate_wire_resolve_as_ieee() {
    // y is driven by two conditional assignments; the inputs are set by the test.
    let src = "module t(input ea, input eb, input a, input b, output y);
        assign y = ea ? a : 1'bz;
        assign y = eb ? b : 1'bz;
      endmodule";
    let mut s = build("t", src);
    let states = ['0', '1', 'x', 'z'];
    for ea in ['0', '1', 'x'] {
        for eb in ['0', '1', 'x'] {
            for a in states {
                for b in states {
                    for (n, c) in [("ea", ea), ("eb", eb), ("a", a), ("b", b)] {
                        s.set4(n, bit4(c)).unwrap();
                    }
                    s.run_until(0).unwrap();
                    let d1 = ref_cond(ea, a, 'z');
                    let d2 = ref_cond(eb, b, 'z');
                    let want = ref_resolve(&[d1, d2]);
                    let got = s.get4("y").unwrap().bit(0).ch();
                    assert_eq!(got, want, "ea={ea} eb={eb} a={a} b={b}");
                }
            }
        }
    }
}

fn bit4(c: char) -> Value4 {
    match c {
        '0' => Value4::known(0),
        '1' => Value4::known(1),
        'x' => Value4 {
            bits: 0,
            unknown: 1,
            z: 0,
        },
        _ => Value4 {
            bits: 0,
            unknown: 1,
            z: 1,
        },
    }
}

#[test]
fn an_undriven_net_is_z_and_a_variable_is_x() {
    let src = "module t;
        wire w;
        wire [3:0] wv;
        reg r;
        reg [3:0] rv;
        integer i;
        bit bb;
        endmodule";
    let mut s = build("t", src);
    s.run_until(0).unwrap();
    assert_eq!(s.get4("w").unwrap().bit(0).ch(), 'z');
    assert_eq!(s.get4("wv").unwrap().to_bin_string(4), "zzzz");
    assert_eq!(s.get4("r").unwrap().bit(0).ch(), 'x');
    assert_eq!(s.get4("rv").unwrap().to_bin_string(4), "xxxx");
    assert_eq!(s.get4("i").unwrap().to_bin_string(32), "x".repeat(32));
    assert_eq!(
        s.get4("bb").unwrap().to_bin_string(1),
        "0",
        "bit is 2-state"
    );
}

#[test]
fn a_bit_or_int_never_holds_x_or_z() {
    let src = "module t; bit b; int n; initial begin b = 1'bx; n = 4'bz; end endmodule";
    let mut s = build("t", src);
    s.run_until(0).unwrap();
    assert_eq!(s.get4("b").unwrap().to_bin_string(1), "0");
    assert!(s.get4("n").unwrap().is_known());
}

// ---------------------------------------------------------------------------
// Control flow: if, case, edges
// ---------------------------------------------------------------------------

#[test]
fn if_on_an_unknown_takes_the_else_branch() {
    let src = "module t;
        reg c; reg r;
        initial begin
          if (c) r = 1; else r = 0;
        end
      endmodule";
    let mut s = build("t", src);
    s.run_until(0).unwrap();
    assert_eq!(s.get4("r").unwrap().bit(0).ch(), '0');
}

#[test]
fn case_casez_and_casex_match_as_ieee() {
    let src = "module t(input [3:0] s, output reg [1:0] c, output reg [1:0] z, output reg [1:0] x);
        always @* begin
          case (s)
            4'b10x1: c = 1;
            default: c = 2;
          endcase
          casez (s)
            4'b1?01: z = 1;
            default: z = 2;
          endcase
          casex (s)
            4'b1x01: x = 1;
            default: x = 2;
          endcase
        end
      endmodule";
    let mut s = build("t", src);
    // (selector, case, casez, casex) by the IEEE rules:
    //  case: === (x matches only x); casez: z is a don't-care on either side;
    //  casex: x and z are don't-cares on either side.
    let vectors: [(&str, u128, u128, u128); 4] = [
        ("1001", 2, 1, 1),
        ("10x1", 1, 2, 1),
        ("1z01", 2, 1, 1),
        ("1101", 2, 1, 1),
    ];
    for (sel, want_case, want_casez, want_casex) in vectors {
        let v = parse_bits(sel);
        s.set4("s", v).unwrap();
        s.run_until(0).unwrap();
        assert_eq!(s.get("c"), Some(want_case), "case with {sel}");
        assert_eq!(s.get("z"), Some(want_casez), "casez with {sel}");
        assert_eq!(s.get("x"), Some(want_casex), "casex with {sel}");
    }
}

fn parse_bits(s: &str) -> Value4 {
    let mut v = Value4::default();
    for c in s.chars() {
        v.bits <<= 1;
        v.unknown <<= 1;
        v.z <<= 1;
        match c {
            '1' => v.bits |= 1,
            'x' => v.unknown |= 1,
            'z' => {
                v.unknown |= 1;
                v.z |= 1;
            }
            _ => {}
        }
    }
    v
}

/// Counts posedge (and negedge) events of `s` for a sequence of driven states.
fn count_edges(seq: &[char], edge: &str) -> u128 {
    let src = format!(
        "module t(input s);
           reg [7:0] n = 0;
           always @({edge} s) n = n + 1;
         endmodule"
    );
    let mut sim = build("t", &src);
    for c in seq {
        sim.set4("s", bit4(*c)).unwrap();
        sim.run_until(0).unwrap();
    }
    sim.get("n").unwrap()
}

#[test]
fn posedge_includes_zero_to_x_x_to_one_and_z_transitions() {
    // Transitions from the start (Z) and back: a posedge is 0->1, 0->x, 0->z, x->1, z->1.
    let seq = ['0', '1', 'x', '1', '0', 'x', 'z', '1', 'z', '0'];
    // Rises: 0->1, x->1, 0->x, z->1 = four (the first 0 is Z->0, a fall into 0).
    // The Z->0 fall and 1->x / 1->z / x->z / z->0 changes are not rises.
    assert_eq!(count_edges(&seq, "posedge"), 4);
    // A negedge is 1->0, 1->x, 1->z, x->0, z->0.
    let fall_seq = ['1', '0', 'x', '0', '1', 'z', '0', '1', 'x'];
    assert_eq!(count_edges(&fall_seq, "negedge"), 5);
}

#[test]
fn an_x_clock_edge_does_not_clock_a_flip_flop_without_a_rise() {
    // x->0 is a negedge, not a posedge; 1->x is a negedge.
    assert_eq!(count_edges(&['x', '0', '1', 'x', '0'], "posedge"), 1);
}

// ---------------------------------------------------------------------------
// Designs: counter without reset, initial blocks, VCD, formatting
// ---------------------------------------------------------------------------

const COUNTER: &str = "module counter(input clk, input rst, output reg [3:0] q);
    always @(posedge clk) if (rst) q <= 0; else q <= q + 1;
  endmodule";

fn pulse(s: &mut Simulator, clk: &str) {
    s.set(clk, 1).unwrap();
    s.run_until(0).unwrap();
    s.set(clk, 0).unwrap();
    s.run_until(0).unwrap();
}

#[test]
fn a_counter_without_reset_is_x_until_reset() {
    let mut s = build("counter", COUNTER);
    s.set("clk", 0).unwrap();
    s.set("rst", 0).unwrap();
    s.run_until(0).unwrap();
    // The classic: X + 1 is X, so the count stays X through every clock.
    for _ in 0..3 {
        pulse(&mut s, "clk");
        assert_eq!(s.get4("q").unwrap().to_bin_string(4), "xxxx");
    }
    s.set("rst", 1).unwrap();
    pulse(&mut s, "clk");
    assert_eq!(s.get4("q").unwrap().to_bin_string(4), "0000");
    s.set("rst", 0).unwrap();
    pulse(&mut s, "clk");
    pulse(&mut s, "clk");
    assert_eq!(s.get("q"), Some(2));
}

#[test]
fn a_counter_with_reset_on_the_first_clock_is_known() {
    let mut s = build("counter", COUNTER);
    s.set("clk", 0).unwrap();
    s.set("rst", 1).unwrap();
    s.run_until(0).unwrap();
    pulse(&mut s, "clk");
    assert!(s.get4("q").unwrap().is_known());
    assert_eq!(s.get("q"), Some(0));
}

#[test]
fn initial_blocks_set_values_at_time_zero_and_later() {
    let src = "module t;
        reg [3:0] v;
        initial begin
          v = 4'bx1x0;
          #5 v = 4'b0101;
        end
      endmodule";
    let mut s = build("t", src);
    s.run_until(0).unwrap();
    assert_eq!(s.get4("v").unwrap().to_bin_string(4), "x1x0");
    s.run_until(4_999).unwrap();
    assert_eq!(s.get4("v").unwrap().to_bin_string(4), "x1x0");
    s.run_until(5_000).unwrap();
    assert_eq!(s.get4("v").unwrap().to_bin_string(4), "0101");
}

#[test]
fn display_prints_x_and_z_digits() {
    let src = "module t;
        reg [3:0] v = 4'b10xz;
        reg [7:0] h = 8'hxF;
        reg [3:0] p = 4'b1x01;
        reg [7:0] zz = 8'bzzzz_zzzz;
        reg [3:0] mix = 4'bzx01;
        reg [3:0] full = 4'b1010;
        initial begin
          $display(\"%b|%h|%h|%h|%h|%h\", v, h, p, zz, mix, full);
          $display(\"%d|%d\", 4'bx, 4'bz);
          $display(\"%b\", 8'hxF);
        end
      endmodule";
    let mut s = build("t", src);
    s.run_until(0).unwrap();
    let log = s.log();
    // %h: a nibble that is all x prints x, a known nibble its digit, a partly unknown
    // nibble X (or Z when its unknown bits are only z).
    assert_eq!(log[0], "10xz|xf|X|zz|X|a");
    // %d pads to the width of the largest 4-bit decimal (2 digits), as Verilog does.
    assert_eq!(log[1], " x| z");
    assert_eq!(log[2], "xxxx1111");
}

#[test]
fn vcd_writes_x_and_z_values() {
    let mut s = build("counter", COUNTER);
    s.set("clk", 0).unwrap();
    s.set("rst", 0).unwrap();
    s.run_until(0).unwrap();
    s.set_recording(true);
    pulse(&mut s, "clk");
    let vcd = s.vcd();
    assert!(vcd.contains("bxxxx "), "the count is x:\n{vcd}");
    assert!(vcd.contains("$dumpvars"));
    let mut t = build("t", "module t; wire w; endmodule");
    t.set_recording(true);
    assert!(
        t.vcd().contains("\nz"),
        "an undriven net is z:\n{}",
        t.vcd()
    );
}

#[test]
fn get4_set4_and_the_two_state_view() {
    let mut s = build("counter", COUNTER);
    s.run_until(0).unwrap();
    // The 2-state accessor reads X as 0; the 4-state accessor keeps it.
    assert_eq!(s.get("q"), Some(0));
    assert!(!s.get4("q").unwrap().is_known());
    s.set4("rst", Value4::known(1)).unwrap();
    s.set("clk", 0).unwrap();
    s.run_until(0).unwrap();
    s.set4("clk", bit4('1')).unwrap();
    s.run_until(0).unwrap();
    assert_eq!(s.get4("q").unwrap().to_bin_string(4), "0000");
    // Setting an X on an input is seen by the design.
    s.set4("rst", bit4('x')).unwrap();
    s.set4("clk", bit4('0')).unwrap();
    s.run_until(0).unwrap();
    s.set4("clk", bit4('1')).unwrap();
    s.run_until(0).unwrap();
    // rst is x: `if (rst)` takes the else branch, so q <= q + 1 (0 + 1).
    assert_eq!(
        s.get4("q").unwrap().to_bin_string(4),
        "0001",
        "if (x) takes the else branch"
    );
}

#[test]
fn state_hash_and_snapshot_include_the_unknown_masks() {
    let mut a = build("counter", COUNTER);
    a.run_until(0).unwrap();
    let mut b = build("counter", COUNTER);
    b.run_until(0).unwrap();
    assert_eq!(a.state_hash(), b.state_hash());
    // A snapshot keeps the unknown q (its two-state view would read 0).
    let snap = a.snapshot();
    let mut e = build("counter", COUNTER);
    e.restore(&snap).unwrap();
    assert_eq!(e.get4("q").unwrap(), Value4::all_x(4));
    assert_eq!(e.state_hash(), a.state_hash());
    // A hash that looks only at known bits would make these two equal.
    let mut f = build("t", "module t; reg [3:0] v = 4'b0000; endmodule");
    let mut g = build("t", "module t; reg [3:0] v = 4'b00x0; endmodule");
    f.run_until(0).unwrap();
    g.run_until(0).unwrap();
    assert_ne!(f.state_hash(), g.state_hash());
}

// ---------------------------------------------------------------------------
// Properties: known inputs agree with 2-state arithmetic; unknown inputs never
// produce a wrong known bit
// ---------------------------------------------------------------------------

/// Small deterministic generator (xorshift) so the properties are reproducible.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

/// Module with inputs `a`, `b` (4 bits) and one output per operator.
const OPS_4BIT: &str = "module ops(input [3:0] a, input [3:0] b, input c,
    output [3:0] o_and, output [3:0] o_or, output [3:0] o_xor, output [3:0] o_xnor,
    output [3:0] o_not, output [3:0] o_add, output [3:0] o_sub, output [3:0] o_shl,
    output [3:0] o_shr, output o_eq, output o_lt, output [3:0] o_mux);
  assign o_and = a & b;
  assign o_or = a | b;
  assign o_xor = a ^ b;
  assign o_xnor = a ~^ b;
  assign o_not = ~a;
  assign o_add = a + b;
  assign o_sub = a - b;
  assign o_shl = a << 1;
  assign o_shr = b >> 2;
  assign o_eq = a == b;
  assign o_lt = a < b;
  assign o_mux = c ? a : b;
endmodule";

/// The 2-state reference of the outputs for known operands.
fn reference(a: u128, b: u128, c: u128) -> [(&'static str, u128); 12] {
    let m = 0xf;
    [
        ("o_and", a & b),
        ("o_or", a | b),
        ("o_xor", a ^ b),
        ("o_xnor", !(a ^ b) & m),
        ("o_not", !a & m),
        ("o_add", (a + b) & m),
        ("o_sub", a.wrapping_sub(b) & m),
        ("o_shl", (a << 1) & m),
        ("o_shr", b >> 2),
        ("o_eq", (a == b) as u128),
        ("o_lt", (a < b) as u128),
        ("o_mux", if c != 0 { a } else { b }),
    ]
}

#[test]
fn known_inputs_give_the_two_state_results() {
    let mut s = build("ops", OPS_4BIT);
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    for _ in 0..500 {
        let (a, b, c) = (
            rng.below(16) as u128,
            rng.below(16) as u128,
            rng.below(2) as u128,
        );
        s.set("a", a).unwrap();
        s.set("b", b).unwrap();
        s.set("c", c).unwrap();
        s.run_until(0).unwrap();
        for (name, want) in reference(a, b, c) {
            let got = s.get4(name).unwrap();
            assert!(
                got.is_known(),
                "{name} has x/z for known inputs a={a} b={b}"
            );
            let w = if name == "o_eq" || name == "o_lt" {
                1
            } else {
                4
            };
            assert_eq!(got.bits & ((1 << w) - 1), want, "{name} a={a} b={b} c={c}");
        }
    }
}

#[test]
fn unknown_inputs_never_give_a_wrong_known_bit() {
    // Pessimism is allowed (an X result is always sound); a known bit must equal its
    // value under every completion of the unknown input bits.
    let mut s = build("ops", OPS_4BIT);
    let mut rng = Rng(0x0123_4567_89ab_cdef);
    for _ in 0..400 {
        let (a4, b4) = (random_state4(&mut rng), random_state4(&mut rng));
        s.set4("a", a4).unwrap();
        s.set4("b", b4).unwrap();
        s.set("c", rng.below(2) as u128).unwrap();
        s.run_until(0).unwrap();
        let c = s.get("c").unwrap();
        for (name, _) in reference(0, 0, 0) {
            let got = s.get4(name).unwrap();
            let w = if name == "o_eq" || name == "o_lt" {
                1
            } else {
                4
            };
            for ca in completions(a4) {
                for cb in completions(b4) {
                    let r = reference(ca, cb, c);
                    let want = r.iter().find(|(n, _)| *n == name).unwrap().1;
                    for bit in 0..w {
                        if got.unknown >> bit & 1 == 0 {
                            let g = (got.bits >> bit) & 1;
                            let v = (want >> bit) & 1;
                            assert_eq!(
                                g, v,
                                "{name} bit {bit} is known {g} but is {v} for a={ca} b={cb}"
                            );
                        }
                    }
                }
            }
        }
    }
}

fn random_state4(rng: &mut Rng) -> Value4 {
    // Mostly known, with some x and z bits.
    let mut v = Value4::default();
    for i in 0..4 {
        match rng.below(6) {
            0 => v.unknown |= 1 << i,
            1 => {
                v.unknown |= 1 << i;
                v.z |= 1 << i;
            }
            2 | 3 => v.bits |= 1 << i,
            _ => {}
        }
    }
    v
}

/// Every known value a 4-bit state can take: each unknown bit is 0 or 1.
fn completions(v: Value4) -> Vec<u128> {
    let unk: Vec<u32> = (0..4).filter(|i| v.unknown >> i & 1 == 1).collect();
    (0..1u128 << unk.len())
        .map(|k| {
            let mut out = v.bits;
            for (j, bit) in unk.iter().enumerate() {
                if k >> j & 1 == 1 {
                    out |= 1 << bit;
                }
            }
            out
        })
        .collect()
}

#[test]
fn x_pessimism_is_documented_and_limited() {
    // Pessimistic (allowed, IEEE also gives x): arithmetic on any unknown bit, and `==`
    // with an unknown bit even when a known bit already differs.
    let v = eval(&[
        "4'bx000 & 4'b0000".into(),
        "4'b0x00 + 4'b0000".into(),
        "4'b1x00 == 4'b0100".into(),
    ]);
    assert_eq!(
        v[0].to_bin_string(4),
        "0000",
        "0 & anything is 0: not pessimistic"
    );
    assert_eq!(v[1].to_bin_string(4), "xxxx", "arithmetic is all x");
    assert_eq!(v[2].bit(0).ch(), 'x', "== is x whenever an operand has x");
}

#[test]
fn a_counter_with_x_clock_bits_does_not_invent_a_known_count() {
    // `q + 1` with an unknown q is unknown; the reset path still works afterwards.
    let mut s = build("counter", COUNTER);
    s.set("clk", 0).unwrap();
    s.set4("rst", bit4('x')).unwrap();
    s.run_until(0).unwrap();
    pulse(&mut s, "clk");
    assert_eq!(s.get4("q").unwrap().to_bin_string(4), "xxxx");
}

#[test]
fn resolution_reference_agrees_with_the_engine_for_three_drivers() {
    // The reference table itself: Z loses, equal values keep, conflicts give x.
    assert_eq!(ref_resolve(&['z', 'z']), 'z');
    assert_eq!(ref_resolve(&['z', '1']), '1');
    assert_eq!(ref_resolve(&['0', '0', 'z']), '0');
    assert_eq!(ref_resolve(&['0', '1']), 'x');
    assert_eq!(ref_resolve(&['x', 'z']), 'x');
    assert_eq!(ref_resolve(&['1', 'x', '1']), 'x');
    let src = "module t(input a, input b, input c, output y);
        assign y = a ? 1'b1 : 1'bz;
        assign y = b ? 1'b0 : 1'bz;
        assign y = c ? 1'b1 : 1'bz;
      endmodule";
    let mut s = build("t", src);
    for a in ['0', '1'] {
        for b in ['0', '1'] {
            for c in ['0', '1'] {
                s.set4("a", bit4(a)).unwrap();
                s.set4("b", bit4(b)).unwrap();
                s.set4("c", bit4(c)).unwrap();
                s.run_until(0).unwrap();
                let drv = |en: char, v: char| if en == '1' { v } else { 'z' };
                let want = ref_resolve(&[drv(a, '1'), drv(b, '0'), drv(c, '1')]);
                assert_eq!(s.get4("y").unwrap().bit(0).ch(), want, "a={a} b={b} c={c}");
            }
        }
    }
}
