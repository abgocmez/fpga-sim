//! Cadence triple timer counter (xttcps): TTC0 at 0xF8001000 (SPI 42..44) and TTC1 at
//! 0xF8002000 (SPI 69..71). Each block has three counters, each with its own clock control,
//! counter control, interval, three match registers, interrupt status and enable.
//!
//! The counters run from CPU_1X (the CPU clock divided by six); the clock control's prescaler
//! divides that by 2^(PS_VAL+1) when PS_EN is set. Counter control bits: 0 disable, 1 interval
//! mode (count 0..INTERVAL-1 then wrap and flag), 2 decrement, 3 match enable, 4 reset (self
//! clearing). Interrupt status (write 1 to clear) bits: 0 interval, 1 overflow (wrap at 0xFFFF
//! in overflow mode), 2..=4 match 1..3.
//!
//! Register offsets are those the `firmware/conf_ttc` checks use, and `VERIFY:` marks the ones
//! not compared with UG585 (the per-counter stride of the match registers, and the interval
//! mode polarity: bit 1 set = interval mode, as the QEMU model does).

use crate::clock::{TTC_DIV, ticks_of};
use mem_sys::{Device, IrqInput, lane_merge, lane_read};
use ps_bus::{BusFault, BusResult};

/// GIC SPI IDs of the three counters of each block, indexed `[block][counter]` (VERIFY: UG585).
pub const TTC_IRQS: [[u16; 3]; 2] = [[42, 43, 44], [69, 70, 71]];

const CNT_DIS: u32 = 1 << 0;
const CNT_INTERVAL: u32 = 1 << 1;
const CNT_DECR: u32 = 1 << 2;
const CNT_MATCH_EN: u32 = 1 << 3;
const CNT_RST: u32 = 1 << 4;
const CNT_MASK: u32 = 0x3F;

const CLK_PS_EN: u32 = 1 << 0;
const CLK_MASK: u32 = 0x5F;

const ISR_INTERVAL: u32 = 1 << 0;
const ISR_OVERFLOW: u32 = 1 << 1;
const ISR_MATCH: [u32; 3] = [1 << 2, 1 << 3, 1 << 4];
const ISR_MASK: u32 = 0x1F;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Reg {
    Clk,
    CntCtrl,
    CntVal,
    Interval,
    Match(usize),
    Isr,
    Ier,
}

/// Decodes an aligned offset into (register, counter). `None` for reserved offsets.
fn decode(a: u64) -> Option<(Reg, usize)> {
    match a {
        0x00..=0x08 => Some((Reg::Clk, (a / 4) as usize)),
        0x0C..=0x14 => Some((Reg::CntCtrl, ((a - 0x0C) / 4) as usize)),
        0x18..=0x20 => Some((Reg::CntVal, ((a - 0x18) / 4) as usize)),
        0x24..=0x2C => Some((Reg::Interval, ((a - 0x24) / 4) as usize)),
        0x30..=0x50 => {
            let idx = a - 0x30;
            let (n, x) = ((idx / 12) as usize, ((idx % 12) / 4) as usize);
            (x < 3 && n < 3).then_some((Reg::Match(x), n))
        }
        0x54..=0x5C => Some((Reg::Isr, ((a - 0x54) / 4) as usize)),
        0x60..=0x68 => Some((Reg::Ier, ((a - 0x60) / 4) as usize)),
        _ => None,
    }
}

/// One counter and its registers.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Counter {
    clk: u32,
    cnt_ctrl: u32,
    value: u32,
    interval: u32,
    matches: [u32; 3],
    isr: u32,
    ier: u32,
    /// Prescaler carry.
    pre: u64,
}

impl Counter {
    fn reset() -> Counter {
        Counter {
            cnt_ctrl: CNT_DIS,
            ..Counter::default()
        }
    }

    /// Advances by `ttc` CPU_1X ticks.
    fn advance(&mut self, ttc: u64) {
        if self.cnt_ctrl & CNT_DIS != 0 || ttc == 0 {
            return;
        }
        let div = if self.clk & CLK_PS_EN != 0 {
            1u64 << (((self.clk >> 1) & 0xF) + 1)
        } else {
            1
        };
        let total = self.pre + ttc;
        self.pre = total % div;
        for _ in 0..total / div {
            self.step();
        }
    }

    /// One counter step: count, wrap, and flag matches.
    fn step(&mut self) {
        let interval_mode = self.cnt_ctrl & CNT_INTERVAL != 0;
        if self.cnt_ctrl & CNT_DECR != 0 {
            if self.value == 0 {
                if interval_mode {
                    // VERIFY: wraps to INTERVAL - 1 so that the period is INTERVAL ticks.
                    self.value = self.interval.saturating_sub(1) & 0xFFFF;
                    self.isr |= ISR_INTERVAL;
                } else {
                    self.value = 0xFFFF;
                    self.isr |= ISR_OVERFLOW;
                }
            } else {
                self.value -= 1;
            }
        } else if interval_mode {
            // Counts 0..INTERVAL-1: the wrap happens when the count reaches INTERVAL, so the
            // period is INTERVAL ticks. VERIFY against UG585 (see the firmware's check 8).
            let next = self.value + 1;
            if next >= self.interval {
                self.value = 0;
                self.isr |= ISR_INTERVAL;
            } else {
                self.value = next;
            }
        } else if self.value == 0xFFFF {
            self.value = 0;
            self.isr |= ISR_OVERFLOW;
        } else {
            self.value += 1;
        }
        if self.cnt_ctrl & CNT_MATCH_EN != 0 {
            for (x, m) in self.matches.iter().enumerate() {
                if self.value == *m {
                    self.isr |= ISR_MATCH[x];
                }
            }
        }
    }

    fn level(&self) -> bool {
        self.isr & self.ier & ISR_MASK != 0
    }
}

/// One TTC block (three counters).
pub struct Ttc {
    block: usize,
    counters: [Counter; 3],
    /// CPU cycles not yet converted to CPU_1X ticks.
    acc: u64,
}

impl Ttc {
    /// Block 0 (TTC0 at 0xF8001000) or block 1 (TTC1 at 0xF8002000); other values give block 1.
    pub fn new(block: usize) -> Ttc {
        Ttc {
            block: block.min(1),
            counters: [Counter::reset(), Counter::reset(), Counter::reset()],
            acc: 0,
        }
    }

    /// Counter `n`'s counter value (for tests and the host).
    pub fn counter_value(&self, n: usize) -> u32 {
        self.counters[n].value
    }

    fn word(&self, reg: Reg, n: usize) -> u32 {
        let c = &self.counters[n];
        match reg {
            Reg::Clk => c.clk,
            Reg::CntCtrl => c.cnt_ctrl,
            Reg::CntVal => c.value,
            Reg::Interval => c.interval,
            Reg::Match(i) => c.matches[i],
            Reg::Isr => c.isr,
            Reg::Ier => c.ier,
        }
    }
}

impl Device for Ttc {
    fn read(&mut self, offset: u64, size: u8) -> BusResult<u64> {
        let a = offset & !3;
        match decode(a) {
            Some((reg, n)) => lane_read(offset, size, self.word(reg, n)),
            None if a < 0x70 => lane_read(offset, size, 0),
            None => Err(BusFault::Unmapped(offset)),
        }
    }

    fn write(&mut self, offset: u64, size: u8, value: u64) -> BusResult<()> {
        let a = offset & !3;
        let Some((reg, n)) = decode(a) else {
            if a < 0x70 {
                lane_merge(offset, size, value, 0)?;
                return Ok(());
            }
            return Err(BusFault::Unmapped(offset));
        };
        let c = &mut self.counters[n];
        match reg {
            Reg::Clk => {
                let (_, w) = lane_merge(offset, size, value, c.clk)?;
                c.clk = w & CLK_MASK;
            }
            Reg::CntCtrl => {
                let (_, w) = lane_merge(offset, size, value, c.cnt_ctrl)?;
                if w & CNT_RST != 0 {
                    c.value = 0;
                }
                c.cnt_ctrl = w & CNT_MASK & !CNT_RST;
            }
            Reg::CntVal => {
                // Read-only: the counter is cleared with the reset bit of CNT_CTRL.
                lane_merge(offset, size, value, 0)?;
            }
            Reg::Interval => {
                let (_, w) = lane_merge(offset, size, value, c.interval)?;
                c.interval = w & 0xFFFF;
            }
            Reg::Match(x) => {
                let (_, w) = lane_merge(offset, size, value, c.matches[x])?;
                c.matches[x] = w & 0xFFFF;
            }
            Reg::Isr => {
                let (_, w) = lane_merge(offset, size, value, 0)?;
                c.isr &= !(w & ISR_MASK);
            }
            Reg::Ier => {
                let (_, w) = lane_merge(offset, size, value, c.ier)?;
                c.ier = w & ISR_MASK;
            }
        }
        Ok(())
    }

    fn tick(&mut self, cycles: u64) {
        let ttc = ticks_of(&mut self.acc, cycles, TTC_DIV);
        for c in &mut self.counters {
            c.advance(ttc);
        }
    }

    fn irq_lines(&self, out: &mut Vec<IrqInput>) {
        for (n, c) in self.counters.iter().enumerate() {
            out.push(IrqInput {
                id: TTC_IRQS[self.block][n],
                cpu: None,
                level: c.level(),
            });
        }
    }

    fn save_state(&self, out: &mut Vec<u8>) {
        for c in &self.counters {
            for v in [
                c.clk,
                c.cnt_ctrl,
                c.value,
                c.interval,
                c.matches[0],
                c.matches[1],
                c.matches[2],
                c.isr,
                c.ier,
            ] {
                out.extend_from_slice(&v.to_le_bytes());
            }
            out.extend_from_slice(&c.pre.to_le_bytes());
        }
        out.extend_from_slice(&self.acc.to_le_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reserved_offsets_read_zero_and_unmapped_ones_fault() {
        let mut t = Ttc::new(0);
        assert_eq!(t.read(0x6C, 4), Ok(0));
        assert_eq!(t.read(0x58, 4), Ok(0), "0x58 is the ISR of counter 1");
        assert_eq!(t.read(0x80, 4), Err(BusFault::Unmapped(0x80)));
    }

    #[test]
    fn counters_are_independent_registers() {
        let mut t = Ttc::new(1);
        t.write(0x24, 4, 0x1234).unwrap();
        t.write(0x24 + 4, 4, 0x0F0F).unwrap();
        assert_eq!(t.read(0x24, 4), Ok(0x1234));
        assert_eq!(t.read(0x28, 4), Ok(0x0F0F));
        assert_eq!(t.read(0x2C, 4), Ok(0));
    }

    #[test]
    fn overflow_mode_wraps_at_0xffff_and_sets_the_flag() {
        let mut c = Counter::reset();
        c.cnt_ctrl = 0;
        c.value = 0xFFFE;
        c.advance(3);
        assert_eq!(c.value, 1);
        assert_ne!(c.isr & ISR_OVERFLOW, 0);
    }

    #[test]
    fn interval_mode_has_a_period_of_interval_ticks_and_flags_the_wrap() {
        let mut c = Counter::reset();
        c.cnt_ctrl = CNT_INTERVAL;
        c.interval = 10;
        c.advance(9);
        assert_eq!(c.value, 9);
        assert_eq!(c.isr & ISR_INTERVAL, 0);
        c.advance(1);
        assert_eq!(c.value, 0);
        assert_ne!(c.isr & ISR_INTERVAL, 0);
        c.advance(10 * 5 + 3);
        assert_eq!(c.value, 3, "five more periods, then three ticks");
    }

    #[test]
    fn match_flag_is_sticky_until_written_one() {
        let mut t = Ttc::new(0);
        t.write(0x30 + 12, 4, 5).unwrap(); // counter 1, match 1
        t.write(0x0C + 4, 4, u64::from(CNT_MATCH_EN)).unwrap();
        t.tick(6 * 6 * 10);
        assert_ne!(t.read(0x54 + 4, 4).unwrap() & 4, 0);
        assert_ne!(
            t.read(0x54 + 4, 4).unwrap() & 4,
            0,
            "reading does not clear"
        );
        t.write(0x54 + 4, 4, 4).unwrap();
        assert_eq!(t.read(0x54 + 4, 4).unwrap() & 4, 0);
    }

    #[test]
    fn interrupt_line_follows_status_and_enable() {
        let mut t = Ttc::new(0);
        t.write(0x24, 4, 100).unwrap();
        t.write(0x0C, 4, u64::from(CNT_INTERVAL)).unwrap();
        t.tick(6 * 101);
        let mut lines = Vec::new();
        t.irq_lines(&mut lines);
        assert_eq!(
            lines[0],
            IrqInput {
                id: 42,
                cpu: None,
                level: false
            },
            "not enabled"
        );
        t.write(0x60, 4, 1).unwrap();
        lines.clear();
        t.irq_lines(&mut lines);
        assert!(lines[0].level);
        assert_eq!(lines[0].id, 42);
    }

    #[test]
    fn prescaler_divides_the_counter_rate() {
        let mut fast = Ttc::new(0);
        let mut slow = Ttc::new(0);
        fast.write(0x00, 4, 0).unwrap();
        slow.write(0x00, 4, u64::from(CLK_PS_EN | (3 << 1)))
            .unwrap();
        fast.write(0x0C, 4, 0).unwrap();
        slow.write(0x0C, 4, 0).unwrap();
        fast.tick(6 * 1600);
        slow.tick(6 * 1600);
        assert_eq!(fast.counter_value(0), 1600);
        assert_eq!(slow.counter_value(0), 100);
    }
}
