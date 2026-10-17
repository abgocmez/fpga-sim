# ADR-037: Icarus Verilog as the third RTL backend, through a Rust VPI module

Status: **Accepted** (milestone P9, third RTL engine; Windows). Date: 2026-10-17.
Supersedes the "Icarus stays a stub" decision of ADR-027 and ADR-002's update.

## Context

ADR-027 chose Verilator second because Icarus has no DPI-C: an Icarus adapter needs the
testbench's `rp_*` functions as VPI system functions, which means a Rust VPI module. That work is
now done for the math design. Icarus is 4-state and event-driven like XSim, so it is a useful
third opinion on the same RTL. The Icarus install on this machine is the portable oss-cad-suite
that the Verilator work unpacked (`iverilog 14.0 (devel)`); nothing is installed.

## Facts found on this machine (checked, not assumed)

| Fact | Evidence |
|---|---|
| `vvp.exe` links `libvvp-1.dll`, which exports the plain C VPI names (`vpi_register_systf`, `vpi_get_value`, `vpi_put_value`, `vpi_iterate`, `vpi_scan`, `vpi_get`, `vpi_handle`, ...) | `objdump -p` on both |
| The VPI jump table `vpip_set_callback(vpip_routines_s*, version 2)` is called by `vvp` and by the compiler when it loads a module with `iverilog -m NAME`; the compiler then uses the table for `sizetf`, so a 64-bit function result is 64 bits wide | the warning `has no vpip_set_callback()` without the export; with it the result is `-1`, not `4294967295` |
| The compiler loads the module from `IVERILOG_VPI_MODULE_PATH` (not `-M`) | `iverilog -m zynqsim` with and without the variable |
| A `$time` argument of a system function reads as 0 in every format; `vpi_get_time` on the call handle or on NULL makes `vvp` abort | probes in the VPI module |
| A vector value read with `vpiVectorVal` is written by `vvp` to its own storage and the pointer is put in the value; reading the buffer passed in gives 0 | probe: constant `7` read as 0 by vector, 7 by `vpiIntVal` |
| Icarus functions cannot have `output` arguments ("Function arguments must be input ports") | `iverilog` error on the first spike |
| `libvvp-1.dll` needs `libstdc++-6.dll` and `libreadline8.dll`, which are in the suite's `lib` folder, not `bin` | loader status `0xC0000135` until `lib` was on `PATH` |

## Decision

1. **`crates/vpi-icarus`** builds `zynqsim.dll`, which `cargo xtask vpi` copies to
   `target/vpi/release/zynqsim.vpi` (the name `vvp -mzynqsim` loads). It exports
   `vlog_startup_routines` (registers the functions) and `vpip_set_callback` (takes the
   jump table). The VPI functions are looked up through the table; without it, by name with
   `GetProcAddress` in `libvvp-1.dll`. No import library is generated and nothing is linked
   against `vvp`. The declarations are Rust `extern "C"` types for the subset used
   (`vpi_user.h`, Icarus 14).
2. **One implementation.** The 31 DPI functions are registered as `$zs_<name>` with the DPI
   argument and result types (`int` = 32-bit system function, `longint` = 64-bit, `void` = system
   task, `output int` = a variable the call writes). Each calls the function that
   `xsim_dpi::abi` exports, so the protocol logic is the archive's. The socket transport and the
   single DPI instance moved from `dpi-cdylib` into `xsim-dpi`'s new `hosted` feature, which
   `dpi-cdylib` and `vpi-icarus` both use (the refactor is the only change to `dpi-cdylib`).
3. **Testbench text.** The generator (`sim-api::tbgen`) and the fixtures put every DPI import
   inside `` `ifdef ZS_ICARUS `` ... `` `else `` ... `` `endif ``. Under `ZS_ICARUS` each import is a
   wrapper (`task rp_done_t(...); $zs_rp_done_t(...); endtask`, and a function for the
   value-returning ones) and the call sites are unchanged. Without the define the `import "DPI-C"`
   lines are the same as before. The XSim and Verilator *compiled* text is unchanged; the source
   file gains directive lines (an exact byte-equal file was not kept: the directives are needed to
   switch). `rp_next` has output arguments, so under `ZS_ICARUS` its call site calls `$zs_rp_next`
   directly, and the time is passed through a `longint zs_now` (the `$time` argument reads 0).
4. **`crates/backend-rtl-icarus`** mirrors `backend-rtl-verilator`: build with
   `iverilog -g2012 -DZS_ICARUS -m zynqsim -s <top> -o design.vvp <sources>` under
   `IVERILOG_VPI_MODULE_PATH=<run>/vpi`, run with `vvp -M <run>/vpi -mzynqsim design.vvp`,
   `RP_SOCKET` pointing at the adapter's listener. Cache keyed by the Icarus version, the sources,
   the defines, the top and the VPI bytes. Diagnostics are `file:line: error: text` lines
   (`diag.rs`). The PL plumbing (PS accesses, master accesses, interrupts, lock-step, pins, media)
   is copied from the Verilator adapter, not shared (the ADR-027 precedent).
5. **Discovery.** `ZYNQSIM_ICARUS` (file or folder), then `PATH`, then
   `<temp>\zs-tools\verilator\oss-cad-suite\bin` for `iverilog.exe`; `vvp.exe` beside it; the
   suite's `lib` folder goes on the tools' `PATH`. The module: `ZYNQSIM_VPI`, then
   `target\vpi\release\zynqsim.vpi`.
6. `backends.pl = "icarus"` is accepted; the project check applies the Verilator limits
   (no `waves`, no `export_dir`/`filelist`, no VHDL). `inspect` reports `icarus` as found only when
   both the compiler and the module are located.

## Outcome (measured on this machine, 2026-10-17)

| Check | Result |
|---|---|
| `cli::icarus::math_firmware_passes_through_icarus_and_is_compared_with_xsim` (real Xilinx QEMU, `examples/math-icarus`, 3 ms) | `RESULT: ALL PASS`, 7 `PASS` lines, 21 bus accesses; second run takes the cached program and gives the same digest |
| Trace digest, Icarus vs XSim (same project, `pl = "xsim"`) | **equal**: `fnv1a64:c801168503080d41`, 329 records, 3,000,814,000 ps |
| `backend-rtl-icarus` unit tests (diagnostics, tool discovery) | pass |
| `vpi-icarus` unit tests (31 names registered, each `$zs_` call of the math testbench is registered) | pass |
| `sim-api --lib` (incl. `tbgen`) | 103 passed |
| XSim `backend-rtl-xsim --test math` and `cargo xtask dpi` | pass, unchanged |

## Consequences and limits

* **Windows only.** The module and the adapter are built for Windows; elsewhere the module reports
  that it cannot resolve the VPI functions. Not run on Linux or macOS.
* **Math design only.** The DMA regression, the board, media, Pmod and pin models and the
  protocol checkers were **not** run under Icarus. Their `$zs_` wrappers are generated but
  unexercised; the Pmod sources (`fixtures/rtl/pmod/zs_pin.sv`) are not wrapped.
* **`$time` as a direct system-call argument reads 0** in vvp. The generated testbench works around
  it for `rp_next`; any other direct `$zs_` call with `$time` must do the same.
* **Icarus rejects `output` arguments on functions**, so such DPI functions are called as system
  functions at their call sites (only `rp_next` in the generated and math testbenches).
* 2-state is not the issue here: Icarus is 4-state; X/Z are not compared against XSim beyond the
  trace digest, which matched.
* Compile-time loading needs `IVERILOG_VPI_MODULE_PATH`; without it the compiler reports the
  module as missing and 64-bit function results are truncated to 32 bits (seen, then fixed).
* `vpi-icarus` declares only the VPI functions it calls (no `vpi_control`, `vpi_free_object`);
  it does not free handles of its own (vvp owns them).
* The jump table layout follows `vpi_user.h` of Icarus 14 (version 2); a different Icarus with a
  different table is refused (`vpip_set_callback` returns 1, then the name lookup is used).
