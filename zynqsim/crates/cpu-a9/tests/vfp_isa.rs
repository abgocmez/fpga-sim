//! VFP instruction tests. Programs are assembled by the Xilinx GNU toolchain (see
//! `data/vfp_groups.s` and `data/gen_vfp.py`) and run from RAM at 0x1000. Expected values are
//! worked out by hand from the ARM ARM and written in the comments as bit patterns.

use cpu_a9::{Cpu, Event, StepResult};
use ps_bus::RamBus;

include!("data/vfp_words.inc.rs");

const CODE: u32 = 0x1000;

const FPSCR_IOC: u32 = 1 << 0;
const FPSCR_DZC: u32 = 1 << 1;
const FPSCR_OFC: u32 = 1 << 2;
const FPSCR_UFC: u32 = 1 << 3;
const FPSCR_IXC: u32 = 1 << 4;
const FPSCR_IDC: u32 = 1 << 7;
/// FPSCR.RMode values, already shifted into place.
const RM_NEAREST: u32 = 0;
const RM_UP: u32 = 1 << 22;
const RM_DOWN: u32 = 2 << 22;
const RM_ZERO: u32 = 3 << 22;
const FZ: u32 = 1 << 24;
const DN: u32 = 1 << 25;
/// RMode, FZ and DN: the settings a result does not report back.
const MODE_BITS: u32 = (3 << 22) | FZ | DN;

struct M {
    cpu: Cpu,
    bus: RamBus,
}

fn machine(words: &[u32]) -> M {
    let mut bus = RamBus::new(0x1000, 0x3000);
    let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    bus.load(u64::from(CODE), &bytes);
    let mut cpu = Cpu::new(0);
    cpu.reset(CODE);
    M { cpu, bus }
}

fn step(m: &mut M) -> StepResult {
    m.cpu.step(&mut m.bus)
}

/// Concatenates program fragments.
fn prog(parts: &[&[u32]]) -> Vec<u32> {
    parts.concat()
}

/// Runs `words` from reset with `regs` and `mem` (address, bytes) set up. Every step must
/// execute without an event.
fn run(words: &[u32], regs: &[(u8, u32)], mem: &[(u64, Vec<u8>)]) -> M {
    let mut m = machine(words);
    for (addr, bytes) in mem {
        m.bus.load(*addr, bytes);
    }
    for &(r, v) in regs {
        m.cpu.set_reg(r, v);
    }
    for _ in 0..words.len() {
        match step(&mut m) {
            StepResult::Executed { event: None, .. } => {}
            other => panic!("unexpected {other:?} at pc {:#x}", m.cpu.pc()),
        }
    }
    m
}

/// Single precision: `op` on s0 = `a`, s1 = `b`, s2 = `acc`, FPSCR = `mode`. Returns (s2, FPSCR).
fn single(op: &[u32], a: u32, b: u32, acc: u32, mode: u32) -> (u32, u32) {
    let m = run(
        &prog(&[
            PROG_ENABLE,
            PROG_SET_FPSCR,
            PROG_IN_S0,
            PROG_IN_S1,
            PROG_IN_S2,
            op,
            PROG_OUT_S2,
            PROG_OUT_FPSCR,
        ]),
        &[(0, a), (1, b), (5, acc), (9, mode)],
        &[],
    );
    (m.cpu.reg(6), m.cpu.reg(8) & !MODE_BITS)
}

fn pair(v: u64) -> (u32, u32) {
    (v as u32, (v >> 32) as u32)
}

/// Double precision: `op` on d0 = `a`, d1 = `b`, d2 = `acc`, FPSCR = `mode`. Returns (d2, FPSCR).
fn double(op: &[u32], a: u64, b: u64, acc: u64, mode: u32) -> (u64, u32) {
    let (a0, a1) = pair(a);
    let (b0, b1) = pair(b);
    let (c0, c1) = pair(acc);
    let m = run(
        &prog(&[
            PROG_ENABLE,
            PROG_SET_FPSCR,
            PROG_IN_D0,
            PROG_IN_D1,
            PROG_IN_D2,
            op,
            PROG_OUT_D2,
            PROG_OUT_FPSCR,
        ]),
        &[
            (0, a0),
            (1, a1),
            (2, b0),
            (3, b1),
            (4, c0),
            (5, c1),
            (9, mode),
        ],
        &[],
    );
    (
        u64::from(m.cpu.reg(6)) | (u64::from(m.cpu.reg(7)) << 32),
        m.cpu.reg(8) & !MODE_BITS,
    )
}

/// Executes the fragments after ENABLE with `regs`, and returns the core registers r6, r7, r8.
fn outputs(body: &[&[u32]], regs: &[(u8, u32)], mem: &[(u64, Vec<u8>)]) -> (u32, u32, u32) {
    let mut parts: Vec<&[u32]> = vec![PROG_ENABLE];
    parts.extend_from_slice(body);
    let m = run(&prog(&parts), regs, mem);
    (m.cpu.reg(6), m.cpu.reg(7), m.cpu.reg(8))
}

/// Runs the first `n` steps of `words` and then returns the event of step `n + 1`.
fn event_after(
    words: &[u32],
    n: usize,
    mem: &[(u64, Vec<u8>)],
    regs: &[(u8, u32)],
) -> (M, StepResult) {
    let mut m = machine(words);
    for (addr, bytes) in mem {
        m.bus.load(*addr, bytes);
    }
    for &(r, v) in regs {
        m.cpu.set_reg(r, v);
    }
    for _ in 0..n {
        match step(&mut m) {
            StepResult::Executed { event: None, .. } => {}
            other => panic!("unexpected {other:?}"),
        }
    }
    let r = step(&mut m);
    (m, r)
}

fn event_of(r: &StepResult) -> Option<Event> {
    match r {
        StepResult::Executed { event, .. } => *event,
        StepResult::Unimplemented { .. } => None,
    }
}

#[test]
fn immediates_and_sign_operations() {
    // 1.0 is 0x3F800000; -2.5 is 0xC004000000000000 (sign, exponent 0x400, fraction 0x4 << 48).
    assert_eq!(single(PROG_OP_IMM_S, 0, 0, 0, 0).0, 0x3F80_0000);
    assert_eq!(double(PROG_OP_IMM_D, 0, 0, 0, 0).0, 0xC004_0000_0000_0000);
    // VABS and VNEG are sign-bit operations: no flags, a signaling NaN is not quietened.
    assert_eq!(single(PROG_OP_ABS_S, 0xBF80_0000, 0, 0, 0).0, 0x3F80_0000);
    assert_eq!(single(PROG_OP_NEG_S, 0x3F80_0000, 0, 0, 0).0, 0xBF80_0000);
    let (v, fpscr) = single(PROG_OP_NEG_S, 0x7F80_0001, 0, 0, 0);
    assert_eq!(v, 0xFF80_0001);
    assert_eq!(fpscr & 0x9F, 0);
    assert_eq!(single(PROG_OP_MOV_S, 0x1234_5678, 0, 0, 0).0, 0x1234_5678);
    assert_eq!(
        double(PROG_OP_ABS_D, 0xBFF0_0000_0000_0000, 0, 0, 0).0,
        0x3FF0_0000_0000_0000
    );
}

#[test]
fn single_arithmetic_has_exact_values() {
    // 1.5 = 0x3FC00000, 2.25 = 0x40100000, 7.5 = 0x40F00000, 2.5 = 0x40200000.
    assert_eq!(
        single(PROG_OP_ADD_S, 0x3FC0_0000, 0x4010_0000, 0, 0),
        (0x4070_0000, 0)
    ); // 3.75
    assert_eq!(
        single(PROG_OP_SUB_S, 0x3FC0_0000, 0x4010_0000, 0, 0).0,
        0xBF40_0000
    ); // -0.75
    assert_eq!(
        single(PROG_OP_MUL_S, 0x3FC0_0000, 0x4010_0000, 0, 0).0,
        0x4058_0000
    ); // 3.375
    assert_eq!(
        single(PROG_OP_DIV_S, 0x40F0_0000, 0x4020_0000, 0, 0).0,
        0x4040_0000
    ); // 3.0
    assert_eq!(single(PROG_OP_SQRT_S, 0x4010_0000, 0, 0, 0).0, 0x3FC0_0000); // sqrt(2.25)
    // Double: 1.5 = 0x3FF8000000000000, 2.25 = 0x4002000000000000, 3.75 = 0x400E000000000000.
    assert_eq!(
        double(
            PROG_OP_ADD_D,
            0x3FF8_0000_0000_0000,
            0x4002_0000_0000_0000,
            0,
            0
        )
        .0,
        0x400E_0000_0000_0000
    );
}

#[test]
fn multiply_accumulate_rounds_the_product_then_the_sum() {
    // a = 1.5, b = 2.0, acc = 1.0: MLA 4.0, MLS -2.0, NMLA -4.0, NMLS 2.0, NMUL -3.0.
    let (a, b, acc) = (0x3FC0_0000, 0x4000_0000, 0x3F80_0000);
    assert_eq!(single(PROG_OP_MLA_S, a, b, acc, 0).0, 0x4080_0000);
    assert_eq!(single(PROG_OP_MLS_S, a, b, acc, 0).0, 0xC000_0000);
    assert_eq!(single(PROG_OP_NMLA_S, a, b, acc, 0).0, 0xC080_0000);
    assert_eq!(single(PROG_OP_NMLS_S, a, b, acc, 0).0, 0x4000_0000);
    assert_eq!(single(PROG_OP_NMUL_S, a, b, acc, 0).0, 0xC040_0000);
    // Double: 4.0 = 0x4010000000000000.
    assert_eq!(
        double(
            PROG_OP_MLA_D,
            0x3FF8_0000_0000_0000,
            0x4000_0000_0000_0000,
            0x3FF0_0000_0000_0000,
            0
        )
        .0,
        0x4010_0000_0000_0000
    );
}

#[test]
fn rounding_modes_on_one_plus_half_an_ulp() {
    // 1.0 = 0x3F800000, 2^-24 = 0x33800000: the exact sum lies halfway between 1.0 and 1 + 2^-23.
    let (one, half) = (0x3F80_0000, 0x3380_0000);
    assert_eq!(
        single(PROG_OP_ADD_S, one, half, 0, RM_NEAREST),
        (0x3F80_0000, FPSCR_IXC)
    );
    assert_eq!(
        single(PROG_OP_ADD_S, one, half, 0, RM_UP),
        (0x3F80_0001, FPSCR_IXC)
    );
    assert_eq!(
        single(PROG_OP_ADD_S, one, half, 0, RM_DOWN),
        (0x3F80_0000, FPSCR_IXC)
    );
    assert_eq!(
        single(PROG_OP_ADD_S, one, half, 0, RM_ZERO),
        (0x3F80_0000, FPSCR_IXC)
    );
    // Negative: -1.0 - 2^-24 is -(1 + 2^-24); toward -inf rounds the magnitude up.
    let (none, nhalf) = (0xBF80_0000, 0xB380_0000);
    assert_eq!(
        single(PROG_OP_ADD_S, none, nhalf, 0, RM_DOWN).0,
        0xBF80_0001
    );
    assert_eq!(single(PROG_OP_ADD_S, none, nhalf, 0, RM_UP).0, 0xBF80_0000);
    // Exact cancellation: +0, or -0 toward -infinity.
    assert_eq!(single(PROG_OP_ADD_S, one, none, 0, RM_NEAREST), (0, 0));
    assert_eq!(
        single(PROG_OP_ADD_S, one, none, 0, RM_DOWN),
        (0x8000_0000, 0)
    );
}

#[test]
fn invalid_division_overflow_underflow_and_denormal_flags() {
    // 1.0 / 0.0: infinity and DZC.
    assert_eq!(
        single(PROG_OP_DIV_S, 0x3F80_0000, 0, 0, 0),
        (0x7F80_0000, FPSCR_DZC)
    );
    // 0 / 0 and inf - inf: default NaN and IOC.
    assert_eq!(single(PROG_OP_DIV_S, 0, 0, 0, 0), (0x7FC0_0000, FPSCR_IOC));
    assert_eq!(
        single(PROG_OP_SUB_S, 0x7F80_0000, 0x7F80_0000, 0, 0),
        (0x7FC0_0000, FPSCR_IOC)
    );
    // 0 * inf and sqrt(-1): invalid.
    assert_eq!(
        single(PROG_OP_MUL_S, 0, 0x7F80_0000, 0, 0).1 & FPSCR_IOC,
        FPSCR_IOC
    );
    assert_eq!(
        single(PROG_OP_SQRT_S, 0xBF80_0000, 0, 0, 0),
        (0x7FC0_0000, FPSCR_IOC)
    );
    // max + max: overflow to infinity (inexact); toward zero it stays at the largest value.
    let max = 0x7F7F_FFFF;
    assert_eq!(
        single(PROG_OP_ADD_S, max, max, 0, RM_NEAREST),
        (0x7F80_0000, FPSCR_OFC | FPSCR_IXC)
    );
    assert_eq!(
        single(PROG_OP_ADD_S, max, max, 0, RM_ZERO),
        (max, FPSCR_OFC | FPSCR_IXC)
    );
    // 2^-126 * 2^-126 = 2^-252 rounds to +0: tiny and inexact.
    assert_eq!(
        single(PROG_OP_MUL_S, 0x0080_0000, 0x0080_0000, 0, RM_NEAREST),
        (0, FPSCR_UFC | FPSCR_IXC)
    );
    // Denormal input: kept without FZ, flushed (IDC) with FZ.
    assert_eq!(
        single(PROG_OP_ADD_S, 0x0000_0001, 0, 0, 0),
        (0x0000_0001, 0)
    );
    assert_eq!(single(PROG_OP_ADD_S, 0x0000_0001, 0, 0, FZ), (0, FPSCR_IDC));
}

#[test]
fn nan_propagation_and_default_nan() {
    // A signaling NaN is quietened (bit 22 set) and raises IOC.
    assert_eq!(
        single(PROG_OP_ADD_S, 0x7F80_0001, 0x3F80_0000, 0, 0),
        (0x7FC0_0001, FPSCR_IOC)
    );
    // With DN the default NaN replaces it.
    assert_eq!(
        single(PROG_OP_ADD_S, 0x7F80_0001, 0x3F80_0000, 0, DN),
        (0x7FC0_0000, FPSCR_IOC)
    );
    // The signaling operand wins over a quiet one, whichever is first.
    assert_eq!(
        single(PROG_OP_ADD_S, 0x7FC0_0005, 0x7F80_0002, 0, 0),
        (0x7FC0_0002, FPSCR_IOC)
    );
    // A quiet NaN passes through without flags.
    assert_eq!(
        single(PROG_OP_ADD_S, 0x7FC0_0005, 0x3F80_0000, 0, 0),
        (0x7FC0_0005, 0)
    );
}

#[test]
fn compare_sets_nzcv_and_exceptions() {
    // FPSCR.NZCV is bits 31:28; the nibble is what we check.
    let nzcv = |(_, f): (u32, u32)| f >> 28;
    // N = 0b1000 less, Z and C = 0b0110 equal, C = 0b0010 greater, C and V = 0b0011 unordered.
    assert_eq!(
        nzcv(single(PROG_OP_CMP_S, 0x3F80_0000, 0x4000_0000, 0, 0)),
        0b1000
    );
    assert_eq!(
        nzcv(single(PROG_OP_CMP_S, 0x4000_0000, 0x3F80_0000, 0, 0)),
        0b0010
    );
    assert_eq!(
        nzcv(single(PROG_OP_CMP_S, 0x3F80_0000, 0x3F80_0000, 0, 0)),
        0b0110
    );
    // -0.0 equals +0.0.
    assert_eq!(nzcv(single(PROG_OP_CMP_S, 0x8000_0000, 0, 0, 0)), 0b0110);
    // Unordered: a quiet NaN raises no IOC for VCMP, but VCMPE raises it for any NaN.
    let quiet = single(PROG_OP_CMP_S, 0x7FC0_0000, 0x3F80_0000, 0, 0);
    assert_eq!(nzcv(quiet), 0b0011);
    assert_eq!(quiet.1 & FPSCR_IOC, 0);
    let quiet_e = single(PROG_OP_CMPE_S, 0x7FC0_0000, 0x3F80_0000, 0, 0);
    assert_eq!(quiet_e.1 & FPSCR_IOC, FPSCR_IOC);
    // A signaling NaN raises IOC for VCMP as well.
    assert_eq!(
        single(PROG_OP_CMP_S, 0x7F80_0001, 0x3F80_0000, 0, 0).1 & FPSCR_IOC,
        FPSCR_IOC
    );
    // Compare with zero: -1.0 is less than +0.0.
    assert_eq!(nzcv(single(PROG_OP_CMPZ_S, 0xBF80_0000, 0, 0, 0)), 0b1000);
    // Double: 2.0 compared with 2.0.
    let (_, f) = double(
        PROG_OP_CMP_D,
        0x4000_0000_0000_0000,
        0x4000_0000_0000_0000,
        0,
        0,
    );
    assert_eq!(f >> 28, 0b0110);
    let (_, f) = double(PROG_OP_CMPEZ_D, 0x3FF0_0000_0000_0000, 0, 0, 0);
    assert_eq!(f >> 28, 0b0010);
}

#[test]
fn conversions_between_formats_and_integers() {
    // -7 as i32 is 0xFFFFFFF9 and -7.0 is 0xC0E00000.
    assert_eq!(single(PROG_OP_I2F_S, 0xFFFF_FFF9, 0, 0, 0).0, 0xC0E0_0000);
    // 2^32 - 1 is not exact in binary32 and rounds up to 2^32 (0x4F800000).
    assert_eq!(
        single(PROG_OP_U2F_S, 0xFFFF_FFFF, 0, 0, 0),
        (0x4F80_0000, FPSCR_IXC)
    );
    // 2.5: round toward zero gives 2; RN (ties to even) gives 2; toward +inf gives 3.
    assert_eq!(single(PROG_OP_F2I_RZ_S, 0x4020_0000, 0, 0, 0).0, 2);
    assert_eq!(single(PROG_OP_F2I_R_S, 0x4020_0000, 0, 0, RM_NEAREST).0, 2);
    assert_eq!(single(PROG_OP_F2I_R_S, 0x4020_0000, 0, 0, RM_UP).0, 3);
    // -2.5 toward zero is -2.
    assert_eq!(
        single(PROG_OP_F2I_RZ_S, 0xC020_0000, 0, 0, 0).0,
        0xFFFF_FFFE
    );
    // 2^33 saturates to i32::MAX with IOC; -1.0 to u32 is 0 with IOC; NaN is 0 with IOC.
    assert_eq!(
        single(PROG_OP_F2I_RZ_S, 0x5000_0000, 0, 0, 0),
        (0x7FFF_FFFF, FPSCR_IOC)
    );
    assert_eq!(
        single(PROG_OP_F2U_RZ_S, 0xBF80_0000, 0, 0, 0),
        (0, FPSCR_IOC)
    );
    assert_eq!(
        single(PROG_OP_F2I_RZ_S, 0x7FC0_0000, 0, 0, 0),
        (0, FPSCR_IOC)
    );
    // Double to integer with the FPSCR mode: 2.5 with RN is 2.
    // (the integer result is in s2, which is the low word of d1)
    let m = run(
        &prog(&[
            PROG_ENABLE,
            PROG_SET_FPSCR,
            PROG_IN_D0,
            PROG_OP_F2I_R_D,
            PROG_OUT_S2,
        ]),
        &[(0, 0), (1, 0x4004_0000), (9, RM_NEAREST)],
        &[],
    );
    assert_eq!(m.cpu.reg(6), 2);
    // Widening 1.5 is exact: 0x3FF8000000000000.
    assert_eq!(
        outputs(
            &[PROG_IN_S0, PROG_OP_CVT_S2D, PROG_OUT_D2, PROG_OUT_FPSCR],
            &[(0, 0x3FC0_0000)],
            &[]
        ),
        (0x0000_0000, 0x3FF8_0000, 0)
    );
    // -2 as a double is 0xC000000000000000.
    let (v, f) = double(PROG_OP_I2F_D, 0xFFFF_FFFE, 0, 0, 0);
    assert_eq!((v, f), (0xC000_0000_0000_0000, 0));
    // Fixed point: 1.5 * 2^16 = 0x18000 (the register is the destination, FPSCR mode RN).
    assert_eq!(
        single(PROG_OP_FIX_F2S32, 0, 0, 0x3FC0_0000, 0).0,
        0x0001_8000
    );
    // #5 is 5 fraction bits (the encoding holds 32 - 5 = 27): 1.5 * 2^5 = 48.
    assert_eq!(single(PROG_OP_FIX_F2S5, 0, 0, 0x3FC0_0000, 0).0, 48);
    // 3 * 2^-16 = 0x38400000.
    assert_eq!(single(PROG_OP_FIX_I2F_S32, 0, 0, 3, 0).0, 0x3840_0000);
    // 0x100 as unsigned 16-bit with 8 fraction bits is 1.0.
    assert_eq!(single(PROG_OP_FIX_I2F_U16, 0, 0, 0x0100, 0).0, 0x3F80_0000);
    // Double fixed to integer with 5 fraction bits: 1.5 * 2^5 = 48 in the low word, upper word zero.
    assert_eq!(
        double(PROG_OP_FIX_F2D_S32, 0, 0, 0x3FF8_0000_0000_0000, 0).0,
        48
    );
}

#[test]
fn core_and_single_transfers_and_pairs() {
    assert_eq!(
        outputs(&[PROG_XFER_S5_R2, PROG_OUT_S5], &[(2, 0x1234_5678)], &[]).0,
        0x1234_5678
    );
    let (s5, s6, _) = outputs(
        &[PROG_XFER_S56_R23, PROG_OUT_S5, PROG_OUT_S6],
        &[(2, 0xAAAA), (3, 0xBBBB)],
        &[],
    );
    assert_eq!((s5, s6), (0xAAAA, 0xBBBB));
    let (lo, hi, _) = outputs(
        &[PROG_XFER_D5_R23, PROG_OUT_D5],
        &[(2, 0x1111_2222), (3, 0x3333_4444)],
        &[],
    );
    assert_eq!((lo, hi), (0x1111_2222, 0x3333_4444));
    // Core -> pair -> core round trip keeps the values.
    let m = run(
        &prog(&[PROG_ENABLE, PROG_XFER_S56_R23, PROG_XFER_R23_S56]),
        &[(2, 7), (3, 9)],
        &[],
    );
    assert_eq!((m.cpu.reg(2), m.cpu.reg(3)), (7, 9));
    // System registers: FPSID and MVFR0 are read with VMRS (privileged, no FPEXC.EN needed).
    let m = run(&prog(&[PROG_ENABLE, PROG_VMRS_FPSID]), &[], &[]);
    assert_eq!(m.cpu.reg(0), 0x4103_3090);
    let m = run(&prog(&[PROG_ENABLE, PROG_VMRS_MVFR0]), &[], &[]);
    assert_eq!(m.cpu.reg(0), 0x1011_0221);
}

#[test]
fn fpscr_write_mask_and_fpexc() {
    // Bit 15 (IDE, a trap enable) is not writable: it reads back as zero.
    assert_eq!(
        outputs(&[PROG_SET_FPSCR, PROG_OUT_FPSCR], &[(9, 0x8000)], &[]).2,
        0
    );
    // The mode bits and the cumulative bits do read back.
    assert_eq!(
        outputs(
            &[PROG_SET_FPSCR, PROG_OUT_FPSCR],
            &[(9, RM_ZERO | FZ | DN | 0x1F)],
            &[]
        )
        .2,
        RM_ZERO | FZ | DN | 0x1F
    );
    // FPEXC keeps only EN and EX; EN is set by the prologue.
    let m = run(&prog(&[PROG_ENABLE, PROG_VMRS_FPEXC]), &[], &[]);
    assert_eq!(m.cpu.reg(0), 0x4000_0000);
}

#[test]
fn memory_access_loads_stores_and_writeback() {
    let words = |vals: &[u32]| -> Vec<u8> { vals.iter().flat_map(|w| w.to_le_bytes()).collect() };
    let data = words(&[
        0x1000_0000,
        0x2000_0001,
        0x3000_0002,
        0x4000_0003,
        0x5000_0004,
        0x6000_0005,
    ]);
    let mem = [(0x2000u64, data)];
    // VLDR s2, [r4, #8] reads the word at 0x2008.
    let (v, _, _) = outputs(&[PROG_MEM_VLDR_S, PROG_OUT_S2], &[(4, 0x2000)], &mem);
    assert_eq!(v, 0x3000_0002);
    // VLDMIA r4!, {d4-d5} reads four words and writes back 0x2010; d5 = words at 0x2008 and 0x200C.
    let m = run(
        &prog(&[PROG_ENABLE, PROG_MEM_VLDMIA_D, PROG_OUT_D5]),
        &[(4, 0x2000)],
        &mem,
    );
    assert_eq!(m.cpu.reg(4), 0x2010);
    assert_eq!((m.cpu.reg(6), m.cpu.reg(7)), (0x3000_0002, 0x4000_0003));
    // VPOP {s16-s17} from sp = 0x2FF0 reads the words there and leaves sp at 0x2FF8.
    let stack = [(0x2FF0u64, words(&[0xDEAD_0001, 0xBEEF_0002]))];
    let m = run(
        &prog(&[PROG_ENABLE, PROG_MEM_VPOP_S, PROG_OUT_S16, PROG_OUT_S17]),
        &[(13, 0x2FF0)],
        &stack,
    );
    assert_eq!(
        (m.cpu.reg(6), m.cpu.reg(7), m.cpu.reg(13)),
        (0xDEAD_0001, 0xBEEF_0002, 0x2FF8)
    );
    // VPUSH {d8-d9} moves sp down by 16.
    let m = run(
        &prog(&[PROG_ENABLE, PROG_MEM_VPUSH_D]),
        &[(13, 0x3000)],
        &[],
    );
    assert_eq!(m.cpu.reg(13), 0x2FF0);
    // VSTMDB r4!, {s4-s5} from 0x2010 writes back 0x2008 and stores two words there.
    let m = run(
        &prog(&[PROG_ENABLE, PROG_MEM_VSTMDB_S]),
        &[(4, 0x2010)],
        &[],
    );
    assert_eq!(m.cpu.reg(4), 0x2008);
}

#[test]
fn unaligned_vfp_load_is_an_alignment_data_abort() {
    // vldr s2, [r4, #8] with r4 = 0x2002 faults at 0x200A.
    let (_, r) = event_after(
        &prog(&[PROG_ENABLE, PROG_MEM_VLDR_S]),
        4,
        &[],
        &[(4, 0x2002)],
    );
    assert!(
        matches!(event_of(&r), Some(Event::DataAbort { addr: 0x200A })),
        "{r:?}"
    );
}

#[test]
fn disabled_or_forbidden_vfp_is_undefined() {
    // Without the prologue CPACR and FPEXC are zero: VMRS FPSCR is UNDEFINED.
    let (_, r) = event_after(&prog(&[PROG_OUT_FPSCR]), 0, &[], &[]);
    assert_eq!(event_of(&r), Some(Event::Undefined));
    // Privileged-only access (CPACR = 0b01) from user mode is UNDEFINED.
    let (_, r) = event_after(
        &prog(&[
            PROG_ENABLE,
            PROG_CPACR_PRIV,
            PROG_ENTER_USER,
            PROG_OUT_FPSCR,
        ]),
        4 + 2 + 1,
        &[],
        &[],
    );
    assert_eq!(event_of(&r), Some(Event::Undefined));
    // Full access from user mode: FPSCR is readable, FPEXC (privileged) is not.
    let (_, r) = event_after(
        &prog(&[PROG_ENABLE, PROG_ENTER_USER, PROG_OUT_FPSCR]),
        4 + 1,
        &[],
        &[],
    );
    assert_eq!(event_of(&r), None);
    let (_, r) = event_after(
        &prog(&[PROG_ENABLE, PROG_ENTER_USER, PROG_VMRS_FPEXC]),
        4 + 1,
        &[],
        &[],
    );
    assert_eq!(event_of(&r), Some(Event::Undefined));
    // VFPv4 fused multiply-add and ARMv8 VRINTZ are UNDEFINED on this VFPv3 unit.
    let (_, r) = event_after(&prog(&[PROG_ENABLE, PROG_OP_VFMA_S]), 4, &[], &[]);
    assert_eq!(event_of(&r), Some(Event::Undefined));
    let (_, r) = event_after(&prog(&[PROG_ENABLE, PROG_OP_VFNMA_D]), 4, &[], &[]);
    assert_eq!(event_of(&r), Some(Event::Undefined));
    let (_, r) = event_after(&prog(&[PROG_ENABLE, PROG_OP_VRINTZ_S]), 4, &[], &[]);
    assert_eq!(event_of(&r), Some(Event::Undefined));
}

#[test]
fn unimplemented_features_are_reported_with_a_reason() {
    // Half-precision conversion (VCVTB): reported with its reason.
    let mut m = machine(&prog(&[PROG_ENABLE, PROG_OP_VCVTB]));
    for _ in 0..4 {
        step(&mut m);
    }
    assert!(matches!(step(&mut m), StepResult::Unimplemented { .. }));
    assert!(
        m.cpu
            .vfp_unimplemented_reason()
            .unwrap()
            .contains("half precision")
    );
    // Short vectors: FPSCR.LEN = 1 makes arithmetic a vector operation.
    let mut m = machine(&prog(&[
        PROG_ENABLE,
        PROG_SET_FPSCR,
        PROG_IN_S0,
        PROG_IN_S1,
        PROG_OP_ADD_S,
    ]));
    m.cpu.set_reg(9, 1 << 16);
    for _ in 0..7 {
        step(&mut m);
    }
    assert!(matches!(step(&mut m), StepResult::Unimplemented { .. }));
    assert!(
        m.cpu
            .vfp_unimplemented_reason()
            .unwrap()
            .contains("short vectors")
    );
}
