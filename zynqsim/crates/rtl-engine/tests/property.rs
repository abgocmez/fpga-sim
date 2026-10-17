//! The expression evaluator against an independent Rust reference on random operands:
//! add, subtract, multiply, shifts, comparisons, and divide and modulo (divisor
//! non-zero), at widths 1 to 64 for both signednesses, and a few wider widths.
//!
//! The reference works on `i128`/`u128` directly, with the width's mask applied by
//! hand; the engine goes through the parser, the elaborator and the scheduler.

mod common;

use common::build;

/// Seeded xorshift64 (deterministic across runs and platforms).
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
    fn wide(&mut self) -> u128 {
        ((self.next() as u128) << 64) | self.next() as u128
    }
}

fn mask(w: u32) -> u128 {
    if w >= 128 {
        u128::MAX
    } else {
        (1u128 << w) - 1
    }
}

fn sext(v: u128, w: u32) -> i128 {
    if w >= 128 {
        return v as i128;
    }
    if (v >> (w - 1)) & 1 == 1 {
        (v | !mask(w)) as i128
    } else {
        v as i128
    }
}

/// Names of the wires the module computes, in the order the reference fills them.
const OUTS: [&str; 11] = [
    "add", "sub", "mul", "shl", "shr", "sar", "lt", "le", "eq", "div", "rem",
];

fn module(w: u32, signed: bool) -> String {
    let s = if signed { "signed " } else { "" };
    format!(
        "module t;
        reg {s}[{hi}:0] a;
        reg {s}[{hi}:0] b;
        reg [6:0] sh;
        wire [{hi}:0] add = a + b;
        wire [{hi}:0] sub = a - b;
        wire [{hi}:0] mul = a * b;
        wire [{hi}:0] shl = a << sh;
        wire [{hi}:0] shr = a >> sh;
        wire [{hi}:0] sar = a >>> sh;
        wire lt = a < b;
        wire le = a <= b;
        wire eq = a == b;
        wire [{hi}:0] div = a / b;
        wire [{hi}:0] rem = a % b;
      endmodule",
        hi = w - 1
    )
}

/// The reference for one operand pair. Returns the values in `OUTS` order.
fn reference(w: u32, signed: bool, a: u128, b: u128, sh: u128) -> [u128; 11] {
    let m = mask(w);
    let (a, b) = (a & m, b & m);
    let shift_ok = sh < 128;
    let shn = if shift_ok { sh as u32 } else { 128 };
    if signed {
        let (sa, sb) = (sext(a, w), sext(b, w));
        let shl = if shn >= w { 0 } else { (a << shn) & m };
        let shr = if shn >= w { 0 } else { (a >> shn) & m };
        let sar = if shn >= w {
            if sa < 0 { m } else { 0 }
        } else {
            ((sa >> shn) as u128) & m
        };
        let div = (sa.wrapping_div(sb)) as u128 & m;
        let rem = (sa.wrapping_rem(sb)) as u128 & m;
        [
            a.wrapping_add(b) & m,
            a.wrapping_sub(b) & m,
            a.wrapping_mul(b) & m,
            shl,
            shr,
            sar,
            (sa < sb) as u128,
            (sa <= sb) as u128,
            (a == b) as u128,
            div,
            rem,
        ]
    } else {
        let shl = if shn >= w { 0 } else { (a << shn) & m };
        let shr = if shn >= w { 0 } else { (a >> shn) & m };
        [
            a.wrapping_add(b) & m,
            a.wrapping_sub(b) & m,
            a.wrapping_mul(b) & m,
            shl,
            shr,
            shr,
            (a < b) as u128,
            (a <= b) as u128,
            (a == b) as u128,
            a / b,
            a % b,
        ]
    }
}

fn run_width(w: u32, signed: bool, iterations: usize, seed: u64) {
    let mut s = build("t", &module(w, signed));
    let mut rng = Rng(seed);
    let m = mask(w);
    for i in 0..iterations {
        // Mix edge values with random ones.
        let pick = |rng: &mut Rng| -> u128 {
            match rng.next() % 8 {
                0 => 0,
                1 => 1,
                2 => m,
                3 => m >> 1,
                4 => (m >> 1) + 1,
                _ => rng.wide() & m,
            }
        };
        let a = pick(&mut rng);
        let mut b = pick(&mut rng);
        if b == 0 {
            b = 1; // division by zero is not in the reference
        }
        // `sh` is a 7-bit register: 0..=127 covers shifts past the width too.
        let sh = if i % 5 == 0 {
            127
        } else {
            (rng.next() % 128) as u128
        };
        s.set("a", a).unwrap();
        s.set("b", b).unwrap();
        s.set("sh", sh).unwrap();
        s.run_until(0).unwrap();
        let want = reference(w, signed, a, b, sh);
        for (k, name) in OUTS.iter().enumerate() {
            let got = s.get(name).unwrap();
            assert_eq!(
                got, want[k],
                "{name}: width {w}, signed {signed}, a={a:#x} b={b:#x} sh={sh}"
            );
        }
    }
}

#[test]
fn every_width_from_one_to_sixty_four_matches_the_reference_unsigned() {
    for w in 1..=64 {
        run_width(w, false, 120, 0x9E37_79B9_7F4A_7C15 ^ w as u64);
    }
}

#[test]
fn every_width_from_one_to_sixty_four_matches_the_reference_signed() {
    for w in 1..=64 {
        run_width(w, true, 120, 0x2545_F491_4F6C_DD1D ^ w as u64);
    }
}

#[test]
fn wide_widths_match_the_reference() {
    for (w, signed) in [
        (65, false),
        (65, true),
        (96, true),
        (127, false),
        (128, false),
        (128, true),
    ] {
        run_width(w, signed, 60, 0xDEAD_BEEF ^ w as u64);
    }
}
