//! Property tests: the same operation as A32 and as Thumb (16-bit or Thumb-2) gives the same
//! registers and flags. Each case runs both instruction sets from identical random register and
//! flag state. The A32 side is the interpreter already checked against the Xilinx assembler
//! (`isa.rs`, `dp_props.rs`), so these tests tie the Thumb decode and executors to it. The
//! encoders below are checked against the assembler in `thumb.rs` (`encoders_match_the_assembler`).

use cpu_a9::{Cpu, StepResult};
use ps_bus::RamBus;

/// Random cases per operation.
const CASES: usize = 10_000;
/// Where the code is loaded.
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
    fn below(&mut self, n: u32) -> u32 {
        self.u32() % n
    }
    /// Random N, Z, C and V bits (the top nibble).
    fn nzcv(&mut self) -> u32 {
        self.u32() & 0xF000_0000
    }
    /// Random register values for r0 to r12.
    fn regs(&mut self) -> [u32; 13] {
        let mut r = [0u32; 13];
        for v in &mut r {
            *v = self.u32();
        }
        r
    }
}

/// The state after a run: r0 to r12 and the N, Z, C, V bits.
#[derive(Debug, PartialEq, Eq)]
struct Out {
    r: [u32; 13],
    nzcv: u32,
}

/// Runs `n` instructions of `code` from [`CODE`] with the given registers and flags.
fn exec(thumb: bool, code: &[u8], n: usize, regs: &[u32; 13], nzcv: u32) -> Out {
    let mut bus = RamBus::new(0x1000, 0x3000);
    bus.load(u64::from(CODE), code);
    let mut cpu = Cpu::new(0);
    cpu.reset(CODE);
    cpu.set_cpsr(0x13 | 0xC0 | if thumb { 0x20 } else { 0 } | nzcv);
    for (i, &v) in regs.iter().enumerate() {
        cpu.set_reg(i as u8, v);
    }
    for _ in 0..n {
        match cpu.step(&mut bus) {
            StepResult::Executed { event: None, .. } => {}
            other => panic!("{other:?} at {:#x}", cpu.pc()),
        }
    }
    let mut r = [0u32; 13];
    for (i, slot) in r.iter_mut().enumerate() {
        *slot = cpu.reg(i as u8);
    }
    Out {
        r,
        nzcv: cpu.cpsr() & 0xF000_0000,
    }
}

/// Runs one A32 word.
fn arm(word: u32, regs: &[u32; 13], nzcv: u32) -> Out {
    exec(false, &word.to_le_bytes(), 1, regs, nzcv)
}

/// Runs one Thumb instruction (one or two halfwords).
fn thumb(hws: &[u16], regs: &[u32; 13], nzcv: u32) -> Out {
    let bytes: Vec<u8> = hws.iter().flat_map(|h| h.to_le_bytes()).collect();
    exec(true, &bytes, 1, regs, nzcv)
}

/// Runs an A32 word and a Thumb encoding from the same state and demands equal results.
fn same(what: &str, case: usize, word: u32, hws: &[u16], regs: &[u32; 13], nzcv: u32) {
    let a = arm(word, regs, nzcv);
    let t = thumb(hws, regs, nzcv);
    assert_eq!(
        t, a,
        "{what} case {case}: A32 {word:#010x} vs T32 {hws:04x?} from regs {regs:x?} nzcv {nzcv:#010x}"
    );
}

// ----- encoders ------------------------------------------------------------------------------

/// 16-bit ADD or SUB, register: `Rd = Rn +/- Rm`.
fn t16_addsub_reg(sub: bool, rd: u16, rn: u16, rm: u16) -> u16 {
    0x1800 | (u16::from(sub) << 9) | (rm << 6) | (rn << 3) | rd
}
/// 16-bit ADD or SUB, 3-bit immediate.
fn t16_addsub_imm3(sub: bool, rd: u16, rn: u16, imm3: u16) -> u16 {
    0x1C00 | (u16::from(sub) << 9) | (imm3 << 6) | (rn << 3) | rd
}
/// 16-bit MOV, CMP, ADD or SUB with an 8-bit immediate (`op` 0 to 3).
fn t16_imm8(op: u16, rdn: u16, imm8: u16) -> u16 {
    0x2000 | (op << 11) | (rdn << 8) | imm8
}
/// 16-bit data-processing (register), opcode 0 to 15.
fn t16_dp(op: u16, rdn: u16, rm: u16) -> u16 {
    0x4000 | (op << 6) | (rm << 3) | rdn
}
/// 16-bit LSL, LSR or ASR by an immediate.
fn t16_shift_imm(ty: u16, rd: u16, rm: u16, imm5: u16) -> u16 {
    (ty << 11) | (imm5 << 6) | (rm << 3) | rd
}

/// Thumb-2 data-processing (modified immediate): hw1 and hw2.
fn t2_dp_imm(opc: u16, s: bool, rn: u16, rd: u16, imm12: u32) -> [u16; 2] {
    let i = ((imm12 >> 11) & 1) as u16;
    let hw1 = 0xF000 | (i << 10) | (opc << 5) | (u16::from(s) << 4) | rn;
    let hw2 = ((((imm12 >> 8) & 7) as u16) << 12) | (rd << 8) | (imm12 & 0xFF) as u16;
    [hw1, hw2]
}

/// Thumb-2 data-processing (shifted register).
fn t2_dp_shift(opc: u16, s: bool, rn: u16, rd: u16, imm5: u16, ty: u16, rm: u16) -> [u16; 2] {
    let hw1 = 0xEA00 | (opc << 5) | (u16::from(s) << 4) | rn;
    let hw2 = ((imm5 >> 2) << 12) | (rd << 8) | ((imm5 & 3) << 6) | (ty << 4) | rm;
    [hw1, hw2]
}

/// A32 data-processing with an immediate (rotation and 8-bit value).
fn a32_dp_imm(opc: u32, s: bool, rn: u32, rd: u32, rot: u32, imm8: u32) -> u32 {
    0xE200_0000 | (opc << 21) | (u32::from(s) << 20) | (rn << 16) | (rd << 12) | (rot << 8) | imm8
}

/// A32 data-processing with a register shifted by an immediate.
fn a32_dp_shift(opc: u32, s: bool, rn: u32, rd: u32, imm5: u32, ty: u32, rm: u32) -> u32 {
    0xE000_0000
        | (opc << 21)
        | (u32::from(s) << 20)
        | (rn << 16)
        | (rd << 12)
        | (imm5 << 7)
        | (ty << 5)
        | rm
}

/// The A32 opcode of a Thumb-2 modified-immediate or shifted-register opcode (the ORN form has
/// no A32 twin and is not used here).
fn a32_opcode(thumb_opc: u16) -> u32 {
    match thumb_opc {
        0 => 0,  // AND
        1 => 14, // BIC
        2 => 12, // ORR
        4 => 1,  // EOR
        8 => 4,  // ADD
        10 => 5, // ADC
        11 => 6, // SBC
        13 => 2, // SUB
        14 => 3, // RSB
        other => panic!("no twin for opcode {other}"),
    }
}

/// The modified-immediate form of `v` for Thumb-2: `(imm12)`, if one encodes it.
fn thumb_imm12(v: u32) -> Option<u32> {
    if v <= 0xFF {
        return Some(v);
    }
    // The pattern forms (imm12 bits 11:10 = 00): 0x00XY00XY, 0xXY00XY00 and 0xXYXYXYXY.
    let b = v & 0xFF;
    if v == b * 0x0101_0101 {
        return Some((3 << 8) | b);
    }
    if v == (b << 16) | b {
        return Some((1 << 8) | b);
    }
    let hi = v >> 24;
    if v == (hi << 24) | (hi << 8) {
        return Some((2 << 8) | hi);
    }
    // Rotated 8-bit values with bit 7 set, rotation 8 to 31.
    for r in 8..32u32 {
        let x = v.rotate_left(r);
        if (0x80..=0xFF).contains(&x) {
            return Some((r << 7) | (x & 0x7F));
        }
    }
    None
}

/// The A32 rotation and 8-bit immediate of `v`, if one encodes it.
fn a32_imm(v: u32) -> Option<(u32, u32)> {
    for rot in 0..16u32 {
        let x = v.rotate_left(2 * rot);
        if x <= 0xFF {
            return Some((rot, x));
        }
    }
    None
}

/// Thumb-2 shift-by-register (LSL, LSR, ASR, ROR): hw1 and hw2.
fn t2_shift_reg(ty: u16, s: bool, rd: u16, rn: u16, rm: u16) -> [u16; 2] {
    [
        0xFA00 | (ty << 5) | (u16::from(s) << 4) | rn,
        0xF000 | (rd << 8) | rm,
    ]
}

// ----- properties ----------------------------------------------------------------------------

#[test]
fn sixteen_bit_add_sub_mov_and_compare_match_a32() {
    let mut p = Prng(0x1601);
    for case in 0..CASES {
        let regs = p.regs();
        let nzcv = p.nzcv();
        let imm3 = p.below(8) as u16;
        let imm8 = p.below(256);
        same(
            "ADDS reg",
            case,
            0xE091_0002,
            &[t16_addsub_reg(false, 0, 1, 2)],
            &regs,
            nzcv,
        );
        same(
            "SUBS reg",
            case,
            0xE051_0002,
            &[t16_addsub_reg(true, 0, 1, 2)],
            &regs,
            nzcv,
        );
        same(
            "ADDS imm3",
            case,
            0xE291_0000 | u32::from(imm3),
            &[t16_addsub_imm3(false, 0, 1, imm3)],
            &regs,
            nzcv,
        );
        same(
            "SUBS imm3",
            case,
            0xE251_0000 | u32::from(imm3),
            &[t16_addsub_imm3(true, 0, 1, imm3)],
            &regs,
            nzcv,
        );
        same(
            "MOVS imm8",
            case,
            0xE3B0_0000 | imm8,
            &[t16_imm8(0, 0, imm8 as u16)],
            &regs,
            nzcv,
        );
        same(
            "CMP imm8",
            case,
            0xE350_0000 | imm8,
            &[t16_imm8(1, 0, imm8 as u16)],
            &regs,
            nzcv,
        );
        same(
            "ADDS imm8",
            case,
            0xE290_0000 | imm8,
            &[t16_imm8(2, 0, imm8 as u16)],
            &regs,
            nzcv,
        );
        same(
            "SUBS imm8",
            case,
            0xE250_0000 | imm8,
            &[t16_imm8(3, 0, imm8 as u16)],
            &regs,
            nzcv,
        );
    }
}

#[test]
fn sixteen_bit_register_data_processing_matches_a32() {
    // (Thumb opcode, A32 word) with r0 = r0 op r2 (or the compare forms), the shifts by r2.
    let table: [(u16, u32); 16] = [
        (0, 0xE010_0002),  // ANDS r0, r0, r2
        (1, 0xE030_0002),  // EORS r0, r0, r2
        (2, 0xE1B0_0210),  // MOVS r0, r0, LSL r2
        (3, 0xE1B0_0230),  // MOVS r0, r0, LSR r2
        (4, 0xE1B0_0250),  // MOVS r0, r0, ASR r2
        (5, 0xE0B0_0002),  // ADCS r0, r0, r2
        (6, 0xE0D0_0002),  // SBCS r0, r0, r2
        (7, 0xE1B0_0270),  // MOVS r0, r0, ROR r2
        (8, 0xE110_0002),  // TST r0, r2
        (9, 0xE272_0000),  // RSBS r0, r2, #0
        (10, 0xE150_0002), // CMP r0, r2
        (11, 0xE170_0002), // CMN r0, r2
        (12, 0xE190_0002), // ORRS r0, r0, r2
        (13, 0xE010_0092), // MULS r0, r2, r0
        (14, 0xE1D0_0002), // BICS r0, r0, r2
        (15, 0xE1F0_0002), // MVNS r0, r2
    ];
    let mut p = Prng(0x1602);
    for case in 0..CASES {
        let regs = p.regs();
        let nzcv = p.nzcv();
        for &(op, word) in &table {
            same(
                &format!("dp op {op}"),
                case,
                word,
                &[t16_dp(op, 0, 2)],
                &regs,
                nzcv,
            );
        }
    }
}

#[test]
fn sixteen_bit_shifts_by_immediate_match_a32() {
    let mut p = Prng(0x1603);
    for case in 0..CASES {
        let regs = p.regs();
        let nzcv = p.nzcv();
        let imm5 = p.below(32) as u16;
        let imm = u32::from(imm5);
        same(
            "LSLS imm",
            case,
            0xE1B0_0000 | (imm << 7) | 1,
            &[t16_shift_imm(0, 0, 1, imm5)],
            &regs,
            nzcv,
        );
        same(
            "LSRS imm",
            case,
            0xE1B0_0020 | (imm << 7) | 1,
            &[t16_shift_imm(1, 0, 1, imm5)],
            &regs,
            nzcv,
        );
        same(
            "ASRS imm",
            case,
            0xE1B0_0040 | (imm << 7) | 1,
            &[t16_shift_imm(2, 0, 1, imm5)],
            &regs,
            nzcv,
        );
    }
}

#[test]
fn thumb2_modified_immediates_match_a32_immediates() {
    // Values that both instruction sets encode, with their A32 and Thumb-2 forms. Logical
    // operations take their carry from the immediate (rotated: bit 31; unrotated: C unchanged).
    let mut p = Prng(0x1604);
    let ops: [(u16, bool); 9] = [
        (0, true),  // ANDS
        (1, true),  // BICS
        (2, true),  // ORRS
        (4, true),  // EORS
        (8, true),  // ADDS
        (10, true), // ADCS
        (11, true), // SBCS
        (13, true), // SUBS
        (14, true), // RSBS
    ];
    let mut checked = 0usize;
    for case in 0..CASES {
        let regs = p.regs();
        let nzcv = p.nzcv();
        // A value encodable in A32 (rotated 8-bit) and in Thumb-2.
        let rot = p.below(16);
        let imm8 = p.below(256) | 1; // non-zero keeps the rotation meaningful
        let v = imm8.rotate_right(2 * rot);
        let Some((a_rot, a_imm8)) = a32_imm(v) else {
            continue;
        };
        let Some(t_imm12) = thumb_imm12(v) else {
            continue;
        };
        // Carry: A32 takes bit 31 of a rotated value, C unchanged for an unrotated one. A
        // Thumb pattern form (0x00XY00XY) keeps C, so only compare when the two agree.
        let thumb_carry_is_bit31 = (t_imm12 >> 10) & 3 != 0;
        let a32_carry_is_bit31 = a_rot != 0;
        if thumb_carry_is_bit31 != a32_carry_is_bit31 && v > 0xFF {
            continue;
        }
        for &(opc, s) in &ops {
            let [hw1, hw2] = t2_dp_imm(opc, s, 1, 0, t_imm12);
            let word = a32_dp_imm(a32_opcode(opc), s, 1, 0, a_rot, a_imm8);
            same(
                &format!("opc {opc} imm {v:#x}"),
                case,
                word,
                &[hw1, hw2],
                &regs,
                nzcv,
            );
            checked += 1;
        }
    }
    assert!(checked > CASES, "too few encodable cases: {checked}");
}

#[test]
fn thumb2_shifted_register_data_processing_matches_a32() {
    let mut p = Prng(0x1605);
    let ops: [u16; 9] = [0, 1, 2, 4, 8, 10, 11, 13, 14];
    for case in 0..CASES {
        let regs = p.regs();
        let nzcv = p.nzcv();
        let imm5 = p.below(32) as u16;
        let ty = p.below(4) as u16;
        let s = p.below(2) == 1;
        for &opc in &ops {
            let [hw1, hw2] = t2_dp_shift(opc, s, 1, 0, imm5, ty, 2);
            let word = a32_dp_shift(a32_opcode(opc), s, 1, 0, u32::from(imm5), u32::from(ty), 2);
            same(
                &format!("shifted opc {opc} ty {ty} imm {imm5}"),
                case,
                word,
                &[hw1, hw2],
                &regs,
                nzcv,
            );
        }
    }
}

#[test]
fn thumb2_shift_by_register_matches_a32() {
    let mut p = Prng(0x1606);
    for case in 0..CASES {
        let regs = p.regs();
        let nzcv = p.nzcv();
        let ty = p.below(4) as u16;
        let s = p.below(2) == 1;
        // A32 MOVS/MOV r0, r1, <ty> r2 (Rs = r2, Rm = r1, register shift).
        let word = 0xE1A0_0010 | (u32::from(s) << 20) | (u32::from(ty) << 5) | (2 << 8) | 1;
        let hws = t2_shift_reg(ty, s, 0, 1, 2);
        same(
            &format!("shift reg ty {ty} s {s}"),
            case,
            word,
            &hws,
            &regs,
            nzcv,
        );
    }
}

#[test]
fn thumb2_multiplies_match_a32() {
    let mut p = Prng(0x1607);
    for case in 0..CASES {
        let regs = p.regs();
        let nzcv = p.nzcv();
        // MUL r0, r1, r2 (A32 MUL: Rd, Rs = r2, Rm = r1).
        same("MUL", case, 0xE000_0291, &[0xFB01, 0xF002], &regs, nzcv);
        // MLA r0, r1, r2, r3.
        same("MLA", case, 0xE020_3291, &[0xFB01, 0x3002], &regs, nzcv);
        // UMULL r0, r1, r2, r3 (low, high, Rn = r2, Rm = r3) and SMULL.
        same("UMULL", case, 0xE081_0293, &[0xFBA2, 0x0103], &regs, nzcv);
        same("SMULL", case, 0xE0C1_0293, &[0xFB82, 0x0103], &regs, nzcv);
        // UMLAL r0, r1, r2, r3 accumulates into r1:r0.
        same("UMLAL", case, 0xE0A1_0293, &[0xFBE2, 0x0103], &regs, nzcv);
    }
}

#[test]
fn thumb2_bitfields_match_a32() {
    let mut p = Prng(0x1608);
    for case in 0..CASES {
        let regs = p.regs();
        let nzcv = p.nzcv();
        let lsb = p.below(32);
        let width = 1 + p.below(32 - lsb);
        let widthm1 = width - 1;
        let msb = lsb + width - 1;
        let imm3 = (lsb >> 2) as u16;
        let imm2 = (lsb & 3) as u16;
        // UBFX r0, r1 ; SBFX r0, r1
        let ubfx = [0xF3C1, (imm3 << 12) | (imm2 << 6) | widthm1 as u16];
        let word = 0xE7E0_0050 | (widthm1 << 16) | (lsb << 7) | 1;
        same("UBFX", case, word, &ubfx, &regs, nzcv);
        let sbfx = [0xF341, (imm3 << 12) | (imm2 << 6) | widthm1 as u16];
        let word = 0xE7A0_0050 | (widthm1 << 16) | (lsb << 7) | 1;
        same("SBFX", case, word, &sbfx, &regs, nzcv);
        // BFI r0, r1, #lsb, #width (msb >= lsb) and BFC r0, #lsb, #width.
        let bfi = [0xF361, (imm3 << 12) | (imm2 << 6) | msb as u16];
        let word = 0xE7C0_0010 | (msb << 16) | (lsb << 7) | 1;
        same("BFI", case, word, &bfi, &regs, nzcv);
        let bfc = [0xF36F, (imm3 << 12) | (imm2 << 6) | msb as u16];
        let word = 0xE7C0_001F | (msb << 16) | (lsb << 7);
        same("BFC", case, word, &bfc, &regs, nzcv);
    }
}

#[test]
fn thumb2_extends_and_reversals_match_a32() {
    let mut p = Prng(0x1609);
    for case in 0..CASES {
        let regs = p.regs();
        let nzcv = p.nzcv();
        let rot = p.below(4);
        // SXTB r0, r2, ROR #(8 rot) ; UXTH r0, r2, ROR ...
        let sxtb = [0xFA4F, 0xF080 | ((rot as u16) << 4) | 2];
        let word = 0xE6AF_0072 | (rot << 10);
        same("SXTB", case, word, &sxtb, &regs, nzcv);
        let uxth = [0xFA1F, 0xF080 | ((rot as u16) << 4) | 2];
        let word = 0xE6FF_0072 | (rot << 10);
        same("UXTH", case, word, &uxth, &regs, nzcv);
        // REV r0, r2 ; RBIT r0, r2 ; CLZ r0, r2 ; REV16 r0, r2 ; REVSH r0, r2
        same("REV", case, 0xE6BF_0F32, &[0xFA92, 0xF082], &regs, nzcv);
        same("RBIT", case, 0xE6FF_0F32, &[0xFA92, 0xF0A2], &regs, nzcv);
        same("REV16", case, 0xE6BF_0FB2, &[0xFA92, 0xF092], &regs, nzcv);
        same("REVSH", case, 0xE6FF_0FB2, &[0xFA92, 0xF0B2], &regs, nzcv);
        same("CLZ", case, 0xE16F_0F12, &[0xFAB2, 0xF082], &regs, nzcv);
    }
}
