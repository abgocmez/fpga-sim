//! Property tests: every data-processing opcode, flag-setting, against an independent reference
//! of the ARM ARM flag rules, on 10000 seeded random inputs per opcode.
//!
//! The reference uses i64 and u64 arithmetic directly (signed overflow as "the i64 result does
//! not fit in i32", carry as "the u64 sum exceeds u32"), which is a different route from the
//! interpreter's `AddWithCarry`.

use cpu_a9::Cpu;
use ps_bus::RamBus;

const CASES: usize = 10_000;
const CODE: u32 = 0x1000;

/// xorshift64*: a seeded, dependency-free PRNG.
struct Prng(u64);

impl Prng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn u32(&mut self) -> u32 {
        (self.next() >> 16) as u32
    }
    fn bit(&mut self) -> bool {
        self.next() & 1 != 0
    }
}

/// Flags as (N, Z, C, V).
type Flags = (bool, bool, bool, bool);

/// The reference result of a data-processing op: the value written (if any) and the flags
/// that S sets. `shifter_c` is the shifter carry-out, `c_in` the incoming C.
fn reference(
    op: u32,
    a: u32,
    sh: u32,
    shifter_c: bool,
    c_in: bool,
    v_in: bool,
) -> (Option<u32>, Flags) {
    let n_z = |r: u32| (r & 0x8000_0000 != 0, r == 0);
    let logical = |r: u32| {
        let (n, z) = n_z(r);
        (Some(r), (n, z, shifter_c, v_in))
    };
    let sum = |x: u32, y: u32, cin: u32, write: bool| {
        // unsigned carry: the u64 sum exceeds u32::MAX
        let u = u64::from(x) + u64::from(y) + u64::from(cin);
        let r = u as u32;
        // signed overflow: the exact i64 sum does not fit i32
        let s = i64::from(x as i32) + i64::from(y as i32) + i64::from(cin);
        let v = s != i64::from(r as i32);
        let (n, z) = n_z(r);
        (
            if write { Some(r) } else { None },
            (n, z, u > u64::from(u32::MAX), v),
        )
    };
    // a - b - (1 - cin) as a + !b + cin: borrow is (x < y + borrow), computed in u64.
    let diff = |x: u32, y: u32, cin: u32, write: bool| {
        let borrow = 1 - u64::from(cin);
        let no_borrow = u64::from(x) >= u64::from(y) + borrow;
        let r = (u64::from(x).wrapping_sub(u64::from(y)).wrapping_sub(borrow)) as u32;
        let s = i64::from(x as i32) - i64::from(y as i32) - borrow as i64;
        let v = s != i64::from(r as i32);
        let (n, z) = n_z(r);
        (if write { Some(r) } else { None }, (n, z, no_borrow, v))
    };
    match op {
        0 | 8 => {
            let r = a & sh;
            let (w, f) = logical(r);
            (if op == 0 { w } else { None }, f)
        }
        1 | 9 => {
            let r = a ^ sh;
            let (w, f) = logical(r);
            (if op == 1 { w } else { None }, f)
        }
        12 => logical(a | sh),
        13 => logical(sh),
        14 => logical(a & !sh),
        15 => logical(!sh),
        2 | 10 => diff(a, sh, 1, op == 2),
        3 => diff(sh, a, 1, true),
        4 | 11 => sum(a, sh, 0, op == 4),
        5 => sum(a, sh, u32::from(c_in), true),
        6 => diff(a, sh, u32::from(c_in), true),
        7 => diff(sh, a, u32::from(c_in), true),
        _ => unreachable!("opcode is 4 bits"),
    }
}

/// Runs one instruction `op{S} r0, r1, r2{, shift}` on the interpreter.
fn run_one(insn: u32, a: u32, b: u32, flags: Flags) -> (u32, Flags) {
    let mut bus = RamBus::new(u64::from(CODE), 0x100);
    bus.load(u64::from(CODE), &insn.to_le_bytes());
    let mut cpu = Cpu::new(0);
    cpu.reset(CODE);
    cpu.set_reg(1, a);
    cpu.set_reg(2, b);
    let (n, z, c, v) = flags;
    let cpsr = 0x10
        | (u32::from(n) << 31)
        | (u32::from(z) << 30)
        | (u32::from(c) << 29)
        | (u32::from(v) << 28);
    cpu.set_cpsr(cpsr);
    let res = cpu.step(&mut bus);
    assert!(
        matches!(res, cpu_a9::StepResult::Executed { event: None, .. }),
        "unexpected step result {res:?} for {insn:#010x}"
    );
    let c2 = cpu.cpsr();
    let f = (
        c2 >> 31 & 1 != 0,
        c2 >> 30 & 1 != 0,
        c2 >> 29 & 1 != 0,
        c2 >> 28 & 1 != 0,
    );
    (cpu.reg(0), f)
}

/// The data-processing word for `op` with S set, Rn = r1, Rd = r0, Rm = r2 and the given
/// shift (`imm5` and type).
fn dp_word(op: u32, imm5: u32, ty: u32) -> u32 {
    0xE000_0000 | (op << 21) | (1 << 20) | (1 << 16) | (imm5 << 7) | (ty << 5) | 2
}

fn check(op: u32, insn: u32, imm5: u32, ty: u32, rng: &mut Prng) {
    for case in 0..CASES {
        let a = rng.u32();
        let b = rng.u32();
        let flags = (rng.bit(), rng.bit(), rng.bit(), rng.bit());
        let (n, z, c, v) = flags;
        let _ = (n, z);
        // The reference needs the shifted operand and its carry.
        let (sh, shc) = shift_ref(b, ty, imm5, c);
        let (want, want_flags) = reference(op, a, sh, shc, c, v);
        let (got, got_flags) = run_one(insn, a, b, flags);
        if let Some(w) = want {
            assert_eq!(
                got, w,
                "op {op} case {case}: a={a:#x} b={b:#x} shift={ty}/{imm5} flags={flags:?}"
            );
        }
        assert_eq!(
            got_flags, want_flags,
            "op {op} case {case}: a={a:#x} b={b:#x} shift={ty}/{imm5} flags={flags:?} (N,Z,C,V)"
        );
    }
}

/// The reference barrel shifter: (value, carry out) for an immediate shift.
fn shift_ref(v: u32, ty: u32, imm5: u32, c_in: bool) -> (u32, bool) {
    match ty {
        0 if imm5 == 0 => (v, c_in),
        0 => (
            ((u64::from(v) << imm5) & 0xFFFF_FFFF) as u32,
            (u64::from(v) >> (32 - imm5)) & 1 == 1,
        ),
        1 => {
            let n = if imm5 == 0 { 32 } else { imm5 };
            if n == 32 {
                (0, v >> 31 == 1)
            } else {
                ((v >> n), (v >> (n - 1)) & 1 == 1)
            }
        }
        2 => {
            let n = if imm5 == 0 { 32 } else { imm5 };
            let s = i64::from(v as i32);
            let shifted = s >> n.min(63);
            (
                (shifted as u64 & 0xFFFF_FFFF) as u32,
                (s >> (n - 1).min(63)) & 1 == 1,
            )
        }
        _ => {
            if imm5 == 0 {
                ((u32::from(c_in) << 31) | (v >> 1), v & 1 == 1)
            } else {
                let r = v.rotate_right(imm5);
                (r, r >> 31 == 1)
            }
        }
    }
}

#[test]
fn every_opcode_matches_the_reference_with_lsl_0() {
    let mut rng = Prng(0x9E37_79B9_7F4A_7C15);
    for op in 0..16 {
        check(op, dp_word(op, 0, 0), 0, 0, &mut rng);
    }
}

#[test]
fn every_opcode_matches_the_reference_with_random_immediate_shifts() {
    let mut rng = Prng(0x0123_4567_89AB_CDEF);
    for op in 0..16 {
        for _ in 0..4 {
            let ty = (rng.next() % 4) as u32;
            let imm5 = (rng.next() % 32) as u32;
            // The immediate ROR #0 encodes RRX and LSR/ASR #0 mean 32; the reference covers them.
            check(op, dp_word(op, imm5, ty), imm5, ty, &mut rng);
        }
    }
}

#[test]
fn register_shift_by_zero_keeps_the_carry() {
    // MOVS r0, r2, LSL r3 with r3 = 0: the carry is the incoming C, the value is Rm.
    let insn: u32 = 0xE1B0_0312; // movs r0, r2, lsl r3
    let mut bus = RamBus::new(u64::from(CODE), 0x100);
    bus.load(u64::from(CODE), &insn.to_le_bytes());
    for c_in in [false, true] {
        let mut cpu = Cpu::new(0);
        cpu.reset(CODE);
        cpu.set_reg(2, 0x8000_0001);
        cpu.set_reg(3, 0);
        cpu.set_cpsr(0x10 | if c_in { 1 << 29 } else { 0 });
        cpu.step(&mut bus);
        assert_eq!(cpu.reg(0), 0x8000_0001);
        assert_eq!(cpu.cpsr() >> 29 & 1 == 1, c_in);
    }
}
