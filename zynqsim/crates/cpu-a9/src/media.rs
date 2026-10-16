//! The media group (bits 27:25 = 011 with bit 4 set): extends, bit fields, saturation,
//! reversal, packing, the parallel add/subtract family and the most-significant-word
//! multiplies. SDIV and UDIV are undefined on the Cortex-A9 and report `Undefined`.

use crate::exec::sat_i64;
use crate::state::{Cpu, Flow, GE_MASK, Q_BIT};

impl Cpu {
    /// Decodes and executes one media instruction (bit 4 set, bits 27:25 = 011).
    pub(crate) fn media(&mut self, insn: u32) -> Flow {
        let op8 = (insn >> 20) & 0xFF;
        let rn = (insn >> 16) & 0xF;
        let rd = (insn >> 12) & 0xF;
        let rm = insn & 0xF;
        if insn & 0x0FFF_0FF0 == 0x06BF_0F30 {
            let v = self.reg_or_pc(rm, 0).swap_bytes();
            self.write_gpr(rd, v);
            return Flow::Done;
        }
        if insn & 0x0FFF_0FF0 == 0x06BF_0FB0 {
            let x = self.reg_or_pc(rm, 0);
            self.write_gpr(rd, ((x & 0x00FF_00FF) << 8) | ((x >> 8) & 0x00FF_00FF));
            return Flow::Done;
        }
        if insn & 0x0FFF_0FF0 == 0x06FF_0FB0 {
            let x = self.reg_or_pc(rm, 0);
            let r = (((x & 0xFF) << 8) | ((x >> 8) & 0xFF)) as u16 as i16 as i32 as u32;
            self.write_gpr(rd, r);
            return Flow::Done;
        }
        if insn & 0x0FFF_0FF0 == 0x06FF_0F30 {
            let v = self.reg_or_pc(rm, 0).reverse_bits();
            self.write_gpr(rd, v);
            return Flow::Done;
        }
        if insn & 0xF0 == 0x70 && self.extend(op8, rn, rd, rm, insn).is_some() {
            return Flow::Done;
        }
        if insn & 0x0FE0_0070 == 0x07E0_0050 || insn & 0x0FE0_0070 == 0x07A0_0050 {
            // UBFX and SBFX.
            let width = ((insn >> 16) & 0x1F) + 1;
            let lsb = (insn >> 7) & 0x1F;
            let src = self.reg_or_pc(rm, 0);
            let field = if width >= 32 {
                src >> lsb
            } else {
                (src >> lsb) & ((1u32 << width) - 1)
            };
            let v = if insn & 0x0FE0_0070 == 0x07A0_0050 && width < 32 {
                let shift = 32 - width;
                (((field << shift) as i32) >> shift) as u32
            } else {
                field
            };
            self.write_gpr(rd, v);
            return Flow::Done;
        }
        if insn & 0x0FE0_0070 == 0x07C0_0010 {
            // BFC (Rn = PC) and BFI.
            let msb = (insn >> 16) & 0x1F;
            let lsb = (insn >> 7) & 0x1F;
            if msb < lsb {
                return Flow::Unimpl;
            }
            let width = msb - lsb + 1;
            let mask = if width >= 32 {
                u32::MAX
            } else {
                ((1u32 << width) - 1) << lsb
            };
            let old = self.r[rd as usize];
            let src = self.reg_or_pc(rm, 0);
            let new = if rm == 15 {
                old & !mask
            } else {
                (old & !mask) | ((src << lsb) & mask)
            };
            self.write_gpr(rd, new);
            return Flow::Done;
        }
        if insn & 0x0FE0_0030 == 0x06A0_0010 || insn & 0x0FE0_0030 == 0x06E0_0010 {
            return self.ssat_usat(insn);
        }
        if insn & 0x0FF0_0030 == 0x0680_0010 {
            return self.pkh(insn);
        }
        if insn & 0x0FF0_00D0 == 0x0750_0010 {
            // SMMLA / SMMUL (with the R bit for rounding).
            let ra = (insn >> 12) & 0xF;
            let round = insn & (1 << 5) != 0;
            let a = i64::from(self.reg_or_pc(rn, 0) as i32);
            let b = i64::from(self.reg_or_pc((insn >> 8) & 0xF, 0) as i32);
            let mut p = a * b;
            if round {
                p += 0x8000_0000;
            }
            let hi = (p >> 32) as i32;
            let v = if ra == 15 {
                hi as u32
            } else {
                (self.reg_or_pc(ra, 0) as i32).wrapping_add(hi) as u32
            };
            self.write_gpr(rd, v);
            return Flow::Done;
        }
        if insn & 0x0F80_0000 == 0x0600_0000 {
            return self.parallel(insn);
        }
        if insn & 0x0FF0_F0F0 == 0x0710_F010 || insn & 0x0FF0_F0F0 == 0x0730_F010 {
            // SDIV and UDIV: absent from the Cortex-A9, so undefined.
            return self.undefined();
        }
        if insn & 0x0FF0_00F0 == 0x07F0_00F0 {
            return self.undefined();
        }
        Flow::Unimpl
    }

    /// SXTB, SXTH, UXTB, UXTH, the 16-bit pair forms, and the add-to-extend forms (Rn != PC).
    /// Returns `None` for the opcodes outside this group.
    fn extend(&mut self, op8: u32, rn: u32, rd: u32, rm: u32, insn: u32) -> Option<()> {
        let rot = ((insn >> 10) & 3) * 8;
        let v = self.reg_or_pc(rm, 0).rotate_right(rot);
        let n = if rn == 15 { 0 } else { self.reg_or_pc(rn, 0) };
        let accumulate = rn != 15;
        let lo = |x: u32| x & 0xFFFF;
        let hi = |x: u32| (x >> 16) & 0xFFFF;
        let b = |x: u32, i: u32| (x >> (8 * i)) & 0xFF;
        let r = match op8 {
            0x68 => {
                // SXTB16 / SXTAB16
                let l = (b(v, 0) as u8 as i8 as i32 as u32) & 0xFFFF;
                let h = (b(v, 2) as u8 as i8 as i32 as u32) & 0xFFFF;
                if accumulate {
                    ((hi(n) + h) & 0xFFFF) << 16 | ((lo(n) + l) & 0xFFFF)
                } else {
                    (h << 16) | l
                }
            }
            0x6C => {
                // UXTB16 / UXTAB16
                let l = b(v, 0);
                let h = b(v, 2);
                if accumulate {
                    ((hi(n) + h) & 0xFFFF) << 16 | ((lo(n) + l) & 0xFFFF)
                } else {
                    (h << 16) | l
                }
            }
            0x6A => {
                let x = v as u8 as i8 as i32 as u32;
                if accumulate { n.wrapping_add(x) } else { x }
            }
            0x6E => {
                let x = v & 0xFF;
                if accumulate { n.wrapping_add(x) } else { x }
            }
            0x6B => {
                let x = v as u16 as i16 as i32 as u32;
                if accumulate { n.wrapping_add(x) } else { x }
            }
            0x6F => {
                let x = v & 0xFFFF;
                if accumulate { n.wrapping_add(x) } else { x }
            }
            _ => return None,
        };
        self.write_gpr(rd, r);
        Some(())
    }

    /// SSAT and USAT (the operand is shifted by `sh`, then saturated; Q on saturation).
    fn ssat_usat(&mut self, insn: u32) -> Flow {
        let signed = insn & (1 << 22) == 0;
        let sat_imm = (insn >> 16) & 0x1F;
        let rd = (insn >> 12) & 0xF;
        let imm5 = (insn >> 7) & 0x1F;
        let sh = insn & (1 << 6) != 0;
        let rn = insn & 0xF;
        let raw = self.reg_or_pc(rn, 0);
        // ASR #32 (imm5 = 0 with sh set) is the sign fill, which `>> 31` gives.
        let operand = if sh {
            let amount = if imm5 == 0 { 31 } else { imm5 };
            i64::from((raw as i32) >> amount)
        } else {
            i64::from((raw << imm5) as i32)
        };
        let (v, sat) = if signed {
            let bits = sat_imm + 1;
            sat_i64(operand, bits)
        } else {
            let hi = (1i64 << sat_imm) - 1;
            if operand > hi {
                (hi, true)
            } else if operand < 0 {
                (0, true)
            } else {
                (operand, false)
            }
        };
        if sat {
            self.cpsr |= Q_BIT;
        }
        self.write_gpr(rd, v as u32);
        Flow::Done
    }

    /// PKHBT and PKHTB: pack a halfword of Rn with a halfword of the shifted Rm.
    fn pkh(&mut self, insn: u32) -> Flow {
        let rn = self.reg_or_pc((insn >> 16) & 0xF, 0);
        let rd = (insn >> 12) & 0xF;
        let imm5 = (insn >> 7) & 0x1F;
        let rm = self.reg_or_pc(insn & 0xF, 0);
        let v = if insn & (1 << 6) == 0 {
            // PKHBT: bottom from Rn, top from Rm << imm5.
            (rn & 0xFFFF) | ((rm << imm5) & 0xFFFF_0000)
        } else {
            // PKHTB: top from Rn, bottom from Rm >> imm5 (arithmetic, 0 means 32).
            let amount = if imm5 == 0 { 31 } else { imm5 };
            let shifted = ((rm as i32) >> amount) as u32;
            (rn & 0xFFFF_0000) | (shifted & 0xFFFF)
        };
        self.write_gpr(rd, v);
        Flow::Done
    }

    /// The parallel add and subtract family: SADD, SSUB, SASX, SAX, ... and the Q, SH, U, UQ and
    /// UH forms, on halfwords (op2 0..3) or bytes (op2 4 and 7).
    fn parallel(&mut self, insn: u32) -> Flow {
        let op1 = (insn >> 20) & 7;
        let op2 = (insn >> 5) & 7;
        let mode = match op1 {
            1 | 5 => Lanes::Plain,
            2 | 6 => Lanes::Sat,
            3 | 7 => Lanes::Half,
            _ => return Flow::Unimpl,
        };
        let signed = op1 < 4;
        let a = self.reg_or_pc((insn >> 16) & 0xF, 0);
        let b = self.reg_or_pc(insn & 0xF, 0);
        // Per-lane (source a index, source b index, subtract?) for the lanes of this op.
        let (width, lanes): (u32, [(usize, usize, bool); 4]) = match op2 {
            0 => (
                16,
                [(0, 0, false), (1, 1, false), (0, 0, false), (0, 0, false)],
            ),
            3 => (
                16,
                [(0, 0, true), (1, 1, true), (0, 0, false), (0, 0, false)],
            ),
            1 => (
                16,
                [(0, 1, true), (1, 0, false), (0, 0, false), (0, 0, false)],
            ),
            2 => (
                16,
                [(0, 1, false), (1, 0, true), (0, 0, false), (0, 0, false)],
            ),
            4 => (
                8,
                [(0, 0, false), (1, 1, false), (2, 2, false), (3, 3, false)],
            ),
            7 => (8, [(0, 0, true), (1, 1, true), (2, 2, true), (3, 3, true)]),
            _ => return Flow::Unimpl,
        };
        let count = if width == 16 { 2 } else { 4 };
        let mut result = 0u32;
        let mut ge = 0u32;
        for (i, &(ia, ib, sub)) in lanes.iter().take(count).enumerate() {
            let x = lane(a, ia, width, signed);
            let y = lane(b, ib, width, signed);
            let s = if sub { x - y } else { x + y };
            let w = width;
            let full = 1i64 << w;
            let (value, ge_bit) = match mode {
                Lanes::Plain => {
                    // Signed sums and differences: no borrow/carry out of the lane is "GE".
                    // Unsigned adds: GE is the carry out of the lane; unsigned subtracts: no borrow.
                    let g = if signed || sub { s >= 0 } else { s >= full };
                    ((s as u32) & ((1u32 << w) - 1), g)
                }
                Lanes::Sat => {
                    let (c, _) = if signed {
                        sat_i64(s, w)
                    } else {
                        (s.clamp(0, full - 1), false)
                    };
                    ((c as u32) & ((1u32 << w) - 1), false)
                }
                Lanes::Half => {
                    let h = if signed {
                        s >> 1
                    } else {
                        // Unsigned halving works on the (w+1)-bit wrapped difference.
                        ((s & ((full << 1) - 1)) >> 1) & (full - 1)
                    };
                    ((h as u32) & ((1u32 << w) - 1), false)
                }
            };
            result |= value << (i as u32 * width);
            if matches!(mode, Lanes::Plain) && ge_bit {
                ge |= if width == 16 { 0b11 << (2 * i) } else { 1 << i };
            }
        }
        if matches!(mode, Lanes::Plain) {
            self.cpsr = (self.cpsr & !GE_MASK) | (ge << 16);
        }
        self.write_gpr((insn >> 12) & 0xF, result);
        Flow::Done
    }
}

/// The three result modes of the parallel family.
#[derive(Clone, Copy)]
enum Lanes {
    /// Modular, sets GE.
    Plain,
    /// Saturating.
    Sat,
    /// Halving.
    Half,
}

/// Lane `i` of width `w` (8 or 16) of `v`, sign- or zero-extended.
fn lane(v: u32, i: usize, w: u32, signed: bool) -> i64 {
    let shifted = (v >> (i as u32 * w)) & ((1u32 << w) - 1);
    if signed {
        let sign = 1u32 << (w - 1);
        if shifted & sign != 0 {
            i64::from(shifted) - (1i64 << w)
        } else {
            i64::from(shifted)
        }
    } else {
        i64::from(shifted)
    }
}
