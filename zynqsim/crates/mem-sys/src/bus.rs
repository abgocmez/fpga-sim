//! The system bus: the PS physical address space, dispatching to DDR, OCM and the attached
//! devices, with the unmapped-access policy, interrupt propagation and state hashing.

use crate::device::{Device, IrqInput};
use crate::ram::{PAGE_SIZE, SparseRam};
use ps_bus::{Bus, BusFault, BusResult};
use std::fmt;

/// Base of the on-chip RAM in its reset mapping (the high mapping of UG585 ch.4).
pub const OCM_BASE: u64 = 0xFFFC_0000;
/// Size of the on-chip RAM.
pub const OCM_SIZE: u64 = 0x4_0000;
/// Largest DDR window, the 1 GiB that addresses 0x0..0x3FFF_FFFF.
pub const DDR_WINDOW: u64 = 0x4000_0000;
/// DDR size when the configuration does not give one (512 MiB).
pub const DDR_DEFAULT: u64 = 0x2000_0000;
/// Number of unmapped accesses kept in the log. Later ones are counted but not stored.
pub const UNMAPPED_LOG_CAP: usize = 1024;

/// What an access to an address with no device does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnmappedPolicy {
    /// The access fails with [`BusFault::Unmapped`] (the core takes an abort).
    Fault,
    /// Reads return zero and writes are dropped, as on hardware with a RAZ/WI slave.
    RazWi,
}

/// One access the bus could not map. Logged whatever the policy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UnmappedAccess {
    /// Absolute address.
    pub addr: u64,
    /// Access size in bytes.
    pub size: u8,
    /// Whether it was a write.
    pub write: bool,
    /// The value written (zero for a read).
    pub value: u64,
}

/// Why a region could not be attached, or the DDR size is wrong.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MapError {
    /// The region has zero length.
    Empty(&'static str),
    /// The region runs past the end of the 64-bit address space.
    Wrap(&'static str),
    /// The region shares an address with another range.
    Overlap {
        /// The region being attached.
        name: &'static str,
        /// The range it collides with.
        with: &'static str,
    },
    /// The DDR size is not a multiple of 64 KiB or exceeds the 1 GiB window.
    DdrSize(u64),
}

impl fmt::Display for MapError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MapError::Empty(n) => write!(f, "region {n} has zero size"),
            MapError::Wrap(n) => write!(f, "region {n} runs past the address space"),
            MapError::Overlap { name, with } => write!(f, "region {name} overlaps {with}"),
            MapError::DdrSize(s) => write!(
                f,
                "DDR size 0x{s:x} must be a multiple of 64 KiB and at most 0x{DDR_WINDOW:x}"
            ),
        }
    }
}

impl std::error::Error for MapError {}

/// Why a RAM snapshot could not be restored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SnapshotError {
    /// The bytes are not a snapshot, or are truncated.
    Format(&'static str),
    /// The snapshot was taken with another DDR size.
    DdrSize {
        /// Size of this bus.
        expected: u64,
        /// Size recorded in the snapshot.
        found: u64,
    },
}

impl fmt::Display for SnapshotError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SnapshotError::Format(why) => write!(f, "bad RAM snapshot: {why}"),
            SnapshotError::DdrSize { expected, found } => {
                write!(
                    f,
                    "RAM snapshot has DDR size 0x{found:x}, this bus has 0x{expected:x}"
                )
            }
        }
    }
}

impl std::error::Error for SnapshotError {}

const SNAPSHOT_MAGIC: &[u8; 8] = b"ZSRAM\x01\0\0";

struct Region {
    name: &'static str,
    base: u64,
    size: u64,
    dev: Box<dyn Device>,
}

/// The physical address space of the PS: DDR from 0, the on-chip RAM at its high mapping,
/// and the devices attached by name at their base addresses.
///
/// The bus is the view of core 0 when used as a [`Bus`]; [`SystemBus::core`] gives the view of
/// any core, so that banked registers answer each core with its own state.
pub struct SystemBus {
    ddr: SparseRam,
    ddr_size: u64,
    ocm: Vec<u8>,
    regions: Vec<Region>,
    policy: UnmappedPolicy,
    unmapped_log: Vec<UnmappedAccess>,
    unmapped_count: u64,
    cycles: u64,
}

/// FNV-1a over bytes, the hash of [`SystemBus::state_hash`].
pub fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

impl SystemBus {
    /// A bus with `ddr_size` bytes of DDR at 0 and the 256 KiB on-chip RAM at `OCM_BASE`.
    /// Unmapped accesses fault.
    pub fn new(ddr_size: u64) -> Result<SystemBus, MapError> {
        if ddr_size == 0 || ddr_size % PAGE_SIZE as u64 != 0 || ddr_size > DDR_WINDOW {
            return Err(MapError::DdrSize(ddr_size));
        }
        Ok(SystemBus {
            ddr: SparseRam::new(),
            ddr_size,
            ocm: vec![0; OCM_SIZE as usize],
            regions: Vec::new(),
            policy: UnmappedPolicy::Fault,
            unmapped_log: Vec::new(),
            unmapped_count: 0,
            cycles: 0,
        })
    }

    /// Sets what unmapped accesses do.
    pub fn set_policy(&mut self, policy: UnmappedPolicy) {
        self.policy = policy;
    }

    /// The current unmapped-access policy.
    pub fn policy(&self) -> UnmappedPolicy {
        self.policy
    }

    /// Size of DDR in bytes.
    pub fn ddr_size(&self) -> u64 {
        self.ddr_size
    }

    /// CPU cycles advanced by [`SystemBus::tick`] so far.
    pub fn cycles(&self) -> u64 {
        self.cycles
    }

    /// Attaches `dev` at `[base, base + size)` under `name`. The range must not overlap DDR,
    /// the on-chip RAM or another region.
    pub fn attach(
        &mut self,
        name: &'static str,
        base: u64,
        size: u64,
        dev: Box<dyn Device>,
    ) -> Result<(), MapError> {
        if size == 0 {
            return Err(MapError::Empty(name));
        }
        let end = base.checked_add(size).ok_or(MapError::Wrap(name))?;
        let overlaps = |a: u64, asz: u64, b: u64, bsz: u64| a < b + bsz && b < a + asz;
        if overlaps(base, size, 0, self.ddr_size) {
            return Err(MapError::Overlap { name, with: "DDR" });
        }
        if overlaps(base, size, OCM_BASE, OCM_SIZE) {
            return Err(MapError::Overlap { name, with: "OCM" });
        }
        for r in &self.regions {
            if overlaps(base, size, r.base, r.size) {
                return Err(MapError::Overlap { name, with: r.name });
            }
        }
        debug_assert!(end > base);
        self.regions.push(Region {
            name,
            base,
            size,
            dev,
        });
        Ok(())
    }

    /// The attached regions as `(name, base, size)`, in attachment order.
    pub fn regions(&self) -> Vec<(&'static str, u64, u64)> {
        self.regions
            .iter()
            .map(|r| (r.name, r.base, r.size))
            .collect()
    }

    /// A view of the bus as core `cpu` sees it.
    pub fn core(&mut self, cpu: usize) -> CoreBus<'_> {
        CoreBus { bus: self, cpu }
    }

    /// Advances every device by `cycles` CPU cycles, then propagates the interrupt lines.
    pub fn tick(&mut self, cycles: u64) {
        self.cycles += cycles;
        for r in &mut self.regions {
            r.dev.tick(cycles);
        }
        self.propagate();
    }

    /// Whether the interrupt output to core `cpu` is asserted.
    pub fn core_irq(&self, cpu: usize) -> bool {
        self.regions.iter().any(|r| r.dev.core_irq(cpu))
    }

    /// Copies `data` into DDR or the on-chip RAM without going through a core (for loading
    /// images). Fails if the range is not entirely in one of them.
    pub fn load(&mut self, addr: u64, data: &[u8]) -> BusResult<()> {
        let len = data.len() as u64;
        if addr.checked_add(len).is_some_and(|e| e <= self.ddr_size) {
            for (i, b) in data.iter().enumerate() {
                self.ddr.set_byte(addr + i as u64, *b);
            }
            return Ok(());
        }
        if addr >= OCM_BASE
            && addr
                .checked_add(len)
                .is_some_and(|e| e <= OCM_BASE + OCM_SIZE)
        {
            let o = (addr - OCM_BASE) as usize;
            self.ocm[o..o + data.len()].copy_from_slice(data);
            return Ok(());
        }
        Err(BusFault::Unmapped(addr))
    }

    /// The first unmapped accesses (up to [`UNMAPPED_LOG_CAP`]), oldest first.
    pub fn unmapped_log(&self) -> &[UnmappedAccess] {
        &self.unmapped_log
    }

    /// Total number of unmapped accesses, including those not stored in the log.
    pub fn unmapped_count(&self) -> u64 {
        self.unmapped_count
    }

    /// The RAM contents (DDR and on-chip RAM) as bytes. Only allocated DDR pages are stored.
    pub fn ram_snapshot(&self) -> Vec<u8> {
        let pages: Vec<(u64, &[u8])> = self.ddr.pages().collect();
        let mut out = Vec::with_capacity(24 + pages.len() * (8 + PAGE_SIZE) + OCM_SIZE as usize);
        out.extend_from_slice(SNAPSHOT_MAGIC);
        out.extend_from_slice(&self.ddr_size.to_le_bytes());
        out.extend_from_slice(&(pages.len() as u64).to_le_bytes());
        for (idx, bytes) in pages {
            out.extend_from_slice(&idx.to_le_bytes());
            out.extend_from_slice(bytes);
        }
        out.extend_from_slice(&self.ocm);
        out
    }

    /// Replaces the RAM contents with a snapshot taken by [`SystemBus::ram_snapshot`]. Device
    /// state is not part of the snapshot.
    pub fn ram_restore(&mut self, bytes: &[u8]) -> Result<(), SnapshotError> {
        let mut cur = SnapshotCursor { bytes, at: 0 };
        if cur.take(8)? != SNAPSHOT_MAGIC {
            return Err(SnapshotError::Format("magic"));
        }
        let found = cur.u64()?;
        if found != self.ddr_size {
            return Err(SnapshotError::DdrSize {
                expected: self.ddr_size,
                found,
            });
        }
        let count = cur.u64()?;
        let mut ddr = SparseRam::new();
        for _ in 0..count {
            let idx = cur.u64()?;
            ddr.put_page(idx, cur.take(PAGE_SIZE)?);
        }
        let ocm = cur.take(OCM_SIZE as usize)?.to_vec();
        if cur.at != bytes.len() {
            return Err(SnapshotError::Format("trailing bytes"));
        }
        self.ddr = ddr;
        self.ocm = ocm;
        Ok(())
    }

    /// A hash of everything that determinism tests compare: RAM, the cycle count, the
    /// unmapped count and each device's [`Device::save_state`], in attachment order.
    pub fn state_hash(&self) -> u64 {
        let mut buf = self.ram_snapshot();
        buf.extend_from_slice(&self.cycles.to_le_bytes());
        buf.extend_from_slice(&self.unmapped_count.to_le_bytes());
        for r in &self.regions {
            buf.extend_from_slice(r.name.as_bytes());
            buf.push(0);
            r.dev.save_state(&mut buf);
        }
        fnv1a64(&buf)
    }

    /// Gathers the interrupt lines of every device and hands the set to every device.
    fn propagate(&mut self) {
        let mut lines: Vec<IrqInput> = Vec::new();
        for r in &self.regions {
            r.dev.irq_lines(&mut lines);
        }
        for r in &mut self.regions {
            r.dev.accept_irqs(&lines);
        }
    }

    /// The one path every access takes: `write` is `None` for a read.
    fn access(&mut self, cpu: usize, addr: u64, size: u8, write: Option<u64>) -> BusResult<u64> {
        if !matches!(size, 1 | 2 | 4 | 8) {
            return Err(BusFault::BadAccess(addr));
        }
        let end = addr.checked_add(u64::from(size));
        // DDR: a straddling access is unmapped.
        if addr < self.ddr_size {
            return match end {
                Some(e) if e <= self.ddr_size => Ok(self.ram_access(addr, size, write)),
                _ => self.unmapped(addr, size, write),
            };
        }
        if (OCM_BASE..OCM_BASE + OCM_SIZE).contains(&addr) {
            return match end {
                Some(e) if e <= OCM_BASE + OCM_SIZE => {
                    let o = (addr - OCM_BASE) as usize;
                    Ok(ocm_access(
                        &mut self.ocm[o..o + usize::from(size)],
                        size,
                        write,
                    ))
                }
                _ => self.unmapped(addr, size, write),
            };
        }
        let Some(i) = self
            .regions
            .iter()
            .position(|r| (r.base..r.base + r.size).contains(&addr))
        else {
            return self.unmapped(addr, size, write);
        };
        let r = &mut self.regions[i];
        if !matches!(end, Some(e) if e <= r.base + r.size) {
            return self.unmapped(addr, size, write);
        }
        let off = addr - r.base;
        r.dev.select_cpu(cpu);
        let res = match write {
            None if size == 8 => r
                .dev
                .read(off, 4)
                .and_then(|lo| r.dev.read(off + 4, 4).map(|hi| lo | (hi << 32))),
            None => r.dev.read(off, size),
            Some(v) if size == 8 => r
                .dev
                .write(off, 4, v & 0xFFFF_FFFF)
                .and_then(|()| r.dev.write(off + 4, 4, v >> 32))
                .map(|()| 0),
            Some(v) => r.dev.write(off, size, v).map(|()| 0),
        };
        match res {
            Ok(v) => {
                if write.is_some() {
                    self.propagate();
                }
                Ok(v)
            }
            Err(BusFault::Unmapped(_)) => self.unmapped(addr, size, write),
            Err(BusFault::BadAccess(_)) => Err(BusFault::BadAccess(addr)),
        }
    }

    /// Logs an unmapped access and applies the policy.
    fn unmapped(&mut self, addr: u64, size: u8, write: Option<u64>) -> BusResult<u64> {
        self.unmapped_count += 1;
        if self.unmapped_log.len() < UNMAPPED_LOG_CAP {
            self.unmapped_log.push(UnmappedAccess {
                addr,
                size,
                write: write.is_some(),
                value: write.unwrap_or(0),
            });
        }
        match self.policy {
            UnmappedPolicy::Fault => Err(BusFault::Unmapped(addr)),
            UnmappedPolicy::RazWi => Ok(0),
        }
    }

    fn ram_access(&mut self, addr: u64, size: u8, write: Option<u64>) -> u64 {
        match write {
            None => self.ddr.read(addr, size),
            Some(v) => {
                self.ddr.write(addr, size, v);
                0
            }
        }
    }
}

/// Little-endian read or write of `bytes` (a slice of the on-chip RAM).
fn ocm_access(bytes: &mut [u8], size: u8, write: Option<u64>) -> u64 {
    match write {
        None => bytes
            .iter()
            .enumerate()
            .fold(0u64, |v, (i, b)| v | (u64::from(*b) << (8 * i))),
        Some(v) => {
            for (i, b) in bytes.iter_mut().enumerate().take(usize::from(size)) {
                *b = (v >> (8 * i)) as u8;
            }
            0
        }
    }
}

impl Bus for SystemBus {
    fn read(&mut self, addr: u64, size: u8) -> BusResult<u64> {
        self.access(0, addr, size, None)
    }
    fn write(&mut self, addr: u64, size: u8, value: u64) -> BusResult<()> {
        self.access(0, addr, size, Some(value)).map(|_| ())
    }
}

/// Reads consecutive fields of a snapshot, failing cleanly on truncation.
struct SnapshotCursor<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> SnapshotCursor<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], SnapshotError> {
        let s = self
            .bytes
            .get(self.at..self.at + n)
            .ok_or(SnapshotError::Format("truncated"))?;
        self.at += n;
        Ok(s)
    }

    fn u64(&mut self) -> Result<u64, SnapshotError> {
        let s = self.take(8)?;
        Ok(u64::from_le_bytes(
            s.try_into().expect("take(8) returns 8 bytes"),
        ))
    }
}

/// The view of the bus that one core sees. Implements [`Bus`] for that core.
pub struct CoreBus<'a> {
    bus: &'a mut SystemBus,
    cpu: usize,
}

impl CoreBus<'_> {
    /// The core this view belongs to.
    pub fn cpu(&self) -> usize {
        self.cpu
    }
}

impl Bus for CoreBus<'_> {
    fn read(&mut self, addr: u64, size: u8) -> BusResult<u64> {
        self.bus.access(self.cpu, addr, size, None)
    }
    fn write(&mut self, addr: u64, size: u8, value: u64) -> BusResult<()> {
        self.bus
            .access(self.cpu, addr, size, Some(value))
            .map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::simple::Stub;

    /// A device that records its last write and counts ticks.
    #[derive(Default)]
    struct Probe {
        last: u64,
        ticks: u64,
        cpu: usize,
    }

    impl Device for Probe {
        fn read(&mut self, offset: u64, size: u8) -> BusResult<u64> {
            if offset > 0x10 {
                return Err(BusFault::Unmapped(offset));
            }
            Ok(self.last ^ offset ^ (u64::from(size) << 56) ^ (self.cpu as u64))
        }
        fn write(&mut self, _offset: u64, _size: u8, value: u64) -> BusResult<()> {
            self.last = value;
            Ok(())
        }
        fn select_cpu(&mut self, cpu: usize) {
            self.cpu = cpu;
        }
        fn tick(&mut self, cycles: u64) {
            self.ticks += cycles;
        }
        fn save_state(&self, out: &mut Vec<u8>) {
            out.extend_from_slice(&self.last.to_le_bytes());
        }
    }

    fn bus() -> SystemBus {
        let mut b = SystemBus::new(0x1000_0000).unwrap();
        b.attach("PROBE", 0xE000_0000, 0x100, Box::new(Probe::default()))
            .unwrap();
        b
    }

    #[test]
    fn ddr_and_ocm_store_bytes_and_bounds_are_enforced() {
        let mut b = bus();
        b.write(0x0FFF_FFFC, 4, 0x1122_3344).unwrap();
        assert_eq!(b.read(0x0FFF_FFFC, 4), Ok(0x1122_3344));
        // Straddling the end of DDR is unmapped, not a partial access.
        assert_eq!(b.read(0x0FFF_FFFE, 4), Err(BusFault::Unmapped(0x0FFF_FFFE)));
        b.write(OCM_BASE, 8, 0x0102_0304_0506_0708).unwrap();
        assert_eq!(b.read(OCM_BASE + 4, 4), Ok(0x0102_0304));
        assert_eq!(
            b.read(OCM_BASE + OCM_SIZE - 1, 2),
            Err(BusFault::Unmapped(OCM_BASE + OCM_SIZE - 1))
        );
    }

    #[test]
    fn unmapped_accesses_fault_by_default_and_are_logged() {
        let mut b = bus();
        assert_eq!(b.read(0x2000_0000, 4), Err(BusFault::Unmapped(0x2000_0000)));
        assert_eq!(
            b.write(0x2000_0004, 2, 7),
            Err(BusFault::Unmapped(0x2000_0004))
        );
        assert_eq!(b.unmapped_count(), 2);
        assert_eq!(
            b.unmapped_log()[1],
            UnmappedAccess {
                addr: 0x2000_0004,
                size: 2,
                write: true,
                value: 7
            }
        );
    }

    #[test]
    fn raz_wi_policy_reads_zero_and_still_logs() {
        let mut b = bus();
        b.set_policy(UnmappedPolicy::RazWi);
        assert_eq!(b.read(0x3000_0000, 4), Ok(0));
        assert!(b.write(0x3000_0000, 4, 9).is_ok());
        assert_eq!(b.unmapped_count(), 2);
    }

    #[test]
    fn a_device_gap_is_unmapped_with_the_absolute_address() {
        let mut b = bus();
        assert_eq!(b.read(0xE000_0020, 4), Err(BusFault::Unmapped(0xE000_0020)));
    }

    #[test]
    fn eight_byte_accesses_reach_a_device_as_two_words() {
        let mut b = bus();
        b.write(0xE000_0000, 8, 0x1111_2222_3333_4444).unwrap();
        // The probe keeps the last 4-byte write, which is the high word.
        assert_eq!(b.read(0xE000_0000, 4).unwrap() & 0xFFFF_FFFF, 0x1111_2222);
    }

    #[test]
    fn overlapping_and_invalid_regions_are_refused() {
        let mut b = bus();
        let err = b
            .attach("CLASH", 0xE000_0080, 0x10, Box::new(Stub::new("x")))
            .unwrap_err();
        assert_eq!(
            err,
            MapError::Overlap {
                name: "CLASH",
                with: "PROBE"
            }
        );
        let err = b
            .attach("LOW", 0x0FFF_0000, 0x1000, Box::new(Stub::new("x")))
            .unwrap_err();
        assert_eq!(
            err,
            MapError::Overlap {
                name: "LOW",
                with: "DDR"
            }
        );
        assert_eq!(
            b.attach("ZERO", 0x4000_0000, 0, Box::new(Stub::new("x")))
                .unwrap_err(),
            MapError::Empty("ZERO")
        );
        assert!(SystemBus::new(0x1000_0001).is_err());
        assert!(SystemBus::new(DDR_WINDOW + 0x1_0000).is_err());
    }

    #[test]
    fn core_views_select_their_bank_and_tick_reaches_devices() {
        let mut b = bus();
        b.core(1).write(0xE000_0000, 4, 5).unwrap();
        assert_eq!(b.core(1).read(0xE000_0000, 4).unwrap() & 0xFF, 5 ^ 1);
        b.tick(10);
        b.tick(5);
        assert_eq!(b.cycles(), 15);
    }

    #[test]
    fn ram_snapshot_round_trips_and_checks_its_size() {
        let mut b = bus();
        b.write(0x0800_0000, 8, 0xCAFE_BABE_0000_0001).unwrap();
        b.write(OCM_BASE + 0x100, 4, 0xABCD).unwrap();
        let snap = b.ram_snapshot();
        let mut c = SystemBus::new(0x1000_0000).unwrap();
        c.ram_restore(&snap).unwrap();
        assert_eq!(c.read(0x0800_0000, 8), Ok(0xCAFE_BABE_0000_0001));
        assert_eq!(c.read(OCM_BASE + 0x100, 4), Ok(0xABCD));
        assert_eq!(c.ram_snapshot(), snap);
        let mut small = SystemBus::new(0x0800_0000).unwrap();
        assert_eq!(
            small.ram_restore(&snap),
            Err(SnapshotError::DdrSize {
                expected: 0x0800_0000,
                found: 0x1000_0000
            })
        );
        assert!(c.ram_restore(&snap[..snap.len() - 1]).is_err());
    }

    #[test]
    fn state_hash_follows_state_and_nothing_else() {
        let mut a = bus();
        let mut b = bus();
        assert_eq!(a.state_hash(), b.state_hash());
        a.write(0x100, 4, 1).unwrap();
        assert_ne!(a.state_hash(), b.state_hash());
        b.write(0x100, 4, 1).unwrap();
        assert_eq!(a.state_hash(), b.state_hash());
    }

    #[test]
    fn load_writes_ram_only() {
        let mut b = bus();
        b.load(0x10, &[1, 2, 3]).unwrap();
        assert_eq!(b.read(0x10, 4), Ok(0x0003_0201));
        assert!(b.load(0xE000_0000, &[1]).is_err());
    }
}
