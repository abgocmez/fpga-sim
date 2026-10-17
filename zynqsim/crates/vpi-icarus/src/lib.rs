//! `zynqsim.vpi`: the remote-port DPI library of ZynqSim as a VPI module for Icarus
//! Verilog (ADR-037).
//!
//! Icarus has no DPI-C. `vvp -mzynqsim` loads this DLL (renamed `zynqsim.vpi`), calls
//! its `vlog_startup_routines`, and this registers every DPI function of the generated
//! testbenches as a system task or function `$zs_<name>`. The calls run the same code
//! as the XSim archive and the Verilator and `dpi-cdylib` shared objects: the export
//! table `xsim_dpi::abi` and the one instance `xsim_dpi::hosted`.
//!
//! The `unsafe` is the VPI C ABI: its declarations and the calls through the
//! addresses vvp exports (`vpi.rs`), and the argument handling in `systf.rs`. Like
//! `dpi-cdylib` the crate does not inherit the workspace `unsafe_code = "forbid"`.
//! No C or C++ is written here.

use std::ffi::c_char;

mod systf;
mod vpi;

// The DPI export table (the same expansion as `dpi-cdylib`). The functions are reached
// from `systf` through this module; the `#[no_mangle]` names are exported as well.
mod abi {
    xsim_dpi::export_dpi_abi!(xsim_dpi::hosted::dpi, xsim_dpi::hosted::open_endpoint);
}

/// The startup routines vvp calls once when it loads this module, then a NULL.
#[no_mangle]
pub static vlog_startup_routines: [Option<extern "C" fn()>; 2] = [Some(startup), None];

/// Registers the DPI functions as system tasks and functions. vvp prints the reason
/// on stderr when the VPI functions cannot be found (it is not the `vvp` it expects).
extern "C" fn startup() {
    if let Err(e) = vpi::load() {
        eprintln!("zynqsim.vpi: {e}");
        return;
    }
    let api = vpi::api();
    for def in systf::TABLE {
        let (kind, sysfunctype) = match def.ret {
            systf::Ret::Task => (vpi::SYS_TASK, 0),
            systf::Ret::Int | systf::Ret::Long => (vpi::SYS_FUNC, vpi::SIZED_SIGNED_FUNC),
        };
        let data = Box::leak(Box::new(vpi::SystfData {
            kind,
            sysfunctype,
            tfname: def.name.as_ptr() as *const c_char,
            calltf: Some(def.calltf),
            compiletf: None,
            sizetf: def.ret.sizetf(),
            user_data: std::ptr::null(),
        }));
        // SAFETY: `data` lives for the rest of the process; vvp copies it on registration.
        unsafe {
            (api.register_systf)(data);
        }
    }
}
