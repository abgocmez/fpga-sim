//! System-level control registers (SLCR) at 0xF8000000: lock and unlock, the PLL and clock
//! control registers, the reset controls, the level shifters and OCM_CFG.
//!
//! Lock: the SLCR starts locked. Writing 0xDF0D to UNLOCK (0x008) unlocks it, 0x767B to LOCK
//! (0x004) locks it, and LOCKSTA (0x00C) reads 1 while locked. While locked, the protected
//! registers ignore writes. Other keys are ignored.
//!
//! Reset values are as remembered from UG585 ch.4 and ch.25; the table in `REGS` marks each
//! one `VERIFY:` where it was not checked. Registers the table does not name read zero and
//! ignore writes, except the MIO pin range, which is stored.

use mem_sys::{Device, lane_merge, lane_read};
use ps_bus::{BusFault, BusResult};
use std::collections::BTreeMap;

/// Key written to LOCK (0x004) to lock the SLCR.
pub const LOCK_KEY: u32 = 0x767B;
/// Key written to UNLOCK (0x008) to unlock the SLCR.
pub const UNLOCK_KEY: u32 = 0xDF0D;

const LOCK: u64 = 0x004;
const UNLOCK: u64 = 0x008;
const LOCKSTA: u64 = 0x00C;
const PSS_RST_CTRL: u64 = 0x200;

/// One named SLCR register.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SlcrReg {
    /// Offset from 0xF8000000.
    pub offset: u64,
    /// Register name as in UG585.
    pub name: &'static str,
    /// Value at reset.
    pub reset: u32,
    /// Bits a write changes (zero for read-only registers).
    pub write_mask: u32,
    /// Whether the SLCR lock blocks writes.
    pub protected: bool,
}

/// The named registers. Reset values and masks are `VERIFY:` against UG585.
pub const REGS: &[SlcrReg] = &[
    SlcrReg {
        offset: 0x100,
        name: "ARM_PLL_CTRL",
        reset: 0x0001_A008,
        write_mask: 0xFFFF_FFFF,
        protected: true,
    },
    SlcrReg {
        offset: 0x104,
        name: "DDR_PLL_CTRL",
        reset: 0x0001_A008,
        write_mask: 0xFFFF_FFFF,
        protected: true,
    },
    SlcrReg {
        offset: 0x108,
        name: "IO_PLL_CTRL",
        reset: 0x0001_A008,
        write_mask: 0xFFFF_FFFF,
        protected: true,
    },
    SlcrReg {
        offset: 0x10C,
        name: "PLL_STATUS",
        reset: 0x0000_003F,
        write_mask: 0,
        protected: false,
    },
    SlcrReg {
        offset: 0x110,
        name: "ARM_PLL_CFG",
        reset: 0x0001_4000,
        write_mask: 0xFFFF_FFFF,
        protected: true,
    },
    SlcrReg {
        offset: 0x120,
        name: "ARM_CLK_CTRL",
        reset: 0x1F00_0400,
        write_mask: 0xFFFF_FFFF,
        protected: true,
    },
    SlcrReg {
        offset: 0x124,
        name: "DDR_CLK_CTRL",
        reset: 0x1840_0003,
        write_mask: 0xFFFF_FFFF,
        protected: true,
    },
    SlcrReg {
        offset: 0x140,
        name: "GEM0_CLK_CTRL",
        reset: 0x0000_0F03,
        write_mask: 0xFFFF_FFFF,
        protected: true,
    },
    SlcrReg {
        offset: 0x154,
        name: "UART_CLK_CTRL",
        reset: 0x0000_0F03,
        write_mask: 0xFFFF_FFFF,
        protected: true,
    },
    SlcrReg {
        offset: 0x200,
        name: "PSS_RST_CTRL",
        reset: 0x0000_0000,
        write_mask: 0x0000_0001,
        protected: true,
    },
    SlcrReg {
        offset: 0x240,
        name: "FPGA_RST_CTRL",
        reset: 0x0000_0000,
        write_mask: 0x01F0_000F,
        protected: true,
    },
    SlcrReg {
        offset: 0x244,
        name: "CPU_RST_CTRL",
        reset: 0x0000_0000,
        write_mask: 0x0000_0033,
        protected: true,
    },
    SlcrReg {
        offset: 0x900,
        name: "LVL_SHFTR_EN",
        reset: 0x0000_0000,
        write_mask: 0x0000_000F,
        protected: true,
    },
    SlcrReg {
        offset: 0x910,
        name: "OCM_CFG",
        reset: 0x0000_0000,
        write_mask: 0x0000_001F,
        protected: true,
    },
    SlcrReg {
        offset: 0xB74,
        name: "DDRIOB_DCI_STATUS",
        reset: 0x0000_2000,
        write_mask: 0,
        protected: false,
    },
];

/// The SLCR register file.
pub struct Slcr {
    locked: bool,
    /// Current values of the named registers and of the MIO pin range.
    values: BTreeMap<u64, u32>,
    /// The last PSS_RST_CTRL.SOFT_RST write. The model does not reset the machine.
    soft_reset_requested: bool,
}

impl Default for Slcr {
    fn default() -> Self {
        Slcr::new()
    }
}

impl Slcr {
    /// An SLCR in reset state: locked, every register at its reset value.
    pub fn new() -> Slcr {
        let values = REGS.iter().map(|r| (r.offset, r.reset)).collect();
        Slcr {
            locked: true,
            values,
            soft_reset_requested: false,
        }
    }

    /// Whether the SLCR is locked.
    pub fn is_locked(&self) -> bool {
        self.locked
    }

    /// The value of a register by offset, or zero if the model does not hold it.
    pub fn value(&self, offset: u64) -> u32 {
        self.values.get(&offset).copied().unwrap_or(0)
    }

    /// Whether PSS_RST_CTRL.SOFT_RST has been written with bit 0 set.
    pub fn soft_reset_requested(&self) -> bool {
        self.soft_reset_requested
    }

    fn spec(offset: u64) -> Option<&'static SlcrReg> {
        REGS.iter().find(|r| r.offset == offset)
    }

    fn word(&self, a: u64) -> u32 {
        match a {
            LOCKSTA => u32::from(self.locked),
            LOCK | UNLOCK => 0,
            _ => self.value(a),
        }
    }
}

impl Device for Slcr {
    fn read(&mut self, offset: u64, size: u8) -> BusResult<u64> {
        if offset >= 0x1000 {
            return Err(BusFault::Unmapped(offset));
        }
        lane_read(offset, size, self.word(offset & !3))
    }

    fn write(&mut self, offset: u64, size: u8, value: u64) -> BusResult<()> {
        if offset >= 0x1000 {
            return Err(BusFault::Unmapped(offset));
        }
        let a = offset & !3;
        let (a, w) = lane_merge(offset, size, value, self.word(a))?;
        match a {
            LOCK if w & 0xFFFF == LOCK_KEY => self.locked = true,
            UNLOCK if w & 0xFFFF == UNLOCK_KEY => self.locked = false,
            _ => {
                if let Some(spec) = Self::spec(a) {
                    if spec.write_mask != 0 && !(spec.protected && self.locked) {
                        let v = (self.value(a) & !spec.write_mask) | (w & spec.write_mask);
                        self.values.insert(a, v);
                        if a == PSS_RST_CTRL {
                            self.soft_reset_requested = v & 1 != 0;
                        }
                    }
                } else if !self.locked {
                    self.values.insert(a, w);
                }
            }
        }
        Ok(())
    }

    fn save_state(&self, out: &mut Vec<u8>) {
        out.push(u8::from(self.locked));
        out.push(u8::from(self.soft_reset_requested));
        for (k, v) in &self.values {
            out.extend_from_slice(&k.to_le_bytes());
            out.extend_from_slice(&v.to_le_bytes());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w32(s: &mut Slcr, off: u64, v: u32) {
        s.write(off, 4, u64::from(v)).unwrap();
    }

    fn r32(s: &mut Slcr, off: u64) -> u32 {
        s.read(off, 4).unwrap() as u32
    }

    #[test]
    fn reset_state_is_locked_with_the_reset_values() {
        let mut s = Slcr::new();
        assert_eq!(r32(&mut s, LOCKSTA), 1);
        assert_eq!(r32(&mut s, 0x100), 0x0001_A008);
        assert_eq!(r32(&mut s, 0x10C), 0x3F);
        assert_eq!(r32(&mut s, 0x7F0), 0, "unnamed register reads zero");
    }

    #[test]
    fn protected_registers_ignore_writes_while_locked() {
        let mut s = Slcr::new();
        w32(&mut s, 0x120, 0x1234_5678);
        assert_eq!(r32(&mut s, 0x120), 0x1F00_0400);
        w32(&mut s, UNLOCK, 0xDF0D);
        assert_eq!(r32(&mut s, LOCKSTA), 0);
        w32(&mut s, 0x120, 0x1234_5678);
        assert_eq!(r32(&mut s, 0x120), 0x1234_5678);
        w32(&mut s, LOCK, 0x767B);
        w32(&mut s, 0x120, 0);
        assert_eq!(r32(&mut s, 0x120), 0x1234_5678);
    }

    #[test]
    fn wrong_keys_do_nothing() {
        let mut s = Slcr::new();
        w32(&mut s, UNLOCK, 0x767B);
        assert!(s.is_locked());
        w32(&mut s, UNLOCK, 0xDF0D);
        w32(&mut s, LOCK, 0xDF0D);
        assert!(!s.is_locked());
    }

    #[test]
    fn fpga_reset_and_level_shifter_bits_are_masked() {
        let mut s = Slcr::new();
        w32(&mut s, UNLOCK, UNLOCK_KEY);
        w32(&mut s, 0x900, 0xFFFF_FFFF);
        assert_eq!(r32(&mut s, 0x900), 0xF);
        w32(&mut s, 0x240, 0xFFFF_FFFF);
        assert_eq!(r32(&mut s, 0x240), 0x01F0_000F);
        w32(&mut s, 0x240, 0);
        assert_eq!(r32(&mut s, 0x240), 0);
    }

    #[test]
    fn soft_reset_is_recorded_not_performed() {
        let mut s = Slcr::new();
        w32(&mut s, UNLOCK, UNLOCK_KEY);
        w32(&mut s, 0x200, 1);
        assert!(s.soft_reset_requested());
        assert_eq!(r32(&mut s, 0x200), 1);
    }

    #[test]
    fn byte_writes_to_a_register_keep_the_other_lanes() {
        let mut s = Slcr::new();
        w32(&mut s, UNLOCK, UNLOCK_KEY);
        w32(&mut s, 0x900, 0x3);
        s.write(0x901, 1, 0xFF).unwrap();
        assert_eq!(r32(&mut s, 0x900), 0x3, "lane 1 is outside the 4-bit field");
    }

    #[test]
    fn mio_range_is_stored_only_when_unlocked() {
        let mut s = Slcr::new();
        w32(&mut s, 0x700, 0x1600);
        assert_eq!(r32(&mut s, 0x700), 0);
        w32(&mut s, UNLOCK, UNLOCK_KEY);
        w32(&mut s, 0x700, 0x1600);
        assert_eq!(r32(&mut s, 0x700), 0x1600);
    }

    #[test]
    fn offsets_beyond_the_block_fault() {
        let mut s = Slcr::new();
        assert_eq!(s.read(0x1000, 4), Err(BusFault::Unmapped(0x1000)));
    }
}
