//! The physical bus the native PS engine (`cpu-a9`, `mem-sys`, `ps-periph`) shares.
//!
//! A [`Bus`] is what a CPU core sees: little-endian reads and writes of 1, 2, 4 or 8 bytes at
//! physical addresses. Devices and memories implement it; a failed access is a [`BusFault`]
//! the core turns into a data or prefetch abort. [`RamBus`] is a flat RAM for tests.

/// Why a bus access failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BusFault {
    /// No device answers at this address.
    Unmapped(u64),
    /// The device refuses this access size or alignment.
    BadAccess(u64),
}

/// Result of a bus access.
pub type BusResult<T> = Result<T, BusFault>;

/// A physical address space seen by one CPU core.
pub trait Bus {
    /// Reads `size` (1, 2, 4 or 8) bytes at `addr`, little endian, zero-extended.
    fn read(&mut self, addr: u64, size: u8) -> BusResult<u64>;
    /// Writes the low `size` (1, 2, 4 or 8) bytes of `value` at `addr`, little endian.
    fn write(&mut self, addr: u64, size: u8, value: u64) -> BusResult<()>;

    /// Reads a 32-bit word.
    fn read32(&mut self, addr: u64) -> BusResult<u32> {
        self.read(addr, 4).map(|v| v as u32)
    }
    /// Writes a 32-bit word.
    fn write32(&mut self, addr: u64, value: u32) -> BusResult<()> {
        self.write(addr, 4, u64::from(value))
    }
}

/// A flat RAM at `base`, for tests.
pub struct RamBus {
    base: u64,
    bytes: Vec<u8>,
}

impl RamBus {
    /// `len` zeroed bytes at `base`.
    pub fn new(base: u64, len: usize) -> RamBus {
        RamBus {
            base,
            bytes: vec![0; len],
        }
    }

    /// Copies `data` to `addr` (panics outside the RAM: test helper).
    pub fn load(&mut self, addr: u64, data: &[u8]) {
        let o = (addr - self.base) as usize;
        self.bytes[o..o + data.len()].copy_from_slice(data);
    }

    fn range(&self, addr: u64, size: u8) -> BusResult<std::ops::Range<usize>> {
        if !matches!(size, 1 | 2 | 4 | 8) {
            return Err(BusFault::BadAccess(addr));
        }
        let o = addr
            .checked_sub(self.base)
            .ok_or(BusFault::Unmapped(addr))? as usize;
        let end = o + usize::from(size);
        if end > self.bytes.len() {
            return Err(BusFault::Unmapped(addr));
        }
        Ok(o..end)
    }
}

impl Bus for RamBus {
    fn read(&mut self, addr: u64, size: u8) -> BusResult<u64> {
        let r = self.range(addr, size)?;
        let mut v = 0u64;
        for (i, b) in self.bytes[r].iter().enumerate() {
            v |= u64::from(*b) << (8 * i);
        }
        Ok(v)
    }

    fn write(&mut self, addr: u64, size: u8, value: u64) -> BusResult<()> {
        let r = self.range(addr, size)?;
        for (i, b) in self.bytes[r].iter_mut().enumerate() {
            *b = (value >> (8 * i)) as u8;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ram_is_little_endian_and_bounded() {
        let mut m = RamBus::new(0x1000, 16);
        m.write32(0x1004, 0xdead_beef).unwrap();
        assert_eq!(m.read(0x1004, 1).unwrap(), 0xef);
        assert_eq!(m.read(0x1005, 2).unwrap(), 0xbead);
        assert_eq!(m.read32(0x1004).unwrap(), 0xdead_beef);
        assert_eq!(m.read32(0x1010), Err(BusFault::Unmapped(0x1010)));
        assert_eq!(m.read(0x1000, 3), Err(BusFault::BadAccess(0x1000)));
        assert_eq!(m.read32(0xfff), Err(BusFault::Unmapped(0xfff)));
    }
}
