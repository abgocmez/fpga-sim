//! The Zynq-7000 physical address map and the system bus of the native PS engine.
//!
//! [`SystemBus`] implements [`ps_bus::Bus`]: DDR (sparse, 64 KiB pages) from 0, the 256 KiB
//! on-chip RAM at 0xFFFC_0000, and the devices attached at their base addresses. A device is
//! anything implementing [`Device`]: it is accessed by offset, ticked by CPU cycles, reports
//! its interrupt lines and receives the lines of the others. The address map itself is data
//! ([`ZYNQ_MAP`], [`MPCORE_BLOCKS`], [`IOP_BLOCKS`]) so that docs and tests can print it.
//!
//! Register models live in `ps-periph`. Accesses to addresses with no device are logged
//! and either fault or read zero, as [`UnmappedPolicy`] says.
//!
//! The model follows UG585 as remembered; `VERIFY:` marks the entries that were not checked
//! against the TRM. See `docs/adr/031-mem-sys-ps-periph.md`.

#![forbid(unsafe_code)]

pub mod bus;
pub mod device;
pub mod map;
pub mod ram;
pub mod simple;

pub use bus::{
    CoreBus, DDR_DEFAULT, DDR_WINDOW, MapError, OCM_BASE, OCM_SIZE, SnapshotError, SystemBus,
    UNMAPPED_LOG_CAP, UnmappedAccess, UnmappedPolicy, fnv1a64,
};
pub use device::{Device, IrqInput, Shared, lane_merge, lane_read, shared};
pub use map::{IOP_BLOCKS, Kind, MPCORE_BLOCKS, MapEntry, ZYNQ_MAP, end, find, render_markdown};
pub use ps_bus::{Bus, BusFault, BusResult};
pub use ram::{PAGE_SIZE, SparseRam};
pub use simple::{ReadOnlyImage, Stub};
