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
//!
//! The SCU (`VERIFY:` against UG585 ch.2): control (bit 0 enables the SCU), configuration
//! (CPU count minus one in bits 1:0), CPU status (every core in normal mode), invalidate-all
//! (accepted; no tag RAM is
//! modelled, so it only counts), filter start and end, and the secure and non-secure access
//! control registers. The filter and access registers are stored and read back; the bus
//! itself is not filtered. Snooping is not modelled: DDR and the on-chip RAM are one coherent
//! store for both cores.

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
const SCU_CONFIG: u64 = 0x04;
const SCU_STATUS: u64 = 0x08;
const SCU_INVALIDATE_ALL: u64 = 0x0C;
const SCU_FILTER_START: u64 = 0x40;
const SCU_FILTER_END: u64 = 0x44;
const SCU_ACCESS: u64 = 0x50;
const SCU_NS_ACCESS: u64 = 0x54;

/// SCU configuration: the number of cores minus one in bits 1:0. `VERIFY:` the other bits.
const SCU_CONFIG_VALUE: u32 = (NCPU as u32) - 1;

/// The SCU registers that hold state.
#[derive(Clone, Copy, Debug, Default)]
struct Scu {
    ctrl: u32,
    filter_start: u32,
    filter_end: u32,
    access: u32,
    ns_access: u32,
    /// Invalidate-all writes accepted (no tag RAM to clear).
    invalidations: u64,
}

/// The MPCore private region.
pub struct MpCore {
    cur: usize,
    scu: Scu,
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
            scu: Scu::default(),
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

    /// Whether the SCU is enabled (SCU control bit 0).
    pub fn scu_enabled(&self) -> bool {
        self.scu.ctrl & 1 != 0
    }

    /// How many invalidate-all writes the SCU has accepted.
    pub fn scu_invalidations(&self) -> u64 {
        self.scu.invalidations
    }
}

impl Device for MpCore {
    fn read(&mut self, offset: u64, size: u8) -> BusResult<u64> {
        match offset {
            0x000..=0x0FF => {
                let w = match offset & !3 {
                    SCU_CTRL => self.scu.ctrl,
                    SCU_CONFIG => SCU_CONFIG_VALUE,
                    SCU_STATUS => 0, // every core in normal mode
                    SCU_FILTER_START => self.scu.filter_start,
                    SCU_FILTER_END => self.scu.filter_end,
                    SCU_ACCESS => self.scu.access,
                    SCU_NS_ACCESS => self.scu.ns_access,
                    _ => 0, // invalidate-all reads as zero
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
                let reg = offset & !3;
                match reg {
                    SCU_CTRL => {
                        let (_, w) = lane_merge(offset, size, value, self.scu.ctrl)?;
                        self.scu.ctrl = w & 1;
                    }
                    SCU_FILTER_START => {
                        let (_, w) = lane_merge(offset, size, value, self.scu.filter_start)?;
                        self.scu.filter_start = w & 0xFFF0_0000;
                    }
                    SCU_FILTER_END => {
                        let (_, w) = lane_merge(offset, size, value, self.scu.filter_end)?;
                        self.scu.filter_end = w & 0xFFF0_0000;
                    }
                    SCU_ACCESS => {
                        let (_, w) = lane_merge(offset, size, value, self.scu.access)?;
                        self.scu.access = w;
                    }
                    SCU_NS_ACCESS => {
                        let (_, w) = lane_merge(offset, size, value, self.scu.ns_access)?;
                        self.scu.ns_access = w;
                    }
                    SCU_INVALIDATE_ALL => {
                        // Checked for the size and alignment, then counted: no tags exist.
                        lane_merge(offset, size, value, 0)?;
                        self.scu.invalidations += 1;
                    }
                    _ => {
                        // Configuration and status are read-only.
                        lane_merge(offset, size, value, 0)?;
                    }
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
        let s = &self.scu;
        for v in [s.ctrl, s.filter_start, s.filter_end, s.access, s.ns_access] {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out.extend_from_slice(&s.invalidations.to_le_bytes());
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

#[cfg(test)]
mod tests {
    use super::*;

    fn w(m: &mut MpCore, off: u64, v: u64) {
        m.write(off, 4, v).expect("SCU write");
    }

    #[test]
    fn scu_control_config_and_invalidate_all() {
        let mut m = MpCore::new();
        assert_eq!(m.read(SCU_CONFIG, 4), Ok(1), "two CPUs");
        assert!(!m.scu_enabled());
        w(&mut m, SCU_CTRL, 0xFFFF_FFFF);
        assert_eq!(m.read(SCU_CTRL, 4), Ok(1), "only the enable bit is kept");
        assert!(m.scu_enabled());
        w(&mut m, SCU_INVALIDATE_ALL, 0xFFFF);
        w(&mut m, SCU_INVALIDATE_ALL, 0xFFFF);
        assert_eq!(m.scu_invalidations(), 2);
        assert_eq!(m.read(SCU_INVALIDATE_ALL, 4), Ok(0));
        assert_eq!(m.read(SCU_STATUS, 4), Ok(0));
        // Configuration and status ignore writes.
        w(&mut m, SCU_CONFIG, 0);
        assert_eq!(m.read(SCU_CONFIG, 4), Ok(1));
    }

    #[test]
    fn scu_filter_and_access_registers_store_their_fields() {
        let mut m = MpCore::new();
        w(&mut m, SCU_FILTER_START, 0x1234_5678);
        w(&mut m, SCU_FILTER_END, 0xABCD_EF01);
        w(&mut m, SCU_ACCESS, 0x0000_1F00);
        w(&mut m, SCU_NS_ACCESS, 0x0000_0C00);
        assert_eq!(
            m.read(SCU_FILTER_START, 4),
            Ok(0x1230_0000),
            "bits 31:20 only"
        );
        assert_eq!(m.read(SCU_FILTER_END, 4), Ok(0xABC0_0000));
        assert_eq!(m.read(SCU_ACCESS, 4), Ok(0x1F00));
        assert_eq!(m.read(SCU_NS_ACCESS, 4), Ok(0x0C00));
        // A byte write touches only its lane.
        m.write(SCU_ACCESS, 1, 0x7).expect("byte write");
        assert_eq!(m.read(SCU_ACCESS, 4), Ok(0x1F07));
    }

    #[test]
    fn the_gic_cpu_interface_is_banked_by_the_accessing_core() {
        let mut m = MpCore::new();
        m.select_cpu(0);
        m.write(0x104, 4, 0xF0).expect("core 0 PMR");
        m.select_cpu(1);
        assert_eq!(m.read(0x104, 4), Ok(0), "core 1 PMR is still reset");
        m.select_cpu(0);
        assert_eq!(m.read(0x104, 4), Ok(0xF0));
    }
}
