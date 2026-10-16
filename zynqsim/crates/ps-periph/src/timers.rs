//! The Cortex-A9 MPCore timers: the private timer and the watchdog (countdown blocks at
//! 0xF8F00600 and 0xF8F00620) and the global timer (0xF8F00200). All three count at PERIPHCLK
//! (CPU clock / 2) divided by their prescaler plus one.
//!
//! Register layouts follow the Cortex-A9 MPCore TRM as remembered, and the firmware checks in
//! `firmware/conf_scutimer`. `VERIFY:` marks what was not compared with the TRM. The global
//! timer follows the TRM's equality comparator: a match sets the flag once per counter value,
//! and write-1-to-clear stays cleared until the counter next reaches the comparator.

use crate::clock::{PERIPH_DIV, ticks_of};
use mem_sys::lane_merge;
use ps_bus::{BusFault, BusResult};

/// Private timer PPI (per core).
pub const PRIVATE_TIMER_PPI: u16 = 29;
/// Watchdog PPI (per core).
pub const WATCHDOG_PPI: u16 = 30;
/// Global timer PPI (delivered to every core).
pub const GLOBAL_TIMER_PPI: u16 = 27;

/// CONTROL bit 0: the timer is enabled.
pub const CTRL_EN: u32 = 1 << 0;
/// CONTROL bit 1: auto-reload (otherwise the timer stops after one event).
pub const CTRL_AR: u32 = 1 << 1;
/// CONTROL bit 2: the event raises the interrupt line.
pub const CTRL_IRQ: u32 = 1 << 2;
/// CONTROL bit 3 (watchdog only): watchdog mode, an event requests a reset.
pub const CTRL_WD_MODE: u32 = 1 << 3;

/// The prescaler field of CONTROL, bits 15:8.
fn prescale(control: u32) -> u64 {
    u64::from((control >> 8) & 0xFF) + 1
}

/// A countdown block: the private timer, and the watchdog's counter. Offsets are relative to
/// the block: LOAD 0x00, COUNTER 0x04, CONTROL 0x08, ISR 0x0C.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Countdown {
    /// LOAD: the value the counter reloads with in auto-reload mode.
    pub load: u32,
    /// COUNTER: the current count.
    pub counter: u32,
    /// CONTROL, masked to the bits this block implements.
    pub control: u32,
    /// The event flag (ISR bit 0), cleared by writing 1.
    pub isr: bool,
    ctrl_mask: u32,
    /// Prescaler carry: periphery ticks not yet counted.
    pre: u64,
}

impl Countdown {
    /// A block in reset state (disabled, zero registers). `ctrl_mask` is the CONTROL bits the
    /// block implements.
    pub fn new(ctrl_mask: u32) -> Countdown {
        Countdown {
            load: 0,
            counter: 0,
            control: 0,
            isr: false,
            ctrl_mask,
            pre: 0,
        }
    }

    /// Whether the counter runs.
    pub fn enabled(&self) -> bool {
        self.control & CTRL_EN != 0
    }

    /// Whether the interrupt line is asserted.
    pub fn irq_asserted(&self) -> bool {
        self.isr && self.control & CTRL_IRQ != 0
    }

    /// Advances by `periph_ticks` PERIPHCLK cycles. Returns whether an event fired.
    ///
    /// An enabled counter that reaches zero sets the flag. Auto-reload reloads with `load` on
    /// the next tick, so events come every `load + 1` ticks. Without auto-reload the counter
    /// stays at zero and does not fire again until it is written.
    pub fn advance(&mut self, periph_ticks: u64) -> bool {
        if !self.enabled() || periph_ticks == 0 {
            return false;
        }
        let total = self.pre + periph_ticks;
        let div = prescale(self.control);
        self.pre = total % div;
        let ticks = total / div;
        if ticks == 0 {
            return false;
        }
        let load = u64::from(self.load);
        let mut c = u64::from(self.counter);
        if self.control & CTRL_AR != 0 {
            // At zero the next tick reloads, so from zero the next event is load + 1 ticks away.
            if c == 0 {
                c = load + 1;
            }
            if ticks < c {
                self.counter = (c - ticks) as u32;
                return false;
            }
            let since = ticks - c;
            let r = since % (load + 1);
            self.counter = if r == 0 { 0 } else { (load - (r - 1)) as u32 };
            self.isr = true;
            true
        } else {
            if c == 0 {
                return false;
            }
            if ticks < c {
                self.counter = (c - ticks) as u32;
                return false;
            }
            self.counter = 0;
            self.isr = true;
            true
        }
    }

    /// Word value of the register at aligned offset `a` (0, 4, 8 or 0xC).
    fn word(&self, a: u64) -> u32 {
        match a {
            0x00 => self.load,
            0x04 => self.counter,
            0x08 => self.control & self.ctrl_mask,
            0x0C => u32::from(self.isr),
            _ => 0,
        }
    }

    /// Reads `size` bytes at `offset` (relative to the block).
    pub fn read(&self, offset: u64, size: u8) -> BusResult<u64> {
        mem_sys::lane_read(offset, size, self.word(offset & !3))
    }

    /// Writes `size` bytes at `offset` (relative to the block).
    pub fn write(&mut self, offset: u64, size: u8, value: u64) -> BusResult<()> {
        let a = offset & !3;
        match a {
            0x00 | 0x04 | 0x08 => {
                let (a, w) = lane_merge(offset, size, value, self.word(a))?;
                match a {
                    0x00 => self.load = w,
                    0x04 => {
                        self.counter = w;
                    }
                    _ => self.control = w & self.ctrl_mask,
                }
            }
            0x0C => {
                // Write-1-to-clear: the lanes that are not written must not clear anything.
                let (_, w) = lane_merge(offset, size, value, 0)?;
                if w & 1 != 0 {
                    self.isr = false;
                }
            }
            _ => {
                lane_merge(offset, size, value, 0)?;
            }
        }
        Ok(())
    }

    /// Appends the registers for determinism hashes.
    pub fn save_state(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.load.to_le_bytes());
        out.extend_from_slice(&self.counter.to_le_bytes());
        out.extend_from_slice(&self.control.to_le_bytes());
        out.push(u8::from(self.isr));
        out.extend_from_slice(&self.pre.to_le_bytes());
    }
}

/// The private timer of one core (block offset 0x600 from the MPCore base).
pub fn private_timer() -> Countdown {
    Countdown::new(0xFF07)
}

/// The watchdog of one core: the countdown block at 0x620, plus its reset status and the
/// two-step disable sequence (DISABLE at +0x14). No reset is performed: a watchdog event in
/// watchdog mode sets RESET_STATUS and the model records it (`VERIFY:` the W1C behaviour).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Watchdog {
    /// The countdown block.
    pub cd: Countdown,
    /// RESET_STATUS bit 0: a watchdog-mode event happened.
    pub reset_status: u32,
    disable_step: u8,
}

impl Default for Watchdog {
    fn default() -> Self {
        Watchdog::new()
    }
}

impl Watchdog {
    /// A watchdog in reset state.
    pub fn new() -> Watchdog {
        Watchdog {
            cd: Countdown::new(0xFF0F),
            reset_status: 0,
            disable_step: 0,
        }
    }

    /// Advances by `periph_ticks`.
    pub fn advance(&mut self, periph_ticks: u64) {
        if self.cd.advance(periph_ticks) && self.cd.control & CTRL_WD_MODE != 0 {
            self.reset_status |= 1;
        }
    }

    /// Reads `size` bytes at `offset` (relative to the 0x620 block).
    pub fn read(&self, offset: u64, size: u8) -> BusResult<u64> {
        match offset & !3 {
            0x10 => mem_sys::lane_read(offset, size, self.reset_status),
            0x14 => mem_sys::lane_read(offset, size, 0),
            _ => self.cd.read(offset, size),
        }
    }

    /// Writes `size` bytes at `offset` (relative to the 0x620 block).
    pub fn write(&mut self, offset: u64, size: u8, value: u64) -> BusResult<()> {
        match offset & !3 {
            0x10 => {
                let (_, w) = lane_merge(offset, size, value, 0)?;
                self.reset_status &= !w;
            }
            0x14 => {
                let (_, w) = lane_merge(offset, size, value, 0)?;
                // The disable sequence: 0x12345678 then 0x87654321 clears enable and mode.
                self.disable_step = match (self.disable_step, w) {
                    (0, 0x1234_5678) => 1,
                    (1, 0x8765_4321) => {
                        self.cd.control &= !(CTRL_EN | CTRL_WD_MODE);
                        0
                    }
                    _ => 0,
                };
            }
            _ => return self.cd.write(offset, size, value),
        }
        Ok(())
    }
}

/// The global timer: a 64-bit counter with an equality comparator, auto-increment and an
/// interrupt. Offsets from the block base: COUNTER 0x00/0x04, CONTROL 0x08, ISR 0x0C,
/// COMPARATOR 0x10/0x14, AUTO_INCREMENT 0x18.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GlobalTimer {
    /// The 64-bit counter.
    pub counter: u64,
    /// CONTROL: bit 0 enable, bit 1 comparator enable, bit 2 IRQ enable, bit 3 auto-increment,
    /// bits 15:8 prescaler.
    pub control: u32,
    /// The event flag, cleared by writing 1.
    pub isr: bool,
    /// The comparator value.
    pub comparator: u64,
    /// The auto-increment value added to the comparator after each match.
    pub incr: u32,
    pre: u64,
}

impl Default for GlobalTimer {
    fn default() -> Self {
        GlobalTimer::new()
    }
}

impl GlobalTimer {
    /// A global timer in reset state.
    pub fn new() -> GlobalTimer {
        GlobalTimer {
            counter: 0,
            control: 0,
            isr: false,
            comparator: 0,
            incr: 0,
            pre: 0,
        }
    }

    /// Whether the interrupt line is asserted.
    pub fn irq_asserted(&self) -> bool {
        self.isr && self.control & CTRL_IRQ != 0
    }

    /// Advances by `periph_ticks`. The counter holds while the timer is disabled. A match
    /// happens when the counter passes the comparator (equality, so each value matches once).
    pub fn advance(&mut self, periph_ticks: u64) {
        if self.control & CTRL_EN == 0 || periph_ticks == 0 {
            return;
        }
        let total = self.pre + periph_ticks;
        let div = prescale(self.control);
        self.pre = total % div;
        let ticks = total / div;
        if ticks == 0 {
            return;
        }
        let old = self.counter;
        let new = old.wrapping_add(ticks);
        if self.control & (1 << 1) != 0 && new > old {
            let auto = self.control & (1 << 3) != 0;
            let mut cmp = self.comparator;
            while cmp > old && cmp <= new {
                self.isr = true;
                if !auto {
                    break;
                }
                let next = cmp.wrapping_add(u64::from(self.incr));
                if next == cmp {
                    break;
                }
                cmp = next;
                self.comparator = cmp;
            }
        }
        self.counter = new;
    }

    fn word(&self, a: u64) -> u32 {
        match a {
            0x00 => self.counter as u32,
            0x04 => (self.counter >> 32) as u32,
            0x08 => self.control & 0xFF0F,
            0x0C => u32::from(self.isr),
            0x10 => self.comparator as u32,
            0x14 => (self.comparator >> 32) as u32,
            0x18 => self.incr,
            _ => 0,
        }
    }

    /// Reads `size` bytes at `offset` (relative to the block).
    pub fn read(&self, offset: u64, size: u8) -> BusResult<u64> {
        if offset > 0x1B {
            return Err(BusFault::Unmapped(offset));
        }
        mem_sys::lane_read(offset, size, self.word(offset & !3))
    }

    /// Writes `size` bytes at `offset` (relative to the block).
    pub fn write(&mut self, offset: u64, size: u8, value: u64) -> BusResult<()> {
        let a = offset & !3;
        match a {
            0x00 | 0x04 => {
                let (_, w) = lane_merge(offset, size, value, self.word(a))?;
                let shift = if a == 0 { 0 } else { 32 };
                let mask = 0xFFFF_FFFFu64 << shift;
                self.counter = (self.counter & !mask) | (u64::from(w) << shift);
            }
            0x08 => {
                let (_, w) = lane_merge(offset, size, value, self.word(a))?;
                self.control = w & 0xFF0F;
            }
            0x0C => {
                let (_, w) = lane_merge(offset, size, value, 0)?;
                if w & 1 != 0 {
                    self.isr = false;
                }
            }
            0x10 | 0x14 => {
                let (_, w) = lane_merge(offset, size, value, self.word(a))?;
                let shift = if a == 0x10 { 0 } else { 32 };
                let mask = 0xFFFF_FFFFu64 << shift;
                self.comparator = (self.comparator & !mask) | (u64::from(w) << shift);
            }
            0x18 => {
                let (_, w) = lane_merge(offset, size, value, self.word(a))?;
                self.incr = w;
            }
            _ => return Err(BusFault::Unmapped(offset)),
        }
        Ok(())
    }

    /// Appends the registers for determinism hashes.
    pub fn save_state(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.counter.to_le_bytes());
        out.extend_from_slice(&self.control.to_le_bytes());
        out.push(u8::from(self.isr));
        out.extend_from_slice(&self.comparator.to_le_bytes());
        out.extend_from_slice(&self.incr.to_le_bytes());
        out.extend_from_slice(&self.pre.to_le_bytes());
    }
}

/// Converts CPU cycles to PERIPHCLK ticks, keeping the remainder in `acc`.
pub fn periph_ticks(acc: &mut u64, cpu_cycles: u64) -> u64 {
    ticks_of(acc, cpu_cycles, PERIPH_DIV)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_shot_counts_down_to_zero_and_holds() {
        let mut t = private_timer();
        t.write(0x00, 4, 100).unwrap();
        t.write(0x04, 4, 100).unwrap();
        t.write(0x08, 4, u64::from(CTRL_EN)).unwrap();
        assert!(!t.advance(40));
        assert_eq!(t.counter, 60);
        assert!(t.advance(60));
        assert_eq!(t.counter, 0);
        assert!(t.isr);
        assert!(!t.advance(1000), "one-shot does not fire again");
        assert_eq!(t.counter, 0);
    }

    #[test]
    fn auto_reload_fires_every_load_plus_one_ticks() {
        let mut t = private_timer();
        t.write(0x00, 4, 3).unwrap();
        t.write(0x04, 4, 3).unwrap();
        t.write(0x08, 4, u64::from(CTRL_EN | CTRL_AR)).unwrap();
        // Events at ticks 3, 7, 11 ... (period 4). Count them one tick at a time.
        let mut events = 0;
        let mut seen = Vec::new();
        for tick in 1..=12u64 {
            t.isr = false;
            if t.advance(1) {
                events += 1;
                seen.push(tick);
            }
        }
        assert_eq!(seen, vec![3, 7, 11]);
        assert_eq!(events, 3);
        assert!(t.counter <= 3);
    }

    #[test]
    fn batched_and_single_ticks_agree() {
        let mut a = private_timer();
        let mut b = private_timer();
        for t in [&mut a, &mut b] {
            t.write(0x00, 4, 1000).unwrap();
            t.write(0x04, 4, 1000).unwrap();
            t.write(0x08, 4, u64::from(CTRL_EN | CTRL_AR | (1 << 8)))
                .unwrap();
        }
        a.advance(70_000);
        for _ in 0..70_000 {
            b.advance(1);
        }
        assert_eq!(a, b);
    }

    #[test]
    fn prescaler_divides_the_count() {
        let mut t = private_timer();
        t.write(0x00, 4, 0xFFFF_FFFF).unwrap();
        t.write(0x04, 4, 0xFFFF_FFFF).unwrap();
        t.write(0x08, 4, u64::from(CTRL_EN | (0xFF << 8))).unwrap();
        t.advance(256 * 10);
        assert_eq!(t.counter, 0xFFFF_FFFF - 10);
    }

    #[test]
    fn isr_is_write_one_to_clear_and_gated_by_irq_enable() {
        let mut t = private_timer();
        t.write(0x00, 4, 5).unwrap();
        t.write(0x04, 4, 5).unwrap();
        t.write(0x08, 4, u64::from(CTRL_EN)).unwrap();
        t.advance(5);
        assert!(t.isr);
        assert!(!t.irq_asserted(), "IRQ enable is off");
        t.write(0x08, 4, u64::from(CTRL_EN | CTRL_IRQ)).unwrap();
        assert!(t.irq_asserted());
        t.write(0x0C, 4, 0).unwrap();
        assert!(t.isr, "writing 0 does not clear");
        t.write(0x0C, 4, 1).unwrap();
        assert!(!t.isr);
    }

    #[test]
    fn global_timer_counts_and_holds_when_disabled() {
        let mut g = GlobalTimer::new();
        g.advance(100);
        assert_eq!(g.counter, 0);
        g.write(0x08, 4, u64::from(CTRL_EN)).unwrap();
        g.advance(100);
        assert_eq!(g.counter, 100);
        g.write(0x08, 4, 0).unwrap();
        g.advance(100);
        assert_eq!(g.counter, 100);
    }

    #[test]
    fn global_comparator_matches_once_and_clears_stay_cleared() {
        let mut g = GlobalTimer::new();
        g.write(0x10, 4, 50).unwrap();
        g.write(0x08, 4, u64::from(CTRL_EN | (1 << 1))).unwrap();
        g.advance(49);
        assert!(!g.isr);
        g.advance(1);
        assert!(g.isr);
        g.write(0x0C, 4, 1).unwrap();
        g.advance(100);
        assert!(!g.isr, "the counter passed the comparator, no re-assertion");
    }

    #[test]
    fn auto_increment_moves_the_comparator_by_the_increment() {
        let mut g = GlobalTimer::new();
        g.write(0x18, 4, 0x1000).unwrap();
        g.write(0x10, 4, 0x2000).unwrap();
        g.write(0x08, 4, u64::from(CTRL_EN | (1 << 1) | (1 << 3)))
            .unwrap();
        g.advance(0x2000);
        assert!(g.isr);
        assert_eq!(g.comparator, 0x3000);
        g.write(0x0C, 4, 1).unwrap();
        g.advance(0x1000);
        assert_eq!(g.comparator, 0x4000);
        assert!(g.isr);
    }

    #[test]
    fn watchdog_mode_event_sets_reset_status_and_disable_sequence_clears_it() {
        let mut w = Watchdog::new();
        w.write(0x00, 4, 4).unwrap(); // LOAD (offset 0x20 from the MPCore base)
        w.write(0x04, 4, 4).unwrap();
        w.write(0x08, 4, u64::from(CTRL_EN | CTRL_WD_MODE)).unwrap();
        w.advance(5);
        assert!(w.cd.isr);
        assert_eq!(w.read(0x10, 4).unwrap(), 1);
        w.write(0x14, 4, 0x1234_5678).unwrap();
        w.write(0x14, 4, 0x8765_4321).unwrap();
        assert_eq!(w.cd.control & CTRL_EN, 0);
    }
}
