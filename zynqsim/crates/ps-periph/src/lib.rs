//! Register-level models of the Zynq-7000 PS peripherals for the native PS engine.
//!
//! Each model implements [`mem_sys::Device`] and is attached to a [`mem_sys::SystemBus`] at
//! the address of `mem_sys::ZYNQ_MAP`; [`system::zynq_system`] builds the whole set. The
//! models follow UG585 and the Cortex-A9 MPCore TRM as remembered. Their checks are the
//! conformance firmware in `firmware/conf_*`, which this crate encodes as unit tests.
//!
//! * [`uart`]: Cadence UART (xuartps), UART0 and UART1.
//! * [`slcr`]: system-level control registers with the lock.
//! * [`mpcore`]: SCU, GIC ([`gic`]), global timer, per-core private timer and watchdog
//!   ([`timers`]), and the L2C stub.
//! * [`ttc`]: triple timer counters TTC0 and TTC1.
//! * stubs: DDRC and DEVCFG read as zero (`mem_sys::Stub`).
//!
//! `VERIFY:` marks every register fact that was not checked against the TRM.

#![forbid(unsafe_code)]

pub mod clock;
pub mod gic;
pub mod mpcore;
pub mod slcr;
pub mod system;
pub mod timers;
pub mod ttc;
pub mod uart;

pub use gic::{Gic, NCPU, NUM_IRQS};
pub use mpcore::MpCore;
pub use slcr::Slcr;
pub use system::{PsConfig, PsHandles, zynq_system};
pub use timers::{Countdown, GlobalTimer, Watchdog};
pub use ttc::Ttc;
pub use uart::Uart;
