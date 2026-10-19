//! Clocking Wizard (`xilinx.com:ip:clk_wiz:6.0`, Xilinx PG065), register level, AXI4-Lite
//! interface with dynamic reconfiguration (ADR-042).
//!
//! What this model does: it reports `LOCKED` after a power-on or software reset and after a
//! dynamic reconfiguration, after a fixed number of ACLK cycles; it stores the clocking
//! configuration registers; it raises an interrupt on lock-status changes through `ISR`/`IER`.
//!
//! What it does **not** do: it does not compute or change any simulated clock frequency. The
//! clocks a design gets come from the handoff, as before, and the configuration registers do
//! not change them. The lock time is a model constant, not a PG065 figure.
//!
//! Register map (offsets from the base address):
//!
//! | offset | register | description | access | reset |
//! |---|---|---|---|---|
//! | `0x000` | `SRR` | Software Reset Register (write key `0xA`) | W | `0x0000_0000` |
//! | `0x004` | `SR` | Status Register: bit 0 `LOCKED` | R | `0x0000_0000` (locks after the lock time) |
//! | `0x00C` | `ISR` | Interrupt Status Register, write 1 to clear | R/W1C | `0x0000_0000` |
//! | `0x010` | `IER` | Interrupt Enable Register | R/W | `0x0000_0000` |
//! | `0x200` | `CCR0` | Clocking configuration 0 (DIVCLK, CLKFBOUT multiply) | R/W | `0x0000_0000` |
//! | `0x204` | `CCR1` | Clocking configuration 1 (CLKFBOUT fractional part) | R/W | `0x0000_0000` |
//! | `0x208` | `CCR2` | Clocking configuration 2 (CLKOUT0 divide, phase) | R/W | `0x0000_0000` |
//! | `0x25C` | `SADDR` | Dynamic reconfiguration load: writing bit 0 = 1 starts a reconfiguration | R/W | `0x0000_0000` |
//!
//! `SR`:
//! - bit 0 `LOCKED`: 1 when the clocks are locked. It falls when a reconfiguration starts and
//!   rises again after the lock time.
//!
//! `ISR` / `IER`:
//! - bit 0 `LOCK_CHANGED`: set on every `LOCKED` transition (falling and rising).
//! - bit 1 `CLK_GLITCH`: set only by [`ClkWizHandle::inject_clock_glitch`] (host injection).
//!   The model never raises it by itself.
//! - `irq = (ISR & IER) != 0`.
//!
//! `VERIFY:` items (not checked against PG065 or a Vivado simulation):
//! * The `SRR` key `0xA` and the `SR` `LOCKED` bit position follow the Xilinx convention for
//!   AXI clocking cores; confirm them in PG065.
//! * The `ISR`/`IER` bit assignment (lock change, clock glitch) and whether the interrupt output
//!   is level or edge.
//! * `CCR0`/`CCR1`/`CCR2` field layout and reset values. The model stores the words as written
//!   and resets them to zero; the real reset values depend on the configured output clocks.
//! * The reconfiguration trigger: the model treats bit 0 of a write to `SADDR` as the load
//!   command. The real core may use a different encoding of the load/save command. Whether the
//!   read-back of `SADDR` is the last written value is also unverified.
//! * The lock time: [`LOCK_CYCLES`] ACLK cycles after reset or reconfiguration. The real MMCM/PLL
//!   lock time depends on the configuration and is not a fixed cycle count.
//! * A write to `SADDR` while a reconfiguration is in progress restarts the lock timer.
//! * Unmapped offsets return 0 and ignore writes.

use crate::params::{Params, merge, ranged};
use pl_native::RegisterBlock;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

/// `SRR`: Software Reset Register.
pub const SRR: u64 = 0x000;
/// `SR`: Status Register.
pub const SR: u64 = 0x004;
/// `ISR`: Interrupt Status Register.
pub const ISR: u64 = 0x00C;
/// `IER`: Interrupt Enable Register.
pub const IER: u64 = 0x010;
/// `CCR0`: clocking configuration register 0.
pub const CCR0: u64 = 0x200;
/// `CCR1`: clocking configuration register 1.
pub const CCR1: u64 = 0x204;
/// `CCR2`: clocking configuration register 2.
pub const CCR2: u64 = 0x208;
/// `SADDR`: dynamic reconfiguration load register.
pub const SADDR: u64 = 0x25C;

/// SRR software reset key.
pub const SRR_RESET_KEY: u32 = 0x0000_000A;
/// SR bit 0: `LOCKED`.
pub const SR_LOCKED: u32 = 1 << 0;
/// ISR/IER bit 0: lock status changed.
pub const INTR_LOCK_CHANGED: u32 = 1 << 0;
/// ISR/IER bit 1: clock glitch (host injection only).
pub const INTR_CLK_GLITCH: u32 = 1 << 1;
/// Mask of the implemented ISR/IER bits.
pub const INTR_MASK: u32 = INTR_LOCK_CHANGED | INTR_CLK_GLITCH;
/// SADDR bit 0: dynamic reconfiguration load command.
pub const SADDR_LOAD: u32 = 1 << 0;
/// ACLK cycles from reset or reconfiguration to `LOCKED` (model constant, see `VERIFY:`).
pub const LOCK_CYCLES: u64 = 1_000;

/// IP configuration parameters.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClkWizConfig {
    /// ACLK frequency in Hz (default 100 MHz); sets the lock time in nanoseconds.
    pub clk_freq_hz: u64,
    /// Address window size in bytes (default 64 KiB).
    pub window: u64,
}

impl Default for ClkWizConfig {
    fn default() -> Self {
        ClkWizConfig {
            clk_freq_hz: 100_000_000,
            window: 0x1_0000,
        }
    }
}

impl ClkWizConfig {
    /// Builds the configuration from the parameter dictionary (`CLK_FREQ_HZ` is filled in by the
    /// XSA importer from the ACLK of the instance).
    pub fn from_params(p: &Params, window: u64) -> Result<ClkWizConfig, String> {
        let clk_freq_hz = ranged(p, "CLK_FREQ_HZ", 100_000_000, 1_000, 1_000_000_000)?;
        Ok(ClkWizConfig {
            clk_freq_hz,
            window,
        })
    }
}

struct State {
    cfg: ClkWizConfig,
    /// Latest time seen through `advance` (PS time, ns).
    now_ns: u64,
    locked: bool,
    /// Time at which `LOCKED` rises, while a (re)lock is in progress.
    lock_deadline: Option<u64>,
    isr: u32,
    ier: u32,
    ccr: [u32; 3],
    saddr: u32,
}

impl State {
    fn new(cfg: ClkWizConfig) -> State {
        let mut s = State {
            cfg,
            now_ns: 0,
            locked: false,
            lock_deadline: None,
            isr: 0,
            ier: 0,
            ccr: [0; 3],
            saddr: 0,
        };
        s.reset();
        s
    }

    fn lock_ns(&self) -> u64 {
        LOCK_CYCLES * 1_000_000_000 / self.cfg.clk_freq_hz
    }

    /// Drops `LOCKED` (a reconfiguration or reset starts) and schedules the relock.
    fn unlock(&mut self) {
        if self.locked {
            self.locked = false;
            self.isr |= INTR_LOCK_CHANGED;
        }
        self.lock_deadline = Some(self.now_ns + self.lock_ns());
    }

    fn reset(&mut self) {
        self.unlock();
        self.isr = 0;
        self.ier = 0;
        self.ccr = [0; 3];
        self.saddr = 0;
    }

    /// Raises `LOCKED` once the lock time has passed.
    fn sync(&mut self, now_ns: u64) {
        if now_ns > self.now_ns {
            self.now_ns = now_ns;
        }
        if let Some(deadline) = self.lock_deadline {
            if self.now_ns >= deadline {
                self.lock_deadline = None;
                self.locked = true;
                self.isr |= INTR_LOCK_CHANGED;
            }
        }
    }

    fn irq(&self) -> bool {
        (self.isr & self.ier) != 0
    }

    fn ccr_index(offset: u64) -> Option<usize> {
        match offset {
            CCR0 => Some(0),
            CCR1 => Some(1),
            CCR2 => Some(2),
            _ => None,
        }
    }
}

/// Clocking Wizard model as a [`RegisterBlock`].
pub struct AxiClkWiz {
    name: String,
    st: Arc<Mutex<State>>,
}

/// Clonable handle for host / test inspection.
#[derive(Clone)]
pub struct ClkWizHandle {
    st: Arc<Mutex<State>>,
}

impl AxiClkWiz {
    /// Creates a new Clocking Wizard instance. Its clocks start unlocked at time 0 and lock
    /// after the lock time.
    pub fn new(name: &str, cfg: ClkWizConfig) -> (AxiClkWiz, ClkWizHandle) {
        let st = Arc::new(Mutex::new(State::new(cfg)));
        (
            AxiClkWiz {
                name: name.to_string(),
                st: Arc::clone(&st),
            },
            ClkWizHandle { st },
        )
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.st.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl ClkWizHandle {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.st.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// True while the clocks report `LOCKED`.
    pub fn locked(&self) -> bool {
        self.lock().locked
    }

    /// Sets the `CLK_GLITCH` status bit, as a host-side fault injection (not a hardware event
    /// the model derives).
    pub fn inject_clock_glitch(&self) {
        self.lock().isr |= INTR_CLK_GLITCH;
    }
}

impl RegisterBlock for AxiClkWiz {
    fn name(&self) -> &str {
        &self.name
    }

    fn size(&self) -> u64 {
        self.lock().cfg.window
    }

    fn read32(&mut self, offset: u64) -> u32 {
        let s = self.lock();
        match offset {
            SRR => 0,
            SR => {
                if s.locked {
                    SR_LOCKED
                } else {
                    0
                }
            }
            ISR => s.isr,
            IER => s.ier,
            SADDR => s.saddr,
            _ => match State::ccr_index(offset) {
                Some(i) => s.ccr[i],
                None => 0,
            },
        }
    }

    fn write32(&mut self, offset: u64, value: u32, strobe: u8) {
        let mut s = self.lock();
        match offset {
            SRR => {
                if merge(0, value, strobe) == SRR_RESET_KEY {
                    s.reset();
                }
            }
            // Status register: read-only.
            SR => {}
            ISR => {
                // Write 1 to clear.
                s.isr &= !(merge(0, value, strobe) & INTR_MASK);
            }
            IER => {
                let cur = s.ier;
                s.ier = merge(cur, value, strobe) & INTR_MASK;
            }
            SADDR => {
                let cur = s.saddr;
                s.saddr = merge(cur, value, strobe);
                if strobe & 1 != 0 && value & SADDR_LOAD != 0 {
                    s.unlock();
                }
            }
            _ => {
                if let Some(i) = State::ccr_index(offset) {
                    let cur = s.ccr[i];
                    s.ccr[i] = merge(cur, value, strobe);
                }
            }
        }
    }

    fn reset(&mut self) {
        self.lock().reset();
    }

    fn advance(&mut self, now_ns: u64) {
        self.lock().sync(now_ns);
    }

    fn irq_level(&self) -> bool {
        self.lock().irq()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Lock time at the default 100 MHz ACLK: 1000 cycles = 10 us.
    const LOCK_NS: u64 = 10_000;

    fn locked_model() -> (AxiClkWiz, ClkWizHandle) {
        let (mut w, h) = AxiClkWiz::new("clk0", ClkWizConfig::default());
        w.advance(LOCK_NS);
        assert!(h.locked());
        (w, h)
    }

    #[test]
    fn reset_state_and_lock_time() {
        let (mut w, h) = AxiClkWiz::new("clk0", ClkWizConfig::default());
        assert_eq!(w.read32(SR), 0, "unlocked at power-on");
        assert_eq!(w.read32(ISR), 0);
        assert_eq!(w.read32(IER), 0);
        assert_eq!(w.read32(CCR0), 0);
        assert_eq!(w.read32(SADDR), 0);
        assert!(!w.irq_level());
        w.advance(LOCK_NS - 1);
        assert!(!h.locked(), "one ns before the lock time");
        w.advance(LOCK_NS);
        assert_eq!(w.read32(SR), SR_LOCKED);
        assert_eq!(w.read32(ISR), INTR_LOCK_CHANGED);
    }

    #[test]
    fn srr_key_resets_and_other_values_do_nothing() {
        let (mut w, h) = locked_model();
        w.write32(CCR0, 0x0A01, 0xF);
        w.write32(IER, INTR_LOCK_CHANGED, 0xF);
        w.write32(SRR, 0x1234, 0xF);
        assert!(h.locked());
        assert_eq!(w.read32(CCR0), 0x0A01);
        assert_eq!(w.read32(IER), INTR_LOCK_CHANGED);
        assert_eq!(w.read32(SRR), 0, "SRR reads as zero");

        w.write32(SRR, SRR_RESET_KEY, 0xF);
        assert_eq!(w.read32(CCR0), 0);
        assert_eq!(w.read32(IER), 0);
        assert_eq!(w.read32(ISR), 0);
        assert!(!h.locked(), "reset drops LOCKED until the lock time");
        assert_eq!(w.read32(SR), 0);
        w.advance(LOCK_NS * 2);
        assert_eq!(w.read32(SR), SR_LOCKED);
    }

    #[test]
    fn status_register_is_read_only() {
        let (mut w, _) = locked_model();
        w.write32(SR, 0, 0xF);
        assert_eq!(w.read32(SR), SR_LOCKED);
    }

    #[test]
    fn isr_is_write_one_to_clear() {
        let (mut w, _) = locked_model();
        assert_eq!(w.read32(ISR), INTR_LOCK_CHANGED);
        // Writing 0 or the other bit leaves the lock bit set.
        w.write32(ISR, 0, 0xF);
        w.write32(ISR, INTR_CLK_GLITCH, 0xF);
        assert_eq!(w.read32(ISR), INTR_LOCK_CHANGED);
        w.write32(ISR, INTR_LOCK_CHANGED, 0xF);
        assert_eq!(w.read32(ISR), 0);
    }

    #[test]
    fn clock_glitch_is_injected_by_the_host_and_cleared_by_w1c() {
        let (mut w, h) = locked_model();
        w.write32(ISR, INTR_LOCK_CHANGED, 0xF);
        h.inject_clock_glitch();
        assert_eq!(w.read32(ISR), INTR_CLK_GLITCH);
        w.write32(ISR, INTR_CLK_GLITCH, 0xF);
        assert_eq!(w.read32(ISR), 0);
    }

    #[test]
    fn ier_gates_the_interrupt_output() {
        let (mut w, _) = locked_model();
        assert!(!w.irq_level(), "pending but not enabled");
        assert_eq!(w.read32(IER), 0);
        w.write32(IER, 0xFFFF_FFFF, 0xF);
        assert_eq!(w.read32(IER), INTR_MASK, "only the implemented bits");
        assert!(w.irq_level());
        w.write32(ISR, INTR_LOCK_CHANGED, 0xF);
        assert!(!w.irq_level());
    }

    #[test]
    fn ccr_registers_store_the_words_as_written() {
        let (mut w, _) = AxiClkWiz::new("clk0", ClkWizConfig::default());
        w.write32(CCR0, 0x0A01, 0xF);
        w.write32(CCR1, 0x0000_0100, 0xF);
        w.write32(CCR2, 0x0000_0A00, 0xF);
        assert_eq!(w.read32(CCR0), 0x0A01);
        assert_eq!(w.read32(CCR1), 0x0000_0100);
        assert_eq!(w.read32(CCR2), 0x0000_0A00);
        // Byte-lane strobes only change the selected lane.
        w.write32(CCR0, 0x00FF_0000, 0b0100);
        assert_eq!(w.read32(CCR0), 0x00FF_0A01);
        // Unmapped offsets read zero and ignore writes.
        w.write32(0x210, 0xFFFF_FFFF, 0xF);
        assert_eq!(w.read32(0x210), 0);
    }

    #[test]
    fn saddr_load_starts_a_reconfiguration_that_relocks() {
        let (mut w, h) = locked_model();
        w.write32(ISR, INTR_LOCK_CHANGED, 0xF);
        w.write32(IER, INTR_LOCK_CHANGED, 0xF);

        // Writing 0 to SADDR does not start a reconfiguration.
        w.write32(SADDR, 0, 0xF);
        assert!(h.locked());

        // Writing 1 drops LOCKED at once; the lock-change bit is set on the falling edge.
        w.advance(LOCK_NS * 3);
        w.write32(SADDR, SADDR_LOAD, 0xF);
        assert!(!h.locked());
        assert_eq!(w.read32(SR), 0);
        assert_eq!(w.read32(ISR), INTR_LOCK_CHANGED);
        assert!(w.irq_level());

        // The relock takes the same deterministic lock time.
        w.advance(LOCK_NS * 3 + LOCK_NS - 1);
        assert!(!h.locked());
        w.advance(LOCK_NS * 3 + LOCK_NS);
        assert!(h.locked());
        assert_eq!(w.read32(SR), SR_LOCKED);
    }

    #[test]
    fn saddr_read_back_is_the_last_written_value() {
        let (mut w, _) = locked_model();
        w.write32(SADDR, 0x0000_0002, 0xF);
        assert_eq!(
            w.read32(SADDR),
            0x0000_0002,
            "bit 0 clear: no reconfiguration"
        );
    }

    #[test]
    fn a_second_reconfiguration_restarts_the_lock_timer() {
        let (mut w, h) = locked_model();
        w.write32(SADDR, SADDR_LOAD, 0xF);
        w.advance(LOCK_NS + 5_000);
        // A new load 5 us into the relock restarts the timer from now.
        w.write32(SADDR, SADDR_LOAD, 0xF);
        w.advance(LOCK_NS + 5_000 + LOCK_NS - 1);
        assert!(!h.locked());
        w.advance(LOCK_NS + 5_000 + LOCK_NS);
        assert!(h.locked());
    }

    #[test]
    fn lock_time_scales_with_the_configured_clock() {
        let cfg = ClkWizConfig {
            clk_freq_hz: 50_000_000,
            ..ClkWizConfig::default()
        };
        let (mut w, h) = AxiClkWiz::new("clk0", cfg);
        w.advance(LOCK_NS);
        assert!(!h.locked(), "50 MHz doubles the lock time");
        w.advance(2 * LOCK_NS);
        assert!(h.locked());
    }
}
