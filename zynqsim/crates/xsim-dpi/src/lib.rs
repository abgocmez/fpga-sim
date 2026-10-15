//! The PL side of remote-port, packaged as a DPI-C library for Vivado XSim.
//!
//! XSim on Windows links DPI code as a *static* archive into `xsimk.exe`
//! (`xelab -sv_root <dir> -sv_lib <name>` wants `<dir>/<name>.a`). A normal
//! Rust static library drags in `std` and its unwinder and cannot be linked
//! that way, so on Windows this crate is `no_std` + `alloc`, depends only on
//! `kernel32`, and binds Winsock at run time with `LoadLibrary`, the same trick
//! the prototype used (`wsdyn.c`) because static `ws2_32` imports stop
//! `xsimk.exe` from starting.
//!
//! The protocol logic ([`core`]) is independent of sockets and is unit-tested
//! on the host. Only `win` touches the OS and `unsafe`.
//!
//! The exported functions have the names and signatures of the prototype's
//! `rp_dpi.c`, so an existing testbench works unchanged.
//!
//! Build the library with `cargo xtask dpi`
//! (`cargo build --release -p xsim-dpi --features freestanding`).

#![cfg_attr(feature = "freestanding", no_std)]

extern crate alloc;

pub mod core;

#[cfg(all(windows, feature = "freestanding"))]
mod win;
