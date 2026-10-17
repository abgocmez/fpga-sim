//! Sparse RAM for DDR: pages of 64 KiB, allocated on the first write.

use std::collections::BTreeMap;

/// Log2 of the allocation granule.
pub const PAGE_SHIFT: u32 = 16;
/// Allocation granule in bytes (64 KiB).
pub const PAGE_SIZE: usize = 1 << PAGE_SHIFT;

/// Byte-addressed RAM that only allocates the pages that are written. Reads of an
/// unallocated page return zero, so a 512 MiB DDR costs nothing until it is used.
#[derive(Default, Clone)]
pub struct SparseRam {
    pages: BTreeMap<u64, Box<[u8]>>,
}

impl SparseRam {
    /// An empty RAM: every byte reads zero.
    pub fn new() -> SparseRam {
        SparseRam::default()
    }

    /// Number of allocated pages.
    pub fn pages_allocated(&self) -> usize {
        self.pages.len()
    }

    /// The byte at `addr`.
    pub fn byte(&self, addr: u64) -> u8 {
        self.pages
            .get(&(addr >> PAGE_SHIFT))
            .map_or(0, |p| p[(addr as usize) & (PAGE_SIZE - 1)])
    }

    /// Stores `value` at `addr`, allocating the page if needed.
    pub fn set_byte(&mut self, addr: u64, value: u8) {
        let page = self
            .pages
            .entry(addr >> PAGE_SHIFT)
            .or_insert_with(|| vec![0u8; PAGE_SIZE].into_boxed_slice());
        page[(addr as usize) & (PAGE_SIZE - 1)] = value;
    }

    /// Reads `size` bytes at `addr`, little endian, zero-extended. `size` must be 1, 2, 4 or 8.
    pub fn read(&self, addr: u64, size: u8) -> u64 {
        // One page lookup for an access inside a page (instruction fetches are the common
        // case); a straddling access takes the byte path.
        let off = (addr as usize) & (PAGE_SIZE - 1);
        if off + usize::from(size) <= PAGE_SIZE {
            return self.pages.get(&(addr >> PAGE_SHIFT)).map_or(0, |p| {
                (0..usize::from(size)).fold(0u64, |v, i| v | (u64::from(p[off + i]) << (8 * i)))
            });
        }
        (0..u64::from(size)).fold(0u64, |v, i| v | (u64::from(self.byte(addr + i)) << (8 * i)))
    }

    /// Writes the low `size` bytes of `value` at `addr`, little endian.
    pub fn write(&mut self, addr: u64, size: u8, value: u64) {
        for i in 0..u64::from(size) {
            self.set_byte(addr + i, (value >> (8 * i)) as u8);
        }
    }

    /// Allocated pages as `(page index, bytes)` in address order.
    pub fn pages(&self) -> impl Iterator<Item = (u64, &[u8])> {
        self.pages.iter().map(|(k, v)| (*k, &v[..]))
    }

    /// Stores a whole allocated page. Used by snapshot restore.
    pub fn put_page(&mut self, index: u64, bytes: &[u8]) {
        self.pages.insert(index, bytes.to_vec().into_boxed_slice());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn untouched_memory_reads_zero_and_allocates_nothing() {
        let mut ram = SparseRam::new();
        assert_eq!(ram.read(0x3FFF_0000, 8), 0);
        assert_eq!(ram.pages_allocated(), 0);
        ram.write(0x1000_0000, 4, 0xDEAD_BEEF);
        assert_eq!(ram.pages_allocated(), 1);
        assert_eq!(ram.read(0x1000_0000, 4), 0xDEAD_BEEF);
        assert_eq!(ram.read(0x1000_0002, 2), 0xDEAD);
    }

    #[test]
    fn accesses_can_straddle_a_page_boundary() {
        let mut ram = SparseRam::new();
        let edge = (PAGE_SIZE as u64) - 2;
        ram.write(edge, 4, 0x0102_0304);
        assert_eq!(ram.pages_allocated(), 2);
        assert_eq!(ram.read(edge, 4), 0x0102_0304);
    }
}
