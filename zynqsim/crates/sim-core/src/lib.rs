//! Deterministic simulation kernel: picosecond time base, event scheduler,
//! seeded PRNG and clock domains.
//!
//! Nothing in this crate reads the host clock or any other ambient state, so a
//! given sequence of calls always produces the same result (requirement §4).

mod clock;
mod prng;
mod sched;
mod time;

pub use clock::ClockDomain;
pub use prng::Prng;
pub use sched::{EventId, SchedError, Scheduler};
pub use time::Ps;
