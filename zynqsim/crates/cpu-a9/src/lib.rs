//! `cpu-a9`: a deterministic interpreter for the integer subset of ARMv7-A (A32 only) as the
//! Cortex-A9 of the Zynq-7000 PS runs it, for the native PS engine (spec P5).
//!
//! One [`Cpu::step`] executes one instruction against a [`ps_bus::Bus`]. Unimplemented
//! encodings are never guessed: the step returns [`StepResult::Unimplemented`] and leaves the
//! state untouched. There is no MMU (virtual = physical; the [`Translate`] hook is the seam
//! for one), no Thumb, no VFP or NEON. See `docs/adr/030-cpu-a9.md` for the instruction table.

#![forbid(unsafe_code)]

mod alu;
mod cp15;
mod exec;
mod media;
mod mem;
mod state;
mod vfp;
mod xlate;

pub use state::{Cpu, Event, Mode, StepResult};
pub use xlate::{Access, Identity, Translate};
