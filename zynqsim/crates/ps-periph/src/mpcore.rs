//! The Cortex-A9 MPCore private region at 0xF8F00000 (0x3000 bytes): the snoop control unit,
//! the GIC CPU interface and distributor, the global timer, and per core a private timer and
//! a watchdog. The L2 cache controller at 0xF8F02000 is a named stub.
//!
//! Banked blocks answer each core with its own state: the bus tells the device which core is
//! accessing it (`select_cpu`), and the CPU interface, the private timer and the watchdog use
//! that core's copy. The global timer and the distributor's shared state are common.
//!
//! Block offsets (relative to 0xF8F00000): SCU 0x000, GIC CPU interface 0x100, global timer
//! 0x200, private timer 0x600, watchdog 0x620, GIC distributor 0x1000, L2C stub 0x2000.

use crate::clock::PERIPH_DIV;
use crate::clock::ticks_of;
use crate::gic::{Gic, NCPU};
use crate::timers::Countdown;
use crate::timers::{
    GLOBAL_TIMER_PPI, GlobalTimer, PRIVATE_TIMER_PPI, WATCHDOG_PPI, Watchdog, private_timer,
};
use mem_sys::{Device, IrqInput, lane_merge, lane_read};
use ps_bus::{BusFault, BusResult};

const SCU_CTRL: u64 = 0x00;
/// SCU configuration: the number of cores minus one in bits 1:0. `VERIFY:` the other bits.
const SCU_CONFIG: u32 = (NCPU as u32) - 1;

/// The MPCore private region.
pub struct MpCore {
    cur: usize,
    scu_ctrl: u32,
    gic: Gic,
    gt: GlobalTimer,
    pt: [Countdown; NCPU],
    wd: [Watchdog; NCPU],
    /// CPU cycles not yet converted to PERIPHCLK ticks.
    acc: u64,
}

impl Default for MpCore {
    fn default() -> Self {
        MpCore::new()
    }
}

impl MpCore {
    /// The region in reset state.
    pub fn new() -> MpCore {
        MpCore {
            cur: 0,
            scu_ctrl: 0,
            gic: Gic::new(),
            gt: GlobalTimer::new(),
            pt: [private_timer(), private_timer()],
            wd: [Watchdog::new(), Watchdog::new()],
            acc: 0,
        }
    }

    /// The interrupt controller, for tests and the host.
    pub fn gic(&self) -> &Gic {
        &self.gic
    }

    /// The global timer, for tests and the host.
    pub fn global_timer(&self) -> &GlobalTimer {
        &self.gt
    }

    /// The private timer of `cpu`, for tests and the host.
    pub fn private_timer(&self, cpu: usize) -> &Countdown {
        &self.pt[cpu.min(NCPU - 1)]
    }
}

impl Device for MpCore {
    fn read(&mut self, offset: u64, size: u8) -> BusResult<u64> {
        match offset {
            0x000..=0x0FF => {
                let w = match offset & !3 {
                    SCU_CTRL => self.scu_ctrl,
                    0x04 => SCU_CONFIG,
                    _ => 0,
                };
                lane_read(offset, size, w)
            }
            0x100..=0x1FF => self.gic.read_cpu(offset - 0x100, size),
            0x200..=0x2FF => self.gt.read(offset - 0x200, size),
            0x600..=0x61F => self.pt[self.cur].read(offset - 0x600, size),
            0x620..=0x63F => self.wd[self.cur].read(offset - 0x620, size),
            0x1000..=0x1FFF => self.gic.read_dist(offset - 0x1000, size),
            0x2000..=0x2FFF => {
                if !matches!(size, 1 | 2 | 4) {
                    return Err(BusFault::BadAccess(offset));
                }
                Ok(0)
            }
            _ => Err(BusFault::Unmapped(offset)),
        }
    }

    fn write(&mut self, offset: u64, size: u8, value: u64) -> BusResult<()> {
        match offset {
            0x000..=0x0FF => {
                if offset & !3 == SCU_CTRL {
                    let (_, w) = lane_merge(offset, size, value, self.scu_ctrl)?;
                    self.scu_ctrl = w & 1;
                } else {
                    lane_merge(offset, size, value, 0)?;
                }
                Ok(())
            }
            0x100..=0x1FF => self.gic.write_cpu(offset - 0x100, size, value),
            0x200..=0x2FF => self.gt.write(offset - 0x200, size, value),
            0x600..=0x61F => self.pt[self.cur].write(offset - 0x600, size, value),
            0x620..=0x63F => self.wd[self.cur].write(offset - 0x620, size, value),
            0x1000..=0x1FFF => self.gic.write_dist(offset - 0x1000, size, value),
            0x2000..=0x2FFF => {
                if !matches!(size, 1 | 2 | 4) {
                    return Err(BusFault::BadAccess(offset));
                }
                Ok(())
            }
            _ => Err(BusFault::Unmapped(offset)),
        }
    }

    fn select_cpu(&mut self, cpu: usize) {
        self.cur = cpu.min(NCPU - 1);
        self.gic.set_cpu(self.cur);
    }

    fn tick(&mut self, cycles: u64) {
        let periph = ticks_of(&mut self.acc, cycles, PERIPH_DIV);
        for c in 0..NCPU {
            self.pt[c].advance(periph);
            self.wd[c].advance(periph);
        }
        self.gt.advance(periph);
    }

    fn irq_lines(&self, out: &mut Vec<IrqInput>) {
        for c in 0..NCPU {
            out.push(IrqInput {
                id: PRIVATE_TIMER_PPI,
                cpu: Some(c as u8),
                level: self.pt[c].irq_asserted(),
            });
            out.push(IrqInput {
                id: WATCHDOG_PPI,
                cpu: Some(c as u8),
                level: self.wd[c].cd.irq_asserted(),
            });
            out.push(IrqInput {
                id: GLOBAL_TIMER_PPI,
                cpu: Some(c as u8),
                level: self.gt.irq_asserted(),
            });
        }
    }

    fn accept_irqs(&mut self, lines: &[IrqInput]) {
        self.gic.accept(lines);
    }

    fn core_irq(&self, cpu: usize) -> bool {
        self.gic.signal(cpu)
    }

    fn save_state(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.scu_ctrl.to_le_bytes());
        out.extend_from_slice(&self.acc.to_le_bytes());
        self.gic.save_state(out);
        self.gt.save_state(out);
        for c in 0..NCPU {
            self.pt[c].save_state(out);
            self.wd[c].cd.save_state(out);
            out.extend_from_slice(&self.wd[c].reset_status.to_le_bytes());
        }
    }
}
