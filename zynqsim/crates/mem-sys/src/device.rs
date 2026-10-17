//! The device interface the system bus dispatches to, and the register-access helpers
//! that device models share.

use ps_bus::{BusFault, BusResult};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

/// The level of one interrupt line, as a device reports it or an interrupt controller
/// receives it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IrqInput {
    /// Interrupt ID: SGI 0..=15, PPI 16..=31, SPI 32..=95 on Zynq-7000.
    pub id: u16,
    /// The core a private (PPI) line belongs to; `None` for a shared (SPI) line.
    pub cpu: Option<u8>,
    /// Whether the line is asserted.
    pub level: bool,
}

/// A memory-mapped device on the [`crate::SystemBus`].
///
/// Offsets are relative to the base of the region the device is attached at. `read` and
/// `write` are called with sizes 1, 2 or 4; the bus splits an 8-byte access into two 4-byte
/// ones. A fault returned by a device carries the offset; the bus reports the absolute
/// address.
pub trait Device {
    /// Reads `size` bytes at `offset`, little endian, zero-extended.
    fn read(&mut self, offset: u64, size: u8) -> BusResult<u64>;
    /// Writes the low `size` bytes of `value` at `offset`.
    fn write(&mut self, offset: u64, size: u8, value: u64) -> BusResult<()>;
    /// Called before each access by core `cpu`, so that banked registers select their bank.
    fn select_cpu(&mut self, _cpu: usize) {}
    /// Advances the device by `cycles` CPU clock cycles (the Cortex-A9 runs at 666.67 MHz).
    fn tick(&mut self, _cycles: u64) {}
    /// Appends the current level of every interrupt line this device drives, both levels,
    /// so that a deasserted line is reported too.
    fn irq_lines(&self, _out: &mut Vec<IrqInput>) {}
    /// Receives the interrupt lines of all devices after a tick or a write. Only interrupt
    /// controllers use it.
    fn accept_irqs(&mut self, _lines: &[IrqInput]) {}
    /// Whether the interrupt output towards core `cpu` is asserted. Only interrupt
    /// controllers answer `true`.
    fn core_irq(&self, _cpu: usize) -> bool {
        false
    }
    /// Appends the state that determinism tests hash.
    fn save_state(&self, _out: &mut Vec<u8>) {}
}

/// Checks that a register access has a supported size and natural alignment.
fn check_access(offset: u64, size: u8) -> BusResult<()> {
    if !matches!(size, 1 | 2 | 4) || offset % u64::from(size) != 0 {
        return Err(BusFault::BadAccess(offset));
    }
    Ok(())
}

/// The bit mask of a `size`-byte lane inside a 32-bit word.
fn lane_mask(size: u8) -> u32 {
    match size {
        1 => 0xFF,
        2 => 0xFFFF,
        _ => u32::MAX,
    }
}

/// Extracts the bytes a read of `size` at `offset` sees from the aligned 32-bit register
/// `word` (the register at `offset & !3`).
pub fn lane_read(offset: u64, size: u8, word: u32) -> BusResult<u64> {
    check_access(offset, size)?;
    let shift = ((offset & 3) * 8) as u32;
    Ok(u64::from((word >> shift) & lane_mask(size)))
}

/// Merges a write of `size` bytes at `offset` into `cur`, the value of the aligned register
/// that the write does not overwrite. Returns the aligned offset and the merged word.
///
/// Pass `cur = 0` for write-1-to-clear, write-1-to-set and pulse registers, so that the
/// lanes a write does not touch cannot change them.
pub fn lane_merge(offset: u64, size: u8, value: u64, cur: u32) -> BusResult<(u64, u32)> {
    check_access(offset, size)?;
    let shift = ((offset & 3) * 8) as u32;
    let mask = lane_mask(size) << shift;
    let data = ((value as u32) << shift) & mask;
    Ok((offset & !3, (cur & !mask) | data))
}

/// A device shared between the bus and the host, so that tests and the host can reach its
/// state (for example a UART's transmit log) while the bus owns the region.
///
/// The bus and the host take the lock one at a time. Lock-based sharing keeps the PS `Send`,
/// so that the native engine can move to the thread of a simulation server (`Engine: Send`).
pub struct Shared<D: Device>(pub Arc<Mutex<D>>);

/// Wraps `dev` in a [`Shared`] and returns the handle the host keeps.
pub fn shared<D: Device>(dev: D) -> (Shared<D>, Arc<Mutex<D>>) {
    let arc = Arc::new(Mutex::new(dev));
    (Shared(Arc::clone(&arc)), arc)
}

impl<D: Device> Shared<D> {
    fn lock(&self) -> MutexGuard<'_, D> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl<D: Device + Send> Device for Shared<D> {
    fn read(&mut self, offset: u64, size: u8) -> BusResult<u64> {
        self.lock().read(offset, size)
    }
    fn write(&mut self, offset: u64, size: u8, value: u64) -> BusResult<()> {
        self.lock().write(offset, size, value)
    }
    fn select_cpu(&mut self, cpu: usize) {
        self.lock().select_cpu(cpu);
    }
    fn tick(&mut self, cycles: u64) {
        self.lock().tick(cycles);
    }
    fn irq_lines(&self, out: &mut Vec<IrqInput>) {
        self.lock().irq_lines(out);
    }
    fn accept_irqs(&mut self, lines: &[IrqInput]) {
        self.lock().accept_irqs(lines);
    }
    fn core_irq(&self, cpu: usize) -> bool {
        self.lock().core_irq(cpu)
    }
    fn save_state(&self, out: &mut Vec<u8>) {
        self.lock().save_state(out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lanes_read_and_merge_as_bytes() {
        let word = 0xDEAD_BEEF;
        assert_eq!(lane_read(0, 1, word), Ok(0xEF));
        assert_eq!(lane_read(3, 1, word), Ok(0xDE));
        assert_eq!(lane_read(2, 2, word), Ok(0xDEAD));
        assert_eq!(lane_read(0, 4, word), Ok(0xDEAD_BEEF));
        assert_eq!(lane_read(2, 4, word), Err(BusFault::BadAccess(2)));
        assert_eq!(lane_read(1, 2, word), Err(BusFault::BadAccess(1)));
        assert_eq!(lane_read(0, 8, word), Err(BusFault::BadAccess(0)));
    }

    #[test]
    fn merge_keeps_untouched_lanes() {
        assert_eq!(lane_merge(0x1, 1, 0xAB, 0x1122_3344), Ok((0, 0x1122_AB44)));
        assert_eq!(lane_merge(0x4, 4, 0x5, 0xFFFF_FFFF), Ok((0x4, 0x5)));
        // A write-1-to-clear register merges against zero: untouched lanes stay clear.
        assert_eq!(lane_merge(0x0, 1, 0x01, 0), Ok((0, 0x01)));
    }
}
