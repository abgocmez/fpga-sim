//! `cpu-a9`: a deterministic interpreter for the integer subset of ARMv7-A (A32 only) as the
//! Cortex-A9 of the Zynq-7000 PS runs it, for the native PS engine (spec P5).
//!
//! One [`Cpu::step`] executes one instruction against a [`ps_bus::Bus`]. Unimplemented
//! encodings are never guessed: the step returns [`StepResult::Unimplemented`] and leaves the
//! state untouched. Address translation is the ARMv7-A short-descriptor MMU of [`mmu`] (the
//! [`Translate`] hook, default [`mmu::Mmu`]). There is no Thumb, no VFP or NEON. See
//! `docs/adr/030-cpu-a9.md` for the instruction table and the MMU scope.
//! state untouched. There is no MMU (virtual = physical; the [`Translate`] hook is the seam
//! for one), no VFP or NEON; Thumb and Thumb-2 are in `thumb.rs`. See `docs/adr/030-cpu-a9.md` for the instruction table.

#![forbid(unsafe_code)]

mod alu;
mod cp15;
mod exec;
mod media;
mod mem;
pub mod mmu;
mod state;
mod thumb;
mod vfp;
mod xlate;

pub use state::{Cpu, Event, Mode, StepResult};
pub use xlate::{Access, Identity, Regs, TlbOp, Translate};
