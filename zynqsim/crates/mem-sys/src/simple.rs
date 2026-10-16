//! Two small devices: a named stub that reads zero, and a read-only memory image.

use crate::device::Device;
use ps_bus::{BusFault, BusResult};

/// A named placeholder for a block whose registers are not modelled. Reads return zero and
/// writes are ignored. Its name appears in the map table, so the gap is visible.
pub struct Stub {
    name: &'static str,
}

impl Stub {
    /// A stub called `name`.
    pub fn new(name: &'static str) -> Stub {
        Stub { name }
    }

    /// The name given at construction.
    pub fn name(&self) -> &'static str {
        self.name
    }
}

impl Device for Stub {
    fn read(&mut self, offset: u64, size: u8) -> BusResult<u64> {
        if !matches!(size, 1 | 2 | 4) {
            return Err(BusFault::BadAccess(offset));
        }
        Ok(0)
    }

    fn write(&mut self, offset: u64, size: u8, _value: u64) -> BusResult<()> {
        if !matches!(size, 1 | 2 | 4) {
            return Err(BusFault::BadAccess(offset));
        }
        Ok(())
    }
}

/// A read-only memory image mapped at a base address, such as a QSPI flash in linear mode.
/// Bytes past the end of the image read `0xFF`, the state of erased NOR flash. Writes are
/// ignored, as a ROM ignores them.
pub struct ReadOnlyImage {
    data: Vec<u8>,
}

impl ReadOnlyImage {
    /// An image with the given contents.
    pub fn new(data: Vec<u8>) -> ReadOnlyImage {
        ReadOnlyImage { data }
    }

    /// Length of the image.
    pub fn len(&self) -> usize {
        self.data.len()
    }

    /// Whether the image is empty.
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }
}

impl Device for ReadOnlyImage {
    fn read(&mut self, offset: u64, size: u8) -> BusResult<u64> {
        if !matches!(size, 1 | 2 | 4 | 8) {
            return Err(BusFault::BadAccess(offset));
        }
        let mut v = 0u64;
        for i in 0..u64::from(size) {
            let b = self
                .data
                .get((offset + i) as usize)
                .copied()
                .unwrap_or(0xFF);
            v |= u64::from(b) << (8 * i);
        }
        Ok(v)
    }

    fn write(&mut self, offset: u64, size: u8, _value: u64) -> BusResult<()> {
        if !matches!(size, 1 | 2 | 4 | 8) {
            return Err(BusFault::BadAccess(offset));
        }
        Ok(())
    }

    fn save_state(&self, out: &mut Vec<u8>) {
        // The image is an input, not state; its length identifies it in a hash.
        out.extend_from_slice(&(self.data.len() as u64).to_le_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stub_reads_zero_and_refuses_odd_sizes() {
        let mut s = Stub::new("L2C");
        assert_eq!(s.read(0x10, 4), Ok(0));
        assert!(s.write(0x10, 4, 0xFFFF_FFFF).is_ok());
        assert_eq!(s.read(0x10, 3), Err(BusFault::BadAccess(0x10)));
        assert_eq!(s.name(), "L2C");
    }

    #[test]
    fn image_reads_erased_past_its_end_and_ignores_writes() {
        let mut rom = ReadOnlyImage::new(vec![0x11, 0x22]);
        assert_eq!(rom.read(0, 2), Ok(0x2211));
        assert_eq!(rom.read(1, 4), Ok(0xFFFF_FF22));
        rom.write(0, 4, 0).unwrap();
        assert_eq!(rom.read(0, 1), Ok(0x11));
    }
}
