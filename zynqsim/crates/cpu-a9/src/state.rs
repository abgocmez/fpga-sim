//! Architectural state of one core, the step loop, banking and exception entry.

use ps_bus::Bus;

use crate::cp15::{Cp15, SCTLR_M, SCTLR_TE, SCTLR_V};
use crate::xlate::{Identity, Translate};

/// CPSR.N.
pub(crate) const N_BIT: u32 = 1 << 31;
/// CPSR.Z.
pub(crate) const Z_BIT: u32 = 1 << 30;
/// CPSR.C.
pub(crate) const C_BIT: u32 = 1 << 29;
/// CPSR.V.
pub(crate) const V_BIT: u32 = 1 << 28;
/// CPSR.Q (sticky saturation flag).
pub(crate) const Q_BIT: u32 = 1 << 27;
/// CPSR.GE[3:0].
pub(crate) const GE_MASK: u32 = 0xF << 16;
/// CPSR.A (asynchronous abort mask; not taken by this core, kept for MRS/MSR).
pub(crate) const A_BIT: u32 = 1 << 8;
/// CPSR.I (IRQ mask).
pub(crate) const I_BIT: u32 = 1 << 7;
/// CPSR.F (FIQ mask).
pub(crate) const F_BIT: u32 = 1 << 6;
/// CPSR.T (Thumb state). Thumb is not implemented: a set T stops the core.
pub(crate) const T_BIT: u32 = 1 << 5;
/// PSR bits that MSR cannot write from CPSR (E, the J bit and the reserved gap).
pub(crate) const PSR_RESERVED: u32 = (1 << 24) | (0xF << 20) | (1 << 9);
/// The CPSR fields MSR may write: NZCVQ, GE, A, I, F and the mode.
pub(crate) const CPSR_MSR_MASK: u32 = 0xF800_0000 | GE_MASK | 0x1C0 | 0x1F;

/// Processor modes (CPSR.M). Monitor and Hyp are not modelled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// User mode (0x10).
    User,
    /// FIQ (0x11).
    Fiq,
    /// IRQ (0x12).
    Irq,
    /// Supervisor (0x13).
    Supervisor,
    /// Abort (0x17).
    Abort,
    /// Undefined (0x1B).
    Undefined,
    /// System (0x1F): user registers, privileged.
    System,
}

impl Mode {
    /// The CPSR.M encoding.
    pub const fn bits(self) -> u32 {
        match self {
            Mode::User => 0x10,
            Mode::Fiq => 0x11,
            Mode::Irq => 0x12,
            Mode::Supervisor => 0x13,
            Mode::Abort => 0x17,
            Mode::Undefined => 0x1B,
            Mode::System => 0x1F,
        }
    }

    /// The mode for a CPSR.M encoding, if it is one this core implements.
    pub fn from_bits(b: u32) -> Option<Mode> {
        Some(match b & 0x1F {
            0x10 => Mode::User,
            0x11 => Mode::Fiq,
            0x12 => Mode::Irq,
            0x13 => Mode::Supervisor,
            0x17 => Mode::Abort,
            0x1B => Mode::Undefined,
            0x1F => Mode::System,
            _ => return None,
        })
    }

    /// Index of this mode's banked registers (User and System share bank 0).
    pub(crate) const fn bank(self) -> usize {
        match self {
            Mode::User | Mode::System => 0,
            Mode::Fiq => 1,
            Mode::Irq => 2,
            Mode::Supervisor => 3,
            Mode::Abort => 4,
            Mode::Undefined => 5,
        }
    }
}

/// Something a step did that the caller may need to act on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    /// WFI executed: the core now waits for an interrupt.
    Wfi,
    /// WFE executed.
    Wfe,
    /// SEV executed.
    Sev,
    /// The core is waiting in WFI/WFE and no wake-up condition holds (nothing executed).
    Idle,
    /// SVC executed; the field is the 24-bit immediate. The exception is already taken.
    Svc(u32),
    /// BKPT executed; the field is the 16-bit immediate. Taken as a prefetch abort.
    Bkpt(u16),
    /// An undefined instruction; the exception is already taken.
    Undefined,
    /// A data abort at `addr`; the exception is already taken.
    DataAbort {
        /// The faulting virtual address.
        addr: u32,
    },
    /// A prefetch abort at `addr`; the exception is already taken.
    PrefetchAbort {
        /// The faulting fetch address.
        addr: u32,
    },
    /// The IRQ exception was taken (nothing executed this step).
    Irq,
    /// The FIQ exception was taken (nothing executed this step).
    Fiq,
}

/// The outcome of one [`Cpu::step`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StepResult {
    /// One step of time passed (`cycles` is always 1). `pc` is the address of the instruction
    /// executed, or of the next one for an exception taken before the fetch.
    Executed {
        /// Address of the instruction (or of the pending one for an interrupt).
        pc: u32,
        /// Always 1: the model is instruction-count timed.
        cycles: u32,
        /// What happened besides the plain execution, if anything.
        event: Option<Event>,
    },
    /// The instruction is not implemented; no state was changed and the PC did not advance.
    Unimplemented {
        /// Address of the instruction.
        pc: u32,
        /// The instruction word (A32), or the halfword for a Thumb state.
        insn: u32,
    },
}

/// Why a step is waiting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Wait {
    Wfi,
    Wfe,
}

/// Result of executing one decoded instruction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Flow {
    /// Completed; the PC is the next instruction (or a branch target).
    Done,
    /// Completed with an event (an exception may already have been taken).
    Event(Event),
    /// Not implemented; nothing was changed.
    Unimpl,
}

/// One ARMv7-A core (A32 only), interpreting instructions one at a time.
pub struct Cpu {
    /// The general registers of the current mode view. `r[15]` is unused: the PC is `pc`.
    pub(crate) r: [u32; 16],
    /// Address of the next instruction (what R15 reads as outside an instruction).
    pub(crate) pc: u32,
    /// Address of the instruction being executed (R15 reads as `cur + 8`).
    pub(crate) cur: u32,
    /// CPSR.
    pub(crate) cpsr: u32,
    /// SPSR per banked mode (index 0 unused).
    pub(crate) spsr: [u32; 6],
    /// R13 and R14 of each bank; the active bank's copies live in `r`.
    pub(crate) bank_sp_lr: [[u32; 2]; 6],
    /// R8 to R12 of FIQ mode while another mode is active.
    pub(crate) fiq_hi: [u32; 5],
    /// R8 to R12 of the non-FIQ modes while FIQ mode is active.
    pub(crate) usr_hi: [u32; 5],
    /// The IRQ input line (level).
    pub(crate) irq_line: bool,
    /// The FIQ input line (level).
    pub(crate) fiq_line: bool,
    /// The WFI/WFE wait state, if any.
    pub(crate) wait: Option<Wait>,
    /// The event register (set by SEV and [`Cpu::send_event`]).
    pub(crate) event_reg: bool,
    /// The local exclusive monitor's address.
    pub(crate) excl: Option<u32>,
    /// The CP15 registers.
    pub(crate) cp15: Cp15,
    /// The virtual-to-physical hook.
    pub(crate) xlate: Box<dyn Translate + Send>,
}

impl Cpu {
    /// A core in its reset state (Supervisor mode, IRQ and FIQ masked, PC 0), numbered
    /// `cpu_id` in MPIDR.
    pub fn new(cpu_id: u32) -> Cpu {
        Cpu {
            r: [0; 16],
            pc: 0,
            cur: 0,
            cpsr: Mode::Supervisor.bits() | I_BIT | F_BIT,
            spsr: [0; 6],
            bank_sp_lr: [[0; 2]; 6],
            fiq_hi: [0; 5],
            usr_hi: [0; 5],
            irq_line: false,
            fiq_line: false,
            wait: None,
            event_reg: false,
            excl: None,
            cp15: Cp15::new(cpu_id),
            xlate: Box::new(Identity),
        }
    }

    /// Returns the core to its reset state with the PC at `entry`.
    pub fn reset(&mut self, entry: u32) {
        let id = self.cp15.cpu_id;
        let xlate = std::mem::replace(&mut self.xlate, Box::new(Identity));
        *self = Cpu::new(id);
        self.xlate = xlate;
        self.pc = entry;
        self.cur = entry;
    }

    /// Reads a CP15 register by its `MRC` encoding (`opc1`, `CRn`, `CRm`, `opc2`); `None` when
    /// the register is not modelled.
    pub fn cp15(&self, opc1: u8, crn: u8, crm: u8, opc2: u8) -> Option<u32> {
        self.cp15.read(
            u32::from(opc1),
            u32::from(crn),
            u32::from(crm),
            u32::from(opc2),
        )
    }

    /// Sets the whole CPSR (mode change included), as an MSR of every field would.
    pub fn set_cpsr(&mut self, v: u32) {
        self.write_cpsr(v, u32::MAX);
    }

    /// Replaces the address translation (the MMU hook).
    pub fn set_translate(&mut self, xlate: Box<dyn Translate + Send>) {
        self.xlate = xlate;
    }

    /// R0 to R15 as the current mode sees them (R15 reads as the next instruction address).
    pub fn reg(&self, n: u8) -> u32 {
        if n == 15 {
            self.pc
        } else {
            self.r[usize::from(n & 15)]
        }
    }

    /// Writes R0 to R15 of the current mode. Writing R15 branches (no interworking).
    pub fn set_reg(&mut self, n: u8, v: u32) {
        if n == 15 {
            self.pc = v & !3;
        } else {
            self.r[usize::from(n & 15)] = v;
        }
    }

    /// The program counter (address of the next instruction).
    pub fn pc(&self) -> u32 {
        self.pc
    }

    /// The current program status register.
    pub fn cpsr(&self) -> u32 {
        self.cpsr
    }

    /// The current processor mode.
    pub fn mode(&self) -> Mode {
        Mode::from_bits(self.cpsr).unwrap_or(Mode::User)
    }

    /// The SPSR of `mode` (the banked copy, not the current one).
    pub fn spsr(&self, mode: Mode) -> u32 {
        self.spsr[mode.bank()]
    }

    /// Sets the SPSR of `mode`.
    pub fn set_spsr(&mut self, mode: Mode, v: u32) {
        self.spsr[mode.bank()] = v & !PSR_RESERVED;
    }

    /// Sets the IRQ input line. The interrupt is taken between instructions when CPSR.I is 0.
    pub fn irq(&mut self, level: bool) {
        self.irq_line = level;
    }

    /// Sets the FIQ input line. Taken between instructions when CPSR.F is 0.
    pub fn fiq(&mut self, level: bool) {
        self.fiq_line = level;
    }

    /// Signals an event to this core: wakes a WFE, sets the event register.
    pub fn send_event(&mut self) {
        self.event_reg = true;
    }

    /// Whether the core sits in WFI or WFE with nothing to wake it.
    pub fn is_waiting(&self) -> bool {
        self.wait.is_some()
    }

    /// The core number (MPIDR affinity 0, bits 1:0).
    pub fn core_id(&self) -> u32 {
        self.cp15.cpu_id
    }

    /// The event register: SEV sets it on every core, WFE consumes it.
    pub fn event_register(&self) -> bool {
        self.event_reg
    }

    /// The address the local exclusive monitor is armed on by an LDREX, if any.
    pub fn exclusive_address(&self) -> Option<u32> {
        self.excl
    }

    /// Disarms the exclusive monitor. The PS's global monitor calls it on the other cores
    /// when a core writes the reserved granule.
    pub fn clear_exclusive(&mut self) {
        self.excl = None;
    }

    /// Executes one instruction (or takes one pending interrupt, or idles in WFI/WFE).
    pub fn step(&mut self, bus: &mut dyn Bus) -> StepResult {
        let pc = self.pc;
        if self.cp15.sctlr & SCTLR_M != 0 {
            // The MMU is not implemented: refuse rather than run with physical addressing.
            let insn = self.fetch(bus, pc).unwrap_or(0);
            return StepResult::Unimplemented { pc, insn };
        }
        if let Some(w) = self.wait {
            let wake_event = w == Wait::Wfe && self.event_reg;
            if !(self.irq_line || self.fiq_line || wake_event) {
                return executed(pc, Some(Event::Idle));
            }
            self.wait = None;
            if w == Wait::Wfe {
                self.event_reg = false;
            }
        }
        if self.fiq_line && self.cpsr & F_BIT == 0 {
            self.exception(Mode::Fiq, 0x1C, pc.wrapping_add(4), true);
            return executed(pc, Some(Event::Fiq));
        }
        if self.irq_line && self.cpsr & I_BIT == 0 {
            self.exception(Mode::Irq, 0x18, pc.wrapping_add(4), false);
            return executed(pc, Some(Event::Irq));
        }
        if self.cpsr & T_BIT != 0 {
            // Thumb is not implemented; report the halfword and do not run it.
            let hw = self.fetch16(bus, pc).unwrap_or(0);
            return StepResult::Unimplemented {
                pc,
                insn: u32::from(hw),
            };
        }
        let insn = match self.fetch(bus, pc) {
            Ok(v) => v,
            Err(status) => {
                self.cur = pc;
                self.prefetch_abort(pc, status);
                return executed(pc, Some(Event::PrefetchAbort { addr: pc }));
            }
        };
        self.cur = pc;
        self.pc = pc.wrapping_add(4);
        match self.execute(bus, insn) {
            Flow::Done => executed(pc, None),
            Flow::Event(e) => executed(pc, Some(e)),
            Flow::Unimpl => {
                self.pc = pc;
                StepResult::Unimplemented { pc, insn }
            }
        }
    }

    fn fetch(&mut self, bus: &mut dyn Bus, pc: u32) -> Result<u32, u8> {
        let pa = self.xlate.translate(pc, crate::xlate::Access::Fetch)?;
        bus.read32(pa).map_err(|_| crate::mem::FSR_EXTERNAL)
    }

    fn fetch16(&mut self, bus: &mut dyn Bus, pc: u32) -> Result<u16, u8> {
        let pa = self.xlate.translate(pc, crate::xlate::Access::Fetch)?;
        bus.read(pa, 2)
            .map(|v| v as u16)
            .map_err(|_| crate::mem::FSR_EXTERNAL)
    }

    /// Address of the vector table: VBAR, or 0xFFFF0000 with SCTLR.V.
    fn vector_base(&self) -> u32 {
        if self.cp15.sctlr & SCTLR_V != 0 {
            0xFFFF_0000
        } else {
            self.cp15.vbar
        }
    }

    /// Takes an exception: switches to `mode`, saves CPSR to its SPSR, sets LR to `lr`,
    /// masks IRQ (and FIQ for `fiq`) and jumps to the vector at `offset`.
    pub(crate) fn exception(&mut self, mode: Mode, offset: u32, lr: u32, fiq: bool) {
        let old = self.cpsr;
        let base = self.vector_base();
        self.switch_mode(mode);
        self.spsr[mode.bank()] = old;
        self.r[14] = lr;
        self.cpsr |= I_BIT;
        if fiq {
            self.cpsr |= F_BIT;
        }
        self.cpsr &= !T_BIT;
        if self.cp15.sctlr & SCTLR_TE != 0 {
            self.cpsr |= T_BIT;
        }
        self.pc = base.wrapping_add(offset);
        self.excl = None;
    }

    /// Records a prefetch abort (IFSR/IFAR) and takes the exception.
    pub(crate) fn prefetch_abort(&mut self, addr: u32, status: u8) {
        self.cp15.ifsr = u32::from(status);
        self.cp15.ifar = addr;
        self.exception(Mode::Abort, 0x0C, addr.wrapping_add(4), false);
    }

    /// Records a data abort at `addr` (DFSR/DFAR) and takes the exception. LR is the faulting
    /// instruction + 8, so `SUBS PC, LR, #8` retries it.
    pub(crate) fn data_abort(&mut self, addr: u32, status: u8) {
        self.cp15.dfsr = u32::from(status);
        self.cp15.dfar = addr;
        let lr = self.cur.wrapping_add(8);
        self.exception(Mode::Abort, 0x10, lr, false);
    }

    /// Switches the banked registers from the current mode to `new` and sets CPSR.M.
    pub(crate) fn switch_mode(&mut self, new: Mode) {
        let old = self.mode();
        let (oi, ni) = (old.bank(), new.bank());
        if oi != ni {
            self.bank_sp_lr[oi] = [self.r[13], self.r[14]];
            self.r[13] = self.bank_sp_lr[ni][0];
            self.r[14] = self.bank_sp_lr[ni][1];
            if old == Mode::Fiq {
                self.fiq_hi.copy_from_slice(&self.r[8..13]);
                self.r[8..13].copy_from_slice(&self.usr_hi);
            } else if new == Mode::Fiq {
                self.usr_hi.copy_from_slice(&self.r[8..13]);
                self.r[8..13].copy_from_slice(&self.fiq_hi);
            }
        }
        self.cpsr = (self.cpsr & !0x1F) | new.bits();
    }

    /// Writes the CPSR fields selected by `mask`, switching banks if the mode changes. An
    /// invalid mode encoding keeps the old mode.
    pub(crate) fn write_cpsr(&mut self, v: u32, mask: u32) {
        let merged = (self.cpsr & !mask) | (v & mask);
        match Mode::from_bits(merged) {
            Some(m) => {
                if m != self.mode() {
                    self.switch_mode(m);
                }
                self.cpsr = (merged & !0x1F) | m.bits();
            }
            None => {
                self.cpsr = (merged & !0x1F) | (self.cpsr & 0x1F);
            }
        }
    }

    /// Reads the user-mode view of register `n` (for LDM/STM with the S bit).
    pub(crate) fn user_reg(&self, n: usize) -> u32 {
        let mode = self.mode();
        match n {
            8..=12 if mode == Mode::Fiq => self.usr_hi[n - 8],
            13 | 14 if !matches!(mode, Mode::User | Mode::System) => self.bank_sp_lr[0][n - 13],
            _ => self.r[n],
        }
    }

    /// Writes the user-mode view of register `n`.
    pub(crate) fn set_user_reg(&mut self, n: usize, v: u32) {
        let mode = self.mode();
        match n {
            8..=12 if mode == Mode::Fiq => self.usr_hi[n - 8] = v,
            13 | 14 if !matches!(mode, Mode::User | Mode::System) => self.bank_sp_lr[0][n - 13] = v,
            _ => self.r[n] = v,
        }
    }
}

fn executed(pc: u32, event: Option<Event>) -> StepResult {
    StepResult::Executed {
        pc,
        cycles: 1,
        event,
    }
}
