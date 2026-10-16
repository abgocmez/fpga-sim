//! Loads, stores, LDM/STM and the exclusive monitor, on top of the bus.

use ps_bus::Bus;

use crate::cp15::SCTLR_A;
use crate::state::{CPSR_MSR_MASK, Cpu, Event, Flow, Mode, T_BIT};
use crate::xlate::Access;

/// DFSR/IFSR status: alignment fault.
pub(crate) const FSR_ALIGN: u8 = 0x01;
/// DFSR/IFSR status: synchronous external abort (any bus fault).
pub(crate) const FSR_EXTERNAL: u8 = 0x08;

impl Cpu {
    /// Reads `size` (1, 2 or 4) bytes at virtual address `va`, little endian. Unaligned
    /// accesses are split into bytes unless SCTLR.A makes them an alignment fault.
    pub(crate) fn read_mem(&mut self, bus: &mut dyn Bus, va: u32, size: u32) -> Result<u32, u8> {
        if size > 1 && va % size != 0 {
            if self.cp15.sctlr & SCTLR_A != 0 {
                return Err(FSR_ALIGN);
            }
            let mut v = 0u32;
            for i in 0..size {
                v |= self.read_mem(bus, va.wrapping_add(i), 1)? << (8 * i);
            }
            return Ok(v);
        }
        let pa = self.xlate.translate(va, Access::Read)?;
        bus.read(pa, size as u8)
            .map(|v| v as u32)
            .map_err(|_| FSR_EXTERNAL)
    }

    /// Writes the low `size` (1, 2 or 4) bytes of `v` at `va`, little endian.
    pub(crate) fn write_mem(
        &mut self,
        bus: &mut dyn Bus,
        va: u32,
        size: u32,
        v: u32,
    ) -> Result<(), u8> {
        if size > 1 && va % size != 0 {
            if self.cp15.sctlr & SCTLR_A != 0 {
                return Err(FSR_ALIGN);
            }
            for i in 0..size {
                self.write_mem(bus, va.wrapping_add(i), 1, v >> (8 * i))?;
            }
            return Ok(());
        }
        let pa = self.xlate.translate(va, Access::Write)?;
        bus.write(pa, size as u8, u64::from(v))
            .map_err(|_| FSR_EXTERNAL)
    }

    /// Turns a failed access into a data abort at `va`.
    pub(crate) fn abort_flow(&mut self, va: u32, status: u8) -> Flow {
        self.data_abort(va, status);
        Flow::Event(Event::DataAbort { addr: va })
    }

    /// Writes a general register from a load or LDM: R15 branches with interworking.
    pub(crate) fn load_write_pc(&mut self, v: u32) {
        self.bx_write(v);
    }

    /// BX semantics: bit 0 selects Thumb (not executed here), the PC takes the rest.
    pub(crate) fn bx_write(&mut self, v: u32) {
        if v & 1 != 0 {
            self.cpsr |= T_BIT;
            self.pc = v & !1;
        } else {
            self.cpsr &= !T_BIT;
            self.pc = v & !3;
        }
    }

    /// The address and writeback value of a single load/store, from `base` and `offset`.
    fn address_of(p: bool, u: bool, base: u32, offset: u32) -> (u32, u32) {
        let off = if u {
            base.wrapping_add(offset)
        } else {
            base.wrapping_sub(offset)
        };
        (if p { off } else { base }, off)
    }

    /// LDR, STR, LDRB and STRB (immediate or register offset, any addressing mode).
    pub(crate) fn ls_single(&mut self, bus: &mut dyn Bus, insn: u32) -> Flow {
        let p = insn & (1 << 24) != 0;
        let u = insn & (1 << 23) != 0;
        let byte = insn & (1 << 22) != 0;
        let w = insn & (1 << 21) != 0;
        let load = insn & (1 << 20) != 0;
        let rn = (insn >> 16) & 0xF;
        let rt = (insn >> 12) & 0xF;
        let offset = if insn & (1 << 25) == 0 {
            insn & 0xFFF
        } else {
            let rm = insn & 0xF;
            let ty = (insn >> 5) & 3;
            let amount = (insn >> 7) & 0x1F;
            let carry = self.cpsr & crate::state::C_BIT != 0;
            crate::alu::shift_imm(self.reg_or_pc(rm, 0), ty, amount, carry).0
        };
        let base = self.reg_or_pc(rn, 0);
        let (addr, wb) = Self::address_of(p, u, base, offset);
        let size = if byte { 1 } else { 4 };
        let store_value = if load { 0 } else { self.reg_or_pc(rt, 4) };
        let result = if load {
            self.read_mem(bus, addr, size)
        } else {
            self.write_mem(bus, addr, size, store_value).map(|()| 0)
        };
        let v = match result {
            Ok(v) => v,
            Err(status) => return self.abort_flow(addr, status),
        };
        if (!p || w) && rn != 15 {
            self.r[rn as usize] = wb;
        }
        if load {
            if rt == 15 {
                self.load_write_pc(v);
            } else {
                self.r[rt as usize] = v;
            }
        }
        Flow::Done
    }

    /// LDRH, STRH, LDRSB, LDRSH, LDRD and STRD (the "extra" load/store group).
    pub(crate) fn ls_extra(&mut self, bus: &mut dyn Bus, insn: u32) -> Flow {
        let p = insn & (1 << 24) != 0;
        let u = insn & (1 << 23) != 0;
        let imm = insn & (1 << 22) != 0;
        let w = insn & (1 << 21) != 0;
        let load = insn & (1 << 20) != 0;
        let sh = (insn >> 5) & 3;
        let rn = (insn >> 16) & 0xF;
        let rt = (insn >> 12) & 0xF;
        let offset = if imm {
            ((insn >> 4) & 0xF0) | (insn & 0xF)
        } else {
            self.reg_or_pc(insn & 0xF, 0)
        };
        let base = self.reg_or_pc(rn, 0);
        let (addr, wb) = Self::address_of(p, u, base, offset);
        let mut loaded: [u32; 2] = [0; 2];
        let mut count = 1;
        let outcome: Result<(), u8> = match (load, sh) {
            (true, 1) => self.read_mem(bus, addr, 2).map(|v| loaded[0] = v),
            (true, 2) => self
                .read_mem(bus, addr, 1)
                .map(|v| loaded[0] = v as u8 as i8 as i32 as u32),
            (true, _) => self
                .read_mem(bus, addr, 2)
                .map(|v| loaded[0] = v as u16 as i16 as i32 as u32),
            (false, 1) => {
                let v = self.reg_or_pc(rt, 4);
                self.write_mem(bus, addr, 2, v)
            }
            (false, 2) => {
                // LDRD: two words into Rt and Rt+1.
                count = 2;
                self.read_mem(bus, addr, 4).and_then(|a| {
                    self.read_mem(bus, addr.wrapping_add(4), 4).map(|b| {
                        loaded = [a, b];
                    })
                })
            }
            _ => {
                // STRD: Rt and Rt+1 to two words.
                let a = self.reg_or_pc(rt, 4);
                let b = self.reg_or_pc((rt + 1) & 0xF, 4);
                self.write_mem(bus, addr, 4, a)
                    .and_then(|()| self.write_mem(bus, addr.wrapping_add(4), 4, b))
            }
        };
        if let Err(status) = outcome {
            return self.abort_flow(addr, status);
        }
        if (!p || w) && rn != 15 {
            self.r[rn as usize] = wb;
        }
        // LDRD is the one register-writing form with L clear (SH = 10).
        if load || sh == 2 {
            if count == 2 {
                self.r[rt as usize] = loaded[0];
                self.r[((rt + 1) & 0xF) as usize] = loaded[1];
            } else {
                self.r[rt as usize] = loaded[0];
            }
        }
        Flow::Done
    }

    /// LDM and STM in all four addressing modes, with writeback and the S bit (user registers,
    /// or an exception return when the list has the PC and the mode is privileged).
    pub(crate) fn ldm_stm(&mut self, bus: &mut dyn Bus, insn: u32) -> Flow {
        let p = insn & (1 << 24) != 0;
        let u = insn & (1 << 23) != 0;
        let s = insn & (1 << 22) != 0;
        let w = insn & (1 << 21) != 0;
        let load = insn & (1 << 20) != 0;
        let rn = (insn >> 16) & 0xF;
        let list = insn & 0xFFFF;
        let n = list.count_ones();
        if n == 0 {
            // Empty list is unpredictable; it transfers nothing.
            return Flow::Done;
        }
        let base = self.reg_or_pc(rn, 0);
        let bytes = 4 * n;
        let (start, wb) = match (p, u) {
            (false, true) => (base, base.wrapping_add(bytes)),
            (true, true) => (base.wrapping_add(4), base.wrapping_add(bytes)),
            (true, false) => (base.wrapping_sub(bytes), base.wrapping_sub(bytes)),
            (false, false) => (
                base.wrapping_sub(bytes).wrapping_add(4),
                base.wrapping_sub(bytes),
            ),
        };
        let privileged = !matches!(self.mode(), Mode::User | Mode::System);
        let exc_return = load && s && list & 0x8000 != 0 && privileged;
        let user_regs = s && !(load && list & 0x8000 != 0);
        let mut addr = start;
        if load {
            let mut values = [0u32; 16];
            for (reg, slot) in values.iter_mut().enumerate() {
                if list & (1 << reg) != 0 {
                    match self.read_mem(bus, addr, 4) {
                        Ok(v) => *slot = v,
                        Err(status) => return self.abort_flow(addr, status),
                    }
                    addr = addr.wrapping_add(4);
                }
            }
            if w && rn != 15 {
                self.r[rn as usize] = wb;
            }
            for (reg, &v) in values.iter().enumerate().take(15) {
                if list & (1 << reg) != 0 {
                    if user_regs {
                        self.set_user_reg(reg, v);
                    } else {
                        self.r[reg] = v;
                    }
                }
            }
            if list & 0x8000 != 0 {
                let v = values[15];
                if exc_return {
                    let spsr = self.spsr(self.mode());
                    self.write_cpsr(spsr, u32::MAX);
                    if self.cpsr & T_BIT != 0 {
                        self.pc = v & !1;
                    } else {
                        self.pc = v & !3;
                    }
                } else {
                    self.load_write_pc(v);
                }
            }
        } else {
            let mut values = [0u32; 16];
            for (reg, slot) in values.iter_mut().enumerate() {
                if list & (1 << reg) != 0 {
                    *slot = match reg {
                        15 => self.cur.wrapping_add(12),
                        _ if user_regs => self.user_reg(reg),
                        _ => self.r[reg],
                    };
                }
            }
            for (reg, v) in values.iter().enumerate() {
                if list & (1 << reg) != 0 {
                    if let Err(status) = self.write_mem(bus, addr, 4, *v) {
                        return self.abort_flow(addr, status);
                    }
                    addr = addr.wrapping_add(4);
                }
            }
            if w && rn != 15 {
                self.r[rn as usize] = wb;
            }
        }
        Flow::Done
    }

    /// LDREX/STREX (word, byte, halfword and doubleword) and their exclusive monitor.
    pub(crate) fn sync_op(&mut self, bus: &mut dyn Bus, insn: u32) -> Flow {
        let x = (insn >> 20) & 7;
        let load = x & 1 != 0;
        let (size, pair) = match x >> 1 {
            0 => (4, false),
            1 => (4, true),
            2 => (1, false),
            _ => (2, false),
        };
        let rn = (insn >> 16) & 0xF;
        let addr = self.reg_or_pc(rn, 0);
        let align = if pair { 8 } else { size };
        if addr % align != 0 {
            return self.abort_flow(addr, FSR_ALIGN);
        }
        if load {
            let rt = (insn >> 12) & 0xF;
            let first = match self.read_mem(bus, addr, size) {
                Ok(v) => v,
                Err(status) => return self.abort_flow(addr, status),
            };
            let second = if pair {
                match self.read_mem(bus, addr.wrapping_add(4), 4) {
                    Ok(v) => v,
                    Err(status) => return self.abort_flow(addr, status),
                }
            } else {
                0
            };
            self.excl = Some(addr);
            self.r[rt as usize] = first;
            if pair {
                self.r[((rt + 1) & 0xF) as usize] = second;
            }
            return Flow::Done;
        }
        let rd = (insn >> 12) & 0xF;
        let rt = insn & 0xF;
        if self.excl != Some(addr) {
            self.excl = None;
            self.r[rd as usize] = 1;
            return Flow::Done;
        }
        let first = self.reg_or_pc(rt, 4);
        let second = if pair {
            self.reg_or_pc((rt + 1) & 0xF, 4)
        } else {
            0
        };
        let result = self.write_mem(bus, addr, size, first).and_then(|()| {
            if pair {
                self.write_mem(bus, addr.wrapping_add(4), 4, second)
            } else {
                Ok(())
            }
        });
        self.excl = None;
        match result {
            Ok(()) => {
                self.r[rd as usize] = 0;
                Flow::Done
            }
            Err(status) => self.abort_flow(addr, status),
        }
    }

    /// Reads R0 to R15 for an operand: R15 is `cur + 8`, or `cur + 12` with `extra` = 4.
    pub(crate) fn reg_or_pc(&self, n: u32, extra: u32) -> u32 {
        if n == 15 {
            self.cur.wrapping_add(8 + extra)
        } else {
            self.r[n as usize]
        }
    }

    /// MSR/CPS-style write of PSR fields selected by `fields` (bit 3 f, 2 s, 1 x, 0 c).
    pub(crate) fn msr_write(&mut self, spsr: bool, fields: u32, value: u32) {
        let mut m = 0u32;
        if fields & 8 != 0 {
            m |= 0xFF00_0000;
        }
        if fields & 4 != 0 {
            m |= 0x00FF_0000;
        }
        if fields & 2 != 0 {
            m |= 0x0000_FF00;
        }
        if fields & 1 != 0 {
            m |= 0x0000_00FF;
        }
        if spsr {
            if matches!(self.mode(), Mode::User | Mode::System) {
                return;
            }
            let i = self.mode().bank();
            self.spsr[i] = ((self.spsr[i] & !m) | (value & m)) & !crate::state::PSR_RESERVED;
        } else {
            if self.mode() == Mode::User {
                m &= 0xF800_0000 | crate::state::GE_MASK;
            }
            m &= CPSR_MSR_MASK;
            self.write_cpsr(value, m);
        }
    }
}
