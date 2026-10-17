//! The DPI-C ABI of the remote-port peer as a shared object.
//!
//! `xsim-dpi` holds the protocol logic, the export table (`xsim_dpi::abi`) and,
//! with its `hosted` feature, the socket transport and the one DPI instance. This
//! crate only expands the export table over that instance. The export names and C
//! signatures are the same as the static archive, so one testbench imports either.
//! Build with `cargo xtask dpi-shared`; the result is `zynqsim_dpi.dll`,
//! `libzynqsim_dpi.so` or `libzynqsim_dpi.dylib`.
//!
//! Like the archive, the library is single-threaded by the DPI contract: one
//! instance per process, never re-entered. It is built with `panic = "abort"` (the
//! workspace profile), so a panic ends the simulator process rather than unwinding
//! into it.
//!
//! This crate is an FFI boundary and must use `unsafe`, so it does not inherit
//! the workspace `unsafe_code = "forbid"` (same exemption as `xsim-dpi`). The
//! `unsafe` is in the export layer, `xsim_dpi::abi` and `xsim_dpi::hosted`.

// The exported symbols. `#[no_mangle]` items are exported from the shared object
// whatever their visibility, so they live in a private module.
mod abi {
    xsim_dpi::export_dpi_abi!(xsim_dpi::hosted::dpi, xsim_dpi::hosted::open_endpoint);
}
