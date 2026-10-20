//! Host build of the showcase model: includes the firmware's `model.rs` unchanged so its
//! `#[cfg(test)]` tests run on the host.
#[path = "../../src/model.rs"]
pub mod model;
