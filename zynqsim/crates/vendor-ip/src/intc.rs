//! AXI Interrupt Controller (`xilinx.com:ip:axi_intc:4.1`, Xilinx PG099), register level.
//!
//! Register map (offsets from the base address; recalled from PG099, **not** checked against
//! hardware or a Xilinx simulation):
//!
//! | offset | register | access |
//! |---|---|---|
//! | `0x00` | `ISR` interrupt status | R, W sets bits in software mode |
//! | `0x04` | `IPR` pending = `ISR & IER` | R |
//! | `0x08` | `IER` enable | R/W |
//! | `0x0C` | `IAR` acknowledge | W (1 clears the `ISR` bit) |
//! | `0x10` | `SIE` set enable bits | W |
//! | `0x14` | `CIE` clear enable bits | W |
//! | `0x18` | `IVR` vector of the lowest pending enabled input, `0xFFFFFFFF` if none | R |
//! | `0x1C` | `MER` bit 0 `ME` master enable, bit 1 `HIE` hardware interrupt enable | R/W |
//! | `0x20` | `IMR` fast interrupt mode (stored) | R/W |
//! | `0x24` | `ILR` interrupt level (stored) | R/W |
//! | `0x100 + 4n` | `IVAR[n]` vector address (stored) | R/W |
//!
//! Inputs are `C_NUM_INTR_INPUTS` wide. Each input is edge (`C_KIND_OF_INTR` bit set) or level
//! sensitive, with the active edge (`C_KIND_OF_EDGE`: 1 rising) or level (`C_KIND_OF_LVL`: 1
//! high). An edge input sets its `ISR` bit on the active edge; a level input shows in `ISR`
//! while it is asserted (`IAR` cannot clear it). The output `irq` is `ME & (IPR != 0)`, a level
//! (see `VERIFY:` for `C_IRQ_IS_LEVEL`).
//!
//! The inputs are the interrupt outputs of other models, routed by the project's `[connect]`
//! entries (`"axi_timer_0.interrupt" = "axi_intc_0.intr[1]"`), or the host through
//! [`IntcHandle::set_input`].
//!
//! The register offsets, names, access types and the existence of `IMR`/`ILR`/`IVAR` (only with
//! `C_HAS_FAST`, reset value `0x10`) were checked against the register metadata in a Vivado
//! 2022.1 hardware handoff (`fixtures/xsa/zs_intc.xsa`); the behaviour below was not.
//!
//! Not modelled (stubs): fast interrupt mode (`IMR`, `ILR`, `IVAR` store what is written and
//! have no effect; the extended vector registers do not exist), cascade mode (`C_HAS_CASC`),
//! the processor interrupt-vector output.
//!
//! `VERIFY:` items:
//! * Hardware inputs are recognised only after `MER.HIE` is set; before that `ISR` can be
//!   written (software interrupts). `HIE` can be set but not cleared by software; only a reset
//!   clears it.
//! * `ISR` writes are ignored once `HIE` is set.
//! * `IVR` returns the lowest-numbered pending enabled input (priority is the bit number).
//! * With `C_IRQ_IS_LEVEL = 0` (edge output) the model still drives a level: the IRQ_F2P line
//!   stays high while an interrupt is pending instead of pulsing once.
//! * `IVR` resets to `0xFFFFFFFF` (the handoff metadata says 0 for the 5-bit field).
//! * Unknown offsets read 0 and ignore writes. Registers for inputs beyond
//!   `C_NUM_INTR_INPUTS` read 0.

use crate::params::{Params, flag, merge, number, ranged};
use pl_native::RegisterBlock;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

/// `ISR`.
pub const ISR: u64 = 0x00;
/// `IPR`.
pub const IPR: u64 = 0x04;
/// `IER`.
pub const IER: u64 = 0x08;
/// `IAR`.
pub const IAR: u64 = 0x0C;
/// `SIE`.
pub const SIE: u64 = 0x10;
/// `CIE`.
pub const CIE: u64 = 0x14;
/// `IVR`.
pub const IVR: u64 = 0x18;
/// `MER`.
pub const MER: u64 = 0x1C;
/// `IMR`.
pub const IMR: u64 = 0x20;
/// `ILR`.
pub const ILR: u64 = 0x24;
/// First `IVAR`.
pub const IVAR: u64 = 0x100;

/// `MER` bit: master enable.
pub const MER_ME: u32 = 1 << 0;
/// `MER` bit: hardware interrupt enable.
pub const MER_HIE: u32 = 1 << 1;

/// The IP's configuration (`C_*` parameters).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IntcConfig {
    /// `C_NUM_INTR_INPUTS`, 1 to 32.
    pub inputs: u32,
    /// `C_KIND_OF_INTR`: bit set = edge sensitive.
    pub kind_edge: u32,
    /// `C_KIND_OF_EDGE`: bit set = rising edge (edge inputs).
    pub edge_rising: u32,
    /// `C_KIND_OF_LVL`: bit set = active high (level inputs).
    pub level_high: u32,
    /// `C_IRQ_IS_LEVEL` (informational, see the module notes).
    pub irq_is_level: bool,
    /// `C_HAS_FAST`: the fast-interrupt registers (`IMR`, `ILR`, `IVAR`) exist (and only store).
    pub has_fast: bool,
    /// `C_IVAR_RESET_VALUE` (default `0x10`).
    pub ivar_reset: u32,
    /// Size of the address window in bytes.
    pub window: u64,
}

impl Default for IntcConfig {
    fn default() -> Self {
        IntcConfig {
            inputs: 2,
            kind_edge: u32::MAX,
            edge_rising: u32::MAX,
            level_high: u32::MAX,
            irq_is_level: true,
            has_fast: false,
            ivar_reset: 0x10,
            window: 0x1_0000,
        }
    }
}

impl IntcConfig {
    /// Reads the configuration from IP parameters.
    pub fn from_params(p: &Params, window: u64) -> Result<IntcConfig, String> {
        if flag(p, "C_HAS_CASC", false)? {
            return Err("cascade mode (C_HAS_CASC) is not modelled".to_string());
        }
        let mask =
            |key: &str| -> Result<u32, String> { Ok(number(p, key, u64::from(u32::MAX))? as u32) };
        Ok(IntcConfig {
            inputs: ranged(p, "C_NUM_INTR_INPUTS", 2, 1, 32)? as u32,
            kind_edge: mask("C_KIND_OF_INTR")?,
            edge_rising: mask("C_KIND_OF_EDGE")?,
            level_high: mask("C_KIND_OF_LVL")?,
            irq_is_level: flag(p, "C_IRQ_IS_LEVEL", true)?,
            has_fast: flag(p, "C_HAS_FAST", false)?,
            ivar_reset: number(p, "C_IVAR_RESET_VALUE", 0x10)? as u32,
            window,
        })
    }

    fn mask(&self) -> u32 {
        if self.inputs >= 32 {
            u32::MAX
        } else {
            (1u32 << self.inputs) - 1
        }
    }
}

#[derive(Debug)]
struct State {
    cfg: IntcConfig,
    /// Latched status: edge captures and software interrupts.
    latched: u32,
    /// Raw input levels.
    level: u32,
    ier: u32,
    mer: u32,
    imr: u32,
    ilr: u32,
    ivar: [u32; 32],
}

impl State {
    fn power_on(cfg: IntcConfig) -> State {
        State {
            ivar: [cfg.ivar_reset; 32],
            cfg,
            latched: 0,
            level: 0,
            ier: 0,
            mer: 0,
            imr: 0,
            ilr: 0,
        }
    }

    fn reset(&mut self) {
        let cfg = self.cfg.clone();
        *self = State::power_on(cfg);
    }

    fn hie(&self) -> bool {
        self.mer & MER_HIE != 0
    }

    /// Level inputs currently asserted.
    fn level_active(&self) -> u32 {
        let lvl = !self.cfg.kind_edge;
        let active = !(self.level ^ self.cfg.level_high);
        lvl & active & self.cfg.mask()
    }

    fn isr(&self) -> u32 {
        let hw = if self.hie() { self.level_active() } else { 0 };
        (self.latched | hw) & self.cfg.mask()
    }

    fn ipr(&self) -> u32 {
        self.isr() & self.ier
    }

    fn input(&mut self, n: u32, level: bool) {
        if n >= self.cfg.inputs {
            return;
        }
        let bit = 1u32 << n;
        let old = self.level & bit != 0;
        if level {
            self.level |= bit;
        } else {
            self.level &= !bit;
        }
        if self.cfg.kind_edge & bit != 0 && self.hie() {
            let rising = self.cfg.edge_rising & bit != 0;
            if (rising && !old && level) || (!rising && old && !level) {
                self.latched |= bit;
            }
        }
    }
}

/// An AXI interrupt controller as a [`RegisterBlock`].
pub struct AxiIntc {
    name: String,
    st: Arc<Mutex<State>>,
}

/// A clonable handle to the inputs of an [`AxiIntc`]: how the host (and tests) raise them.
#[derive(Clone)]
pub struct IntcHandle {
    st: Arc<Mutex<State>>,
}

impl AxiIntc {
    /// A freshly reset instance named `name` and the handle to its inputs.
    pub fn new(name: &str, cfg: IntcConfig) -> (AxiIntc, IntcHandle) {
        let st = Arc::new(Mutex::new(State::power_on(cfg)));
        (
            AxiIntc {
                name: name.to_string(),
                st: Arc::clone(&st),
            },
            IntcHandle { st },
        )
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.st.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl IntcHandle {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.st.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The configuration.
    pub fn config(&self) -> IntcConfig {
        self.lock().cfg.clone()
    }

    /// Drives interrupt input `n` to `level`.
    pub fn set_input(&self, n: u32, level: bool) {
        self.lock().input(n, level);
    }

    /// `ISR`, `IER` and `MER` as the registers read now.
    pub fn registers(&self) -> (u32, u32, u32) {
        let s = self.lock();
        (s.isr(), s.ier, s.mer)
    }

    /// The output level.
    pub fn irq(&self) -> bool {
        let s = self.lock();
        s.mer & MER_ME != 0 && s.ipr() != 0
    }
}

impl RegisterBlock for AxiIntc {
    fn name(&self) -> &str {
        &self.name
    }

    fn size(&self) -> u64 {
        self.lock().cfg.window
    }

    fn read32(&mut self, offset: u64) -> u32 {
        let s = self.lock();
        match offset {
            ISR => s.isr(),
            IPR => s.ipr(),
            IER => s.ier,
            IVR => {
                let p = s.ipr();
                if p == 0 { u32::MAX } else { p.trailing_zeros() }
            }
            MER => s.mer,
            IMR if s.cfg.has_fast => s.imr,
            ILR if s.cfg.has_fast => s.ilr,
            o if s.cfg.has_fast && (IVAR..IVAR + 4 * u64::from(s.cfg.inputs)).contains(&o) => {
                s.ivar[((o - IVAR) / 4) as usize]
            }
            _ => 0,
        }
    }

    fn write32(&mut self, offset: u64, value: u32, strobe: u8) {
        let mut s = self.lock();
        let mask = s.cfg.mask();
        let v = merge(0, value, strobe);
        match offset {
            ISR => {
                if !s.hie() {
                    s.latched |= v & mask;
                }
            }
            IER => s.ier = merge(s.ier, value, strobe) & mask,
            IAR => s.latched &= !(v & mask),
            SIE => s.ier |= v & mask,
            CIE => s.ier &= !(v & mask),
            MER => {
                let new = merge(s.mer, value, strobe) & (MER_ME | MER_HIE);
                // HIE is write-once: only a reset clears it.
                s.mer = (new & MER_ME) | (new & MER_HIE) | (s.mer & MER_HIE);
            }
            IMR if s.cfg.has_fast => s.imr = merge(s.imr, value, strobe),
            ILR if s.cfg.has_fast => s.ilr = merge(s.ilr, value, strobe),
            o if s.cfg.has_fast && (IVAR..IVAR + 4 * u64::from(s.cfg.inputs)).contains(&o) => {
                let i = ((o - IVAR) / 4) as usize;
                s.ivar[i] = merge(s.ivar[i], value, strobe);
            }
            _ => {}
        }
    }

    fn reset(&mut self) {
        self.lock().reset();
    }

    fn irq_level(&self) -> bool {
        let s = self.lock();
        s.mer & MER_ME != 0 && s.ipr() != 0
    }

    fn set_irq_input(&mut self, input: u32, level: bool, _now_ns: u64) {
        self.lock().input(input, level);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn intc(p: &[(&str, &str)]) -> (AxiIntc, IntcHandle) {
        let params: Params = p
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect::<BTreeMap<_, _>>();
        AxiIntc::new("intc", IntcConfig::from_params(&params, 0x1_0000).unwrap())
    }

    fn wr(b: &mut AxiIntc, off: u64, v: u32) {
        b.write32(off, v, 0xF);
    }

    fn real_mode(b: &mut AxiIntc) {
        wr(b, MER, MER_ME | MER_HIE);
    }

    #[test]
    fn reset_state() {
        let (mut b, _) = intc(&[]);
        for off in [ISR, IPR, IER, MER] {
            assert_eq!(b.read32(off), 0, "{off:#x}");
        }
        assert_eq!(b.read32(IVR), 0xFFFF_FFFF);
        assert!(!b.irq_level());
    }

    #[test]
    fn edge_input_latches_until_acknowledged() {
        let (mut b, _) = intc(&[("C_NUM_INTR_INPUTS", "4")]);
        real_mode(&mut b);
        wr(&mut b, SIE, 0b0110);
        b.set_irq_input(1, true, 0);
        assert_eq!(b.read32(ISR), 0b0010);
        assert_eq!(b.read32(IPR), 0b0010);
        assert_eq!(b.read32(IVR), 1);
        assert!(b.irq_level());
        // The input falling does not clear the captured edge.
        b.set_irq_input(1, false, 10);
        assert_eq!(b.read32(ISR), 0b0010);
        assert!(b.irq_level());
        // Acknowledge.
        wr(&mut b, IAR, 0b0010);
        assert_eq!(b.read32(ISR), 0);
        assert!(!b.irq_level());
        assert_eq!(b.read32(IVR), 0xFFFF_FFFF);
        // A second edge is captured again; priority is the lower number.
        b.set_irq_input(2, true, 20);
        b.set_irq_input(1, true, 20);
        assert_eq!(b.read32(IVR), 1);
        wr(&mut b, IAR, 0b0010);
        assert_eq!(b.read32(IVR), 2);
    }

    #[test]
    fn disabled_inputs_are_pending_in_isr_but_not_in_ipr() {
        let (mut b, _) = intc(&[]);
        real_mode(&mut b);
        b.set_irq_input(0, true, 0);
        assert_eq!(b.read32(ISR), 1);
        assert_eq!(b.read32(IPR), 0);
        assert!(!b.irq_level());
        wr(&mut b, IER, 1);
        assert!(b.irq_level());
        wr(&mut b, CIE, 1);
        assert!(!b.irq_level());
        assert_eq!(b.read32(IER), 0);
    }

    #[test]
    fn master_enable_gates_the_output() {
        let (mut b, _) = intc(&[]);
        wr(&mut b, MER, MER_HIE);
        wr(&mut b, IER, 3);
        b.set_irq_input(0, true, 0);
        assert_eq!(b.read32(IPR), 1);
        assert!(!b.irq_level(), "ME is clear");
        wr(&mut b, MER, MER_ME | MER_HIE);
        assert!(b.irq_level());
    }

    #[test]
    fn level_inputs_follow_the_line() {
        // Input 0 level high, input 1 level low (active low), input 2 falling edge.
        let (mut b, _) = intc(&[
            ("C_NUM_INTR_INPUTS", "3"),
            ("C_KIND_OF_INTR", "0x4"),
            ("C_KIND_OF_LVL", "0x1"),
            ("C_KIND_OF_EDGE", "0x0"),
        ]);
        real_mode(&mut b);
        wr(&mut b, IER, 7);
        // Input 1 is active low and idles low in the model: it is asserted from the start.
        assert_eq!(b.read32(ISR), 0b010);
        b.set_irq_input(1, true, 0);
        assert_eq!(b.read32(ISR), 0);
        b.set_irq_input(0, true, 0);
        assert_eq!(b.read32(ISR), 0b001);
        // IAR cannot clear an asserted level input.
        wr(&mut b, IAR, 1);
        assert_eq!(b.read32(ISR), 0b001);
        b.set_irq_input(0, false, 5);
        assert_eq!(b.read32(ISR), 0);
        // Falling-edge input.
        b.set_irq_input(2, true, 6);
        assert_eq!(b.read32(ISR), 0);
        b.set_irq_input(2, false, 7);
        assert_eq!(b.read32(ISR), 0b100);
    }

    #[test]
    fn software_interrupts_until_hie_is_set() {
        let (mut b, _) = intc(&[]);
        wr(&mut b, MER, MER_ME);
        wr(&mut b, IER, 1);
        wr(&mut b, ISR, 1);
        assert_eq!(b.read32(ISR), 1);
        assert!(b.irq_level());
        // Hardware inputs are not recognised yet.
        b.set_irq_input(1, true, 0);
        assert_eq!(b.read32(ISR), 1);
        wr(&mut b, IAR, 1);
        assert!(!b.irq_level());
        // HIE is write-once; ISR writes stop working.
        wr(&mut b, MER, MER_ME | MER_HIE);
        wr(&mut b, MER, MER_ME);
        assert_eq!(b.read32(MER), MER_ME | MER_HIE);
        wr(&mut b, ISR, 1);
        assert_eq!(b.read32(ISR), 0);
        // Reset clears it.
        b.reset();
        assert_eq!(b.read32(MER), 0);
    }

    #[test]
    fn fast_interrupt_registers_exist_only_with_c_has_fast() {
        let (mut b, _) = intc(&[]);
        wr(&mut b, IVAR, 0x1234);
        wr(&mut b, ILR, 3);
        wr(&mut b, IMR, 3);
        assert_eq!((b.read32(IVAR), b.read32(ILR), b.read32(IMR)), (0, 0, 0));
    }

    #[test]
    fn inputs_beyond_the_width_and_vector_registers() {
        let (mut b, h) = intc(&[("C_NUM_INTR_INPUTS", "2"), ("C_HAS_FAST", "1")]);
        real_mode(&mut b);
        assert_eq!(b.read32(IVAR), 0x10, "C_IVAR_RESET_VALUE");
        wr(&mut b, IER, 0xFFFF_FFFF);
        assert_eq!(b.read32(IER), 3);
        h.set_input(5, true);
        assert_eq!(b.read32(ISR), 0);
        wr(&mut b, IVAR + 4, 0x1234_5678);
        assert_eq!(b.read32(IVAR + 4), 0x1234_5678);
        assert_eq!(b.read32(IVAR + 8), 0, "no vector register for input 2");
        h.set_input(0, true);
        assert!(h.irq());
        assert_eq!(h.registers().0, 1);
        assert!(
            IntcConfig::from_params(&[("C_HAS_CASC".to_string(), "1".to_string())].into(), 4096)
                .is_err()
        );
    }
}
