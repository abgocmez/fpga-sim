//! Helpers shared by the engine tests.

#![allow(dead_code)]

use rtl_engine::{Design, Simulator};
use rtl_frontend::parser::{Options, parse_text};

/// Elaborates `top` from one in-memory source and returns a simulator.
pub fn build(top: &str, src: &str) -> Simulator {
    let parsed = parse_text("test.v", src, &Options::default());
    let design = Design::elaborate_parsed(top, &parsed);
    Simulator::new(design).unwrap_or_else(|d| {
        let msgs: Vec<String> = d.iter().map(|x| x.to_string()).collect();
        panic!("design refused:\n{}", msgs.join("\n"))
    })
}

/// Elaborates `top` and starts a simulator, returning its diagnostics on failure.
pub fn try_build(top: &str, src: &str) -> Result<Simulator, Vec<String>> {
    let parsed = parse_text("test.v", src, &Options::default());
    let design = Design::elaborate_parsed(top, &parsed);
    Simulator::new(design).map_err(|d| d.iter().map(|x| x.to_string()).collect())
}

/// Elaborates and returns the diagnostics (empty when the design is accepted).
pub fn diagnostics(top: &str, src: &str) -> Vec<String> {
    let parsed = parse_text("test.v", src, &Options::default());
    let design = Design::elaborate_parsed(top, &parsed);
    design.diagnostics.iter().map(|d| d.to_string()).collect()
}
