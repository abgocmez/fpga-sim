//! Native RTL engine (milestone P6): a deterministic event-driven simulator of the
//! synthesizable Verilog subset that `rtl-frontend` parses, with no external tool.
//!
//! * [`Design::elaborate`] flattens the hierarchy from the top module (parameters,
//!   generate, widths, port wiring) into signals, processes and functions.
//! * [`Simulator`] runs it with Verilog scheduling (active, non-blocking, delta
//!   cycles, timed events), snapshots, a value-change recorder and VCD output.
//! * [`RtlModule`] puts a design with an AXI4-Lite slave behind `pl-native`'s
//!   [`pl_native::RegisterBlock`] interface.
//!
//! Values are 2-state (x and z read as 0) and at most 128 bits wide. The subset and
//! its limits are in `docs/RTL_SUBSET.md`, the status in `docs/STATUS.md`.

#![forbid(unsafe_code)]

pub mod axi;
pub mod design;
pub mod ir;
pub mod sim;

mod elab;
mod lower;

pub use axi::RtlModule;
pub use design::{Design, timescale_ps};
pub use rtl_frontend::{Diagnostic, Pos};
pub use sim::{Change, SimError, Simulator};
