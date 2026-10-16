//! AXI GPIO (`xilinx.com:ip:axi_gpio:2.0`, Xilinx PG144), register level.
//!
//! Register map (offsets from the base address; recalled from PG144, **not** checked against
//! hardware or Xilinx's simulation model):
//!
//! | offset | register | access |
//! |---|---|---|
//! | `0x000` | `GPIO_DATA`, channel 1 data | R/W |
//! | `0x004` | `GPIO_TRI`, channel 1 direction (1 = input) | R/W |
//! | `0x008` | `GPIO2_DATA`, channel 2 data (dual channel only) | R/W |
//! | `0x00C` | `GPIO2_TRI` | R/W |
//! | `0x11C` | `GIER`, bit 31 global interrupt enable | R/W |
//! | `0x120` | `IP_ISR`, bit 0 channel 1, bit 1 channel 2, toggle on write | R/TOW |
//! | `0x128` | `IP_IER`, bit 0 channel 1, bit 1 channel 2 | R/W |
//!
//! Behaviour modelled: reset values from `C_DOUT_DEFAULT*`/`C_TRI_DEFAULT*`; widths
//! (`C_GPIO_WIDTH`, `C_GPIO2_WIDTH`); `C_ALL_INPUTS*`/`C_ALL_OUTPUTS*` (the direction register
//! is then fixed); the interrupt status bit of a channel is set when its input pins change;
//! the interrupt output is `GIER[31] & (IP_ISR & IP_IER) != 0`, a level.
//!
//! `VERIFY:` items (each one is a guess that a Xilinx simulation or a board would settle):
//! * IP_ISR is toggle-on-write (PG144 says so as far as recalled); a write of 1 flips the bit.
//! * An input change sets the status bit whatever the direction bit says (the pad value is
//!   compared, not the masked one).
//! * Reading `GPIO_DATA` of a bidirectional channel returns the pin value for input bits and
//!   the output register for output bits (the pad is assumed to follow the driver).
//! * With `C_ALL_OUTPUTS`/`C_ALL_INPUTS` the TRI register reads its fixed value (0 / all ones)
//!   and ignores writes.
//! * Offsets other than the table's read as 0 and ignore writes (no SLVERR).
//! * Bits above the channel width read as 0.

use crate::params::{Params, flag, number, ranged};
use pl_native::RegisterBlock;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

/// `GPIO_DATA`.
pub const GPIO_DATA: u64 = 0x000;
/// `GPIO_TRI`.
pub const GPIO_TRI: u64 = 0x004;
/// `GPIO2_DATA`.
pub const GPIO2_DATA: u64 = 0x008;
/// `GPIO2_TRI`.
pub const GPIO2_TRI: u64 = 0x00C;
/// Global interrupt enable register.
pub const GIER: u64 = 0x11C;
/// Interrupt status register.
pub const IP_ISR: u64 = 0x120;
/// Interrupt enable register.
pub const IP_IER: u64 = 0x128;

/// The IP's configuration (`C_*` parameters).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GpioConfig {
    /// `C_GPIO_WIDTH`, `C_GPIO2_WIDTH`.
    pub width: [u32; 2],
    /// `C_IS_DUAL`.
    pub dual: bool,
    /// `C_ALL_INPUTS`, `C_ALL_INPUTS_2`.
    pub all_inputs: [bool; 2],
    /// `C_ALL_OUTPUTS`, `C_ALL_OUTPUTS_2`.
    pub all_outputs: [bool; 2],
    /// `C_DOUT_DEFAULT`, `C_DOUT_DEFAULT_2`.
    pub dout_default: [u32; 2],
    /// `C_TRI_DEFAULT`, `C_TRI_DEFAULT_2`.
    pub tri_default: [u32; 2],
    /// `C_INTERRUPT_PRESENT`.
    pub interrupt: bool,
    /// Size of the address window in bytes.
    pub window: u64,
}

impl Default for GpioConfig {
    /// The IP's defaults: one 32-bit bidirectional channel, no interrupt.
    fn default() -> Self {
        GpioConfig {
            width: [32, 32],
            dual: false,
            all_inputs: [false; 2],
            all_outputs: [false; 2],
            dout_default: [0; 2],
            tri_default: [u32::MAX; 2],
            interrupt: false,
            window: 0x1_0000,
        }
    }
}

impl GpioConfig {
    /// Reads the configuration from IP parameters (Vivado names). `window` is the size of the
    /// address window in bytes.
    pub fn from_params(p: &Params, window: u64) -> Result<GpioConfig, String> {
        let mut c = GpioConfig {
            window,
            ..GpioConfig::default()
        };
        c.dual = flag(p, "C_IS_DUAL", false)?;
        c.interrupt = flag(p, "C_INTERRUPT_PRESENT", false)?;
        for (i, (w, i_key, o_key, d_key, t_key)) in [
            (
                "C_GPIO_WIDTH",
                "C_ALL_INPUTS",
                "C_ALL_OUTPUTS",
                "C_DOUT_DEFAULT",
                "C_TRI_DEFAULT",
            ),
            (
                "C_GPIO2_WIDTH",
                "C_ALL_INPUTS_2",
                "C_ALL_OUTPUTS_2",
                "C_DOUT_DEFAULT_2",
                "C_TRI_DEFAULT_2",
            ),
        ]
        .into_iter()
        .enumerate()
        {
            c.width[i] = ranged(p, w, 32, 1, 32)? as u32;
            c.all_inputs[i] = flag(p, i_key, false)?;
            c.all_outputs[i] = flag(p, o_key, false)?;
            if c.all_inputs[i] && c.all_outputs[i] {
                return Err(format!("{i_key} and {o_key} are both set"));
            }
            c.dout_default[i] = number(p, d_key, 0)? as u32;
            c.tri_default[i] = number(p, t_key, u64::from(u32::MAX))? as u32;
        }
        Ok(c)
    }

    /// Number of channels (1 or 2).
    pub fn channels(&self) -> usize {
        if self.dual { 2 } else { 1 }
    }

    fn mask(&self, ch: usize) -> u32 {
        if self.width[ch] >= 32 {
            u32::MAX
        } else {
            (1u32 << self.width[ch]) - 1
        }
    }
}

#[derive(Debug)]
struct State {
    cfg: GpioConfig,
    data: [u32; 2],
    tri: [u32; 2],
    pin_in: [u32; 2],
    gier: bool,
    isr: u32,
    ier: u32,
}

impl State {
    fn power_on(cfg: GpioConfig) -> State {
        let mut s = State {
            cfg,
            data: [0; 2],
            tri: [0; 2],
            pin_in: [0; 2],
            gier: false,
            isr: 0,
            ier: 0,
        };
        s.reset();
        s
    }

    fn reset(&mut self) {
        for ch in 0..2 {
            let m = self.cfg.mask(ch);
            self.data[ch] = self.cfg.dout_default[ch] & m;
            self.tri[ch] = if self.cfg.all_inputs[ch] {
                m
            } else if self.cfg.all_outputs[ch] {
                0
            } else {
                self.cfg.tri_default[ch] & m
            };
        }
        self.gier = false;
        self.isr = 0;
        self.ier = 0;
    }

    fn data_read(&self, ch: usize) -> u32 {
        let m = self.cfg.mask(ch);
        if self.cfg.all_outputs[ch] {
            self.data[ch] & m
        } else if self.cfg.all_inputs[ch] {
            self.pin_in[ch] & m
        } else {
            ((self.pin_in[ch] & self.tri[ch]) | (self.data[ch] & !self.tri[ch])) & m
        }
    }

    fn irq(&self) -> bool {
        let chans = (1u32 << self.cfg.channels()) - 1;
        self.cfg.interrupt && self.gier && (self.isr & self.ier & chans) != 0
    }
}

fn merge(old: u32, new: u32, strobe: u8) -> u32 {
    let mut v = old;
    for lane in 0..4 {
        if strobe & (1 << lane) != 0 {
            let m = 0xFFu32 << (8 * lane);
            v = (v & !m) | (new & m);
        }
    }
    v
}

/// An AXI GPIO instance as a [`RegisterBlock`].
pub struct AxiGpio {
    name: String,
    st: Arc<Mutex<State>>,
}

/// A clonable handle to the pins and interrupt state of an [`AxiGpio`]: how the board side
/// (and tests) drive its inputs and read its outputs.
#[derive(Clone)]
pub struct GpioHandle {
    st: Arc<Mutex<State>>,
}

impl AxiGpio {
    /// A freshly reset instance named `name` and the handle to its pins.
    pub fn new(name: &str, cfg: GpioConfig) -> (AxiGpio, GpioHandle) {
        let st = Arc::new(Mutex::new(State::power_on(cfg)));
        (
            AxiGpio {
                name: name.to_string(),
                st: Arc::clone(&st),
            },
            GpioHandle { st },
        )
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.st.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl GpioHandle {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.st.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The instance's configuration.
    pub fn config(&self) -> GpioConfig {
        self.lock().cfg.clone()
    }

    /// What the instance drives on channel `ch`'s output pins (`gpio_io_o` for the bits whose
    /// direction bit is 0, 0 for bits that are inputs).
    pub fn pins_out(&self, ch: usize) -> u32 {
        let s = self.lock();
        if ch >= 2 {
            return 0;
        }
        s.data[ch] & !s.tri[ch] & s.cfg.mask(ch)
    }

    /// The direction register of channel `ch` (1 = input).
    pub fn tri(&self, ch: usize) -> u32 {
        self.lock().tri.get(ch).copied().unwrap_or(0)
    }

    /// Sets the level the pads of channel `ch` present to the IP (`gpio_io_i`). A change sets
    /// the channel's interrupt status bit when the IP has the interrupt and the channel has
    /// inputs.
    pub fn set_input(&self, ch: usize, value: u32) {
        let mut s = self.lock();
        if ch >= s.cfg.channels() {
            return;
        }
        let v = value & s.cfg.mask(ch);
        if v != s.pin_in[ch] {
            s.pin_in[ch] = v;
            if s.cfg.interrupt && !s.cfg.all_outputs[ch] {
                s.isr |= 1 << ch;
            }
        }
    }

    /// The input level last set with [`GpioHandle::set_input`].
    pub fn input(&self, ch: usize) -> u32 {
        self.lock().pin_in.get(ch).copied().unwrap_or(0)
    }

    /// Interrupt output level.
    pub fn irq(&self) -> bool {
        self.lock().irq()
    }
}

impl RegisterBlock for AxiGpio {
    fn name(&self) -> &str {
        &self.name
    }

    fn size(&self) -> u64 {
        self.lock().cfg.window
    }

    fn read32(&mut self, offset: u64) -> u32 {
        let s = self.lock();
        let dual = s.cfg.dual;
        match offset {
            GPIO_DATA => s.data_read(0),
            GPIO_TRI => s.tri[0],
            GPIO2_DATA if dual => s.data_read(1),
            GPIO2_TRI if dual => s.tri[1],
            GIER if s.cfg.interrupt => u32::from(s.gier) << 31,
            IP_ISR if s.cfg.interrupt => s.isr,
            IP_IER if s.cfg.interrupt => s.ier,
            _ => 0,
        }
    }

    fn write32(&mut self, offset: u64, value: u32, strobe: u8) {
        let mut s = self.lock();
        let dual = s.cfg.dual;
        let chans = (1u32 << s.cfg.channels()) - 1;
        let tri_fixed = |s: &State, ch: usize| s.cfg.all_inputs[ch] || s.cfg.all_outputs[ch];
        match offset {
            GPIO_DATA => {
                let m = s.cfg.mask(0);
                s.data[0] = merge(s.data[0], value, strobe) & m;
            }
            GPIO_TRI if !tri_fixed(&s, 0) => {
                let m = s.cfg.mask(0);
                s.tri[0] = merge(s.tri[0], value, strobe) & m;
            }
            GPIO2_DATA if dual => {
                let m = s.cfg.mask(1);
                s.data[1] = merge(s.data[1], value, strobe) & m;
            }
            GPIO2_TRI if dual && !tri_fixed(&s, 1) => {
                let m = s.cfg.mask(1);
                s.tri[1] = merge(s.tri[1], value, strobe) & m;
            }
            GIER if s.cfg.interrupt && strobe & 0x8 != 0 => s.gier = value >> 31 != 0,
            IP_ISR if s.cfg.interrupt && strobe & 0x1 != 0 => {
                // Toggle on write.
                s.isr ^= value & chans;
            }
            IP_IER if s.cfg.interrupt && strobe & 0x1 != 0 => {
                s.ier = value & chans;
            }
            _ => {}
        }
    }

    fn reset(&mut self) {
        self.lock().reset();
    }

    fn irq_level(&self) -> bool {
        self.lock().irq()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gpio(p: &[(&str, &str)]) -> (AxiGpio, GpioHandle) {
        let params: Params = p
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        AxiGpio::new(
            "g",
            GpioConfig::from_params(&params, 0x1_0000).expect("config"),
        )
    }

    #[test]
    fn reset_values_follow_the_defaults() {
        let (mut g, _) = gpio(&[
            ("C_GPIO_WIDTH", "8"),
            ("C_DOUT_DEFAULT", "0xA5"),
            ("C_TRI_DEFAULT", "0x0F"),
        ]);
        assert_eq!(g.read32(GPIO_TRI), 0x0F);
        // Bits 0..3 are inputs (pins read 0), bits 4..7 outputs holding 0xA from the default.
        assert_eq!(g.read32(GPIO_DATA), 0xA0);
        assert_eq!(g.read32(GIER), 0);
        assert_eq!(g.read32(IP_ISR), 0);
        // Default IP: 32-bit, all inputs by direction register.
        let (mut d, _) = gpio(&[]);
        assert_eq!(d.read32(GPIO_TRI), 0xFFFF_FFFF);
        assert_eq!(d.read32(GPIO2_TRI), 0, "no second channel");
    }

    #[test]
    fn all_outputs_channel_holds_the_written_value() {
        let (mut g, h) = gpio(&[
            ("C_GPIO_WIDTH", "4"),
            ("C_ALL_OUTPUTS", "1"),
            ("C_INTERRUPT_PRESENT", "1"),
        ]);
        assert_eq!(g.read32(GPIO_TRI), 0);
        g.write32(GPIO_DATA, 0xFFFF_FFF5, 0xF);
        assert_eq!(g.read32(GPIO_DATA), 0x5, "masked to the width");
        assert_eq!(h.pins_out(0), 0x5);
        g.write32(GPIO_TRI, 0xFF, 0xF);
        assert_eq!(g.read32(GPIO_TRI), 0, "fixed direction");
        // Byte strobes.
        g.write32(GPIO_DATA, 0x0, 0b0010);
        assert_eq!(g.read32(GPIO_DATA), 0x5);
        // No input pins: input changes never raise the interrupt.
        h.set_input(0, 0xF);
        assert_eq!(g.read32(IP_ISR), 0);
    }

    #[test]
    fn all_inputs_follow_the_pins_and_interrupt_on_change() {
        let (mut g, h) = gpio(&[
            ("C_GPIO_WIDTH", "4"),
            ("C_ALL_INPUTS", "1"),
            ("C_INTERRUPT_PRESENT", "1"),
        ]);
        assert_eq!(g.read32(GPIO_TRI), 0xF);
        h.set_input(0, 0x6);
        assert_eq!(g.read32(GPIO_DATA), 0x6);
        g.write32(GPIO_DATA, 0xF, 0xF); // writes land in the register, pins still rule
        assert_eq!(g.read32(GPIO_DATA), 0x6);
        // The status bit is set but the output is gated by IER and GIER.
        assert_eq!(g.read32(IP_ISR), 1);
        assert!(!g.irq_level());
        g.write32(IP_IER, 1, 0xF);
        assert!(!g.irq_level(), "GIER still off");
        g.write32(GIER, 0x8000_0000, 0xF);
        assert_eq!(g.read32(GIER), 0x8000_0000);
        assert!(g.irq_level());
        // Toggle on write clears it; a repeated write sets it again.
        g.write32(IP_ISR, 1, 0xF);
        assert_eq!(g.read32(IP_ISR), 0);
        assert!(!g.irq_level());
        g.write32(IP_ISR, 1, 0xF);
        assert_eq!(g.read32(IP_ISR), 1);
        g.write32(IP_ISR, 1, 0xF);
        // An input that does not change does not set the bit.
        h.set_input(0, 0x6);
        assert_eq!(g.read32(IP_ISR), 0);
        h.set_input(0, 0x7);
        assert_eq!(g.read32(IP_ISR), 1);
        assert!(g.irq_level());
    }

    #[test]
    fn dual_channel_and_bidirectional_read_back() {
        let (mut g, h) = gpio(&[
            ("C_IS_DUAL", "1"),
            ("C_GPIO_WIDTH", "8"),
            ("C_GPIO2_WIDTH", "3"),
            ("C_ALL_OUTPUTS_2", "1"),
            ("C_INTERRUPT_PRESENT", "1"),
        ]);
        // Channel 1 bidirectional: low nibble outputs, high nibble inputs.
        g.write32(GPIO_TRI, 0xF0, 0xF);
        g.write32(GPIO_DATA, 0xAB, 0xF);
        h.set_input(0, 0xC0);
        // High nibble reads the pins, low nibble reads the output register.
        assert_eq!(g.read32(GPIO_DATA), 0xCB);
        assert_eq!(h.pins_out(0), 0x0B);
        // Channel 2: three output bits.
        g.write32(GPIO2_DATA, 0xFF, 0xF);
        assert_eq!(g.read32(GPIO2_DATA), 0x7);
        assert_eq!(h.pins_out(1), 0x7);
        // Channel 2 has its own status bit (bit 1) and ier bit.
        g.write32(IP_IER, 0xFFFF_FFFF, 0xF);
        assert_eq!(g.read32(IP_IER), 0x3, "two channels");
        // The change of channel 1's pads above set its status bit; clear it (toggle).
        assert_eq!(g.read32(IP_ISR), 0b01);
        g.write32(IP_ISR, 0b01, 0xF);
        h.set_input(1, 0x5);
        assert_eq!(g.read32(IP_ISR), 0, "all-output channel 2: no status bit");
        h.set_input(0, 0x40);
        assert_eq!(g.read32(IP_ISR), 0b01);
    }

    #[test]
    fn reset_restores_defaults_and_keeps_the_pads() {
        let (mut g, h) = gpio(&[
            ("C_GPIO_WIDTH", "4"),
            ("C_ALL_INPUTS", "1"),
            ("C_INTERRUPT_PRESENT", "1"),
        ]);
        h.set_input(0, 0x9);
        g.write32(IP_IER, 1, 0xF);
        g.write32(GIER, 0x8000_0000, 0xF);
        g.reset();
        assert_eq!(g.read32(IP_ISR), 0);
        assert_eq!(g.read32(IP_IER), 0);
        assert!(!g.irq_level());
        assert_eq!(g.read32(GPIO_DATA), 0x9, "the pads are not part of the IP");
    }

    #[test]
    fn bad_parameters_are_rejected() {
        let mut p = Params::new();
        p.insert("C_GPIO_WIDTH".into(), "33".into());
        assert!(GpioConfig::from_params(&p, 0x1000).is_err());
        let mut p = Params::new();
        p.insert("C_ALL_INPUTS".into(), "1".into());
        p.insert("C_ALL_OUTPUTS".into(), "1".into());
        assert!(
            GpioConfig::from_params(&p, 0x1000)
                .unwrap_err()
                .contains("both")
        );
    }
}
