//! A32 decode and the non-memory executors: data processing, multiplies, branches, status
//! registers, hints, system instructions and CP15 access. Memory and media instructions are in
//! `mem.rs` and `media.rs`.

use ps_bus::Bus;

use crate::state::{
    A_BIT, C_BIT, Cpu, Event, F_BIT, Flow, I_BIT, Mode, N_BIT, Q_BIT, T_BIT, V_BIT, Wait, Z_BIT,
};

impl Cpu {
    /// Executes `insn` at `self.cur`; `self.pc` already points at the next instruction.
    pub(crate) fn execute(&mut self, bus: &mut dyn Bus, insn: u32) -> Flow {
        let cond = insn >> 28;
        if cond == 0xF {
            return self.uncond(insn);
        }
        if !self.cond_passed(cond) {
            return Flow::Done;
        }
        match (insn >> 25) & 7 {
            0 => self.space_000(bus, insn),
            1 => self.space_001(insn),
            2 => self.ls_single(bus, insn),
            3 => {
                if insn & 0x10 != 0 {
                    self.media(insn)
                } else {
                    self.ls_single(bus, insn)
                }
            }
            4 => self.ldm_stm(bus, insn),
            5 => self.branch(insn),
            6 => Flow::Unimpl, // LDC/STC: VFP/NEON loads, or a coprocessor we do not model
            _ => {
                if insn & (1 << 24) != 0 {
                    self.svc(insn)
                } else {
                    self.coproc(bus, insn)
                }
            }
        }
    }

    /// The condition code check of the ARM ARM (`cond` is not 0b1111 here).
    fn cond_passed(&self, cond: u32) -> bool {
        let (n, z, c, v) = (
            self.cpsr & N_BIT != 0,
            self.cpsr & Z_BIT != 0,
            self.cpsr & C_BIT != 0,
            self.cpsr & V_BIT != 0,
        );
        let base = match cond >> 1 {
            0 => z,
            1 => c,
            2 => n,
            3 => v,
            4 => c && !z,
            5 => n == v,
            6 => !z && n == v,
            _ => true,
        };
        if cond & 1 == 1 && cond != 0xE {
            !base
        } else {
            base
        }
    }

    /// Writes a general register from an ALU result: R15 branches without interworking.
    pub(crate) fn write_gpr(&mut self, rd: u32, v: u32) {
        if rd == 15 {
            self.pc = v & !3;
        } else {
            self.r[rd as usize] = v;
        }
    }

    pub(crate) fn set_nz(&mut self, v: u32) {
        self.cpsr &= !(N_BIT | Z_BIT);
        if v & 0x8000_0000 != 0 {
            self.cpsr |= N_BIT;
        }
        if v == 0 {
            self.cpsr |= Z_BIT;
        }
    }

    pub(crate) fn set_flag(&mut self, bit: u32, on: bool) {
        if on {
            self.cpsr |= bit;
        } else {
            self.cpsr &= !bit;
        }
    }

    /// Data-processing, the miscellaneous space, the multiply and extra-load space (bits 27:25
    /// = 000), by the decode tables of the ARM ARM.
    fn space_000(&mut self, bus: &mut dyn Bus, insn: u32) -> Flow {
        let op = (insn >> 21) & 0xF;
        let s = insn & (1 << 20) != 0;
        if insn & 0x90 == 0x90 {
            return self.mul_or_extra(bus, insn);
        }
        if (8..=11).contains(&op) && !s {
            return self.misc(insn);
        }
        self.dp(insn)
    }

    /// Bits 27:25 = 001: the immediate forms, MOVW/MOVT, MSR (immediate) and the hints.
    fn space_001(&mut self, insn: u32) -> Flow {
        if insn & 0x0FFF_FF00 == 0x0320_F000 {
            return match insn & 0xFF {
                2 => {
                    if self.event_reg {
                        self.event_reg = false;
                    } else {
                        self.wait = Some(Wait::Wfe);
                    }
                    Flow::Event(Event::Wfe)
                }
                3 => {
                    self.wait = Some(Wait::Wfi);
                    Flow::Event(Event::Wfi)
                }
                4 => {
                    self.event_reg = true;
                    Flow::Event(Event::Sev)
                }
                // NOP, YIELD and the reserved hints.
                _ => Flow::Done,
            };
        }
        if insn & 0x0FF0_0000 == 0x0300_0000 {
            let imm = ((insn >> 4) & 0xF000) | (insn & 0xFFF);
            self.write_gpr((insn >> 12) & 0xF, imm);
            return Flow::Done;
        }
        if insn & 0x0FF0_0000 == 0x0340_0000 {
            let rd = (insn >> 12) & 0xF;
            let hi = ((insn >> 4) & 0xF000) | (insn & 0xFFF);
            let old = self.r[rd as usize];
            self.write_gpr(rd, (old & 0xFFFF) | (hi << 16));
            return Flow::Done;
        }
        if insn & 0x0FB0_0000 == 0x0320_0000 {
            let (v, _) = crate::alu::expand_imm(insn, false);
            self.msr_write(insn & (1 << 22) != 0, (insn >> 16) & 0xF, v);
            return Flow::Done;
        }
        let op = (insn >> 21) & 0xF;
        if (8..=11).contains(&op) && insn & (1 << 20) == 0 {
            return Flow::Unimpl;
        }
        self.dp(insn)
    }

    /// The data-processing instructions: all sixteen opcodes with the S bit and the shifter.
    fn dp(&mut self, insn: u32) -> Flow {
        let op = (insn >> 21) & 0xF;
        let s = insn & (1 << 20) != 0;
        let rn = (insn >> 16) & 0xF;
        let rd = (insn >> 12) & 0xF;
        let by_reg = insn & (1 << 25) == 0 && insn & 0x10 != 0;
        let (sh, sc) = self.shifter(insn, by_reg);
        let a = self.reg_or_pc(rn, if by_reg { 4 } else { 0 });
        let c_in = self.cpsr & C_BIT != 0;
        // (result, carry, overflow, arithmetic?) : arithmetic ops take C and V from the adder.
        let (res, carry, ovf, arith) = match op {
            0 | 8 => (a & sh, sc, false, false),
            1 | 9 => (a ^ sh, sc, false, false),
            12 => (a | sh, sc, false, false),
            13 => (sh, sc, false, false),
            14 => (a & !sh, sc, false, false),
            15 => (!sh, sc, false, false),
            2 | 10 => add3(a, !sh, true),
            3 => add3(!a, sh, true),
            4 | 11 => add3(a, sh, false),
            5 => add3(a, sh, c_in),
            6 => add3(a, !sh, c_in),
            _ => add3(!a, sh, c_in),
        };
        let writes = !(8..=11).contains(&op);
        let user = matches!(self.mode(), Mode::User | Mode::System);
        if s && rd == 15 && writes && !user {
            // Exception return: CPSR := SPSR, then branch to the result.
            let spsr = self.spsr(self.mode());
            self.write_cpsr(spsr, u32::MAX);
            self.pc = res & if self.cpsr & T_BIT != 0 { !1 } else { !3 };
            return Flow::Done;
        }
        if s {
            self.set_nz(res);
            if arith {
                self.set_flag(C_BIT, carry);
                self.set_flag(V_BIT, ovf);
            } else {
                self.set_flag(C_BIT, carry);
            }
        }
        if writes {
            self.write_gpr(rd, res);
        }
        Flow::Done
    }

    /// The shifter operand and its carry-out (ARM ARM A8.4.1 and A8.4.2).
    fn shifter(&self, insn: u32, by_reg: bool) -> (u32, bool) {
        let c_in = self.cpsr & C_BIT != 0;
        if insn & (1 << 25) != 0 {
            return crate::alu::expand_imm(insn, c_in);
        }
        let rm = insn & 0xF;
        let ty = (insn >> 5) & 3;
        if by_reg {
            let amount = self.reg_or_pc((insn >> 8) & 0xF, 4) & 0xFF;
            crate::alu::shift_reg(self.reg_or_pc(rm, 4), ty, amount, c_in)
        } else {
            crate::alu::shift_imm(self.reg_or_pc(rm, 0), ty, (insn >> 7) & 0x1F, c_in)
        }
    }

    /// Bits 27:25 = 000 with S clear and opcode 8..11: BX, BLX, BKPT, CLZ, MRS, MSR (register),
    /// the saturating adds and the halfword multiplies (SMULxy, SMLAxy, SMULWy, SMLAWy).
    fn misc(&mut self, insn: u32) -> Flow {
        if insn & 0x0FFF_FFF0 == 0x012F_FF10 {
            // BX (BXJ is treated as BX: Jazelle is not implemented).
            let v = self.reg_or_pc(insn & 0xF, 0);
            self.bx_write(v);
            return Flow::Done;
        }
        if insn & 0x0FFF_FFF0 == 0x012F_FF20 {
            let v = self.reg_or_pc(insn & 0xF, 0);
            self.bx_write(v);
            return Flow::Done;
        }
        if insn & 0x0FFF_FFF0 == 0x012F_FF30 {
            let target = self.reg_or_pc(insn & 0xF, 0);
            self.r[14] = self.cur.wrapping_add(4);
            self.bx_write(target);
            return Flow::Done;
        }
        if insn & 0x0FBF_0FFF == 0x010F_0000 {
            let rd = (insn >> 12) & 0xF;
            let v = if insn & (1 << 22) != 0 {
                self.spsr(self.mode())
            } else {
                self.cpsr
            };
            self.write_gpr(rd, v);
            return Flow::Done;
        }
        if insn & 0x0FB0_FFF0 == 0x0120_F000 {
            let v = self.reg_or_pc(insn & 0xF, 0);
            self.msr_write(insn & (1 << 22) != 0, (insn >> 16) & 0xF, v);
            return Flow::Done;
        }
        if insn & 0x0FFF_0FF0 == 0x016F_0F10 {
            let v = self.reg_or_pc(insn & 0xF, 0);
            self.write_gpr((insn >> 12) & 0xF, v.leading_zeros());
            return Flow::Done;
        }
        if insn & 0x0FF0_00F0 == 0x0120_0070 {
            let imm = (((insn >> 4) & 0xFFF0) | (insn & 0xF)) as u16;
            self.exception_bkpt();
            return Flow::Event(Event::Bkpt(imm));
        }
        if insn & 0x0F90_0FF0 == 0x0100_0050 {
            return self.saturating_add_sub(insn);
        }
        if insn & 0x0FF0_F090 == 0x0160_0080 {
            // SMULxy
            let (x, y) = halves(insn);
            let p = half_of(self.reg_or_pc(insn & 0xF, 0), x)
                .wrapping_mul(half_of(self.reg_or_pc((insn >> 8) & 0xF, 0), y));
            self.write_gpr((insn >> 16) & 0xF, p as u32);
            return Flow::Done;
        }
        if insn & 0x0FF0_0090 == 0x0100_0080 {
            // SMLAxy: Rd = Rn.x * Rm.y + Ra, Q on overflow.
            let (x, y) = halves(insn);
            let p = i64::from(half_of(self.reg_or_pc(insn & 0xF, 0), x))
                * i64::from(half_of(self.reg_or_pc((insn >> 8) & 0xF, 0), y));
            let acc = i64::from(self.reg_or_pc((insn >> 12) & 0xF, 0) as i32);
            let sum = p + acc;
            let (v, sat) = sat_i64(sum, 32);
            if sat {
                self.cpsr |= Q_BIT;
            }
            self.write_gpr((insn >> 16) & 0xF, v as u32);
            return Flow::Done;
        }
        if insn & 0x0FF0_F0B0 == 0x0120_00A0 || insn & 0x0FF0_00B0 == 0x0120_0080 {
            // SMULWy (bit 5 set) and SMLAWy (bit 5 clear): (Rn * Rm.y) >> 16, plus Ra for SMLAW.
            let y = insn & (1 << 6) != 0;
            let prod = i64::from(self.reg_or_pc(insn & 0xF, 0) as i32)
                * i64::from(half_of(self.reg_or_pc((insn >> 8) & 0xF, 0), y));
            let base = (prod >> 16) as i32;
            let v = if insn & 0x0FF0_F0B0 == 0x0120_00A0 {
                base
            } else {
                let acc = self.reg_or_pc((insn >> 12) & 0xF, 0) as i32;
                let (sum, sat) = sat_i64(i64::from(base) + i64::from(acc), 32);
                if sat {
                    self.cpsr |= Q_BIT;
                }
                sum as i32
            };
            self.write_gpr((insn >> 16) & 0xF, v as u32);
            return Flow::Done;
        }
        Flow::Unimpl
    }

    /// QADD, QSUB, QDADD and QDSUB (ARM ARM A8.8.166 onward).
    fn saturating_add_sub(&mut self, insn: u32) -> Flow {
        let op = (insn >> 21) & 3;
        let rm = self.reg_or_pc(insn & 0xF, 0) as i32 as i64;
        let rn = self.reg_or_pc((insn >> 16) & 0xF, 0) as i32 as i64;
        let mut sat_any = false;
        let doubled = if op & 2 != 0 {
            let (d, s) = sat_i64(rn * 2, 32);
            sat_any |= s;
            d
        } else {
            rn
        };
        let sum = if op & 1 != 0 {
            rm - doubled
        } else {
            rm + doubled
        };
        let (v, s) = sat_i64(sum, 32);
        sat_any |= s;
        if sat_any {
            self.cpsr |= Q_BIT;
        }
        self.write_gpr((insn >> 12) & 0xF, v as u32);
        Flow::Done
    }

    /// Bits 27:25 = 000, bit 7 and bit 4 set: multiplies, synchronisation and the extra
    /// load/store group.
    fn mul_or_extra(&mut self, bus: &mut dyn Bus, insn: u32) -> Flow {
        let sh = (insn >> 5) & 3;
        if insn & 0x0F80_00F0 == 0x0180_0090 {
            return self.sync_op(bus, insn);
        }
        if sh != 0 {
            return self.ls_extra(bus, insn);
        }
        if insn & 0x0FC0_00F0 == 0x0000_0090 {
            // MUL / MLA.
            let s = insn & (1 << 20) != 0;
            let a = insn & (1 << 21) != 0;
            let rd = (insn >> 16) & 0xF;
            let ra = (insn >> 12) & 0xF;
            let rs = self.reg_or_pc((insn >> 8) & 0xF, 0);
            let rm = self.reg_or_pc(insn & 0xF, 0);
            let mut v = rm.wrapping_mul(rs);
            if a {
                v = v.wrapping_add(self.reg_or_pc(ra, 0));
            }
            if s {
                self.set_nz(v);
            }
            self.write_gpr(rd, v);
            return Flow::Done;
        }
        if insn & 0x0FF0_00F0 == 0x0060_0090 {
            // MLS: Rd = Ra - Rm * Rs.
            let rd = (insn >> 16) & 0xF;
            let ra = self.reg_or_pc((insn >> 12) & 0xF, 0);
            let rs = self.reg_or_pc((insn >> 8) & 0xF, 0);
            let rm = self.reg_or_pc(insn & 0xF, 0);
            self.write_gpr(rd, ra.wrapping_sub(rm.wrapping_mul(rs)));
            return Flow::Done;
        }
        if insn & 0x0F80_00F0 == 0x0080_0090 {
            // UMULL, UMLAL, SMULL, SMLAL.
            let signed = insn & (1 << 22) != 0;
            let acc = insn & (1 << 21) != 0;
            let s = insn & (1 << 20) != 0;
            let rdhi = (insn >> 16) & 0xF;
            let rdlo = (insn >> 12) & 0xF;
            let rs = self.reg_or_pc((insn >> 8) & 0xF, 0);
            let rm = self.reg_or_pc(insn & 0xF, 0);
            let prod: u64 = if signed {
                (i64::from(rm as i32) * i64::from(rs as i32)) as u64
            } else {
                u64::from(rm) * u64::from(rs)
            };
            let total = if acc {
                let old =
                    (u64::from(self.r[rdhi as usize]) << 32) | u64::from(self.r[rdlo as usize]);
                prod.wrapping_add(old)
            } else {
                prod
            };
            if s {
                self.set_nz64(total);
            }
            self.r[rdlo as usize] = total as u32;
            self.r[rdhi as usize] = (total >> 32) as u32;
            return Flow::Done;
        }
        if insn & 0x0FB0_00F0 == 0x0100_0090 {
            return Flow::Unimpl; // SWP and SWPB are not needed by the target firmware
        }
        Flow::Unimpl
    }

    fn set_nz64(&mut self, v: u64) {
        self.cpsr &= !(N_BIT | Z_BIT);
        if v >> 63 != 0 {
            self.cpsr |= N_BIT;
        }
        if v == 0 {
            self.cpsr |= Z_BIT;
        }
    }

    /// B and BL.
    fn branch(&mut self, insn: u32) -> Flow {
        let imm = (((insn & 0x00FF_FFFF) << 8) as i32 >> 6) as u32;
        if insn & (1 << 24) != 0 {
            self.r[14] = self.cur.wrapping_add(4);
        }
        self.pc = self.cur.wrapping_add(8).wrapping_add(imm);
        Flow::Done
    }

    /// An undefined instruction: takes the undefined-instruction exception.
    pub(crate) fn undefined(&mut self) -> Flow {
        self.exception(Mode::Undefined, 0x04, self.cur.wrapping_add(4), false);
        Flow::Event(Event::Undefined)
    }

    /// SVC: the supervisor call exception.
    fn svc(&mut self, insn: u32) -> Flow {
        let imm = insn & 0x00FF_FFFF;
        self.exception(Mode::Supervisor, 0x08, self.cur.wrapping_add(4), false);
        Flow::Event(Event::Svc(imm))
    }

    /// BKPT: a prefetch abort with the debug-event status.
    fn exception_bkpt(&mut self) {
        self.cp15.ifsr = 0x02;
        self.cp15.ifar = self.cur;
        self.exception(Mode::Abort, 0x0C, self.cur.wrapping_add(4), false);
    }

    /// Coprocessor register transfers (MRC and MCR) to CP15. Everything else is not modelled.
    fn coproc(&mut self, bus: &mut dyn Bus, insn: u32) -> Flow {
        if insn & 0x10 == 0 {
            return Flow::Unimpl; // CDP
        }
        if (insn >> 8) & 0xF != 15 {
            return Flow::Unimpl; // VFP/NEON (cp10, cp11) and debug (cp14)
        }
        let opc1 = (insn >> 21) & 7;
        let load = insn & (1 << 20) != 0;
        let crn = (insn >> 16) & 0xF;
        let rt = (insn >> 12) & 0xF;
        let opc2 = (insn >> 5) & 7;
        let crm = insn & 0xF;
        if load {
            let Some(v) = self.cp15.read(opc1, crn, crm, opc2) else {
                return Flow::Unimpl;
            };
            if rt == 15 {
                // MRC with Rt = PC sets the NZCV flags from the top four bits.
                self.cpsr = (self.cpsr & 0x0FFF_FFFF) | (v & 0xF000_0000);
            } else {
                self.r[rt as usize] = v;
            }
        } else {
            let v = self.reg_or_pc(rt, 4);
            // TLB maintenance (c8) and address translation (c7, c8) live in mmu.rs.
            if opc1 == 0 && crn == 8 {
                self.tlb_op(crm, opc2, v);
                return Flow::Done;
            }
            if opc1 == 0 && crn == 7 && crm == 8 {
                self.ats(bus, opc2, v);
                return Flow::Done;
            }
            if !self.cp15.write(opc1, crn, crm, opc2, v) {
                return Flow::Unimpl;
            }
        }
        Flow::Done
    }

    /// Instructions in the unconditional space (cond = 0b1111).
    fn uncond(&mut self, insn: u32) -> Flow {
        if insn & 0xFFFF_FFF0 == 0xF57F_F010 {
            self.excl = None; // CLREX
            return Flow::Done;
        }
        if insn & 0xFFFF_FFF0 == 0xF57F_F040
            || insn & 0xFFFF_FFF0 == 0xF57F_F050
            || insn & 0xFFFF_FFF0 == 0xF57F_F060
        {
            return Flow::Done; // DSB, DMB, ISB: the model has no reordering to fence
        }
        if insn & 0xFFFF_FDFF == 0xF101_0000 {
            // SETEND: only little endian is modelled.
            return if insn & (1 << 9) != 0 {
                Flow::Unimpl
            } else {
                Flow::Done
            };
        }
        if insn & 0xFFF0_0000 == 0xF100_0000 {
            return self.cps(insn);
        }
        if insn & 0xFD70_F000 == 0xF550_F000
            || insn & 0xFF70_F000 == 0xF510_F000
            || insn & 0xFF70_F000 == 0xF450_F000
        {
            return Flow::Done; // PLD, PLDW, PLI (immediate): hints
        }
        if (insn >> 25) & 7 == 5 {
            // BLX (immediate): switches to Thumb, which is not implemented.
            let h = (insn >> 24) & 1;
            let imm = (((insn & 0x00FF_FFFF) << 8) as i32 >> 6) as u32;
            self.r[14] = self.cur.wrapping_add(4);
            self.cpsr |= T_BIT;
            self.pc = self
                .cur
                .wrapping_add(8)
                .wrapping_add(imm)
                .wrapping_add(h << 1);
            return Flow::Done;
        }
        Flow::Unimpl
    }

    /// CPS: change the interrupt/abort masks and optionally the mode (privileged only).
    fn cps(&mut self, insn: u32) -> Flow {
        if matches!(self.mode(), Mode::User) {
            return Flow::Done;
        }
        let imod = (insn >> 18) & 3;
        let mut bits = 0;
        if insn & (1 << 8) != 0 {
            bits |= A_BIT;
        }
        if insn & (1 << 7) != 0 {
            bits |= I_BIT;
        }
        if insn & (1 << 6) != 0 {
            bits |= F_BIT;
        }
        let mut cp = self.cpsr;
        match imod {
            2 => cp &= !bits,
            3 => cp |= bits,
            _ => {}
        }
        self.cpsr = cp;
        if insn & (1 << 17) != 0 {
            let mode = insn & 0x1F;
            self.write_cpsr(mode, 0x1F);
        }
        Flow::Done
    }
}

/// `AddWithCarry` as a `(result, carry, overflow, arithmetic)` tuple for `dp`.
fn add3(x: u32, y: u32, c: bool) -> (u32, bool, bool, bool) {
    let (r, carry, ovf) = crate::alu::add_with_carry(x, y, c);
    (r, carry, ovf, true)
}

/// Which halfword of a register: `true` is the top half.
fn half_of(v: u32, top: bool) -> i32 {
    if top {
        i32::from((v >> 16) as u16 as i16)
    } else {
        i32::from(v as u16 as i16)
    }
}

/// The `x` (bit 5) and `y` (bit 6) half selectors of the halfword multiplies.
fn halves(insn: u32) -> (bool, bool) {
    (insn & (1 << 5) != 0, insn & (1 << 6) != 0)
}

/// Saturates `v` to a signed `bits`-wide value; returns the value and whether it saturated.
pub(crate) fn sat_i64(v: i64, bits: u32) -> (i64, bool) {
    let hi = (1i64 << (bits - 1)) - 1;
    let lo = -(1i64 << (bits - 1));
    if v > hi {
        (hi, true)
    } else if v < lo {
        (lo, true)
    } else {
        (v, false)
    }
}
