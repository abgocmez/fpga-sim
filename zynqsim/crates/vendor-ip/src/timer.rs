//! AXI Timer/Counter (`xilinx.com:ip:axi_timer:2.0`, Xilinx PG079), register level.
//!
//! Register map (recalled from PG079, **not** checked against hardware):
//!
//! | offset | register |
//! |---|---|
//! | `0x00` | `TCSR0` control/status of timer 0 |
//! | `0x04` | `TLR0` load register |
//! | `0x08` | `TCR0` counter (read only) |
//! | `0x10` | `TCSR1` |
//! | `0x14` | `TLR1` |
//! | `0x18` | `TCR1` |
//!
//! `TCSR` bits: 0 `MDT` (0 generate, 1 capture), 1 `UDT` (1 = count down), 2 `GENT`, 3 `CAPT`,
//! 4 `ARHT` (auto reload), 5 `LOAD`, 6 `ENIT` (interrupt enable), 7 `ENT` (enable), 8 `TINT`
//! (interrupt, write 1 to clear), 9 `PWMA` (PWM), 10 `ENALL` (enable both timers), 11 `CASC`
//! (cascade).
//!
//! Modelled: generate mode of both 32-bit (or `C_COUNT_WIDTH`) timers, counting up or down, load,
//! auto reload, the interrupt status bit and the interrupt output (`TINT & ENIT` of either timer,
//! a level), the one-clock `generateout` pulses as a count and PWM mode as a level. Time comes
//! from PS time and the AXI clock frequency, so the counter is a pure function of the
//! simulated time at which it is read, and the interrupt reaches the PS at the next lock-step
//! sync (see `pl_native::NativePl::advance`).
//!
//! Capture mode (`MDT` = 1): a rising edge on `CAPTURETRIG0` (timer 0) or `CAPTURETRIG1`
//! (timer 1) while `CAPT` and `ENT` are set copies the counter into `TLR` (the capture register),
//! sets `TINT` and counts a capture. The trigger comes from a board switch or button bound with
//! `[connect] "<inst>.CAPTURETRIG0" = "board.btn0"` (or `board.swN`), or programmatically through
//! [`TimerHandle::capture_input`]. Rollovers set no interrupt in capture mode.
//!
//! Cascade mode (`CASC` in `TCSR0`): the two timers form one counter of `2 * C_COUNT_WIDTH` bits,
//! timer 1 is the high word (`TCR1`, `TLR1`), timer 0 the low word. Timer 0's `ENT`, `UDT`,
//! `ARHT`, `MDT`, `GENT`, `LOAD` and `CAPT` control it; the carry from timer 0 is internal. A
//! rollover of the 64-bit counter sets `TINT` of both timers, and a capture on `CAPTURETRIG0`
//! stores the 64-bit count in `TLR1:TLR0` and sets `TINT0`.
//!
//! `VERIFY:` items (capture and cascade are from memory of PG079, not from hardware):
//! * Capture: the trigger is a rising edge; capture needs `ENT` as well as `CAPT`; `TLR` holds
//!   the captured count; `TINT` and `ENIT` report it.
//! * Capture with `ARHT` ("hold"): the counter stops at the captured value until `TINT` is
//!   cleared by a write of 1 (what a driver's "hold" means is not checked).
//! * Cascade: which timer's `TINT` and interrupt a 64-bit rollover uses (both are set here),
//!   whether `CAPTURETRIG1` is ignored in cascade mode, and whether timer 1's own `ENT`, `UDT`
//!   and `ARHT` are ignored (they are, here).
//! * `pwm` and `GENT` pulses in cascade mode count the 64-bit rollovers.
//!
//! `VERIFY:` items:
//! * Period: up `MAX - TLR + 1` clocks, down `TLR + 1` clocks (PG079's period formula as
//!   recalled), measured from the cycle `ENT` is set with `LOAD` clear.
//! * Without `ARHT` the counter rolls over and keeps counting from 0 (up) or all ones (down);
//!   it does not stop ("hold" applies to capture mode).
//! * `LOAD` loads the counter at the write and keeps it loaded while the bit is 1.
//! * `ENALL` written as 1 sets `ENT` of both timers and reads back as written.
//! * With `C_ONE_TIMER_ONLY` the timer 1 registers read 0 and ignore writes.
//! * `TCR` reads reflect the exact clock count; real hardware reads are a few clocks stale.
//! * Offsets other than the table's read as 0 and ignore writes.

use crate::params::{Params, flag, number, ranged};
use pl_native::RegisterBlock;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

/// `TCSR0`.
pub const TCSR0: u64 = 0x00;
/// `TLR0`.
pub const TLR0: u64 = 0x04;
/// `TCR0`.
pub const TCR0: u64 = 0x08;
/// `TCSR1`.
pub const TCSR1: u64 = 0x10;
/// `TLR1`.
pub const TLR1: u64 = 0x14;
/// `TCR1`.
pub const TCR1: u64 = 0x18;

/// `TCSR` bit: capture mode.
pub const MDT: u32 = 1 << 0;
/// `TCSR` bit: count down.
pub const UDT: u32 = 1 << 1;
/// `TCSR` bit: enable the external generate output.
pub const GENT: u32 = 1 << 2;
/// `TCSR` bit: enable the external capture trigger.
pub const CAPT: u32 = 1 << 3;
/// `TCSR` bit: auto reload.
pub const ARHT: u32 = 1 << 4;
/// `TCSR` bit: load the counter from `TLR`.
pub const LOAD: u32 = 1 << 5;
/// `TCSR` bit: interrupt enable.
pub const ENIT: u32 = 1 << 6;
/// `TCSR` bit: enable the timer.
pub const ENT: u32 = 1 << 7;
/// `TCSR` bit: interrupt status (write 1 to clear).
pub const TINT: u32 = 1 << 8;
/// `TCSR` bit: PWM mode (timer 0 only).
pub const PWM: u32 = 1 << 9;
/// `TCSR` bit: enable all timers.
pub const ENALL: u32 = 1 << 10;
/// `TCSR` bit: cascade mode.
pub const CASC: u32 = 1 << 11;
const TCSR_MASK: u32 = 0xFFF;

/// The IP's configuration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TimerConfig {
    /// `C_COUNT_WIDTH`, 8 to 32.
    pub count_width: u32,
    /// `C_ONE_TIMER_ONLY`.
    pub one_timer_only: bool,
    /// AXI clock frequency in Hz (`CLK_FREQ_HZ`, from the block design's clock).
    pub clock_hz: u64,
    /// Size of the address window in bytes.
    pub window: u64,
}

impl Default for TimerConfig {
    fn default() -> Self {
        TimerConfig {
            count_width: 32,
            one_timer_only: false,
            clock_hz: 100_000_000,
            window: 0x1_0000,
        }
    }
}

impl TimerConfig {
    /// Reads the configuration from IP parameters.
    pub fn from_params(p: &Params, window: u64) -> Result<TimerConfig, String> {
        Ok(TimerConfig {
            count_width: ranged(p, "C_COUNT_WIDTH", 32, 8, 32)? as u32,
            one_timer_only: flag(p, "C_ONE_TIMER_ONLY", false)?,
            clock_hz: ranged(p, "CLK_FREQ_HZ", 100_000_000, 1, 1_000_000_000)?,
            window,
        })
    }

    fn max(&self) -> u64 {
        if self.count_width >= 32 {
            u64::from(u32::MAX)
        } else {
            (1u64 << self.count_width) - 1
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct Chan {
    tcsr: u32,
    tlr: u32,
    count: u32,
    rollovers: u64,
    pulses: u64,
    captures: u64,
    /// Cycle number of the last rollover.
    last_roll: Option<u64>,
}

#[derive(Debug)]
struct State {
    cfg: TimerConfig,
    t: [Chan; 2],
    /// Last level seen on `CAPTURETRIG0` and `CAPTURETRIG1` (for the rising edge).
    trig: [bool; 2],
    /// Clock cycle (since time 0) of the last synchronisation.
    cyc: u64,
}

impl State {
    fn cycle_of(&self, now_ns: u64) -> u64 {
        (u128::from(now_ns) * u128::from(self.cfg.clock_hz) / 1_000_000_000) as u64
    }

    fn channels(&self) -> usize {
        if self.cfg.one_timer_only { 1 } else { 2 }
    }

    /// True when the two timers form one 64-bit counter (`CASC` in `TCSR0`).
    fn cascaded(&self) -> bool {
        self.channels() == 2 && self.t[0].tcsr & CASC != 0
    }

    fn word_mask(&self) -> u128 {
        (1u128 << self.cfg.count_width) - 1
    }

    /// The cascaded counter: timer 1 is the high word.
    fn joined(&self) -> u128 {
        let w = self.cfg.count_width;
        ((u128::from(self.t[1].count) << w) | u128::from(self.t[0].count))
            & ((1u128 << (2 * w)) - 1)
    }

    /// The cascaded load value `TLR1:TLR0`.
    fn joined_tlr(&self) -> u128 {
        let w = self.cfg.count_width;
        let m = self.word_mask();
        ((u128::from(self.t[1].tlr) & m) << w) | (u128::from(self.t[0].tlr) & m)
    }

    /// Stores a cascaded count into the two counter registers.
    fn split(&mut self, v: u128) {
        let w = self.cfg.count_width;
        let m = self.word_mask();
        self.t[0].count = (v & m) as u32;
        self.t[1].count = ((v >> w) & m) as u32;
    }

    fn sync(&mut self, now_ns: u64) {
        let c = self.cycle_of(now_ns);
        if c > self.cyc {
            let d = c - self.cyc;
            let base = self.cyc;
            if self.cascaded() {
                self.step_cascade(d, base);
            } else {
                for i in 0..self.channels() {
                    self.step(i, d, base);
                }
            }
            self.cyc = c;
        }
    }

    /// Advances timer `i` by `d` clocks that start at cycle `base`.
    fn step(&mut self, i: usize, d: u64, base: u64) {
        let max = self.cfg.max();
        let t = &mut self.t[i];
        if t.tcsr & LOAD != 0 {
            t.count = t.tlr & max as u32;
            return;
        }
        if t.tcsr & ENT == 0 {
            return;
        }
        let down = t.tcsr & UDT != 0;
        let count = u64::from(t.count);
        // Clocks until the next rollover event.
        let to_roll = if down { count + 1 } else { max - count + 1 };
        if t.tcsr & MDT != 0 {
            // Capture mode: free running and wrapping, no events. With ARHT ("hold") the
            // counter stops at a captured value until its TINT is cleared.
            if t.tcsr & ARHT != 0 && t.tcsr & TINT != 0 {
                return;
            }
            let m = max + 1;
            let v = if down {
                (count + m - d % m) % m
            } else {
                (count + d) % m
            };
            t.count = v as u32;
            return;
        }
        if d < to_roll {
            t.count = if down { count - d } else { count + d } as u32;
            return;
        }
        let rem = d - to_roll;
        let reload = u64::from(t.tlr) & max;
        let (period, start) = if t.tcsr & ARHT != 0 {
            let p = if down { reload + 1 } else { max - reload + 1 };
            (p, reload)
        } else {
            (max + 1, if down { max } else { 0 })
        };
        let n = 1 + rem / period;
        let r = rem % period;
        t.count = if down { start - r } else { start + r } as u32;
        t.rollovers += n;
        t.last_roll = Some(base + to_roll + (n - 1) * period);
        t.tcsr |= TINT;
        if t.tcsr & GENT != 0 {
            t.pulses += n;
        }
    }

    /// Cascade mode: one counter of `2 * C_COUNT_WIDTH` bits driven by timer 0's control bits.
    fn step_cascade(&mut self, d: u64, base: u64) {
        let w = self.cfg.count_width;
        let max = (1u128 << (2 * w)) - 1;
        let tcsr = self.t[0].tcsr;
        let reload = self.joined_tlr();
        if tcsr & LOAD != 0 {
            self.split(reload);
            return;
        }
        if tcsr & ENT == 0 {
            return;
        }
        if tcsr & MDT != 0 && tcsr & ARHT != 0 && self.t[0].tcsr & TINT != 0 {
            return; // hold after a capture
        }
        let down = tcsr & UDT != 0;
        let count = self.joined();
        let d = u128::from(d);
        if tcsr & MDT != 0 {
            let m = max + 1;
            let v = if down {
                (count + m - d % m) % m
            } else {
                (count + d) % m
            };
            self.split(v);
            return;
        }
        let to_roll = if down { count + 1 } else { max - count + 1 };
        if d < to_roll {
            self.split(if down { count - d } else { count + d });
            return;
        }
        let rem = d - to_roll;
        let (period, start) = if tcsr & ARHT != 0 {
            let p = if down { reload + 1 } else { max - reload + 1 };
            (p, reload)
        } else {
            (max + 1, if down { max } else { 0 })
        };
        let n = 1 + rem / period;
        let r = rem % period;
        self.split(if down { start - r } else { start + r });
        let last = base + (to_roll + (n - 1) * period) as u64;
        self.t[0].rollovers += n as u64;
        self.t[0].last_roll = Some(last);
        if tcsr & GENT != 0 {
            self.t[0].pulses += n as u64;
        }
        self.t[0].tcsr |= TINT;
        self.t[1].tcsr |= TINT;
    }

    /// A level on `CAPTURETRIG<ch>` at `now_ns`: a rising edge captures the counter.
    fn capture_input(&mut self, ch: usize, level: bool, now_ns: u64) {
        self.sync(now_ns);
        let rising = level && !self.trig[ch];
        self.trig[ch] = level;
        if !rising {
            return;
        }
        const NEED: u32 = MDT | CAPT | ENT;
        if self.cascaded() {
            // The cascade captures on CAPTURETRIG0 only.
            if ch != 0 || self.t[0].tcsr & NEED != NEED {
                return;
            }
            let v = self.joined();
            let w = self.cfg.count_width;
            let m = self.word_mask();
            self.t[0].tlr = (v & m) as u32;
            self.t[1].tlr = ((v >> w) & m) as u32;
            self.t[0].captures += 1;
            self.t[0].tcsr |= TINT;
        } else if ch < self.channels() && self.t[ch].tcsr & NEED == NEED {
            let t = &mut self.t[ch];
            t.tlr = t.count;
            t.captures += 1;
            t.tcsr |= TINT;
        }
    }

    fn irq(&self) -> bool {
        (0..self.channels()).any(|i| self.t[i].tcsr & (TINT | ENIT) == (TINT | ENIT))
    }

    fn reset(&mut self) {
        self.t = [Chan::default(); 2];
        self.trig = [false; 2];
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

/// An AXI Timer instance as a [`RegisterBlock`].
pub struct AxiTimer {
    name: String,
    st: Arc<Mutex<State>>,
}

/// Read access to the timer's outputs for tests and the engine.
#[derive(Clone)]
pub struct TimerHandle {
    st: Arc<Mutex<State>>,
}

impl AxiTimer {
    /// A freshly reset instance named `name`.
    pub fn new(name: &str, cfg: TimerConfig) -> (AxiTimer, TimerHandle) {
        let st = Arc::new(Mutex::new(State {
            cfg,
            t: [Chan::default(); 2],
            trig: [false; 2],
            cyc: 0,
        }));
        (
            AxiTimer {
                name: name.to_string(),
                st: Arc::clone(&st),
            },
            TimerHandle { st },
        )
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.st.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl TimerHandle {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.st.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Number of rollovers of timer `i` since reset.
    pub fn rollovers(&self, i: usize) -> u64 {
        self.lock().t.get(i).map_or(0, |t| t.rollovers)
    }

    /// Number of one-clock `generateout` pulses timer `i` produced (rollovers with `GENT` set).
    pub fn generate_pulses(&self, i: usize) -> u64 {
        self.lock().t.get(i).map_or(0, |t| t.pulses)
    }

    /// The PWM output (`pwm0`): high from a rollover of timer 0 to the next one of timer 1.
    /// Always false when `PWMA` is clear.
    pub fn pwm_out(&self) -> bool {
        let s = self.lock();
        if s.t[0].tcsr & PWM == 0 {
            return false;
        }
        match (s.t[0].last_roll, s.t[1].last_roll) {
            (Some(a), Some(b)) => a > b,
            (Some(_), None) => true,
            _ => false,
        }
    }

    /// Number of capture events of timer `i` since reset (rising edges on its trigger while
    /// capture mode is armed).
    pub fn captures(&self, i: usize) -> u64 {
        self.lock().t.get(i).map_or(0, |t| t.captures)
    }

    /// Drives `CAPTURETRIG<ch>` (0 or 1) to `level` at simulated time `now_ns`. A rising edge
    /// captures the counter into `TLR` when the timer is in capture mode with `CAPT` and `ENT`
    /// set. The board binding calls this with the switch or button level.
    pub fn capture_input(&self, ch: usize, level: bool, now_ns: u64) {
        if ch < 2 {
            self.lock().capture_input(ch, level, now_ns);
        }
    }

    /// Interrupt output level.
    pub fn irq(&self) -> bool {
        self.lock().irq()
    }
}

impl RegisterBlock for AxiTimer {
    fn name(&self) -> &str {
        &self.name
    }

    fn size(&self) -> u64 {
        self.lock().cfg.window
    }

    fn advance(&mut self, now_ns: u64) {
        self.lock().sync(now_ns);
    }

    fn read32(&mut self, offset: u64) -> u32 {
        let s = self.lock();
        let two = s.channels() == 2;
        match offset {
            TCSR0 => s.t[0].tcsr,
            TLR0 => s.t[0].tlr,
            TCR0 => s.t[0].count,
            TCSR1 if two => s.t[1].tcsr,
            TLR1 if two => s.t[1].tlr,
            TCR1 if two => s.t[1].count,
            _ => 0,
        }
    }

    fn write32(&mut self, offset: u64, value: u32, strobe: u8) {
        let mut s = self.lock();
        let max = s.cfg.max() as u32;
        let two = s.channels() == 2;
        let (i, reg) = match offset {
            TCSR0 | TLR0 => (0, offset),
            TCSR1 | TLR1 if two => (1, offset - 0x10),
            _ => return,
        };
        match reg {
            TCSR0 => {
                let old = s.t[i].tcsr;
                let v = merge(old, value, strobe) & TCSR_MASK;
                // TINT: write 1 to clear, 0 leaves it.
                let tint = old & TINT & !(value & TINT);
                let new = (v & !TINT) | tint;
                s.t[i].tcsr = new;
                if new & LOAD != 0 {
                    if s.cascaded() {
                        let r = s.joined_tlr();
                        s.split(r);
                    } else {
                        s.t[i].count = s.t[i].tlr & max;
                    }
                }
                if new & ENALL != 0 {
                    for k in 0..s.channels() {
                        s.t[k].tcsr |= ENT;
                    }
                }
            }
            TLR0 => s.t[i].tlr = merge(s.t[i].tlr, value, strobe) & max,
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

/// Convenience for tests and the engine: the number of clocks in `ns` at `hz`.
pub fn clocks_in(ns: u64, hz: u64) -> u64 {
    (u128::from(ns) * u128::from(hz) / 1_000_000_000) as u64
}

/// The value of a numeric parameter used by [`TimerConfig`] callers that only have text.
pub fn clock_param(p: &Params) -> Result<u64, String> {
    number(p, "CLK_FREQ_HZ", 100_000_000)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 100 MHz: one clock is 10 ns.
    fn timer() -> (AxiTimer, TimerHandle) {
        AxiTimer::new("t", TimerConfig::default())
    }

    fn w(t: &mut AxiTimer, off: u64, v: u32) {
        t.write32(off, v, 0xF);
    }

    /// Starts timer 0 the way the Xilinx driver does: TLR, LOAD, then enable.
    fn start(t: &mut AxiTimer, now: u64, tlr: u32, extra: u32) {
        t.advance(now);
        w(t, TLR0, tlr);
        w(t, TCSR0, LOAD);
        w(t, TCSR0, ENT | extra);
    }

    #[test]
    fn loads_and_counts_up_with_simulated_time() {
        let (mut t, _) = timer();
        start(&mut t, 0, 100, 0);
        assert_eq!(t.read32(TCR0), 100);
        t.advance(50); // 5 clocks
        assert_eq!(t.read32(TCR0), 105);
        t.advance(1_000);
        assert_eq!(t.read32(TCR0), 200);
        // Disabled: holds.
        w(&mut t, TCSR0, 0);
        t.advance(5_000);
        assert_eq!(t.read32(TCR0), 200);
    }

    #[test]
    fn load_holds_the_counter_while_set() {
        let (mut t, _) = timer();
        t.advance(0);
        w(&mut t, TLR0, 7);
        w(&mut t, TCSR0, LOAD | ENT);
        t.advance(10_000);
        assert_eq!(t.read32(TCR0), 7, "LOAD keeps the counter loaded");
        w(&mut t, TCSR0, ENT);
        t.advance(10_100);
        assert_eq!(t.read32(TCR0), 7 + 10);
    }

    #[test]
    fn down_count_rolls_over_after_tlr_plus_one_clocks_and_interrupts() {
        let (mut t, h) = timer();
        start(&mut t, 0, 9, UDT | ENIT);
        // After 9 clocks the counter is 0, no rollover yet.
        t.advance(90);
        assert_eq!(t.read32(TCR0), 0);
        assert_eq!(h.rollovers(0), 0);
        assert!(!t.irq_level());
        // The 10th clock rolls over (TLR + 1 = 10 clocks).
        t.advance(100);
        assert_eq!(h.rollovers(0), 1);
        assert_eq!(t.read32(TCSR0) & TINT, TINT);
        assert!(t.irq_level());
        // No auto reload: wraps to all ones and keeps going down.
        assert_eq!(t.read32(TCR0), u32::MAX);
        // Write 1 to clear TINT; the interrupt drops.
        w(&mut t, TCSR0, ENT | UDT | ENIT | TINT);
        assert_eq!(t.read32(TCSR0) & TINT, 0);
        assert!(!t.irq_level());
        // Writing 0 to TINT leaves a set bit alone.
        t.advance(100 + 10 * 0x1_0000_0000u64); // 2^32 clocks later: rolls over again
        assert_eq!(t.read32(TCSR0) & TINT, TINT);
        w(&mut t, TCSR0, ENT | UDT | ENIT);
        assert_eq!(t.read32(TCSR0) & TINT, TINT);
    }

    #[test]
    fn auto_reload_makes_a_periodic_interrupt() {
        let (mut t, h) = timer();
        // Period 1000 clocks = 10 us.
        start(&mut t, 0, 999, UDT | ARHT | ENIT);
        t.advance(10_000 - 10);
        assert_eq!(h.rollovers(0), 0);
        assert_eq!(t.read32(TCR0), 0);
        t.advance(10_000);
        assert_eq!(h.rollovers(0), 1);
        assert_eq!(t.read32(TCR0), 999, "reloaded");
        // 100 more periods in one jump: exactly 100 rollovers and the phase kept.
        t.advance(10_000 + 100 * 10_000 + 500);
        assert_eq!(h.rollovers(0), 101);
        assert_eq!(t.read32(TCR0), 999 - 50);
    }

    #[test]
    fn up_count_rolls_over_after_max_minus_tlr_plus_one() {
        let (mut t, h) = timer();
        // 4 clocks to rollover: TLR = MAX - 3.
        start(&mut t, 0, u32::MAX - 3, ARHT | ENIT);
        t.advance(30);
        assert_eq!(h.rollovers(0), 0);
        assert_eq!(t.read32(TCR0), u32::MAX);
        t.advance(40);
        assert_eq!(h.rollovers(0), 1);
        assert_eq!(t.read32(TCR0), u32::MAX - 3);
    }

    #[test]
    fn narrow_counter_width() {
        let p: Params = [("C_COUNT_WIDTH".to_string(), "8".to_string())]
            .into_iter()
            .collect();
        let (mut t, h) = AxiTimer::new("t", TimerConfig::from_params(&p, 0x1000).unwrap());
        start(&mut t, 0, 0xFFFF_FFF0, ENIT); // masked to 8 bits: 0xF0
        assert_eq!(t.read32(TLR0), 0xF0);
        t.advance(10 * 0x0F); // 15 clocks reaches 0xFF
        assert_eq!(t.read32(TCR0), 0xFF);
        t.advance(10 * 0x10); // the 16th rolls over
        assert_eq!(h.rollovers(0), 1);
        assert_eq!(t.read32(TCR0), 0);
    }

    #[test]
    fn timer1_is_independent_and_enall_starts_both() {
        let (mut t, h) = timer();
        t.advance(0);
        w(&mut t, TLR0, 5);
        w(&mut t, TLR1, 20);
        w(&mut t, TCSR0, LOAD | UDT);
        w(&mut t, TCSR1, LOAD | UDT);
        w(&mut t, TCSR0, UDT | ENALL);
        assert_eq!(t.read32(TCSR1) & ENT, ENT);
        // timer 1 still loaded: counts from the load because LOAD was cleared only on timer 0.
        w(&mut t, TCSR1, UDT | ENT | ENIT);
        t.advance(100); // 10 clocks
        assert_eq!(h.rollovers(0), 1, "timer 0 (6 clocks) rolled over");
        assert_eq!(t.read32(TCR1), 10);
        assert!(
            !t.irq_level(),
            "only timer 1 has ENIT and it has not rolled over"
        );
        t.advance(300);
        assert!(t.irq_level());
        assert_eq!(t.read32(TCSR1) & TINT, TINT);
    }

    #[test]
    fn generate_pulses_and_pwm_level() {
        let (mut t, h) = timer();
        t.advance(0);
        w(&mut t, TLR0, 9); // period 10 clocks
        w(&mut t, TLR1, 4); // period 5 clocks
        w(&mut t, TCSR0, LOAD | UDT);
        w(&mut t, TCSR1, LOAD | UDT);
        w(&mut t, TCSR0, UDT | ARHT | GENT | PWM | ENT);
        w(&mut t, TCSR1, UDT | ARHT | GENT | ENT);
        t.advance(100); // 10 clocks: timer 0 rolled over at 10, timer 1 at 5 and 10
        assert_eq!(h.generate_pulses(0), 1);
        assert_eq!(h.generate_pulses(1), 2);
        // Timer 0's last rollover (clock 10) is not after timer 1's (clock 10): level low.
        assert!(!h.pwm_out());
        t.advance(110); // clock 11: nothing new
        assert!(!h.pwm_out());
    }

    #[test]
    fn one_timer_only_hides_timer_1() {
        let p: Params = [("C_ONE_TIMER_ONLY".to_string(), "1".to_string())]
            .into_iter()
            .collect();
        let (mut t, _) = AxiTimer::new("t", TimerConfig::from_params(&p, 0x1000).unwrap());
        w(&mut t, TLR1, 5);
        assert_eq!(t.read32(TLR1), 0);
        assert_eq!(t.read32(TCSR1), 0);
    }

    #[test]
    fn reset_clears_everything() {
        let (mut t, h) = timer();
        start(&mut t, 0, 3, UDT | ENIT);
        t.advance(1_000);
        assert!(t.irq_level());
        t.reset();
        assert!(!t.irq_level());
        assert_eq!(t.read32(TCSR0), 0);
        assert_eq!(t.read32(TLR0), 0);
        assert_eq!(h.rollovers(0), 0);
    }

    #[test]
    fn clock_frequency_scales_time() {
        let p: Params = [("CLK_FREQ_HZ".to_string(), "50000000".to_string())]
            .into_iter()
            .collect();
        let (mut t, _) = AxiTimer::new("t", TimerConfig::from_params(&p, 0x1000).unwrap());
        start(&mut t, 0, 0, 0);
        t.advance(1_000); // 50 clocks at 50 MHz
        assert_eq!(t.read32(TCR0), 50);
        assert_eq!(clocks_in(1_000, 50_000_000), 50);
        assert_eq!(clock_param(&p), Ok(50_000_000));
    }

    /// Capture mode on timer 0 from count 0 at time 0 (`MDT | CAPT | ENT`, plus `extra`).
    fn armed_capture(t: &mut AxiTimer, extra: u32) {
        t.advance(0);
        w(t, TLR0, 0);
        w(t, TCSR0, LOAD);
        w(t, TCSR0, MDT | CAPT | ENT | extra);
    }

    #[test]
    fn capture_on_a_rising_edge_copies_the_counter_into_tlr() {
        let (mut t, h) = timer();
        armed_capture(&mut t, ENIT);
        // 1 us after the start the counter is 100 (10 ns clocks).
        h.capture_input(0, true, 1_000);
        assert_eq!(h.captures(0), 1);
        assert_eq!(t.read32(TLR0), 100, "the captured count is in TLR0");
        assert_eq!(t.read32(TCSR0) & TINT, TINT, "the capture sets TINT");
        assert!(t.irq_level(), "TINT and ENIT raise the interrupt");
        // The counter keeps running; the capture register does not follow it.
        t.advance(3_000);
        assert_eq!(t.read32(TCR0), 300);
        assert_eq!(t.read32(TLR0), 100);
        // Write 1 to TINT clears the flag and the interrupt; writing 0 leaves it.
        w(&mut t, TCSR0, MDT | CAPT | ENT | ENIT);
        assert_eq!(t.read32(TCSR0) & TINT, TINT);
        w(&mut t, TCSR0, MDT | CAPT | ENT | ENIT | TINT);
        assert_eq!(t.read32(TCSR0) & TINT, 0);
        assert!(!t.irq_level());
    }

    #[test]
    fn capture_needs_a_rising_edge_not_a_level() {
        let (mut t, h) = timer();
        armed_capture(&mut t, 0);
        h.capture_input(0, true, 1_000);
        // The input stays high: no second edge, no second capture.
        h.capture_input(0, true, 2_000);
        assert_eq!(h.captures(0), 1);
        assert_eq!(t.read32(TLR0), 100, "the first edge's value");
        // Falling then rising again: a new capture at the new time.
        h.capture_input(0, false, 2_500);
        h.capture_input(0, true, 4_000);
        assert_eq!(h.captures(0), 2);
        assert_eq!(t.read32(TLR0), 400);
    }

    #[test]
    fn capture_is_ignored_when_capt_or_ent_or_mdt_is_clear() {
        let (mut t, h) = timer();
        // Capture mode but CAPT clear: the trigger does nothing.
        armed_capture(&mut t, 0);
        w(&mut t, TCSR0, MDT | ENT);
        h.capture_input(0, true, 1_000);
        assert_eq!(h.captures(0), 0);
        assert_eq!(t.read32(TLR0), 0);
        assert_eq!(t.read32(TCSR0) & TINT, 0);
        // Generate mode: the same edge on CAPTURETRIG0 is not a capture.
        h.capture_input(0, false, 1_100);
        w(&mut t, TCSR0, CAPT | ENT);
        h.capture_input(0, true, 1_200);
        assert_eq!(h.captures(0), 0);
        // A stopped timer does not capture either.
        h.capture_input(0, false, 1_300);
        w(&mut t, TCSR0, MDT | CAPT);
        h.capture_input(0, true, 1_400);
        assert_eq!(h.captures(0), 0);
        assert_eq!(t.read32(TLR0), 0);
    }

    #[test]
    fn timer1_has_its_own_capture_trigger() {
        let (mut t, h) = timer();
        t.advance(0);
        w(&mut t, TLR1, 0);
        w(&mut t, TCSR1, LOAD);
        w(&mut t, TCSR1, MDT | CAPT | ENT);
        h.capture_input(0, true, 500); // CAPTURETRIG0 is not wired to timer 1
        assert_eq!(h.captures(1), 0);
        h.capture_input(1, true, 500);
        assert_eq!(h.captures(1), 1);
        assert_eq!(t.read32(TLR1), 50);
        assert_eq!(h.captures(0), 0);
    }

    #[test]
    fn capture_with_arht_holds_the_counter_until_tint_is_cleared() {
        let (mut t, h) = timer();
        armed_capture(&mut t, ENIT | ARHT);
        h.capture_input(0, true, 1_000);
        assert_eq!(t.read32(TLR0), 100);
        t.advance(5_000);
        assert_eq!(t.read32(TCR0), 100, "held after the capture");
        // Clearing TINT releases the hold; the time spent held is not counted.
        t.advance(5_000);
        w(&mut t, TCSR0, MDT | CAPT | ENT | ENIT | ARHT | TINT);
        t.advance(6_000);
        assert_eq!(t.read32(TCR0), 200);
    }

    #[test]
    fn cascade_carries_from_timer0_into_timer1_across_the_32_bit_boundary() {
        let (mut t, h) = timer();
        t.advance(0);
        w(&mut t, TLR1, 0);
        w(&mut t, TLR0, 0xFFFF_FFF0);
        w(&mut t, TCSR0, LOAD | CASC);
        assert_eq!(t.read32(TCR0), 0xFFFF_FFF0);
        assert_eq!(t.read32(TCR1), 0);
        w(&mut t, TCSR0, ENT | CASC);
        // 16 clocks reach 2^32: the low word wraps and the high word counts the carry.
        t.advance(160);
        assert_eq!(t.read32(TCR0), 0);
        assert_eq!(t.read32(TCR1), 1);
        t.advance(170);
        assert_eq!(t.read32(TCR0), 1);
        assert_eq!(t.read32(TCR1), 1);
        // A carry is not a 64-bit rollover: no interrupt, no rollover count.
        assert_eq!(t.read32(TCSR0) & TINT, 0);
        assert_eq!(h.rollovers(0), 0);
    }

    #[test]
    fn cascade_rollover_is_a_64_bit_rollover_with_a_periodic_interrupt() {
        let (mut t, h) = timer();
        t.advance(0);
        // Reload 2^64 - 16 with auto reload: a 16-clock period of the 64-bit counter.
        w(&mut t, TLR1, u32::MAX);
        w(&mut t, TLR0, 0xFFFF_FFF0);
        w(&mut t, TCSR0, LOAD | CASC);
        w(&mut t, TCSR0, ENT | CASC | ARHT | ENIT);
        t.advance(160);
        assert_eq!(h.rollovers(0), 1, "one 64-bit rollover in 16 clocks");
        assert_eq!(t.read32(TCR1), u32::MAX, "reloaded, high word kept");
        assert_eq!(t.read32(TCR0), 0xFFFF_FFF0);
        assert!(t.irq_level());
        // Both timers show TINT; clearing timer 0's flag drops the output.
        assert_eq!(t.read32(TCSR1) & TINT, TINT);
        w(&mut t, TCSR0, ENT | CASC | ARHT | ENIT | TINT);
        assert!(!t.irq_level());
        // The next rollover is 16 clocks later (clock 32) and raises it again.
        t.advance(320);
        assert_eq!(h.rollovers(0), 2);
        assert!(t.irq_level());
        // Clocks 33..64 hold two more rollovers (48 and 64): four in total.
        t.advance(640);
        assert_eq!(h.rollovers(0), 4);
    }

    #[test]
    fn cascade_capture_stores_the_64_bit_count_in_tlr1_tlr0() {
        let (mut t, h) = timer();
        t.advance(0);
        w(&mut t, TLR1, 0);
        w(&mut t, TLR0, 0xFFFF_FFF0);
        w(&mut t, TCSR0, LOAD | CASC);
        w(&mut t, TCSR0, MDT | CAPT | ENT | CASC);
        // 100 clocks after the start: 0xFFFF_FFF0 + 100 = 0x1_0000_0054.
        h.capture_input(0, true, 1_000);
        assert_eq!(h.captures(0), 1);
        assert_eq!(t.read32(TLR0), 0x54);
        assert_eq!(t.read32(TLR1), 1);
        assert_eq!(t.read32(TCSR0) & TINT, TINT);
        // Timer 1's trigger is not part of the cascade.
        h.capture_input(1, true, 2_000);
        assert_eq!(h.captures(1), 0);
    }

    #[test]
    fn reset_clears_captures_and_the_trigger_level() {
        let (mut t, h) = timer();
        armed_capture(&mut t, ENIT);
        h.capture_input(0, true, 1_000);
        assert_eq!(h.captures(0), 1);
        t.reset();
        assert_eq!(h.captures(0), 0);
        assert_eq!(t.read32(TCSR0), 0);
        // After reset the input is low again: a high level is a fresh rising edge.
        armed_capture(&mut t, 0);
        h.capture_input(0, true, 2_000);
        assert_eq!(h.captures(0), 1);
    }
}
