//! Simulation trace: a timestamped event log with a canonical content hash,
//! plus VCD and Perfetto (Chrome trace-event) exporters.
//!
//! The hash is what determinism tests compare (requirement §0.5): two runs
//! with the same inputs must produce the same hash on the same backend set.

mod export;
mod hash;
mod log;

pub use export::{to_perfetto_json, to_vcd};
pub use hash::Fnv64;
pub use log::{TraceEvent, TraceLog, TraceRecord};
