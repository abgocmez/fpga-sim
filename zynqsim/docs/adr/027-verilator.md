# ADR-027: Verilator as the second RTL backend; Icarus stays a stub

Status: **Accepted** (milestone P1, second RTL engine). Date: 2026-10-17.
Update (ADR-037): Icarus Verilog is now the third RTL backend through a Rust VPI module; the "Icarus stays a stub" decision below is superseded.

## Context

ADR-002 chose XSim first and named Icarus Verilog as the second backend, so that CI could run
RTL tests without Vivado. Two facts found on this machine change that:

* Icarus Verilog's DPI-C support is absent, so the testbench's `rp_*` imports (the same ones XSim
  uses) cannot be satisfied by `xsim-dpi`. Icarus would need a VPI module written in Rust, a
  second `cdylib` with the `vpi_*` entry points, plus its own build of the glue. That is a
  project of its own and is not done.
* Verilator 5 is available as a portable build (the YosysHQ `oss-cad-suite` Windows archive,
  about 600 MB, unpacked into a folder under `%TEMP%`, nothing installed or registered). It
  implements DPI-C, and `--binary --timing` builds an executable from SystemVerilog without a
  handwritten C++ main.

Prompt §9.2 lists Verilator as "speed, wide SV support (2-state)" and §15 allows any of the three.

## Decision

1. **The second backend is Verilator**, through the crate `backend-rtl-verilator`, which mirrors
   `backend-rtl-xsim`: `VerilatorConfig`, a build step in the run directory, a content-keyed build
   cache, the same `PlTarget` implementation with the same remote-port protocol, pin, media and
   protocol-checker plumbing (copied, not shared: the shared part is small and the two adapters
   are allowed to diverge).
2. **Invocation**: `verilator_bin.exe --binary --timing -Wno-fatal -j 0 --top-module <top>
   -Mdir obj -o zs_sim -D... -LDFLAGS <libxsim_dpi.a> -LDFLAGS -static -LDFLAGS
   -Wl,--allow-multiple-definition <sources>`. The launcher `verilator` is a Perl script and is
   not used. The environment carries `VERILATOR_ROOT` and a `PATH` with the tools' folders first.
3. **The DPI archive is the same `xsim-dpi` library** (`cargo xtask dpi`). Two link flags are
   needed on Windows: `-static`, so that the executable needs no MinGW DLLs at run time, and
   `--allow-multiple-definition`, because the Rust archive and libgcc both define
   `_Unwind_Resume`. Nothing in the simulator unwinds, so the archive's definition is not relied
   on; this is recorded as a known risk, not a proof.
4. **No C or C++ is written here.** The model is Verilator's generated C++ compiled by the host's
   g++, run by GNU make and a POSIX `sh`. Each is a prerequisite that the adapter checks for and
   names in its error (`VerilatorError::MissingTool`).
5. **Locating Verilator**: `ZYNQSIM_VERILATOR` (a file, the launcher beside the binary, or a
   folder), then `VERILATOR_ROOT`, `PATH`, then `<temp>\zs-tools\verilator\oss-cad-suite\bin`.
   `make` and `sh` are searched on `PATH` and then in `<temp>\zs-tools\tools` and Git for
   Windows' `usr\bin`. Nothing is written outside the run directory and the temp folder.
6. **Cache**: the key covers the Verilator version line, the sources (names and bytes), top,
   defines, the build flags and the DPI archive. A hit copies the executable and reuses the build
   log; the second start of the math design takes 0.2 s instead of 10.9 s.
7. **Icarus Verilog stays a stub.** `backends.pl = "icarus"` is refused by the project check
   ("not implemented in this build"); there is no Icarus adapter and no VPI glue. Doing it means
   writing a Rust VPI `cdylib` and an Icarus adapter; that is a separate decision.
8. **Limits enforced by the project check, not by silent behaviour**: no `[rtl] waves` (Verilator
   would need `--trace` and a waveform reader), no VHDL, no Vivado simulation exports
   (`export_dir`, `filelist`).

## Consequences

* Two RTL engines run the same testbench and firmware, so a difference between them is visible.
  `cli::verilator` asserts equal UART output and equal bus-access counts; the trace digests are
  reported, not asserted.
* Verilator is 2-state. An uninitialised variable reads as 0, an X from XSim does not appear.
  Designs that depend on X are out of scope for this backend.
* Build cost: about 11 s the first time a design is built (Verilator's runtime and the design are
  compiled); XSim's first build of the math design was of the same order.
* Windows only, like the XSim path: the DPI archive is the Windows build.
* The `-Wl,--allow-multiple-definition` flag is a link-time workaround; a later change to the DPI
  crate that adds unwinding would make it a real conflict, and the build would then have to
  exclude libgcc's unwinder explicitly.

## Outcome (recorded after implementation, 2026-10-17)

Measured on this machine (Verilator 5.053 devel, MinGW g++ 13.1, Xilinx QEMU, Vivado 2022.1):

| Check | Result |
|---|---|
| Standalone math design, 7 writes and 14 reads through the model (`backend-rtl-verilator::tests::math`) | pass; cached second start 0.2 s, first 10.9 s |
| Unsupported `interface` inside a module | refused: `%Error-UNSUPPORTED`, file 5:3, text `bus_if b();` |
| `specify` block | built, ignored with `%Warning-SPECIFYIGN` |
| QEMU math firmware, `examples/math-verilator`, 3 ms | `RESULT: ALL PASS`, 7 `PASS` lines, 21 bus accesses |
| The same project with `pl = "xsim"` | same UART text, same 21 bus accesses, same trace digest and length (329 records, 3,000,814,000 ps) |
| Repeat run, same project | same digest, same end time |

The two simulators gave the same trace digest for this design. The trace records the PS-side
traffic and the end time; the PL's internal timing is not in it, so this does not show that the
two agree on everything inside the RTL. Not measured: the DMA regression, the board, media and pin
models, `$readmemh`, X propagation, and XSim's behaviour after `$finish` (the adapter saw Verilator
run the statements after a `$finish` in an `initial` block until the next suspension point, which
is why the testbench's `rp_open` failure path prints a second line).

Caveats: the first CLI test run needs Verilator unpacked and `make`, a C++ compiler and `sh`
available (see `docs/INSTALL.md`); the test skips with `SKIPPED:` when Verilator is absent.
