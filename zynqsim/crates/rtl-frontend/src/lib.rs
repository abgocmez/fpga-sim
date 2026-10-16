//! Native RTL front end for the ZynqSim P6 subset engine.
//!
//! Pure Rust, no external tools. It reads Verilog-2001 and the common synthesizable
//! SystemVerilog subset, and produces:
//!
//! * a preprocessed token stream ([`pp`], [`lexer`]): `define` (object and function
//!   macros), conditionals, `include`, `timescale`, `default_nettype`;
//! * an AST ([`ast`]) with positions, from [`parse_files`] / [`parse_text`];
//! * an elaborated design ([`elaborate`]): parameters with overrides, generate
//!   unrolling, the instance tree, widths, and undefined module, port and identifier
//!   checks, with the constant evaluator in [`consteval`].
//!
//! Anything outside the subset is reported as a [`Diagnostic`] that names the
//! construct and its position; parsing continues after it where possible. The
//! subset is listed in `docs/RTL_SUBSET.md`. This crate does not simulate anything.

#![forbid(unsafe_code)]

pub mod ast;
pub mod consteval;
pub mod diag;
pub mod elab;
pub mod lexer;
pub mod parser;
pub mod pp;

pub use diag::{Diagnostic, Pos};
pub use elab::{
    Design, InstanceNode, ModuleInfo, NetInfo, ParamInfo, PortInfo, elaborate, elaborate_parsed,
    summarize_parsed,
};
pub use parser::{DEFAULT_STEP_LIMIT, Options, Parsed, parse_files, parse_text};
