//! Property tests of the VFP arithmetic against the independent reference in `vfp_ref`.
//! Each operation runs 10 000 seeded random cases (operands from special values, narrow
//! exponent windows and random bit patterns; rounding mode, FZ and DN drawn per case). The
//! machine executes the real instruction; the reference decides the expected result and flags.

mod vfp_ref;

use cpu_a9::{Cpu, StepResult};
use ps_bus::RamBus;
use vfp_ref::{
    Bin, DOUBLE, Exp, F, SINGLE, expect_binary, expect_convert, expect_from_int, expect_sqrt,
    expect_to_int, verify,
};

include!("data/vfp_words.inc.rs");

const CODE: u32 = 0x1000;
const CASES: usize = 10_000;

/// xorshift64*: a seeded, dependency-free generator.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
    fn bits(&mut self, n: u32) -> u64 {
        self.next() & ((1u64 << n) - 1)
    }
}

/// A random operand of format `f`: specials, a narrow exponent window, or random bits.
fn operand(rng: &mut Rng, f: F) -> u64 {
    let sign = rng.bits(1) << f.sign_shift();
    let frac = rng.bits(f.fbits);
    let bias = f.bias() as u64;
    match rng.below(20) {
        0 => sign,
        1 => sign | f.inf_mag(),
        2 => sign | f.inf_mag() | nan_frac(rng, f),
        3 => sign | (frac & f.frac_mask()) | 1, // subnormal (exponent field 0, nonzero fraction)
        4 => sign | (f.max_mag()),
        5 => sign | (1u64 << f.fbits),
        6..=10 => {
            let e = bias - 12 + rng.below(24);
            sign | (e << f.fbits) | frac
        }
        _ => sign | rng.bits(f.sign_shift()),
    }
}

/// A NaN fraction: random payload with the quiet bit random.
fn nan_frac(rng: &mut Rng, f: F) -> u64 {
    let payload = rng.bits(f.fbits - 1) | 1;
    let quiet = if rng.below(2) == 0 { f.quiet_bit() } else { 0 };
    (payload | quiet) & f.frac_mask()
}

/// Operand `b` near `a` (same magnitude, random small perturbation) about one time in five,
/// to exercise cancellation and the sticky path.
fn partner(rng: &mut Rng, f: F, a: u64) -> u64 {
    if rng.below(5) != 0 {
        return operand(rng, f);
    }
    let flip = rng.bits(1) << f.sign_shift();
    let delta = rng.below(9) as i64 - 4;
    let mag = (f.mag(a) as i64 + delta).max(0) as u64;
    (mag & ((1 << f.sign_shift()) - 1)) | flip
}

fn fpscr_bits(rm: u32, fz: bool, dn: bool) -> u32 {
    (rm << 22) | (u32::from(fz) << 24) | (u32::from(dn) << 25)
}

/// A program on the machine: `run` executes it from reset with the given registers and
/// returns r6, r7 and r8 (the result and FPSCR, see the programs below).
struct Prog {
    cpu: Cpu,
    bus: RamBus,
    n: usize,
}

impl Prog {
    fn new(parts: &[&[u32]]) -> Prog {
        let words: Vec<u32> = parts.concat();
        let mut bus = RamBus::new(0x1000, 0x1000);
        let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        bus.load(u64::from(CODE), &bytes);
        Prog {
            cpu: Cpu::new(0),
            bus,
            n: words.len(),
        }
    }

    fn run(&mut self, regs: &[(u8, u32)]) -> (u32, u32, u32) {
        self.cpu.reset(CODE);
        for &(r, v) in regs {
            self.cpu.set_reg(r, v);
        }
        for _ in 0..self.n {
            match self.cpu.step(&mut self.bus) {
                StepResult::Executed { event: None, .. } => {}
                other => panic!("unexpected {other:?} at {:#x}", self.cpu.pc()),
            }
        }
        (self.cpu.reg(6), self.cpu.reg(7), self.cpu.reg(8))
    }
}

fn wide(lo: u32, hi: u32) -> u64 {
    u64::from(lo) | (u64::from(hi) << 32)
}

/// Collects failures and reports the first few.
#[derive(Default)]
struct Report {
    failures: Vec<String>,
}

impl Report {
    fn check(&mut self, what: impl FnOnce() -> String, r: Result<(), String>) {
        if let Err(e) = r {
            if self.failures.len() < 8 {
                self.failures.push(format!("{}: {e}", what()));
            } else {
                self.failures.push(String::new());
            }
        }
    }
    fn finish(self, name: &str) {
        let total = self.failures.len();
        assert!(
            total == 0,
            "{name}: {total} failing cases, first ones:\n{}",
            self.failures
                .iter()
                .filter(|s| !s.is_empty())
                .cloned()
                .collect::<Vec<_>>()
                .join("\n")
        );
    }
}

/// Binary arithmetic (add, sub, mul, div) in one precision.
fn binary(op: Bin, f: F, seed: u64) {
    let (name, word_op) = match (op, f == DOUBLE) {
        (Bin::Add, false) => ("add.f32", PROG_OP_ADD_S),
        (Bin::Sub, false) => ("sub.f32", PROG_OP_SUB_S),
        (Bin::Mul, false) => ("mul.f32", PROG_OP_MUL_S),
        (Bin::Div, false) => ("div.f32", PROG_OP_DIV_S),
        (Bin::Add, true) => ("add.f64", PROG_OP_ADD_D),
        (Bin::Sub, true) => ("sub.f64", PROG_OP_SUB_D),
        (Bin::Mul, true) => ("mul.f64", PROG_OP_MUL_D),
        (Bin::Div, true) => ("div.f64", PROG_OP_DIV_D),
    };
    let dbl = f == DOUBLE;
    let mut prog = if dbl {
        Prog::new(&[
            PROG_ENABLE,
            PROG_SET_FPSCR,
            PROG_IN_D0,
            PROG_IN_D1,
            PROG_IN_D2,
            word_op,
            PROG_OUT_D2,
            PROG_OUT_FPSCR,
        ])
    } else {
        Prog::new(&[
            PROG_ENABLE,
            PROG_SET_FPSCR,
            PROG_IN_S0,
            PROG_IN_S1,
            PROG_IN_S2,
            word_op,
            PROG_OUT_S2,
            PROG_OUT_FPSCR,
        ])
    };
    let mut rng = Rng(seed);
    let mut rep = Report::default();
    for _ in 0..CASES {
        let a = operand(&mut rng, f);
        let b = partner(&mut rng, f, a);
        let rm = rng.below(4) as u32;
        let fz = rng.below(4) == 0;
        let dn = rng.below(2) == 0;
        let mode = fpscr_bits(rm, fz, dn);
        let regs = if dbl {
            vec![
                (0, a as u32),
                (1, (a >> 32) as u32),
                (2, b as u32),
                (3, (b >> 32) as u32),
                (9, mode),
            ]
        } else {
            vec![(0, a as u32), (1, b as u32), (9, mode)]
        };
        let (r6, r7, r8) = prog.run(&regs);
        let got = if dbl { wide(r6, r7) } else { u64::from(r6) };
        let exp = expect_binary(op, f, a, b, fz, dn, rm);
        rep.check(
            || format!("{name} a={a:#x} b={b:#x} rm={rm} fz={fz} dn={dn}"),
            verify(&exp, f, rm, fz, got, r8),
        );
    }
    rep.finish(name);
}

fn sqrt(f: F, seed: u64) {
    let dbl = f == DOUBLE;
    let mut prog = if dbl {
        Prog::new(&[
            PROG_ENABLE,
            PROG_SET_FPSCR,
            PROG_IN_D0,
            PROG_OP_SQRT_D,
            PROG_OUT_D2,
            PROG_OUT_FPSCR,
        ])
    } else {
        Prog::new(&[
            PROG_ENABLE,
            PROG_SET_FPSCR,
            PROG_IN_S0,
            PROG_OP_SQRT_S,
            PROG_OUT_S2,
            PROG_OUT_FPSCR,
        ])
    };
    let name = if dbl { "sqrt.f64" } else { "sqrt.f32" };
    let mut rng = Rng(seed);
    let mut rep = Report::default();
    for _ in 0..CASES {
        let a = operand(&mut rng, f);
        let rm = rng.below(4) as u32;
        let fz = rng.below(4) == 0;
        let dn = rng.below(2) == 0;
        let mode = fpscr_bits(rm, fz, dn);
        let regs = if dbl {
            vec![(0, a as u32), (1, (a >> 32) as u32), (9, mode)]
        } else {
            vec![(0, a as u32), (9, mode)]
        };
        let (r6, r7, r8) = prog.run(&regs);
        let got = if dbl { wide(r6, r7) } else { u64::from(r6) };
        let exp = expect_sqrt(f, a, fz, dn);
        rep.check(
            || format!("{name} a={a:#x} rm={rm} fz={fz} dn={dn}"),
            verify(&exp, f, rm, fz, got, r8),
        );
    }
    rep.finish(name);
}

fn widen_and_narrow(seed: u64) {
    let mut wide_prog = Prog::new(&[
        PROG_ENABLE,
        PROG_SET_FPSCR,
        PROG_IN_S0,
        PROG_OP_CVT_S2D,
        PROG_OUT_D2,
        PROG_OUT_FPSCR,
    ]);
    let mut narrow_prog = Prog::new(&[
        PROG_ENABLE,
        PROG_SET_FPSCR,
        PROG_IN_D0,
        PROG_OP_CVT_D2S,
        PROG_OUT_S2,
        PROG_OUT_FPSCR,
    ]);
    let mut rng = Rng(seed);
    let mut rep = Report::default();
    for _ in 0..CASES {
        let rm = rng.below(4) as u32;
        let fz = rng.below(4) == 0;
        let dn = rng.below(2) == 0;
        let mode = fpscr_bits(rm, fz, dn);
        // Widening: single to double (exact, so the result must be exact).
        let a = operand(&mut rng, SINGLE);
        let (r6, r7, r8) = wide_prog.run(&[(0, a as u32), (9, mode)]);
        let exp = expect_convert(SINGLE, DOUBLE, a, fz, dn);
        rep.check(
            || format!("cvt.f64.f32 a={a:#x} fz={fz} dn={dn}"),
            verify(&exp, DOUBLE, rm, fz, wide(r6, r7), r8),
        );
        // Narrowing: double to single with rounding.
        let a = operand(&mut rng, DOUBLE);
        let (r6, _, r8) = narrow_prog.run(&[(0, a as u32), (1, (a >> 32) as u32), (9, mode)]);
        let exp = expect_convert(DOUBLE, SINGLE, a, fz, dn);
        rep.check(
            || format!("cvt.f32.f64 a={a:#x} rm={rm} fz={fz} dn={dn}"),
            verify(&exp, SINGLE, rm, fz, u64::from(r6), r8),
        );
    }
    rep.finish("cvt f32<->f64");
}

fn int_to_float(f: F, signed: bool, seed: u64) {
    let dbl = f == DOUBLE;
    let word = match (dbl, signed) {
        (false, true) => PROG_OP_I2F_S,
        (false, false) => PROG_OP_U2F_S,
        (true, true) => PROG_OP_I2F_D,
        (true, false) => PROG_OP_U2F_D,
    };
    let out = if dbl { PROG_OUT_D2 } else { PROG_OUT_S2 };
    let mut prog = Prog::new(&[
        PROG_ENABLE,
        PROG_SET_FPSCR,
        PROG_IN_S0,
        word,
        out,
        PROG_OUT_FPSCR,
    ]);
    let name = format!(
        "{}->{}",
        if signed { "i32" } else { "u32" },
        if dbl { "f64" } else { "f32" }
    );
    let mut rng = Rng(seed);
    let mut rep = Report::default();
    for _ in 0..CASES {
        // Mix of small integers (exact) and full 32-bit values (rounded).
        let x = match rng.below(3) {
            0 => rng.bits(8) as u32,
            1 => rng.bits(24) as u32,
            _ => rng.bits(32) as u32,
        };
        let rm = rng.below(4) as u32;
        let mode = fpscr_bits(rm, false, false);
        let (r6, r7, r8) = prog.run(&[(0, x), (9, mode)]);
        let (sign, mag) = if signed {
            let v = x as i32;
            (u64::from(v < 0), u128::from(v.unsigned_abs()))
        } else {
            (0, u128::from(x))
        };
        let exp = expect_from_int(f, sign, mag);
        rep.check(
            || format!("{name} x={x:#x} rm={rm}"),
            verify(
                &exp,
                f,
                rm,
                false,
                if dbl { wide(r6, r7) } else { u64::from(r6) },
                r8,
            ),
        );
    }
    rep.finish(&name);
}

fn float_to_int(f: F, signed: bool, seed: u64) {
    let dbl = f == DOUBLE;
    let word = match (dbl, signed) {
        (false, true) => PROG_OP_F2I_R_S,
        (false, false) => PROG_OP_F2U_R_S,
        (true, true) => PROG_OP_F2I_R_D,
        (true, false) => PROG_OP_F2U_R_D,
    };
    let name = format!(
        "{}->{}",
        if dbl { "f64" } else { "f32" },
        if signed { "i32" } else { "u32" }
    );
    let mut prog = if dbl {
        Prog::new(&[
            PROG_ENABLE,
            PROG_SET_FPSCR,
            PROG_IN_D0,
            word,
            PROG_OUT_S2,
            PROG_OUT_FPSCR,
        ])
    } else {
        Prog::new(&[
            PROG_ENABLE,
            PROG_SET_FPSCR,
            PROG_IN_S0,
            word,
            PROG_OUT_S2,
            PROG_OUT_FPSCR,
        ])
    };
    let mut rng = Rng(seed);
    let mut rep = Report::default();
    for _ in 0..CASES {
        let a = operand(&mut rng, f);
        let rm = rng.below(4) as u32;
        let fz = rng.below(4) == 0;
        let mode = fpscr_bits(rm, fz, false);
        let regs = if dbl {
            vec![(0, a as u32), (1, (a >> 32) as u32), (9, mode)]
        } else {
            vec![(0, a as u32), (9, mode)]
        };
        let (r6, _, r8) = prog.run(&regs);
        let (bits, flags) = expect_to_int(f, a, rm, fz, signed);
        rep.check(
            || format!("{name} a={a:#x} rm={rm} fz={fz}"),
            if r6 == bits && (r8 & 0x9F) == flags {
                Ok(())
            } else {
                Err(format!(
                    "got {r6:#x} flags {:#x}, want {bits:#x} flags {flags:#x}",
                    r8 & 0x9F
                ))
            },
        );
    }
    rep.finish(&name);
}

#[test]
fn add_sub_mul_div_single_10000_cases() {
    binary(Bin::Add, SINGLE, 0x1111);
    binary(Bin::Sub, SINGLE, 0x2222);
    binary(Bin::Mul, SINGLE, 0x3333);
    binary(Bin::Div, SINGLE, 0x4444);
}

#[test]
fn add_sub_mul_div_double_10000_cases() {
    binary(Bin::Add, DOUBLE, 0x5555);
    binary(Bin::Sub, DOUBLE, 0x6666);
    binary(Bin::Mul, DOUBLE, 0x7777);
    binary(Bin::Div, DOUBLE, 0x8888);
}

#[test]
fn sqrt_both_precisions_10000_cases() {
    sqrt(SINGLE, 0x9999);
    sqrt(DOUBLE, 0xAAAA);
}

#[test]
fn conversions_between_formats_10000_cases() {
    widen_and_narrow(0xBBBB);
}

#[test]
fn integer_to_float_10000_cases() {
    int_to_float(SINGLE, true, 0xCCCC);
    int_to_float(SINGLE, false, 0xDDDD);
    int_to_float(DOUBLE, true, 0xEEEE);
    int_to_float(DOUBLE, false, 0xF0F0);
}

#[test]
fn float_to_integer_10000_cases() {
    float_to_int(SINGLE, true, 0x1234);
    float_to_int(SINGLE, false, 0x2345);
    float_to_int(DOUBLE, true, 0x3456);
    float_to_int(DOUBLE, false, 0x4567);
}

#[test]
fn the_reference_rejects_a_wrong_rounding() {
    // Sanity of the oracle itself: 1.0 + 2^-24 under round-to-nearest is 1.0 (ties to even),
    // and 1.0 + 2^-24 + 2^-40 is 1 + 2^-23 (above the tie).
    let one = 0x3F80_0000u64;
    let half = 0x3380_0000u64;
    let exp = expect_binary(Bin::Add, SINGLE, one, half, false, false, 0);
    assert!(verify(&exp, SINGLE, 0, false, 0x3F80_0000, 0x10).is_ok());
    assert!(verify(&exp, SINGLE, 0, false, 0x3F80_0001, 0x10).is_err());
    let _ = Exp::Exact { bits: 0, flags: 0 };
}
