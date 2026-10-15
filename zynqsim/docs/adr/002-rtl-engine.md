# ADR-002: First RTL backend and the simulator-side interop

Status: **Accepted** (project owner delegated the choice: "do the best"). Date: 2026-10-15.

## Context

PL logic must execute with RTL semantics and be bound to the PS ports, interrupts, clocks and
board pins. A native Verilog engine is a P6 option; P1 needs an external simulator behind
`PlBackend` (prompt §2.2). Prompt §15 allows Verilator **or** Icarus **or** XSim for the first one.

Two constraints interact with the choice:

* **No C/C++ is authored** (prompt §2.1). Every simulator needs some native glue (DPI for XSim,
  Questa and Verilator; VPI for Icarus). The glue should therefore be a **Rust `cdylib`**
  exporting the DPI/VPI C ABI (`interop` crate, prompt §9.7), which needs isolated, documented
  `unsafe`. Verilator additionally requires compiling a generated C++ model on the host.
* The repository must build and test without any external simulator installed (§2.5).

## Facts about this machine

| Engine | Present | Notes |
|---|---|---|
| Vivado/XSim 2022.1 | yes (`C:\Xilinx\Vivado\2022.1`) | proven by the prototype; DPI-C with a static `dpi.a`, winsock must be bound at run time (see `TROUBLESHOOTING.md`) |
| Icarus Verilog | no | |
| Verilator | no | |

## Options

| | XSim | Icarus | Verilator |
|---|---|---|---|
| Install | needs Vivado (large, licence terms) | small, everywhere | small, needs a C++ toolchain |
| 4-state, behavioural test-benches | yes | yes | no (2-state, limited timing) |
| Vendor/UNISIM/encrypted IP, VHDL | yes | no | no |
| Speed | slow start-up | slow | fast |
| Runs in CI (Linux/macOS) | no | yes | yes |
| Interop mechanism | DPI-C | VPI | DPI-C / C++ harness |

## Decision

1. Implement the **interop `cdylib` once** with a thin layer per mechanism (DPI first).
2. First backend: **XSim**, because it is installed here, is already proven against this exact
   QEMU co-simulation, and is the only option that will run the Vivado exports of milestone P3
   (UNISIM, encrypted IP).
3. Second backend: **Icarus**, specifically so CI can run RTL tests on all three operating
   systems; XSim-dependent tests are skipped (and listed) where Vivado is absent.

## Consequences

* The regression of prompt §14.1 can run locally at the end of P1 but not on hosted CI until
  the Icarus adapter exists; CI would show those tests as skipped, not passed.
* `unsafe` is confined to the interop crate, which is documented and fuzzed (§2.5).
* If the DPI approach proves unworkable on XSim's Windows static-linking model, fall back to the
  prototype's approach with the glue produced by `cargo` as a static library for `xsc`.

## Outcome (recorded after implementation)

* XSim on Windows links DPI as a **static archive** `<name>.a` (`xelab -sv_root <dir> -sv_lib
  <name>`); a shared library is not accepted. A normal Rust `staticlib` fails to link (it needs
  the unwinder and `std`), so `xsim-dpi` is `no_std` + `alloc`, `panic = "abort"`, depends only on
  `kernel32` and binds Winsock with `LoadLibrary` (as the prototype's `wsdyn.c` did, because static
  `ws2_32` imports stop `xsimk.exe` from starting). Verified with the prototype regression.
* The protocol codec is shared with the host through `interop` (`no_std` + `alloc`), so there is
  one implementation of the wire format, tested byte-for-byte against the C reference.
* The DPI archive is built into its own target directory (`target/dpi`) because the same crate in
  a normal workspace build has the same file name and no exports.
* `unsafe` is confined to `xsim-dpi/src/win.rs`, not the `interop` crate as first written above.

## Sequencing note

To get a verifiable PS+PL path early and without any RTL simulator, P1 first runs QEMU against
**native Rust PL models** (`pl-native`), which also gives CI a PL path on every OS where QEMU is
installed. The XSim adapter follows, then Icarus.
