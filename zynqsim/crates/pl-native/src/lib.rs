//! Native Rust PL models.
//!
//! These are **transaction-level** models: they reproduce the register-level
//! behaviour of an IP block but have no clock-cycle timing, so they are *not*
//! cycle-exact at their ports. They exist so a PS engine can be exercised
//! against a PL without any RTL simulator installed. RTL-accurate behaviour
//! comes from the RTL adapters.

mod math_ip;
mod regs;

pub use math_ip::MathIp;
pub use regs::{Mapped, NativePl, RegisterBlock};
