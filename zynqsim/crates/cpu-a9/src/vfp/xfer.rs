//! VFP memory access (VLDR, VSTR, VLDM, VSTM, VPUSH, VPOP), the two-register transfers
//! (VMOV between core and D or two S registers) and the core/single transfers and system
//! registers (VMOV, VMRS, VMSR).

use ps_bus::Bus;

use super::{
    FPEXC_EN, FPEXC_EX, FPSCR_NZCV, FPSCR_WRITABLE, FPSID, LEGACY_LDM, MVFR0, MVFR1, NEON_SCALAR,
    d_reg, field, s_reg,
};
use crate::mem::FSR_ALIGN;
use crate::state::{Cpu, Flow};

impl Cpu {
    /// Transfers `count` consecutive registers (S, or D when `dbl`) starting at `first`, to or
    /// from memory at `addr` in address order. A fault returns the faulting address and status.
    fn vfp_transfer_mem(
        &mut self,
        bus: &mut dyn Bus,
        addr: u32,
        dbl: bool,
        first: u32,
        count: u32,
        load: bool,
    ) -> Result<(), (u32, u16)> {
        if addr % 4 != 0 {
            return Err((addr, FSR_ALIGN));
        }
        let mut a = addr;
        for i in 0..count {
            let reg = first + i;
            if dbl {
                let hi_addr = a.wrapping_add(4);
                if load {
                    let lo = self.read_mem(bus, a, 4).map_err(|s| (a, s))?;
                    let hi = self.read_mem(bus, hi_addr, 4).map_err(|s| (hi_addr, s))?;
                    self.vfp.d[reg as usize] = u64::from(lo) | (u64::from(hi) << 32);
                } else {
                    let v = self.vfp.d[reg as usize];
                    self.write_mem(bus, a, 4, v as u32).map_err(|s| (a, s))?;
                    self.write_mem(bus, hi_addr, 4, (v >> 32) as u32)
                        .map_err(|s| (hi_addr, s))?;
                }
                a = a.wrapping_add(8);
            } else {
                if load {
                    let v = self.read_mem(bus, a, 4).map_err(|s| (a, s))?;
                    self.vfp.set_s(reg, v);
                } else {
                    let v = self.vfp.s(reg);
                    self.write_mem(bus, a, 4, v).map_err(|s| (a, s))?;
                }
                a = a.wrapping_add(4);
            }
        }
        Ok(())
    }

    /// VLDR, VSTR, VLDM, VSTM, VPUSH and VPOP (LDC/STC with cp10 or cp11).
    pub(super) fn vfp_mem(&mut self, bus: &mut dyn Bus, insn: u32) -> Flow {
        if !self.vfp_access(true, false) {
            return self.undefined();
        }
        let dbl = field(insn, 11, 8) == 11;
        let p = insn & (1 << 24) != 0;
        let u = insn & (1 << 23) != 0;
        let w = insn & (1 << 21) != 0;
        let load = insn & (1 << 20) != 0;
        let rn = field(insn, 19, 16);
        let vd = field(insn, 15, 12);
        let dbit = field(insn, 22, 22);
        let imm8 = field(insn, 7, 0);
        let base = if rn == 15 {
            self.cur.wrapping_add(8) & !3
        } else {
            self.r[rn as usize]
        };
        if p && !w {
            // VLDR and VSTR: base +/- imm8 * 4.
            let off = imm8 << 2;
            let addr = if u {
                base.wrapping_add(off)
            } else {
                base.wrapping_sub(off)
            };
            let first = if dbl {
                d_reg(vd, dbit)
            } else {
                s_reg(vd, dbit)
            };
            return match self.vfp_transfer_mem(bus, addr, dbl, first, 1, load) {
                Ok(()) => Flow::Done,
                Err((a, s)) => self.abort_flow(a, s),
            };
        }
        // Multiple transfers: IA (P=0, U=1, W optional) or DB (P=1, U=0, W=1, as VPUSH).
        let ia = !p && u;
        let db = p && !u;
        if !(ia || db) || (w && rn == 15) {
            return self.undefined();
        }
        if imm8 == 0 {
            return self.undefined();
        }
        let (first, count) = if dbl {
            if imm8 & 1 != 0 {
                return self.vfp_unimpl(LEGACY_LDM);
            }
            (d_reg(vd, dbit), imm8 / 2)
        } else {
            (s_reg(vd, dbit), imm8)
        };
        if first + count > 32 {
            return self.undefined();
        }
        let start = if ia {
            base
        } else {
            base.wrapping_sub(imm8 << 2)
        };
        let wb = if ia {
            base.wrapping_add(imm8 << 2)
        } else {
            start
        };
        match self.vfp_transfer_mem(bus, start, dbl, first, count, load) {
            Ok(()) => {
                if w {
                    self.r[rn as usize] = wb;
                }
                Flow::Done
            }
            Err((a, s)) => self.abort_flow(a, s),
        }
    }

    /// VMOV between two core registers and one D register (cp11) or two consecutive S
    /// registers (cp10).
    pub(super) fn vfp_pair(&mut self, insn: u32) -> Flow {
        if !self.vfp_access(true, false) {
            return self.undefined();
        }
        let dbl = field(insn, 11, 8) == 11;
        let load = insn & (1 << 20) != 0;
        let rt = field(insn, 15, 12) as usize;
        let rt2 = field(insn, 19, 16) as usize;
        if rt == 15 || rt2 == 15 {
            return self.undefined();
        }
        let m = field(insn, 5, 5);
        let vm = field(insn, 3, 0);
        if dbl {
            let d = d_reg(vm, m) as usize;
            if load {
                self.r[rt] = self.vfp.d[d] as u32;
                self.r[rt2] = (self.vfp.d[d] >> 32) as u32;
            } else {
                self.vfp.d[d] = u64::from(self.r[rt]) | (u64::from(self.r[rt2]) << 32);
            }
        } else {
            let s = s_reg(vm, m);
            if s == 31 {
                return self.undefined();
            }
            if load {
                self.r[rt] = self.vfp.s(s);
                self.r[rt2] = self.vfp.s(s + 1);
            } else {
                let (a, b) = (self.r[rt], self.r[rt2]);
                self.vfp.set_s(s, a);
                self.vfp.set_s(s + 1, b);
            }
        }
        Flow::Done
    }

    /// MCR and MRC on cp10 or cp11: VMOV core/single, and VMRS/VMSR.
    pub(super) fn vfp_transfer(&mut self, insn: u32) -> Flow {
        let cp = field(insn, 11, 8);
        let load = insn & (1 << 20) != 0;
        let opc1 = field(insn, 23, 21);
        // Bits 6:5 are opc2 (must be zero); bit 7 is the N bit of the single register.
        let opc2 = field(insn, 6, 5);
        let rt = field(insn, 15, 12);
        let reg = field(insn, 19, 16);
        if cp == 11 {
            if !self.vfp_access(true, false) {
                return self.undefined();
            }
            return if opc1 == 7 {
                self.undefined()
            } else {
                self.vfp_unimpl(NEON_SCALAR)
            };
        }
        if opc2 != 0 {
            return self.undefined();
        }
        match opc1 {
            7 if insn & (1 << 7) == 0 => self.vfp_sysreg(reg, rt, load),
            0 => {
                if !self.vfp_access(true, false) || rt == 15 {
                    return self.undefined();
                }
                let n = s_reg(reg, field(insn, 7, 7));
                if load {
                    self.r[rt as usize] = self.vfp.s(n);
                } else {
                    let v = self.r[rt as usize];
                    self.vfp.set_s(n, v);
                }
                Flow::Done
            }
            _ => self.undefined(),
        }
    }

    /// VMRS and VMSR. FPSID and FPEXC are privileged and need no FPEXC.EN; MVFR0/1 and FPSCR
    /// need it. FPSID and the MVFR registers ignore writes.
    fn vfp_sysreg(&mut self, reg: u32, rt: u32, read: bool) -> Flow {
        let (need_en, privileged) = match reg {
            0 | 8 => (false, true),
            1 | 6 | 7 => (true, false),
            _ => return self.undefined(),
        };
        if !self.vfp_access(need_en, privileged) {
            return self.undefined();
        }
        if rt == 15 && (!read || reg != 1) {
            return self.undefined();
        }
        if read {
            let v = match reg {
                0 => FPSID,
                1 => self.vfp.fpscr,
                6 => MVFR1,
                7 => MVFR0,
                _ => self.vfp.fpexc,
            };
            if rt == 15 {
                self.cpsr = (self.cpsr & !FPSCR_NZCV) | (v & FPSCR_NZCV);
            } else {
                self.r[rt as usize] = v;
            }
        } else {
            let v = self.r[rt as usize];
            match reg {
                1 => self.vfp.fpscr = v & FPSCR_WRITABLE,
                8 => self.vfp.fpexc = v & (FPEXC_EN | FPEXC_EX),
                _ => {}
            }
        }
        Flow::Done
    }
}
