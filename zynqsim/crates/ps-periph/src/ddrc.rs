//! DDR controller (DDRC) model at 0xF8006000.
//!
//! UG585 Chapter 10: Multi-Protocol Dynamic Memory Controller.
//! Reset values and behavior are marked `VERIFY:` against UG585.
//! In particular, the status register at offset `0x54` indicates DDR controller operating mode,
//! where bits [2:0] = 0b001 (or non-zero `0x7`) indicate normal operating mode.

use mem_sys::{Device, lane_merge, lane_read};
use ps_bus::{BusFault, BusResult};
use std::collections::BTreeMap;

/// DDR controller register block at 0xF8006000.
pub struct DdrC {
    values: BTreeMap<u64, u32>,
}

impl Default for DdrC {
    fn default() -> Self {
        Self::new()
    }
}

impl DdrC {
    /// Creates a new DDR controller model with operating mode set to normal (0x7).
    pub fn new() -> Self {
        let mut values = BTreeMap::new();
        // VERIFY: 0x54 status_reg: operating mode bits [2:0] = 0b111 (normal).
        values.insert(0x54, 0x0000_0007);
        DdrC { values }
    }
}

impl Device for DdrC {
    fn read(&mut self, offset: u64, size: u8) -> BusResult<u64> {
        if offset >= 0x1000 {
            return Err(BusFault::Unmapped(offset));
        }
        let a = offset & !3;
        let v = self.values.get(&a).copied().unwrap_or(0);
        lane_read(offset, size, v)
    }

    fn write(&mut self, offset: u64, size: u8, value: u64) -> BusResult<()> {
        if offset >= 0x1000 {
            return Err(BusFault::Unmapped(offset));
        }
        let a = offset & !3;
        let cur = self.values.get(&a).copied().unwrap_or(0);
        let (_, w) = lane_merge(offset, size, value, cur)?;
        self.values.insert(a, w);
        Ok(())
    }

    fn save_state(&self, out: &mut Vec<u8>) {
        for (k, v) in &self.values {
            out.extend_from_slice(&k.to_le_bytes());
            out.extend_from_slice(&v.to_le_bytes());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_reg_indicates_normal_mode() {
        let mut ddrc = DdrC::new();
        let val = ddrc.read(0x54, 4).unwrap();
        assert_eq!(val, 7);
    }

    #[test]
    fn ddrc_stores_writes() {
        let mut ddrc = DdrC::new();
        ddrc.write(0x10, 4, 0x1234_5678).unwrap();
        assert_eq!(ddrc.read(0x10, 4).unwrap(), 0x1234_5678);
    }
}
