# ADR-028: Shared-object DPI library (`dpi-cdylib`)

Status: **Accepted** (milestone P4, the interop `cdylib` of prompt §9.7). Date: 2026-10-17.

## Context

ADR-002 made XSim on Windows link the DPI code as a static archive (`libxsim_dpi.a`). Other
simulators load DPI code from a shared object: Questa and ModelSim (`-sv_lib`), VCS, Xcelium,
XSim on Linux and Verilator when linked with `--exe`. Prompt §9.7 asks for a Rust `cdylib`
with the same DPI-compatible C ABI so those testbenches can import it. STATUS had the row
"Questa DPI cdylib: stub".

The constraints are the same as before: no C or C++ is written; `unsafe` is confined to the
FFI boundary; the testbench is unchanged; `cargo xtask dpi` (the archive for XSim) keeps working.

No commercial simulator is installed here, so the shared object can be tested only from Rust.

## Decision

1. **One export table for both libraries.** The 31 exported functions (`rp_*`, `zs_*`, `tel_*`)
   are written once, as the macro `export_dpi_abi!` in `xsim_dpi::abi`. The archive's `win`
   module and the new crate both expand it. The macro takes the accessor of the one `Dpi`
   instance and the platform's `rp_open` body, which is the only export that differs (the
   endpoint text may be empty, meaning `RP_SOCKET`). The archive's exported names were
   compared with `nm` before and after the move: identical.
2. **A new crate, `crates/dpi-cdylib`** (`crate-type = ["cdylib"]`, library name
   `zynqsim_dpi`). It adds only what a shared object needs on top of `xsim-dpi`: a socket
   transport over `std::net::TcpStream` (the same on every platform, so no hand-written
   Winsock here), the `RP_SOCKET` lookup, and the single-instance cell. It is not a feature of
   `xsim-dpi`: the archive is a `no_std` build with a panic handler and allocator, which cannot
   share a crate with `std` without splitting the feature matrix.
3. **Build and ship.** `cargo xtask dpi-shared` builds the crate in the normal target directory
   and copies it to `target/dpi/release/zynqsim_dpi.dll` (`.so`, `.dylib`); the copy drops the
   `lib` prefix that Cargo puts on Unix. `cargo xtask dist` includes it in `lib/` when it
   builds and leaves it out when it does not. The archive is never affected.
4. **Unsafe exemption.** `dpi-cdylib` does not inherit the workspace `unsafe_code = "forbid"`,
   the same minimal way `xsim-dpi` does (no `[lints] workspace = true`; `missing_docs` still
   denied). Its `unsafe` is the single-instance cell in `lib.rs` and the generated export
   layer in `xsim_dpi::abi`.
5. **Edition 2021**, because the exports are plain `#[no_mangle]` functions, as in `xsim-dpi`.
   Edition 2024 would require `#[unsafe(no_mangle)]` in the macro.

## Consequences

* **Verified (Windows, MinGW build):** the shared object exports exactly the 31 DPI names, undecorated
  (the PE export table is read by the test and compared with the imports of every `.sv` fixture plus
  a generated testbench). The names resolve through `libloading`. A Rust test loads the library and calls
  it through `extern "C"` function pointers with the DPI-C argument types, while a fake remote-port peer
  (the `interop` crate) on another thread checks each packet: hello, a PS read with its latency, a PS
  write, pin events in and out, a blocking pin exchange, an acknowledged wire update, a codec register
  write, an LED event from a C string, board inputs, lock-step sync, a PL-to-PS write and read through
  the memory master, and close. A mutated expectation makes the test fail (checked).
* **Not verified:** no simulator has loaded the shared object. Questa/ModelSim, VCS, Xcelium, XSim on
  Linux and Verilator are not installed here. The claim that they can import it rests on the export
  set and the unchanged testbench, not on a run. Linux and macOS builds are not run here either.
* `cargo test` does not build a `cdylib` target, so the ABI test builds the shared object itself into
  the same target directory when it is missing (a nested `cargo build -p dpi-cdylib`).
* The MinGW linker prints `corrupt .drectve at end of def file` when it links the library. This is the
  same harmless warning as for the other crates.
* The library is single-threaded by the DPI contract, like the archive. It is built with
  `panic = "abort"` (the workspace profile), so a panic ends the simulator process.
* On Windows the shared object imports only system DLLs (`KERNEL32`, `msvcrt`, `ntdll`, `WS2_32`).
  It needs no MinGW runtime DLL on the simulator's path.

## Not decided

* Running the generated testbench under Questa (`vlog`/`vsim -sv_lib zynqsim_dpi`) and the other
  simulators. That needs their installs; the generated testbench is not changed for it.
* Making the shared object part of the `ci` gate. It is built by `dist` and by the ABI test, which
  the workspace test run includes.
