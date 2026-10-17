//! The part of the VPI C ABI (Icarus Verilog's `vpi_user.h`) that this module uses,
//! declared in Rust.
//!
//! Icarus's `vvp.exe` links `libvvp-1.dll`, which exports the plain C names
//! (`vpi_register_systf`, `vpi_get_value`, `vpi_iterate`, ...; checked with
//! `objdump -p` on the suite of ADR-037). This module does not link against them:
//! it looks them up with `GetProcAddress` in the module that exports them, when
//! `vvp` loads the VPI module. So no import library is generated, and the module
//! has no link-time dependency on `vvp`.
//!
//! Only the constants and structures below are declared; each is the C layout on
//! 64-bit Windows (`repr(C)`). Every call goes through a pointer from [`load`].

use std::ffi::{c_char, c_void, CStr};
use std::sync::OnceLock;

/// `vpiHandle`: an opaque pointer owned by vvp.
pub type Handle = *mut c_void;

/// `vpiSysTask`.
pub const SYS_TASK: i32 = 1;
/// `vpiSysFunc`.
pub const SYS_FUNC: i32 = 2;
/// `vpiSizedSignedFunc`: a function whose result has the width its `sizetf` reports.
pub const SIZED_SIGNED_FUNC: i32 = 5;
/// `vpiSize`.
pub const SIZE: i32 = 4;
/// `vpiIntVal`.
pub const INT_VAL: i32 = 6;
/// `vpiStringVal`.
pub const STRING_VAL: i32 = 8;
/// `vpiVectorVal`.
pub const VECTOR_VAL: i32 = 9;
/// `vpiTimeVal`: a time value; vvp accepts it only for time-typed objects (`$time`).
pub const TIME_VAL: i32 = 11;
/// `vpiSimTime`: the time type to ask for in a [`Time`].
pub const SIM_TIME: i32 = 2;
/// `vpiType`.
pub const TYPE: i32 = 1;
/// `vpiName`: the name of a handle (for a system call, its `$name`).
pub const NAME: i32 = 2;
/// `vpiSysFuncCall`: a call of a system function, such as `$time` used as an argument.
pub const SYS_FUNC_CALL: i32 = 56;
/// `vpiTimeVar`: a `time` variable.
pub const TIME_VAR: i32 = 63;
/// `vpiSysTfCall`: the handle of the system task or function being called.
pub const SYS_TF_CALL: i32 = 85;
/// `vpiArgument`: the arguments of a system call, through `vpi_iterate`.
pub const ARGUMENT: i32 = 89;
/// `vpiNoDelay`: assign now, with no time argument.
pub const NO_DELAY: i32 = 1;

/// `s_vpi_systf_data`: how a system task or function is registered.
#[repr(C)]
pub struct SystfData {
    /// `vpiSysTask` or `vpiSysFunc`.
    pub kind: i32,
    /// For a function: `vpiSizedSignedFunc`; unused for a task.
    pub sysfunctype: i32,
    /// The name with its `$`, NUL-terminated.
    pub tfname: *const c_char,
    /// Called for each call; returns 0.
    pub calltf: Option<extern "C" fn(*const c_char) -> i32>,
    /// Called when the call is compiled (not used here).
    pub compiletf: Option<extern "C" fn(*const c_char) -> i32>,
    /// For a sized function: the result width in bits.
    pub sizetf: Option<extern "C" fn(*const c_char) -> i32>,
    /// Passed back to the callbacks (not used here).
    pub user_data: *const c_char,
}

/// `s_vpi_vecval`: one 32-bit word of a vector, in the 2-bit a/b encoding.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct VecVal {
    /// The `aval` bits (for a 0/1 value, the value).
    pub aval: i32,
    /// The `bval` bits (X and Z); always 0 here.
    pub bval: i32,
}

/// `s_vpi_time`: a simulation time.
#[repr(C)]
pub struct Time {
    /// `vpiSimTime` and the like.
    pub kind: i32,
    /// High 32 bits.
    pub high: u32,
    /// Low 32 bits.
    pub low: u32,
    /// `vpiScaledRealTime` value (unused here).
    pub real: f64,
}

/// The `value` union of `s_vpi_value`.
#[repr(C)]
#[derive(Clone, Copy)]
pub union ValueUnion {
    /// `vpiStringVal`.
    pub str_: *mut c_char,
    /// `vpiIntVal`.
    pub integer: i32,
    /// `vpiRealVal`.
    pub real: f64,
    /// `vpiTimeVal`.
    pub time: *mut Time,
    /// `vpiVectorVal`: an array of `VecVal`, one per 32 bits.
    pub vector: *mut VecVal,
    /// Any other member (the strength pointer, `misc`).
    pub misc: *mut c_void,
}

/// `s_vpi_value`: a value passed to or from vvp, with its `format`.
#[repr(C)]
pub struct Value {
    /// The format of `value` (`INT_VAL`, `STRING_VAL`, `VECTOR_VAL`, ...).
    pub format: i32,
    /// The value in that format.
    pub value: ValueUnion,
}

/// The VPI functions this module calls, resolved once by [`load`].
pub struct Api {
    /// `vpi_register_systf`.
    pub register_systf: unsafe extern "C" fn(*const SystfData) -> Handle,
    /// `vpi_handle`.
    pub handle: unsafe extern "C" fn(i32, Handle) -> Handle,
    /// `vpi_iterate`.
    pub iterate: unsafe extern "C" fn(i32, Handle) -> Handle,
    /// `vpi_scan`: the next handle of an iterator, NULL at its end.
    pub scan: unsafe extern "C" fn(Handle) -> Handle,
    /// `vpi_get`: an integer property of a handle.
    pub get: unsafe extern "C" fn(i32, Handle) -> i32,
    /// `vpi_get_str`: a string property of a handle (NUL-terminated, owned by vvp).
    pub get_str: unsafe extern "C" fn(i32, Handle) -> *const c_char,
    /// `vpi_get_value`.
    pub get_value: unsafe extern "C" fn(Handle, *mut Value),
    /// `vpi_put_value`.
    pub put_value: unsafe extern "C" fn(Handle, *mut Value, *mut Time, i32) -> Handle,
}

static API: OnceLock<Api> = OnceLock::new();

/// The resolved VPI functions. Panics if [`load`] has not succeeded; it runs first, in
/// the startup routine, so the system functions are only called after it.
pub fn api() -> &'static Api {
    API.get().expect("zynqsim.vpi: the VPI functions are not resolved")
}

/// The version of the jump table this module reads (`vpip_routines_version` of vpi_user.h).
const ROUTINES_VERSION: u32 = 2;

/// `vpip_routines_s` of vpi_user.h (version 2): the jump table that vvp (and `iverilog -m`,
/// the compile-time loader) passes to a VPI module on Windows. Every entry is a function
/// pointer; the order is the header's. Only the entries in [`Api`] are read.
#[repr(C)]
struct Routines {
    register_cb: *const c_void,
    remove_cb: *const c_void,
    register_systf: *const c_void,
    get_systf_info: *const c_void,
    handle_by_name: *const c_void,
    handle_by_index: *const c_void,
    handle_multi: *const c_void,
    handle: *const c_void,
    iterate: *const c_void,
    scan: *const c_void,
    get: *const c_void,
    get64: *const c_void,
    get_str: *const c_void,
    get_delays: *const c_void,
    put_delays: *const c_void,
    get_value: *const c_void,
    put_value: *const c_void,
    get_time: *const c_void,
    get_userdata: *const c_void,
    put_userdata: *const c_void,
    mcd_open: *const c_void,
    mcd_close: *const c_void,
    mcd_flush: *const c_void,
    mcd_name: *const c_void,
    mcd_vprintf: *const c_void,
    flush: *const c_void,
    vprintf: *const c_void,
    chk_error: *const c_void,
    compare_objects: *const c_void,
    free_object: *const c_void,
    release_handle: *const c_void,
    get_vlog_info: *const c_void,
    vcontrol: *const c_void,
    fopen: *const c_void,
    get_file: *const c_void,
    calc_clog2: *const c_void,
    count_drivers: *const c_void,
    format_strength: *const c_void,
    format_pretty: *const c_void,
    make_systf_system_defined: *const c_void,
    mcd_rawwrite: *const c_void,
    set_return_value: *const c_void,
}

/// `vpip_set_callback`: vvp and the compiler (`iverilog -m`) call this with their jump
/// table before the module's startup routines. Returns 0 when the table is taken.
///
/// # Safety
///
/// `routines` is NULL or the `vpip_routines_s` of the calling vvp or compiler, valid for
/// the life of the process.
#[no_mangle]
pub unsafe extern "C" fn vpip_set_callback(routines: *const c_void, version: u32) -> u32 {
    if routines.is_null() || version != ROUTINES_VERSION {
        return 1;
    }
    let r = &*(routines as *const Routines);
    // SAFETY: each entry is the function of that name with the signature in `Api`.
    let api = Api {
        register_systf: std::mem::transmute(r.register_systf),
        handle: std::mem::transmute(r.handle),
        iterate: std::mem::transmute(r.iterate),
        scan: std::mem::transmute(r.scan),
        get: std::mem::transmute(r.get),
        get_str: std::mem::transmute(r.get_str),
        get_value: std::mem::transmute(r.get_value),
        put_value: std::mem::transmute(r.put_value),
    };
    let _ = API.set(api);
    1
}

// The Windows loader, from kernel32 (already linked by the Rust standard library).
#[cfg(windows)]
#[link(name = "kernel32")]
extern "system" {
    fn GetModuleHandleW(name: *const u16) -> *mut c_void;
    fn GetProcAddress(module: *mut c_void, name: *const c_char) -> *mut c_void;
}

/// Resolves the VPI functions: from the jump table if vvp or the compiler gave one
/// (`vpip_set_callback`), otherwise by name from the module that exports them. vvp
/// exports them from `libvvp-1.dll` (the DLL it links); the executable is tried too.
#[cfg(windows)]
pub fn load() -> Result<(), String> {
    if API.get().is_some() {
        return Ok(());
    }
    let candidates: [Option<&str>; 2] = [Some("libvvp-1.dll"), None];
    let mut module = std::ptr::null_mut();
    for name in candidates {
        let wide: Vec<u16>;
        let ptr = match name {
            Some(n) => {
                wide = n.encode_utf16().chain(std::iter::once(0)).collect();
                wide.as_ptr()
            }
            None => std::ptr::null(),
        };
        // SAFETY: a NUL-terminated wide string (or NULL for the executable).
        module = unsafe { GetModuleHandleW(ptr) };
        if !module.is_null() && proc(module, c"vpi_register_systf").is_some() {
            break;
        }
    }
    if module.is_null() {
        return Err("no loaded module exports the VPI functions (is this loaded by vvp?)".into());
    }
    let p = |name: &CStr| -> Result<*mut c_void, String> {
        proc(module, name).ok_or_else(|| format!("missing VPI function {}", name.to_string_lossy()))
    };
    // SAFETY: each address is the function of that name in vvp's VPI interface, whose
    // C signature is the one declared in `Api`.
    let api = unsafe {
        Api {
            register_systf: std::mem::transmute::<*mut c_void, _>(p(c"vpi_register_systf")?),
            handle: std::mem::transmute::<*mut c_void, _>(p(c"vpi_handle")?),
            iterate: std::mem::transmute::<*mut c_void, _>(p(c"vpi_iterate")?),
            scan: std::mem::transmute::<*mut c_void, _>(p(c"vpi_scan")?),
            get: std::mem::transmute::<*mut c_void, _>(p(c"vpi_get")?),
            get_str: std::mem::transmute::<*mut c_void, _>(p(c"vpi_get_str")?),
            get_value: std::mem::transmute::<*mut c_void, _>(p(c"vpi_get_value")?),
            put_value: std::mem::transmute::<*mut c_void, _>(p(c"vpi_put_value")?),
        }
    };
    let _ = API.set(api);
    Ok(())
}

/// The address of `name` in `module`, if it exports it.
#[cfg(windows)]
fn proc(module: *mut c_void, name: &CStr) -> Option<*mut c_void> {
    // SAFETY: the module handle came from GetModuleHandleW; the name is NUL-terminated.
    let p = unsafe { GetProcAddress(module, name.as_ptr()) };
    (!p.is_null()).then_some(p)
}

/// Only Windows is supported (ADR-037); elsewhere the module reports that and does nothing.
#[cfg(not(windows))]
pub fn load() -> Result<(), String> {
    if API.get().is_some() {
        return Ok(());
    }
    Err("the zynqsim VPI module is built for Windows only".into())
}
