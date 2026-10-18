//! Thumb and Thumb-2 execution (ARMv7-A, CPSR.T set): the 16-bit and 32-bit encodings, IT
//! blocks, and the interworking rules of BX, BLX, LDR/LDM/POP into PC and exception returns.
//!
//! The shared semantics come from the A32 side: the barrel shifter and add-with-carry from
//! `alu.rs`, loads and stores (alignment splitting, SCTLR.A) from `mem.rs`, exception entry and
//! the banked registers from `state.rs`, CP15 from `cp15.rs`, the MSR field writer from `mem.rs`.
//! What differs is the Thumb PC (R15 reads as the instruction address + 4, not + 8), the PC
//! write rules (ALU writes clear only bit 0; loads interwork), the 16-bit flag rule (no flags
//! inside an IT block) and the instruction lengths, so those live here. Unallocated encodings
//! are never guessed: they return [`Flow::Unimpl`] and leave the state untouched.

use ps_bus::Bus;

use crate::alu;
use crate::state::{
    A_BIT, C_BIT, Cpu, Event, F_BIT, Flow, I_BIT, Mode, Q_BIT, StepResult, T_BIT, V_BIT, Wait,
};

/// CPSR.IT[1:0] (bits 26:25) and IT[7:2] (bits 15:10): the IT state. Cleared on exception entry.
pub(crate) const IT_MASK: u32 = (3 << 25) | (0x3F << 10);

/// The address of the next instruction after a Thumb step of `len` bytes.
fn executed(pc: u32, event: Option<Event>) -> StepResult {
    StepResult::Executed {
        pc,
        cycles: 1,
        event,
    }
}

/// `AddWithCarry` as the `(result, carry, overflow, arithmetic)` tuple of the data-processing core.
fn add3(x: u32, y: u32, c: bool) -> (u32, bool, bool, bool) {
    let (r, carry, ovf) = alu::add_with_carry(x, y, c);
    (r, carry, ovf, true)
}

/// ITAdvance: the mask shifts left after each instruction of the block; the block ends after
/// the last instruction selected by the mask.
fn it_advance(it: u32) -> u32 {
    if it & 0x7 == 0 {
        0
    } else {
        (it & 0xE0) | ((it << 1) & 0x1F)
    }
}

/// ThumbExpandImm_C: the 12-bit modified immediate and its carry-out.
fn thumb_expand(imm12: u32, carry_in: bool) -> (u32, bool) {
    let imm8 = imm12 & 0xFF;
    if (imm12 >> 10) & 3 == 0 {
        let v = match (imm12 >> 8) & 3 {
            0 => imm8,
            1 => imm8.wrapping_mul(0x0001_0001),
            2 => imm8.wrapping_mul(0x0100_0100),
            _ => imm8.wrapping_mul(0x0101_0101),
        };
        (v, carry_in)
    } else {
        let v = (0x80 | (imm12 & 0x7F)).rotate_right((imm12 >> 7) & 0x1F);
        (v, v >> 31 != 0)
    }
}

/// Sign-extends the low `bits` bits of `v`.
fn sext(v: u32, bits: u32) -> u32 {
    let sh = 32 - bits;
    (((v << sh) as i32) >> sh) as u32
}

/// The 16-bit halfword `high` (true) or low half of `v`, sign-extended.
fn half(v: u32, high: bool) -> i32 {
    if high {
        i32::from((v >> 16) as u16 as i16)
    } else {
        i32::from(v as u16 as i16)
    }
}

impl Cpu {
    /// The IT state (ITSTATE: IT[7:0]).
    pub(crate) fn itstate(&self) -> u32 {
        (((self.cpsr >> 10) & 0x3F) << 2) | ((self.cpsr >> 25) & 3)
    }

    /// Writes the IT state.
    pub(crate) fn set_itstate(&mut self, it: u32) {
        self.cpsr = (self.cpsr & !IT_MASK) | (((it >> 2) & 0x3F) << 10) | ((it & 3) << 25);
    }

    /// R0 to R15 as a Thumb instruction reads them: R15 is the address + 4.
    fn tr(&self, n: u32) -> u32 {
        if n == 15 {
            self.cur.wrapping_add(4)
        } else {
            self.r[n as usize]
        }
    }

    /// The Thumb ALU PC write (BranchWritePC): bit 0 is cleared, the state does not change.
    fn tw(&mut self, n: u32, v: u32) {
        if n == 15 {
            self.pc = v & !1;
        } else {
            self.r[n as usize] = v;
        }
    }

    /// Sets NZCV from an adder result (when `setf`) and returns the sum.
    fn t_addc(&mut self, x: u32, y: u32, cin: bool, setf: bool) -> u32 {
        let (r, c, v) = alu::add_with_carry(x, y, cin);
        if setf {
            self.set_nz(r);
            self.set_flag(C_BIT, c);
            self.set_flag(V_BIT, v);
        }
        r
    }

    /// An undefined instruction of `len` bytes: the undefined-instruction exception.
    fn t_undef(&mut self, len: u32) -> Flow {
        self.exception(Mode::Undefined, 0x04, self.cur.wrapping_add(len), false);
        Flow::Event(Event::Undefined)
    }

    /// Executes one Thumb instruction at `pc`, fetching one or two halfwords. The IT state is
    /// advanced before the instruction runs, so an exception taken by it saves the next state.
    pub(crate) fn step_thumb(&mut self, bus: &mut dyn Bus, pc: u32) -> StepResult {
        let hw1 = match self.fetch16(bus, pc) {
            Ok(v) => v,
            Err(status) => {
                self.cur = pc;
                self.prefetch_abort(pc, status);
                return executed(pc, Some(Event::PrefetchAbort { addr: pc }));
            }
        };
        let wide = hw1 >> 11 >= 0b11101;
        let (insn, len) = if wide {
            let hw2 = match self.fetch16(bus, pc.wrapping_add(2)) {
                Ok(v) => v,
                Err(status) => {
                    self.cur = pc;
                    self.prefetch_abort(pc.wrapping_add(2), status);
                    return executed(
                        pc,
                        Some(Event::PrefetchAbort {
                            addr: pc.wrapping_add(2),
                        }),
                    );
                }
            };
            ((u32::from(hw1) << 16) | u32::from(hw2), 4)
        } else {
            (u32::from(hw1), 2)
        };
        let saved = self.cpsr;
        let it = self.itstate();
        let in_it = it & 0xF != 0;
        self.cur = pc;
        self.pc = pc.wrapping_add(len);
        if in_it {
            self.set_itstate(it_advance(it));
            if !self.cond_passed(it >> 4) {
                return executed(pc, None);
            }
        }
        let flow = if wide {
            self.exec32(bus, insn)
        } else {
            self.exec16(bus, hw1, in_it)
        };
        match flow {
            Flow::Done => executed(pc, None),
            Flow::Event(e) => executed(pc, Some(e)),
            Flow::Unimpl => {
                self.cpsr = saved;
                self.pc = pc;
                StepResult::Unimplemented { pc, insn }
            }
        }
    }

    // ----- 16-bit encodings ---------------------------------------------------------------

    fn exec16(&mut self, bus: &mut dyn Bus, hw: u16, in_it: bool) -> Flow {
        let h = u32::from(hw);
        let setf = !in_it;
        match h >> 11 {
            0..=2 => return self.t_shift_imm(h, setf),
            3 => return self.t_addsub3(h, setf),
            4..=7 => return self.t_imm8(h, setf),
            _ => {}
        }
        match h >> 10 {
            0b010000 => return self.t_dp_reg(h, setf),
            0b010001 => return self.t_special(h),
            _ => {}
        }
        if h >> 11 == 0b01001 {
            return self.t_ldr_lit(bus, h);
        }
        match h >> 12 {
            0x5 => self.t_ls_reg(bus, h),
            0x6 | 0x7 => self.t_ls_imm(bus, h),
            0x8 => self.t_ls_half(bus, h),
            0x9 => self.t_ls_sp(bus, h),
            0xA => self.t_adr_addsp(h),
            0xB => self.t_misc(bus, h),
            0xC => {
                let rn = (h >> 8) & 7;
                self.t_block(bus, rn, h & 0xFF, false, (h >> 11) & 1 != 0, true)
            }
            0xD => self.t_cond(h),
            0xE => {
                let off = (((h & 0x7FF) << 21) as i32 >> 20) as u32;
                self.pc = self.cur.wrapping_add(4).wrapping_add(off);
                Flow::Done
            }
            _ => Flow::Unimpl,
        }
    }

    /// LSL, LSR and ASR by an immediate (T1).
    fn t_shift_imm(&mut self, h: u32, setf: bool) -> Flow {
        let ty = (h >> 11) & 3;
        let imm5 = (h >> 6) & 0x1F;
        let (rm, rd) = (((h >> 3) & 7) as usize, (h & 7) as usize);
        let cin = self.cpsr & C_BIT != 0;
        let (v, c) = alu::shift_imm(self.r[rm], ty, imm5, cin);
        self.r[rd] = v;
        if setf {
            self.set_nz(v);
            self.set_flag(C_BIT, c);
        }
        Flow::Done
    }

    /// ADD and SUB, register or 3-bit immediate (T1).
    fn t_addsub3(&mut self, h: u32, setf: bool) -> Flow {
        let imm = h & (1 << 10) != 0;
        let sub = h & (1 << 9) != 0;
        let n = (h >> 6) & 7;
        let a = self.r[((h >> 3) & 7) as usize];
        let b = if imm { n } else { self.r[n as usize] };
        let r = if sub {
            self.t_addc(a, !b, true, setf)
        } else {
            self.t_addc(a, b, false, setf)
        };
        self.r[(h & 7) as usize] = r;
        Flow::Done
    }

    /// MOV, CMP, ADD and SUB with an 8-bit immediate (T1, T2).
    fn t_imm8(&mut self, h: u32, setf: bool) -> Flow {
        let rdn = ((h >> 8) & 7) as usize;
        let imm = h & 0xFF;
        let a = self.r[rdn];
        match (h >> 11) & 3 {
            0 => {
                self.r[rdn] = imm;
                if setf {
                    self.set_nz(imm);
                }
            }
            1 => {
                self.t_addc(a, !imm, true, true);
            }
            2 => self.r[rdn] = self.t_addc(a, imm, false, setf),
            _ => self.r[rdn] = self.t_addc(a, !imm, true, setf),
        }
        Flow::Done
    }

    /// The register data-processing group (010000): AND to MVN.
    fn t_dp_reg(&mut self, h: u32, setf: bool) -> Flow {
        let op = (h >> 6) & 0xF;
        let b = self.r[((h >> 3) & 7) as usize];
        let rdn = (h & 7) as usize;
        let a = self.r[rdn];
        let cin = self.cpsr & C_BIT != 0;
        match op {
            0 | 1 | 12 | 14 | 15 => {
                let v = match op {
                    0 => a & b,
                    1 => a ^ b,
                    12 => a | b,
                    14 => a & !b,
                    _ => !b,
                };
                self.r[rdn] = v;
                if setf {
                    self.set_nz(v);
                }
            }
            8 => {
                let v = a & b;
                if setf {
                    self.set_nz(v);
                }
            }
            2 | 3 | 4 | 7 => {
                let ty = match op {
                    2 => 0,
                    3 => 1,
                    4 => 2,
                    _ => 3,
                };
                let (v, c) = alu::shift_reg(a, ty, b & 0xFF, cin);
                self.r[rdn] = v;
                if setf {
                    self.set_nz(v);
                    self.set_flag(C_BIT, c);
                }
            }
            5 => self.r[rdn] = self.t_addc(a, b, cin, setf),
            6 => self.r[rdn] = self.t_addc(a, !b, cin, setf),
            9 => self.r[rdn] = self.t_addc(!b, 0, true, setf),
            10 => {
                self.t_addc(a, !b, true, true);
            }
            11 => {
                self.t_addc(a, b, false, true);
            }
            _ => {
                // MUL: N and Z only; C and V are unchanged.
                let v = a.wrapping_mul(b);
                self.r[rdn] = v;
                if setf {
                    self.set_nz(v);
                }
            }
        }
        Flow::Done
    }

    /// Special data instructions and branch exchange (010001): ADD, CMP and MOV with high
    /// registers, BX and BLX.
    fn t_special(&mut self, h: u32) -> Flow {
        let dn = ((h >> 4) & 8) | (h & 7);
        let rm = (h >> 3) & 0xF;
        match (h >> 8) & 3 {
            0 => {
                let v = self.tr(dn).wrapping_add(self.tr(rm));
                self.tw(dn, v);
            }
            1 => {
                let (a, b) = (self.tr(dn), self.tr(rm));
                self.t_addc(a, !b, true, true);
            }
            2 => {
                let v = self.tr(rm);
                self.tw(dn, v);
            }
            _ => {
                let target = self.tr(rm);
                if h & 0x80 != 0 {
                    self.r[14] = self.cur.wrapping_add(2) | 1;
                }
                self.bx_write(target);
            }
        }
        Flow::Done
    }

    /// LDR (literal): the word at the 4-aligned PC plus an 8-bit scaled offset.
    fn t_ldr_lit(&mut self, bus: &mut dyn Bus, h: u32) -> Flow {
        let addr = (self.cur.wrapping_add(4) & !3).wrapping_add((h & 0xFF) << 2);
        let rt = (h >> 8) & 7;
        self.t_xfer(bus, addr, 4, rt, false, false)
    }

    /// LDR, STR, LDRB, STRB, LDRH, STRH and the signed loads, with a register offset (0101).
    fn t_ls_reg(&mut self, bus: &mut dyn Bus, h: u32) -> Flow {
        let op = (h >> 9) & 7;
        let addr = self.r[((h >> 3) & 7) as usize].wrapping_add(self.r[((h >> 6) & 7) as usize]);
        let rt = h & 7;
        let (size, store, sign) = match op {
            0 => (4, true, false),
            1 => (2, true, false),
            2 => (1, true, false),
            3 => (1, false, true),
            4 => (4, false, false),
            5 => (2, false, false),
            6 => (1, false, false),
            _ => (2, false, true),
        };
        self.t_xfer(bus, addr, size, rt, store, sign)
    }

    /// LDR, STR, LDRB and STRB with a 5-bit immediate offset (011).
    fn t_ls_imm(&mut self, bus: &mut dyn Bus, h: u32) -> Flow {
        let byte = h & (1 << 12) != 0;
        let load = h & (1 << 11) != 0;
        let imm5 = (h >> 6) & 0x1F;
        let (size, off) = if byte { (1, imm5) } else { (4, imm5 << 2) };
        let addr = self.r[((h >> 3) & 7) as usize].wrapping_add(off);
        self.t_xfer(bus, addr, size, h & 7, !load, false)
    }

    /// LDRH and STRH with a 5-bit scaled immediate offset (1000).
    fn t_ls_half(&mut self, bus: &mut dyn Bus, h: u32) -> Flow {
        let load = h & (1 << 11) != 0;
        let addr = self.r[((h >> 3) & 7) as usize].wrapping_add(((h >> 6) & 0x1F) << 1);
        self.t_xfer(bus, addr, 2, h & 7, !load, false)
    }

    /// LDR and STR relative to SP (1001).
    fn t_ls_sp(&mut self, bus: &mut dyn Bus, h: u32) -> Flow {
        let load = h & (1 << 11) != 0;
        let addr = self.r[13].wrapping_add((h & 0xFF) << 2);
        self.t_xfer(bus, addr, 4, (h >> 8) & 7, !load, false)
    }

    /// ADR (PC-relative) and ADD (SP plus an 8-bit scaled immediate) (1010).
    fn t_adr_addsp(&mut self, h: u32) -> Flow {
        let imm = (h & 0xFF) << 2;
        let rd = (h >> 8) & 7;
        self.r[rd as usize] = if h & (1 << 11) != 0 {
            self.r[13].wrapping_add(imm)
        } else {
            (self.cur.wrapping_add(4) & !3).wrapping_add(imm)
        };
        Flow::Done
    }

    /// Miscellaneous 16-bit instructions (1011): ADD and SUB SP, PUSH, POP, the extends and
    /// reversals, CPS, BKPT and the IT and hint group.
    fn t_misc(&mut self, bus: &mut dyn Bus, h: u32) -> Flow {
        if (h >> 8) & 0xF == 0 {
            let off = (h & 0x7F) << 2;
            let sp = self.r[13];
            self.r[13] = if h & 0x80 != 0 {
                sp.wrapping_sub(off)
            } else {
                sp.wrapping_add(off)
            };
            return Flow::Done;
        }
        match (h >> 9) & 7 {
            2 => {
                // PUSH: the list plus LR as STMDB SP!.
                let list = (h & 0xFF) | (((h >> 8) & 1) << 14);
                return self.t_block(bus, 13, list, true, false, true);
            }
            6 => {
                // POP: the list plus PC as LDMIA SP!.
                let list = (h & 0xFF) | (((h >> 8) & 1) << 15);
                return self.t_block(bus, 13, list, false, true, true);
            }
            _ => {}
        }
        if h & 0xF500 == 0xB100 {
            // CBZ (bit 11 clear) and CBNZ: a forward branch of up to 126 bytes.
            let nonzero = h & (1 << 11) != 0;
            let off = (((h >> 9) & 1) << 6) | (((h >> 3) & 0x1F) << 1);
            if (self.r[(h & 7) as usize] == 0) != nonzero {
                self.pc = self.cur.wrapping_add(4).wrapping_add(off);
            }
            return Flow::Done;
        }
        let (rd, rm) = (h & 7, (h >> 3) & 7);
        let v = self.r[rm as usize];
        let out = match h & 0xFFC0 {
            0xB200 => Some((v as u16 as i16) as i32 as u32), // SXTH
            0xB240 => Some((v as u8 as i8) as i32 as u32),   // SXTB
            0xB280 => Some(v & 0xFFFF),                      // UXTH
            0xB2C0 => Some(v & 0xFF),                        // UXTB
            0xBA00 => Some(v.swap_bytes()),                  // REV
            0xBA40 => Some(rev16(v)),                        // REV16
            0xBAC0 => Some(((v as u16).swap_bytes() as i16) as i32 as u32), // REVSH
            _ => None,
        };
        if let Some(r) = out {
            self.r[rd as usize] = r;
            return Flow::Done;
        }
        if h & 0xFFE8 == 0xB660 {
            // CPS: CPSIE and CPSID of A, I and F (privileged only).
            if matches!(self.mode(), Mode::User) {
                return Flow::Done;
            }
            let mut bits = 0;
            if h & 4 != 0 {
                bits |= A_BIT;
            }
            if h & 2 != 0 {
                bits |= I_BIT;
            }
            if h & 1 != 0 {
                bits |= F_BIT;
            }
            if h & 0x10 != 0 {
                self.cpsr |= bits;
            } else {
                self.cpsr &= !bits;
            }
            return Flow::Done;
        }
        if h & 0xFF00 == 0xBE00 {
            // BKPT: a prefetch abort with the debug-event status.
            let imm = (h & 0xFF) as u16;
            self.cp15.ifsr = 0x02;
            self.cp15.ifar = self.cur;
            self.exception(Mode::Abort, 0x0C, self.cur.wrapping_add(4), false);
            return Flow::Event(Event::Bkpt(imm));
        }
        if h & 0xFF00 == 0xBF00 {
            let mask = h & 0xF;
            let cond = (h >> 4) & 0xF;
            if mask != 0 {
                // IT: the first condition and the mask of the block.
                self.set_itstate((cond << 4) | mask);
                return Flow::Done;
            }
            return self.t_hint(cond);
        }
        Flow::Unimpl
    }

    /// NOP, YIELD, WFE, WFI and SEV (the hint group): the same events as the A32 hints.
    fn t_hint(&mut self, op: u32) -> Flow {
        match op {
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
            _ => Flow::Done,
        }
    }

    /// Conditional branch (B<cond>, T1), UDF and SVC (1101).
    fn t_cond(&mut self, h: u32) -> Flow {
        let cond = (h >> 8) & 0xF;
        let imm8 = h & 0xFF;
        match cond {
            0xE => self.t_undef(2),
            0xF => {
                self.exception(Mode::Supervisor, 0x08, self.cur.wrapping_add(2), false);
                Flow::Event(Event::Svc(imm8))
            }
            _ => {
                if self.cond_passed(cond) {
                    let off = (((imm8) << 24) as i32 >> 23) as u32;
                    self.pc = self.cur.wrapping_add(4).wrapping_add(off);
                }
                Flow::Done
            }
        }
    }

    /// The block transfer of LDM, STM, PUSH and POP. `db` selects decrement-before (PUSH and
    /// STMDB) over increment-after. A load into PC interworks.
    fn t_block(
        &mut self,
        bus: &mut dyn Bus,
        rn: u32,
        list: u32,
        db: bool,
        load: bool,
        wb: bool,
    ) -> Flow {
        let n = list.count_ones();
        if n == 0 {
            return Flow::Done; // unpredictable: transfers nothing
        }
        let base = self.r[rn as usize];
        let bytes = 4 * n;
        let (mut addr, fin) = if db {
            let s = base.wrapping_sub(bytes);
            (s, s)
        } else {
            (base, base.wrapping_add(bytes))
        };
        let write_back = wb && (!load || list & (1 << rn) == 0);
        if load {
            let mut vals = [0u32; 16];
            for (reg, slot) in vals.iter_mut().enumerate() {
                if list & (1 << reg) != 0 {
                    match self.read_mem(bus, addr, 4) {
                        Ok(v) => *slot = v,
                        Err(status) => return self.abort_flow(addr, status),
                    }
                    addr = addr.wrapping_add(4);
                }
            }
            if write_back {
                self.r[rn as usize] = fin;
            }
            for (reg, &v) in vals.iter().enumerate().take(15) {
                if list & (1 << reg) != 0 {
                    self.r[reg] = v;
                }
            }
            if list & 0x8000 != 0 {
                self.bx_write(vals[15]);
            }
        } else {
            for reg in 0..16u32 {
                if list & (1 << reg) != 0 {
                    let v = self.tr(reg);
                    if let Err(status) = self.write_mem(bus, addr, 4, v) {
                        return self.abort_flow(addr, status);
                    }
                    addr = addr.wrapping_add(4);
                }
            }
            if write_back {
                self.r[rn as usize] = fin;
            }
        }
        Flow::Done
    }

    /// One load or store of `size` bytes at `addr` into or from `rt`. A word load into PC
    /// interworks; a sub-word load into PC is a hint (PLD and PLI) and does nothing.
    fn t_xfer(
        &mut self,
        bus: &mut dyn Bus,
        addr: u32,
        size: u32,
        rt: u32,
        store: bool,
        sign: bool,
    ) -> Flow {
        if store {
            let v = self.tr(rt);
            return match self.write_mem(bus, addr, size, v) {
                Ok(()) => Flow::Done,
                Err(status) => self.abort_flow(addr, status),
            };
        }
        let raw = match self.read_mem(bus, addr, size) {
            Ok(v) => v,
            Err(status) => return self.abort_flow(addr, status),
        };
        let v = if sign {
            match size {
                1 => raw as u8 as i8 as i32 as u32,
                2 => raw as u16 as i16 as i32 as u32,
                _ => raw,
            }
        } else {
            raw
        };
        if rt == 15 {
            if size == 4 {
                self.bx_write(v);
            }
            return Flow::Done;
        }
        self.r[rt as usize] = v;
        Flow::Done
    }

    // ----- 32-bit encodings ---------------------------------------------------------------

    fn exec32(&mut self, bus: &mut dyn Bus, insn: u32) -> Flow {
        if crate::vfp::is_fp_space(insn) {
            return self.vfp_insn(bus, insn);
        }
        let hw1 = insn >> 16;
        let hw2 = insn & 0xFFFF;
        match (hw1 >> 11) & 3 {
            1 => {
                if hw1 & 0x0400 != 0 {
                    self.t32_coproc(hw1, hw2)
                } else if hw1 & 0x0200 != 0 {
                    self.t32_dp_shifted(hw1, hw2)
                } else {
                    self.t32_block_dual(bus, hw1, hw2)
                }
            }
            2 => {
                if hw2 & 0x8000 == 0 {
                    if hw1 & 0x0200 == 0 {
                        self.t32_dp_imm(hw1, hw2)
                    } else {
                        self.t32_bin_imm(hw1, hw2)
                    }
                } else {
                    self.t32_branch_misc(hw1, hw2)
                }
            }
            3 => match hw1 >> 9 {
                0x7C => self.t32_single(bus, hw1, hw2),
                0x7D => match (hw1 >> 8) & 0xF {
                    0xA => self.t32_shift_extend(hw1, hw2),
                    0xB => self.t32_mul(hw1, hw2),
                    _ => Flow::Unimpl,
                },
                _ => Flow::Unimpl,
            },
            _ => Flow::Unimpl,
        }
    }

    /// Data processing (shifted register): the opcode, the flags and the write rules.
    fn t32_dp_shifted(&mut self, hw1: u32, hw2: u32) -> Flow {
        let opc = (hw1 >> 5) & 0xF;
        let s = hw1 & (1 << 4) != 0;
        let rn = hw1 & 0xF;
        let rd = (hw2 >> 8) & 0xF;
        let rm = hw2 & 0xF;
        let imm5 = (((hw2 >> 12) & 7) << 2) | ((hw2 >> 6) & 3);
        let ty = (hw2 >> 4) & 3;
        let cin = self.cpsr & C_BIT != 0;
        let (sh, sc) = alu::shift_imm(self.tr(rm), ty, imm5, cin);
        let a = self.dp_operand(opc, rn);
        self.t_dp_core(opc, a, sh, sc, s, rd)
    }

    /// Data processing (modified immediate).
    fn t32_dp_imm(&mut self, hw1: u32, hw2: u32) -> Flow {
        let opc = (hw1 >> 5) & 0xF;
        let s = hw1 & (1 << 4) != 0;
        let rn = hw1 & 0xF;
        let rd = (hw2 >> 8) & 0xF;
        let imm12 = (((hw1 >> 10) & 1) << 11) | (((hw2 >> 12) & 7) << 8) | (hw2 & 0xFF);
        let cin = self.cpsr & C_BIT != 0;
        let (imm, c) = thumb_expand(imm12, cin);
        let a = self.dp_operand(opc, rn);
        self.t_dp_core(opc, a, imm, c, s, rd)
    }

    /// The first operand of a data-processing instruction. MOV and MVN (ORR and ORN with Rn =
    /// PC) use zero, not the PC.
    fn dp_operand(&self, opc: u32, rn: u32) -> u32 {
        if rn == 15 && (opc == 2 || opc == 3) {
            0
        } else {
            self.tr(rn)
        }
    }

    /// The shared data-processing core of the 32-bit forms (same table as A32 `dp`).
    fn t_dp_core(&mut self, opc: u32, a: u32, b: u32, sc: bool, s: bool, rd: u32) -> Flow {
        let cin = self.cpsr & C_BIT != 0;
        let (res, carry, ovf, arith) = match opc {
            0 => (a & b, sc, false, false),
            1 => (a & !b, sc, false, false),
            2 => (a | b, sc, false, false),
            3 => (a | !b, sc, false, false),
            4 => (a ^ b, sc, false, false),
            8 => add3(a, b, false),
            10 => add3(a, b, cin),
            11 => add3(a, !b, cin),
            13 => add3(a, !b, true),
            14 => add3(!a, b, true),
            _ => return Flow::Unimpl,
        };
        // TST, TEQ, CMN and CMP are these opcodes with Rd = PC and S set: flags only.
        let compare = rd == 15 && s && matches!(opc, 0 | 4 | 8 | 13);
        let user = matches!(self.mode(), Mode::User | Mode::System);
        if s && rd == 15 && !compare && !user {
            // Exception return: CPSR := SPSR, then the branch with the new T bit.
            let spsr = self.spsr(self.mode());
            self.write_cpsr(spsr, u32::MAX);
            self.pc = res & if self.cpsr & T_BIT != 0 { !1 } else { !3 };
            return Flow::Done;
        }
        if s {
            self.set_nz(res);
            self.set_flag(C_BIT, carry);
            if arith {
                self.set_flag(V_BIT, ovf);
            }
        }
        if !compare {
            self.tw(rd, res);
        }
        Flow::Done
    }

    /// Plain binary immediates: ADDW and SUBW (ADR with PC), MOVW, MOVT and the bitfields.
    fn t32_bin_imm(&mut self, hw1: u32, hw2: u32) -> Flow {
        let op = (hw1 >> 4) & 0x1F;
        let rn = hw1 & 0xF;
        let rd = (hw2 >> 8) & 0xF;
        let imm12 = (((hw1 >> 10) & 1) << 11) | (((hw2 >> 12) & 7) << 8) | (hw2 & 0xFF);
        let imm16 = ((hw1 & 0xF) << 12) | imm12;
        let aligned_pc = self.cur.wrapping_add(4) & !3;
        match op {
            0b00000 => {
                let base = if rn == 15 {
                    aligned_pc
                } else {
                    self.r[rn as usize]
                };
                self.tw_nopc(rd, base.wrapping_add(imm12));
                Flow::Done
            }
            0b01010 => {
                let base = if rn == 15 {
                    aligned_pc
                } else {
                    self.r[rn as usize]
                };
                self.tw_nopc(rd, base.wrapping_sub(imm12));
                Flow::Done
            }
            0b00100 => {
                self.tw_nopc(rd, imm16);
                Flow::Done
            }
            0b01100 => {
                let v = (self.r[rd as usize] & 0xFFFF) | (imm16 << 16);
                self.tw_nopc(rd, v);
                Flow::Done
            }
            0b10000 => self.t32_ssat_usat(hw1, hw2, true),
            0b11000 => self.t32_ssat_usat(hw1, hw2, false),
            0b10100 | 0b11100 => {
                // SBFX (10100) and UBFX (11100): extract `width` bits at `lsb`.
                let lsb = (((hw2 >> 12) & 7) << 2) | ((hw2 >> 6) & 3);
                let width = (hw2 & 0x1F) + 1;
                let field = if width >= 32 {
                    self.r[rn as usize] >> lsb
                } else {
                    (self.r[rn as usize] >> lsb) & ((1u32 << width) - 1)
                };
                let v = if op == 0b10100 {
                    sext(field, width)
                } else {
                    field
                };
                self.tw_nopc(rd, v);
                Flow::Done
            }
            0b10110 => {
                // BFI (Rn != PC) and BFC (Rn = PC): replace bits [msb:lsb] of Rd.
                let lsb = (((hw2 >> 12) & 7) << 2) | ((hw2 >> 6) & 3);
                let msb = hw2 & 0x1F;
                if msb < lsb {
                    return Flow::Unimpl;
                }
                let width = msb - lsb + 1;
                let mask = if width >= 32 {
                    u32::MAX
                } else {
                    ((1u32 << width) - 1) << lsb
                };
                let src = if rn == 15 {
                    0
                } else {
                    self.r[rn as usize] << lsb
                };
                let v = (self.r[rd as usize] & !mask) | (src & mask);
                self.tw_nopc(rd, v);
                Flow::Done
            }
            _ => Flow::Unimpl,
        }
    }

    /// Writes a register that is never the PC in these encodings.
    fn tw_nopc(&mut self, rd: u32, v: u32) {
        self.r[rd as usize] = v;
    }

    /// SSAT (`signed`) and USAT: saturate a shifted register to the bit width in the low
    /// five bits of `hw2` plus one (SSAT) or as is (USAT); sets Q on saturation.
    fn t32_ssat_usat(&mut self, hw1: u32, hw2: u32, signed: bool) -> Flow {
        let rn = hw1 & 0xF;
        let rd = (hw2 >> 8) & 0xF;
        let imm5 = (((hw2 >> 12) & 7) << 2) | ((hw2 >> 6) & 3);
        let ty = if hw2 & (1 << 5) != 0 { 2 } else { 0 };
        let cin = self.cpsr & C_BIT != 0;
        let (v, _) = alu::shift_imm(self.r[rn as usize], ty, imm5, cin);
        let (value, sat) = if signed {
            let n = (hw2 & 0x1F) + 1;
            let hi = (1i64 << (n - 1)) - 1;
            let lo = -(1i64 << (n - 1));
            let x = i64::from(v as i32);
            let y = x.clamp(lo, hi);
            (y as i32 as u32, y != x)
        } else {
            let n = hw2 & 0x1F;
            let hi = (1i64 << n) - 1;
            let x = i64::from(v as i32);
            let y = x.clamp(0, hi);
            (y as u32, y != x)
        };
        if sat {
            self.cpsr |= Q_BIT;
        }
        self.tw_nopc(rd, value);
        Flow::Done
    }

    /// Load and store multiple (LDM, STM, PUSH, POP), dual (LDRD, STRD), exclusive (LDREX,
    /// STREX and their byte, halfword and doubleword forms) and table branch (TBB, TBH).
    fn t32_block_dual(&mut self, bus: &mut dyn Bus, hw1: u32, hw2: u32) -> Flow {
        let p = hw1 & (1 << 8) != 0;
        let u = hw1 & (1 << 7) != 0;
        let dual = hw1 & (1 << 6) != 0;
        let w = hw1 & (1 << 5) != 0;
        let l = hw1 & (1 << 4) != 0;
        let rn = hw1 & 0xF;
        if !dual {
            return match (p, u) {
                (false, true) => self.t_block(bus, rn, hw2, false, l, w),
                (true, false) => self.t_block(bus, rn, hw2, true, l, w),
                _ => Flow::Unimpl,
            };
        }
        if !p && !w {
            return if !u {
                let rt = hw2 >> 12;
                let addr = self.tr(rn).wrapping_add((hw2 & 0xFF) << 2);
                if l {
                    // LDREX: the address is exclusive; the monitor records it.
                    match self.read_mem(bus, addr, 4) {
                        Ok(v) => {
                            self.r[rt as usize] = v;
                            self.excl = Some(addr);
                            Flow::Done
                        }
                        Err(status) => self.abort_flow(addr, status),
                    }
                } else {
                    // STREX: Rd gets 0 on success, 1 when the monitor does not match.
                    let rd = (hw2 >> 8) & 0xF;
                    if self.excl != Some(addr) {
                        self.excl = None;
                        self.r[rd as usize] = 1;
                        return Flow::Done;
                    }
                    let v = self.tr(rt);
                    self.excl = None;
                    match self.write_mem(bus, addr, 4, v) {
                        Ok(()) => {
                            self.r[rd as usize] = 0;
                            Flow::Done
                        }
                        Err(status) => self.abort_flow(addr, status),
                    }
                }
            } else {
                self.t32_exclusive_byte(bus, hw1, hw2, l)
            };
        }
        // LDRD and STRD with an 8-bit scaled immediate.
        let rt = hw2 >> 12;
        let rt2 = (hw2 >> 8) & 0xF;
        let base = self.tr(rn);
        let off = (hw2 & 0xFF) << 2;
        let target = if u {
            base.wrapping_add(off)
        } else {
            base.wrapping_sub(off)
        };
        let addr = if p { target } else { base };
        if l {
            let a = match self.read_mem(bus, addr, 4) {
                Ok(v) => v,
                Err(status) => return self.abort_flow(addr, status),
            };
            let b = match self.read_mem(bus, addr.wrapping_add(4), 4) {
                Ok(v) => v,
                Err(status) => return self.abort_flow(addr, status),
            };
            self.r[rt as usize] = a;
            self.r[rt2 as usize] = b;
        } else {
            let (a, b) = (self.tr(rt), self.tr(rt2));
            if let Err(status) = self.write_mem(bus, addr, 4, a) {
                return self.abort_flow(addr, status);
            }
            if let Err(status) = self.write_mem(bus, addr.wrapping_add(4), 4, b) {
                return self.abort_flow(addr, status);
            }
        }
        if w && rn != 15 {
            self.r[rn as usize] = target;
        }
        Flow::Done
    }

    /// Byte, halfword and doubleword exclusives (U set), and TBB and TBH (L set, low byte 0).
    fn t32_exclusive_byte(&mut self, bus: &mut dyn Bus, hw1: u32, hw2: u32, l: bool) -> Flow {
        let rn = hw1 & 0xF;
        if l {
            if hw2 & 0xFFE0 == 0xF000 {
                // TBB (H = 0) and TBH (H = 1): PC + 2 * the table entry.
                let halfword = hw2 & (1 << 4) != 0;
                let idx = self.r[(hw2 & 0xF) as usize];
                let base = self.tr(rn);
                let entry = if halfword {
                    let a = base.wrapping_add(idx << 1);
                    match self.read_mem(bus, a, 2) {
                        Ok(v) => v,
                        Err(status) => return self.abort_flow(a, status),
                    }
                } else {
                    let a = base.wrapping_add(idx);
                    match self.read_mem(bus, a, 1) {
                        Ok(v) => v,
                        Err(status) => return self.abort_flow(a, status),
                    }
                };
                self.pc = self.cur.wrapping_add(4).wrapping_add(entry << 1);
                return Flow::Done;
            }
            let addr = self.r[rn as usize];
            let rt = hw2 >> 12;
            let (size, pair) = match hw2 & 0x0FFF {
                0xF4F => (1, false),
                0xF5F => (2, false),
                _ if hw2 & 0x00FF == 0x007F => (4, true),
                _ => return Flow::Unimpl,
            };
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
                self.r[((hw2 >> 8) & 0xF) as usize] = second;
            }
            return Flow::Done;
        }
        // STREXB, STREXH (low byte 0x40, 0x50) and STREXD (low byte 0x70).
        let addr = self.r[rn as usize];
        let rt = hw2 >> 12;
        let (size, pair, rd) = match hw2 & 0x0FF0 {
            0x0F40 => (1, false, hw2 & 0xF),
            0x0F50 => (2, false, hw2 & 0xF),
            0x0070 => (4, true, hw2 & 0xF),
            _ => return Flow::Unimpl,
        };
        if self.excl != Some(addr) {
            self.excl = None;
            self.r[rd as usize] = 1;
            return Flow::Done;
        }
        let first = self.tr(rt);
        let second = if pair { self.tr((hw2 >> 8) & 0xF) } else { 0 };
        self.excl = None;
        let res = self.write_mem(bus, addr, size, first).and_then(|()| {
            if pair {
                self.write_mem(bus, addr.wrapping_add(4), 4, second)
            } else {
                Ok(())
            }
        });
        match res {
            Ok(()) => {
                self.r[rd as usize] = 0;
                Flow::Done
            }
            Err(status) => self.abort_flow(addr, status),
        }
    }

    /// Single loads and stores in all forms: the 12-bit offset, the 8-bit offset with the
    /// pre, post and writeback bits, the register offset with a shift, and literals.
    fn t32_single(&mut self, bus: &mut dyn Bus, hw1: u32, hw2: u32) -> Flow {
        let size_sel = (hw1 >> 5) & 3;
        if size_sel == 3 {
            return Flow::Unimpl;
        }
        let size = [1, 2, 4][size_sel as usize];
        let load = hw1 & (1 << 4) != 0;
        let signed = hw1 & (1 << 8) != 0;
        if signed && !load {
            return Flow::Unimpl; // there is no store with a sign bit
        }
        let rn = hw1 & 0xF;
        let rt = hw2 >> 12;
        let aligned_pc = self.cur.wrapping_add(4) & !3;
        if rn == 15 {
            // Literal (PC-relative) load, offset 12 bits with the U bit in hw1.
            if !load {
                return Flow::Unimpl;
            }
            let off = hw2 & 0xFFF;
            let addr = if hw1 & (1 << 7) != 0 {
                aligned_pc.wrapping_add(off)
            } else {
                aligned_pc.wrapping_sub(off)
            };
            return self.t_xfer(bus, addr, size, rt, false, signed);
        }
        let base = self.r[rn as usize];
        // (access address, value for Rn, whether Rn is written back)
        let (addr, wb, writeback) = if hw1 & (1 << 7) != 0 {
            // 12-bit unsigned offset, no writeback.
            let a = base.wrapping_add(hw2 & 0xFFF);
            (a, a, false)
        } else if hw2 & (1 << 11) != 0 {
            // 8-bit offset: P (bit 10), U (bit 9), W (bit 8).
            let p = hw2 & (1 << 10) != 0;
            let u = hw2 & (1 << 9) != 0;
            let w = hw2 & (1 << 8) != 0;
            if !p && !w {
                return Flow::Unimpl; // unprivileged LDRT and STRT are not modelled
            }
            let off = hw2 & 0xFF;
            let target = if u {
                base.wrapping_add(off)
            } else {
                base.wrapping_sub(off)
            };
            (if p { target } else { base }, target, w)
        } else {
            // Register offset, LSL by 0 to 3.
            if hw2 & 0x0FC0 != 0 {
                return Flow::Unimpl;
            }
            let shift = (hw2 >> 4) & 3;
            let rm = hw2 & 0xF;
            let a = base.wrapping_add(self.r[rm as usize] << shift);
            (a, a, false)
        };
        if load && rt == 15 && size == 1 {
            return Flow::Done; // PLD and PLI: hints
        }
        let flow = self.t_xfer(bus, addr, size, rt, !load, signed);
        // Writeback happens only after a successful access: a fault leaves Rn unchanged.
        if writeback && matches!(flow, Flow::Done) {
            self.r[rn as usize] = wb;
        }
        flow
    }

    /// Shift by register (LSL, LSR, ASR, ROR), the extends (SXTH to UXTB, with the add forms),
    /// REV, REV16, RBIT, REVSH and CLZ.
    fn t32_shift_extend(&mut self, hw1: u32, hw2: u32) -> Flow {
        let rd = (hw2 >> 8) & 0xF;
        let rm = hw2 & 0xF;
        let rn = hw1 & 0xF;
        if hw1 & 0x80 != 0 {
            let top1 = (hw1 >> 4) & 0xF;
            let top2 = (hw2 >> 4) & 0xF;
            if top2 >= 8 {
                if top1 == 0xA && top2 == 0x8 {
                    return self.sel(rd, rn, rm);
                }
                let v = self.r[rm as usize];
                let out = match (top1, top2) {
                    (0x9, 0x8) => v.swap_bytes(),
                    (0x9, 0x9) => rev16(v),
                    (0x9, 0xA) => v.reverse_bits(),
                    (0x9, 0xB) => ((v as u16).swap_bytes() as i16) as i32 as u32,
                    (0xB, 0x8) => v.leading_zeros(),
                    _ => return Flow::Unimpl,
                };
                self.r[rd as usize] = out;
                return Flow::Done;
            }
            if (top2 & 3) != 3 {
                let sub_lanes = match top1 {
                    0x8 => 4, // 8-bit add (SADD8, QADD8, SHADD8, UADD8, UQADD8, UHADD8)
                    0x9 => 0, // 16-bit add (SADD16, QADD16, SHADD16, UADD16, UQADD16, UHADD16)
                    0xA => 1, // 16-bit asx (SASX, QASX, SHASX, UASX, UQASX, UHASX)
                    0xC => 7, // 8-bit sub (SSUB8, QSUB8, SHSUB8, USUB8, UQSUB8, UHSUB8)
                    0xD => 3, // 16-bit sub (SSUB16, QSUB16, SHSUB16, USUB16, UQSUB16, UHSUB16)
                    0xE => 2, // 16-bit sax (SSAX, QSAX, SHSAX, USAX, UQSAX, UHSAX)
                    _ => return Flow::Unimpl,
                };
                let signed = (top2 & 4) == 0;
                let mode = match top2 & 3 {
                    0 => crate::media::Lanes::Plain,
                    1 => crate::media::Lanes::Sat,
                    2 => crate::media::Lanes::Half,
                    _ => return Flow::Unimpl,
                };
                return self.parallel_core(mode, signed, sub_lanes, rn, rd, rm);
            }
            return Flow::Unimpl;
        }
        if hw2 & 0x80 == 0 {
            // LSL, LSR, ASR and ROR by a register; the low byte of Rm is the amount.
            let ty = (hw1 >> 5) & 3;
            let cin = self.cpsr & C_BIT != 0;
            let (v, c) = alu::shift_reg(self.r[rn as usize], ty, self.r[rm as usize] & 0xFF, cin);
            self.r[rd as usize] = v;
            if hw1 & (1 << 4) != 0 {
                self.set_nz(v);
                self.set_flag(C_BIT, c);
            }
            return Flow::Done;
        }
        // Extend with an optional rotation of 0, 8, 16 or 24 bits; Rn = PC is the plain form.
        let rot = (hw2 >> 4) & 3;
        let x = self.r[rm as usize].rotate_right(8 * rot);
        let op = (hw1 >> 4) & 7;
        let ext = match op {
            0 => (x as u16 as i16) as i32 as u32,
            1 => x & 0xFFFF,
            2 => {
                let l = (x as u8 as i8 as i32 as u32) & 0xFFFF;
                let h = ((x >> 16) as u8 as i8 as i32 as u32) & 0xFFFF;
                (h << 16) | l
            }
            3 => x & 0x00FF_00FF,
            4 => (x as u8 as i8) as i32 as u32,
            5 => x & 0xFF,
            _ => return Flow::Unimpl,
        };
        let v = if rn == 15 {
            ext
        } else if op == 2 || op == 3 {
            let n = self.r[rn as usize];
            let l = ((n & 0xFFFF).wrapping_add(ext & 0xFFFF)) & 0xFFFF;
            let h = ((n >> 16).wrapping_add(ext >> 16)) & 0xFFFF;
            (h << 16) | l
        } else {
            self.r[rn as usize].wrapping_add(ext)
        };
        self.r[rd as usize] = v;
        Flow::Done
    }

    /// Multiplies: MUL, MLA, MLS, the halfword multiplies, the long multiplies. The divides
    /// are undefined on the Cortex-A9 and take the undefined-instruction exception.
    fn t32_mul(&mut self, hw1: u32, hw2: u32) -> Flow {
        let rn = hw1 & 0xF;
        let m = (hw1 >> 4) & 0xF;
        let n = (hw2 >> 4) & 0xF;
        let rd = (hw2 >> 8) & 0xF;
        let ra = hw2 >> 12;
        let rm = hw2 & 0xF;
        match m {
            0 if n == 0 => {
                let v = self.r[rn as usize].wrapping_mul(self.r[rm as usize]);
                let v = if ra == 15 {
                    v
                } else {
                    v.wrapping_add(self.r[ra as usize])
                };
                self.r[rd as usize] = v;
                Flow::Done
            }
            0 if n == 1 => {
                let v = self.r[ra as usize]
                    .wrapping_sub(self.r[rn as usize].wrapping_mul(self.r[rm as usize]));
                self.r[rd as usize] = v;
                Flow::Done
            }
            1 if n & 0xC == 0 => {
                // SMULxy, SMLAxy: N (bit 5) selects the Rn half, M (bit 4) the Rm half.
                let x = half(self.r[rn as usize], hw2 & (1 << 5) != 0);
                let y = half(self.r[rm as usize], hw2 & (1 << 4) != 0);
                let p = x * y;
                let v = if ra == 15 {
                    p as u32
                } else {
                    let sum = i64::from(p) + i64::from(self.r[ra as usize] as i32);
                    if sum != i64::from(sum as i32) {
                        self.cpsr |= Q_BIT;
                    }
                    sum as i32 as u32
                };
                self.r[rd as usize] = v;
                Flow::Done
            }
            8 | 0xA | 0xC | 0xE if n == 0 => {
                // SMULL (1000), UMULL (1010), SMLAL (1100), UMLAL (1110).
                let signed = m & 2 == 0;
                let acc = m & 4 != 0;
                let (a, b) = (self.r[rn as usize], self.r[rm as usize]);
                let prod: u64 = if signed {
                    (i64::from(a as i32) * i64::from(b as i32)) as u64
                } else {
                    u64::from(a) * u64::from(b)
                };
                let total = if acc {
                    let old =
                        (u64::from(self.r[rd as usize]) << 32) | u64::from(self.r[ra as usize]);
                    prod.wrapping_add(old)
                } else {
                    prod
                };
                self.r[ra as usize] = total as u32;
                self.r[rd as usize] = (total >> 32) as u32;
                Flow::Done
            }
            0x9 | 0xB if n == 0xF => self.t_undef(4), // SDIV, UDIV: undefined on the A9
            _ => Flow::Unimpl,
        }
    }

    /// Coprocessor register transfers to CP15 (MCR and MRC). Other coprocessors are not modelled.
    fn t32_coproc(&mut self, hw1: u32, hw2: u32) -> Flow {
        if hw1 & 0xFE00 != 0xEE00 || hw2 & 0x10 == 0 || (hw2 >> 8) & 0xF != 15 {
            return Flow::Unimpl;
        }
        let opc1 = (hw1 >> 5) & 7;
        let load = hw1 & (1 << 4) != 0;
        let crn = hw1 & 0xF;
        let rt = hw2 >> 12;
        let opc2 = (hw2 >> 5) & 7;
        let crm = hw2 & 0xF;
        if load {
            let Some(v) = self.cp15.read(opc1, crn, crm, opc2) else {
                return Flow::Unimpl;
            };
            if rt == 15 {
                self.cpsr = (self.cpsr & 0x0FFF_FFFF) | (v & 0xF000_0000);
            } else {
                self.r[rt as usize] = v;
            }
        } else {
            let v = self.tr(rt);
            if !self.cp15.write(opc1, crn, crm, opc2, v) {
                return Flow::Unimpl;
            }
        }
        Flow::Done
    }

    /// Branches (B, BL, BLX, the conditional B.W), and the miscellaneous control group: MSR,
    /// MRS, the hints, the barriers and CLREX, and UDF.W.
    fn t32_branch_misc(&mut self, hw1: u32, hw2: u32) -> Flow {
        let s = (hw1 >> 10) & 1;
        let j1 = (hw2 >> 13) & 1;
        let j2 = (hw2 >> 11) & 1;
        let bit12 = hw2 & (1 << 12) != 0;
        let bit14 = hw2 & (1 << 14) != 0;
        let next = self.cur.wrapping_add(4);
        if bit12 || bit14 {
            let i1 = 1 - (j1 ^ s);
            let i2 = 1 - (j2 ^ s);
            let imm10 = hw1 & 0x3FF;
            if bit12 {
                // B.W (bit 14 clear) and BL (bit 14 set): a 25-bit offset.
                let imm11 = hw2 & 0x7FF;
                let x = (s << 24) | (i1 << 23) | (i2 << 22) | (imm10 << 12) | (imm11 << 1);
                let off = sext(x, 25);
                if bit14 {
                    self.r[14] = next | 1;
                }
                self.pc = next.wrapping_add(off);
                return Flow::Done;
            }
            // BLX (immediate): to ARM state; the offset is word aligned.
            if hw2 & 1 != 0 {
                return Flow::Unimpl;
            }
            let x = (s << 24) | (i1 << 23) | (i2 << 22) | (imm10 << 12) | ((hw2 & 0x7FE) << 1);
            let off = sext(x, 25);
            self.r[14] = next | 1;
            self.cpsr &= !T_BIT;
            self.pc = (next & !3).wrapping_add(off);
            return Flow::Done;
        }
        // MRS and MSR of the status registers: cond bits 1111 (MRS) and 1110 (MSR) in hw1.
        if hw1 & 0xFFE0 == 0xF380 || hw1 & 0xFFE0 == 0xF3E0 {
            return self.t32_msr_mrs(hw1, hw2);
        }
        let cond = (hw1 >> 6) & 0xF;
        if cond < 0xE {
            // B<cond>.W: a 21-bit offset.
            let imm6 = hw1 & 0x3F;
            let imm11 = hw2 & 0x7FF;
            let x = (s << 20) | (j2 << 19) | (j1 << 18) | (imm6 << 12) | (imm11 << 1);
            let off = sext(x, 21);
            if self.cond_passed(cond) {
                self.pc = next.wrapping_add(off);
            }
            return Flow::Done;
        }
        if cond == 0xF {
            return self.t_undef(4);
        }
        // Miscellaneous control (cond field 1110): hints and barriers.
        if hw1 == 0xF3AF && hw2 & 0xFF00 == 0x8000 {
            return self.t_hint(hw2 & 0xFF);
        }
        if hw1 == 0xF3BF && hw2 & 0xFF00 == 0x8F00 {
            // CLREX (2), DSB (4), DMB (5) and ISB (6). The model has no reordering to fence.
            let op = (hw2 >> 4) & 0xF;
            if op == 2 {
                self.excl = None;
            }
            return if matches!(op, 2 | 4 | 5 | 6) {
                Flow::Done
            } else {
                Flow::Unimpl
            };
        }
        Flow::Unimpl
    }

    /// MRS and MSR of the APSR, CPSR and SPSR (SYSm 0). Other system registers are not modelled.
    fn t32_msr_mrs(&mut self, hw1: u32, hw2: u32) -> Flow {
        if hw2 & 0x2000 != 0 || hw2 & 0x8000 == 0 || hw2 & 0xFF != 0 {
            return Flow::Unimpl;
        }
        let spsr = hw1 & (1 << 4) != 0;
        let rd_or_rn = if hw1 & 0xFFE0 == 0xF3E0 {
            (hw2 >> 8) & 0xF
        } else {
            hw1 & 0xF
        };
        if hw1 & 0xFFE0 == 0xF3E0 {
            // MRS
            let v = if spsr {
                if matches!(self.mode(), Mode::User | Mode::System) {
                    return Flow::Unimpl;
                }
                self.spsr(self.mode())
            } else {
                self.cpsr
            };
            self.r[rd_or_rn as usize] = v;
            return Flow::Done;
        }
        // MSR: the field mask is bits 11:8 of hw2.
        let fields = (hw2 >> 8) & 0xF;
        let v = self.tr(rd_or_rn);
        self.msr_write(spsr, fields, v);
        Flow::Done
    }
}

/// Byte-reverses each halfword of `v`.
fn rev16(v: u32) -> u32 {
    ((v & 0x00FF_00FF) << 8) | ((v >> 8) & 0x00FF_00FF)
}
