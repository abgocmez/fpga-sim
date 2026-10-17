//! VFPv3-D32 floating point of the Cortex-A9 in A32 (ARM ARM A7.5 and A7.7): the register
//! file, FPSCR, FPEXC and the CPACR access rules, the decoder and the executors. The arithmetic
//! is in `vfp/softfp.rs`.
//!
//! Reported `Unimplemented` (with a reason, see [`Cpu::vfp_unimplemented_reason`]): NEON, short
//! vectors (FPSCR.LEN != 0), half-precision conversions, the legacy FLDMX/FSTMX forms and the
//! scalar moves to Dn[x]. UNDEFINED: VFPv4 (VFMA, VFMS, VFNMA, VFNMS) and ARMv8 (VRINT*, VCVTA,
//! VCVTN, VCVTP, VCVTM, VSEL, VMAXNM, VMINNM), as on the Cortex-A9.

pub(crate) mod softfp;
mod xfer;

use ps_bus::Bus;

use crate::state::{Cpu, Flow, Mode};
use softfp::{DOUBLE, Env, Fmt, Round, SINGLE};

/// FPSID (modelled value, not checked against a TRM: see ADR-030).
const FPSID: u32 = 0x4103_3090;
/// MVFR0: VFPv3 with 32 double registers, single and double precision, rounding modes.
const MVFR0: u32 = 0x1011_0221;
/// MVFR1: no VFPv4 fused multiply-add, no half precision (modelled, see ADR-030).
const MVFR1: u32 = 0x1100_0011;
/// FPEXC.EN.
const FPEXC_EN: u32 = 1 << 30;
/// FPEXC.EX.
const FPEXC_EX: u32 = 1 << 31;
/// FPSCR.NZCV.
const FPSCR_NZCV: u32 = 0xF000_0000;
/// FPSCR.LEN (short vector length minus one).
const FPSCR_LEN: u32 = 7 << 16;
/// The FPSCR bits VMSR writes: NZCV, QC, AHP, DN, FZ, RMode, Stride, Len, IDC and the
/// cumulative flags. Trap enables (IDE, IXE, UFE, OFE, DZE, IOE) and the rest read as zero.
const FPSCR_WRITABLE: u32 = FPSCR_NZCV
    | (1 << 27)
    | (1 << 26)
    | (1 << 25)
    | (1 << 24)
    | (3 << 22)
    | (3 << 20)
    | FPSCR_LEN
    | (1 << 7)
    | 0x1F;

const SHORT_VECTOR: &str = "VFP short vectors (FPSCR.LEN != 0) are not implemented";
const HALF_PRECISION: &str = "VCVTB/VCVTT (half precision) are not implemented";
const LEGACY_LDM: &str = "FLDMX/FSTMX (odd word count, legacy) are not implemented";
const NEON_SCALAR: &str = "VMOV between a core register and Dn[x] (NEON) is not implemented";

/// The floating-point state of one core.
pub(crate) struct Vfp {
    /// D0 to D31; S(2n) is the low word of D(n), S(2n+1) the high word.
    pub(crate) d: [u64; 32],
    pub(crate) fpscr: u32,
    pub(crate) fpexc: u32,
    /// Why the last VFP instruction was reported `Unimplemented`.
    pub(crate) why: Option<&'static str>,
}

impl Vfp {
    pub(crate) fn new() -> Vfp {
        Vfp {
            d: [0; 32],
            fpscr: 0,
            fpexc: 0,
            why: None,
        }
    }

    fn s(&self, n: u32) -> u32 {
        let w = self.d[(n >> 1) as usize];
        if n & 1 == 0 {
            w as u32
        } else {
            (w >> 32) as u32
        }
    }

    fn set_s(&mut self, n: u32, v: u32) {
        let w = &mut self.d[(n >> 1) as usize];
        if n & 1 == 0 {
            *w = (*w & 0xFFFF_FFFF_0000_0000) | u64::from(v);
        } else {
            *w = (*w & 0xFFFF_FFFF) | (u64::from(v) << 32);
        }
    }

    /// The value of register `n` (an S or D register number) at precision `dbl`.
    fn get(&self, dbl: bool, n: u32) -> u64 {
        if dbl {
            self.d[n as usize]
        } else {
            u64::from(self.s(n))
        }
    }

    fn set(&mut self, dbl: bool, n: u32, v: u64) {
        if dbl {
            self.d[n as usize] = v;
        } else {
            self.set_s(n, v as u32);
        }
    }
}

/// Bits `hi` down to `lo` of `insn`.
fn field(insn: u32, hi: u32, lo: u32) -> u32 {
    (insn >> lo) & ((1u32 << (hi - lo + 1)) - 1)
}

/// The single register number `hi4:lowbit`.
fn s_reg(hi4: u32, lowbit: u32) -> u32 {
    (hi4 << 1) | lowbit
}

/// The double register number `msb:lo4`.
fn d_reg(lo4: u32, msb: u32) -> u32 {
    (msb << 4) | lo4
}

/// Register numbers of a VFP data-processing instruction: Vd/D, Vn/N and Vm/M.
#[derive(Clone, Copy)]
struct Regs {
    d: u32,
    n: u32,
    m: u32,
}

fn regs(insn: u32, dbl: bool) -> Regs {
    let (vd, vn, vm) = (field(insn, 15, 12), field(insn, 19, 16), field(insn, 3, 0));
    let (dd, dn, dm) = (field(insn, 22, 22), field(insn, 7, 7), field(insn, 5, 5));
    if dbl {
        Regs {
            d: d_reg(vd, dd),
            n: d_reg(vn, dn),
            m: d_reg(vm, dm),
        }
    } else {
        Regs {
            d: s_reg(vd, dd),
            n: s_reg(vn, dn),
            m: s_reg(vm, dm),
        }
    }
}

/// The single register Vd:D of `insn`.
fn sd(insn: u32) -> u32 {
    s_reg(field(insn, 15, 12), field(insn, 22, 22))
}

/// The single register Vm:M of `insn`.
fn sm(insn: u32) -> u32 {
    s_reg(field(insn, 3, 0), field(insn, 5, 5))
}

/// The double register D:Vd of `insn`.
fn dd(insn: u32) -> u32 {
    d_reg(field(insn, 15, 12), field(insn, 22, 22))
}

/// The double register M:Vm of `insn`.
fn dm(insn: u32) -> u32 {
    d_reg(field(insn, 3, 0), field(insn, 5, 5))
}

/// Whether `insn` is a coprocessor 10 or 11 encoding that belongs to the FPU: LDC/STC and
/// MCRR/MRRC (bits 27:25 = 110), or CDP/MCR/MRC (bits 27:24 = 1110).
pub(crate) fn is_fp_space(insn: u32) -> bool {
    let cp = field(insn, 11, 8);
    let space = (insn >> 25) & 7;
    (cp == 10 || cp == 11) && (space == 6 || (space == 7 && insn & (1 << 24) == 0))
}

/// The VFP operations of the three-operand group.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Op {
    Mla,
    Mls,
    Nmla,
    Nmls,
    Mul,
    Nmul,
    Add,
    Sub,
    Div,
}

/// The one-operand operations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Unary {
    Mov,
    Abs,
    Neg,
    Sqrt,
}

fn fmt_of(dbl: bool) -> Fmt {
    if dbl { DOUBLE } else { SINGLE }
}

/// `op` on bit patterns of format `f`. Multiply-accumulate rounds the product and then the sum.
fn arith(f: Fmt, op: Op, d: u64, n: u64, m: u64, env: &Env, fl: &mut u32) -> u64 {
    match op {
        Op::Add => softfp::add(f, n, m, false, env, fl),
        Op::Sub => softfp::add(f, n, m, true, env, fl),
        Op::Mul => softfp::mul(f, n, m, env, fl),
        Op::Div => softfp::div(f, n, m, env, fl),
        Op::Nmul => f.negate(softfp::mul(f, n, m, env, fl)),
        Op::Mla => {
            let p = softfp::mul(f, n, m, env, fl);
            softfp::add(f, d, p, false, env, fl)
        }
        Op::Mls => {
            let p = softfp::mul(f, n, m, env, fl);
            softfp::add(f, d, p, true, env, fl)
        }
        Op::Nmls => {
            let p = softfp::mul(f, n, m, env, fl);
            softfp::add(f, p, d, true, env, fl)
        }
        Op::Nmla => {
            let p = softfp::mul(f, n, m, env, fl);
            softfp::add(f, f.negate(d), f.negate(p), false, env, fl)
        }
    }
}

impl Cpu {
    /// The reason the last VFP instruction was reported `Unimplemented`, if it was one.
    pub fn vfp_unimplemented_reason(&self) -> Option<&'static str> {
        self.vfp.why
    }

    /// Entry point for the coprocessor 10/11 encodings (see [`is_fp_space`]).
    pub(crate) fn vfp_insn(&mut self, bus: &mut dyn Bus, insn: u32) -> Flow {
        if (insn >> 25) & 7 == 6 {
            if field(insn, 27, 21) == 0b1100010 {
                return self.vfp_pair(insn);
            }
            return self.vfp_mem(bus, insn);
        }
        if insn & (1 << 4) == 0 {
            self.vfp_data(insn)
        } else {
            self.vfp_transfer(insn)
        }
    }

    /// CheckVFPEnabled: CPACR.cp10 and cp11 grant access (both 0b11, or both 0b01 for
    /// privileged code; any other combination denies), FPEXC.EN when `need_en`, and privilege
    /// when `privileged`.
    fn vfp_access(&self, need_en: bool, privileged: bool) -> bool {
        let cp10 = (self.cp15.cpacr >> 20) & 3;
        let cp11 = (self.cp15.cpacr >> 22) & 3;
        let user = self.mode() == Mode::User;
        let granted = (cp10 == 3 && cp11 == 3) || (cp10 == 1 && cp11 == 1 && !user);
        granted && !(privileged && user) && !(need_en && self.vfp.fpexc & FPEXC_EN == 0)
    }

    fn vfp_env(&self) -> Env {
        Env {
            rm: Round::from_bits(self.vfp.fpscr >> 22),
            fz: self.vfp.fpscr & (1 << 24) != 0,
            dn: self.vfp.fpscr & (1 << 25) != 0,
        }
    }

    fn vfp_unimpl(&mut self, why: &'static str) -> Flow {
        self.vfp.why = Some(why);
        Flow::Unimpl
    }

    /// CDP-form data processing (bit 4 clear).
    fn vfp_data(&mut self, insn: u32) -> Flow {
        if !self.vfp_access(true, false) {
            return self.undefined();
        }
        let dbl = insn & (1 << 8) != 0;
        let opc = field(insn, 21, 20);
        let b6 = insn & (1 << 6) != 0;
        if insn & (1 << 23) == 0 {
            let op = match (opc, b6) {
                (0, false) => Op::Mla,
                (0, true) => Op::Mls,
                (1, false) => Op::Nmls,
                (1, true) => Op::Nmla,
                (2, false) => Op::Mul,
                (2, true) => Op::Nmul,
                (3, false) => Op::Add,
                _ => Op::Sub,
            };
            return self.vfp_arith(insn, dbl, op);
        }
        match (opc, b6) {
            (0, false) => self.vfp_arith(insn, dbl, Op::Div),
            (3, _) => self.vfp_two_op(insn, dbl),
            // VFMA, VFMS, VFNMA, VFNMS (VFPv4) and the reserved (0, true): UNDEFINED here.
            _ => self.undefined(),
        }
    }

    fn vfp_arith(&mut self, insn: u32, dbl: bool, op: Op) -> Flow {
        if self.vfp.fpscr & FPSCR_LEN != 0 {
            return self.vfp_unimpl(SHORT_VECTOR);
        }
        let r = regs(insn, dbl);
        let env = self.vfp_env();
        let mut fl = 0;
        let f = fmt_of(dbl);
        let res = arith(
            f,
            op,
            self.vfp.get(dbl, r.d),
            self.vfp.get(dbl, r.n),
            self.vfp.get(dbl, r.m),
            &env,
            &mut fl,
        );
        self.vfp.set(dbl, r.d, res);
        self.vfp.fpscr |= fl;
        Flow::Done
    }

    /// The two-operand group (bit 23 set, opc 0b11): immediates, moves, unary operations,
    /// comparisons and conversions. `opc2` is bits 19:16.
    fn vfp_two_op(&mut self, insn: u32, dbl: bool) -> Flow {
        let opc2 = field(insn, 19, 16);
        let b7 = insn & (1 << 7) != 0;
        let b6 = insn & (1 << 6) != 0;
        if !b7 && !b6 {
            return self.vfp_imm(insn, dbl);
        }
        if !b6 {
            return self.undefined();
        }
        match opc2 {
            0 if b7 => self.vfp_unary(insn, dbl, Unary::Abs),
            0 => self.vfp_unary(insn, dbl, Unary::Mov),
            1 if b7 => self.vfp_unary(insn, dbl, Unary::Sqrt),
            1 => self.vfp_unary(insn, dbl, Unary::Neg),
            2 | 3 => {
                if !self.vfp_access(true, false) {
                    return self.undefined();
                }
                self.vfp_unimpl(HALF_PRECISION)
            }
            4 | 5 => self.vfp_cmp(insn, dbl, b7, opc2 == 5),
            7 if b7 => self.vfp_cvt_ff(insn, dbl),
            8 => self.vfp_int_to_float(insn, dbl, b7),
            0b1100 | 0b1101 => self.vfp_float_to_int(insn, dbl, b7, opc2 & 1 == 1),
            0b1010 | 0b1011 => self.vfp_fixed_to_float(insn, dbl, b7, opc2 & 1 == 1),
            0b1110 | 0b1111 => self.vfp_float_to_fixed(insn, dbl, b7, opc2 & 1 == 1),
            // VRINT* and VCVT{A,N,P,M} (ARMv8), and the reserved opcodes.
            _ => self.undefined(),
        }
    }

    /// VMOV (immediate).
    fn vfp_imm(&mut self, insn: u32, dbl: bool) -> Flow {
        if self.vfp.fpscr & FPSCR_LEN != 0 {
            return self.vfp_unimpl(SHORT_VECTOR);
        }
        let imm8 = (field(insn, 19, 16) << 4) | field(insn, 3, 0);
        let d = regs(insn, dbl).d;
        let v = softfp::expand_imm(fmt_of(dbl), imm8);
        self.vfp.set(dbl, d, v);
        Flow::Done
    }

    /// VMOV (register), VABS, VNEG and VSQRT. The sign operations are bit operations: they
    /// raise no exception and do not quieten a signaling NaN.
    fn vfp_unary(&mut self, insn: u32, dbl: bool, kind: Unary) -> Flow {
        if self.vfp.fpscr & FPSCR_LEN != 0 {
            return self.vfp_unimpl(SHORT_VECTOR);
        }
        let r = regs(insn, dbl);
        let f = fmt_of(dbl);
        let m = self.vfp.get(dbl, r.m);
        let res = match kind {
            Unary::Mov => m,
            Unary::Abs => f.abs(m),
            Unary::Neg => f.negate(m),
            Unary::Sqrt => {
                let env = self.vfp_env();
                let mut fl = 0;
                let v = softfp::sqrt(f, m, &env, &mut fl);
                self.vfp.fpscr |= fl;
                v
            }
        };
        self.vfp.set(dbl, r.d, res);
        Flow::Done
    }

    /// VCMP and VCMPE (`zero` compares with +0.0; the Vm field is then ignored).
    fn vfp_cmp(&mut self, insn: u32, dbl: bool, signaling: bool, zero: bool) -> Flow {
        let r = regs(insn, dbl);
        let env = self.vfp_env();
        let a = self.vfp.get(dbl, r.d);
        let b = if zero { 0 } else { self.vfp.get(dbl, r.m) };
        let mut fl = 0;
        let nzcv = softfp::compare(fmt_of(dbl), a, b, signaling, &env, &mut fl);
        self.vfp.fpscr = (self.vfp.fpscr & !FPSCR_NZCV) | nzcv;
        self.vfp.fpscr |= fl;
        Flow::Done
    }

    /// VCVT between single and double: the source precision is bit 8.
    fn vfp_cvt_ff(&mut self, insn: u32, src_dbl: bool) -> Flow {
        if self.vfp.fpscr & FPSCR_LEN != 0 {
            return self.vfp_unimpl(SHORT_VECTOR);
        }
        let env = self.vfp_env();
        let (from, to) = if src_dbl {
            (DOUBLE, SINGLE)
        } else {
            (SINGLE, DOUBLE)
        };
        let (m, d) = if src_dbl {
            (dm(insn), sd(insn))
        } else {
            (sm(insn), dd(insn))
        };
        let v = self.vfp.get(src_dbl, m);
        let mut fl = 0;
        let res = softfp::convert(from, to, v, &env, &mut fl);
        self.vfp.set(!src_dbl, d, res);
        self.vfp.fpscr |= fl;
        Flow::Done
    }

    /// VCVT from a signed or unsigned 32-bit integer in Sm to a float (precision bit 8).
    fn vfp_int_to_float(&mut self, insn: u32, dst_dbl: bool, signed: bool) -> Flow {
        if self.vfp.fpscr & FPSCR_LEN != 0 {
            return self.vfp_unimpl(SHORT_VECTOR);
        }
        let (m, d) = (sm(insn), if dst_dbl { dd(insn) } else { sd(insn) });
        let v = self.vfp.s(m);
        let (sign, mag) = if signed {
            let x = v as i32;
            (u64::from(x < 0), u64::from(x.unsigned_abs()))
        } else {
            (0, u64::from(v))
        };
        let env = self.vfp_env();
        let mut fl = 0;
        let res = softfp::from_int(fmt_of(dst_dbl), sign, mag, 0, &env, &mut fl);
        self.vfp.set(dst_dbl, d, res);
        self.vfp.fpscr |= fl;
        Flow::Done
    }

    /// VCVT (VCVTR) from a float to a 32-bit integer in Sd. bit 7 selects round to zero;
    /// otherwise FPSCR.RMode applies. bit 16 selects signed.
    fn vfp_float_to_int(
        &mut self,
        insn: u32,
        src_dbl: bool,
        round_zero: bool,
        signed: bool,
    ) -> Flow {
        if self.vfp.fpscr & FPSCR_LEN != 0 {
            return self.vfp_unimpl(SHORT_VECTOR);
        }
        let (m, d) = (if src_dbl { dm(insn) } else { sm(insn) }, sd(insn));
        let env = self.vfp_env();
        let rm = if round_zero { Round::Zero } else { env.rm };
        let a = self.vfp.get(src_dbl, m);
        let mut fl = 0;
        let res = softfp::to_int(fmt_of(src_dbl), a, 0, rm, signed, 32, &env, &mut fl);
        self.vfp.set_s(d, res);
        self.vfp.fpscr |= fl;
        Flow::Done
    }

    /// The fraction-bit count of a fixed-point VCVT: `width - imm`, where imm is bits 3:0 with
    /// bit 5 as the least significant bit. `None` when it is out of range.
    fn fixed_bits(insn: u32, width: u32) -> Option<i64> {
        let imm = (field(insn, 3, 0) << 1) | field(insn, 5, 5);
        if imm > width {
            None
        } else {
            Some(i64::from(width - imm))
        }
    }

    /// VCVT from a fixed-point integer held in Vd to a float in Vd. bit 7 set means 32 bits,
    /// clear 16 bits; bit 16 set means unsigned.
    fn vfp_fixed_to_float(&mut self, insn: u32, dbl: bool, wide: bool, unsigned: bool) -> Flow {
        if self.vfp.fpscr & FPSCR_LEN != 0 {
            return self.vfp_unimpl(SHORT_VECTOR);
        }
        let width = if wide { 32 } else { 16 };
        let Some(fbits) = Self::fixed_bits(insn, width) else {
            return self.undefined();
        };
        let r = regs(insn, dbl);
        // The integer sits in the low word of the destination register.
        let raw = if dbl {
            self.vfp.d[r.d as usize] as u32
        } else {
            self.vfp.s(r.d)
        };
        let (sign, mag) = match (wide, unsigned) {
            (true, false) => {
                let x = raw as i32;
                (u64::from(x < 0), u64::from(x.unsigned_abs()))
            }
            (true, true) => (0, u64::from(raw)),
            (false, false) => {
                let x = raw as u16 as i16;
                (u64::from(x < 0), u64::from(x.unsigned_abs()))
            }
            (false, true) => (0, u64::from(raw as u16)),
        };
        let env = self.vfp_env();
        let mut fl = 0;
        let res = softfp::from_int(fmt_of(dbl), sign, mag, -fbits, &env, &mut fl);
        self.vfp.set(dbl, r.d, res);
        self.vfp.fpscr |= fl;
        Flow::Done
    }

    /// VCVT from a float in Vd to a fixed-point integer in Vd. Rounds toward zero (the
    /// fixed-point form has no rounding-mode variant; see ADR-030).
    fn vfp_float_to_fixed(&mut self, insn: u32, dbl: bool, wide: bool, unsigned: bool) -> Flow {
        if self.vfp.fpscr & FPSCR_LEN != 0 {
            return self.vfp_unimpl(SHORT_VECTOR);
        }
        let width = if wide { 32 } else { 16 };
        let Some(fbits) = Self::fixed_bits(insn, width) else {
            return self.undefined();
        };
        let r = regs(insn, dbl);
        let env = self.vfp_env();
        let a = self.vfp.get(dbl, r.d);
        let mut fl = 0;
        let res = softfp::to_int(
            fmt_of(dbl),
            a,
            fbits,
            Round::Zero,
            !unsigned,
            width,
            &env,
            &mut fl,
        );
        if dbl {
            self.vfp.d[r.d as usize] = u64::from(res);
        } else {
            self.vfp.set_s(r.d, res);
        }
        self.vfp.fpscr |= fl;
        Flow::Done
    }
}
