# ADR-019: P3 exit demonstration (a Vivado design and its BOOT.BIN, end to end)

Status: **Accepted** (end of milestone P3). Date: 2026-10-16.

## Context

Prompt §15 gives milestone P3 the exit criterion "a Vivado-exported design and its `BOOT.BIN` run
end to end". The pieces existed separately: `init --xsa` (ADR-015), the Xilinx IP models
(ADR-016), `export_simulation` ingestion (ADR-017) and `BOOT.BIN` boot (ADR-018), but each had
its own fixture, and `init --boot` and `init --xsa` / `--export-sim` were separate commands, so no
single project had both an XSA-derived PL and a boot image. This ADR records the one coherent
demonstration, how it is built, and, precisely, what it does and does not prove.

## Decision

* **One block design, two Vivado hand-offs.** `fixtures/handoff/make_handoff.tcl` (Vivado 2022.1,
  batch) builds a Zybo Z7-20 block design: Zynq PS7 (FCLK0 100 MHz), the user RTL `my_math_ip` as
  a module reference (`0x43C00000`), an AXI GPIO on four LEDs (`0x41200000`), an AXI Timer
  (`0x42800000`, `IRQ_F2P[0]`) and an AXI UART Lite (`0x40600000`, `IRQ_F2P[1]`), interrupts
  through an `xlconcat`. The same project writes `design.xsa` (`write_hw_platform -fixed`, after
  one small synthesis, as in ADR-015) and `export/xsim` (`export_simulation -simulator xsim`).
* **The firmware** `firmware/handoff_demo` (Rust, `no_std`) exercises every piece: seven vectors
  on the user RTL, a walk and a pattern on the GPIO LEDs, three timer interrupts, a UART Lite
  line checked through its status register; `RESULT: ALL PASS` only if every check held.
* **`BOOT.BIN`** is `bootgen -arch zynq -image boot.bif` (Vitis 2022.1): the stub FSBL and the
  placeholder bitstream of `fixtures/boot` plus the firmware ELF.
* **One `init`.** `--boot` is now also `--boot-bin` and is accepted next to `--xsa` and
  `--export-sim` (before, `--boot` silently ignored them). The planners (`xsa::plan`,
  `exportinit::plan_init`) take an additive `boot_bin` option: the image is validated like
  `init --boot` validates it (a damaged or unrunnable one creates nothing), copied to `BOOT.BIN`
  in the project directory, `[boot] mode = "boot_bin"` is written, `[cpu0]` is not (an `--elf` is
  ignored with a warning), and the report lists what a BOOT.BIN boot does not do. `init --boot`
  alone and `init --xsa --elf` are unchanged (tested). MCP `project_create_from_xsa` and
  `project_create_from_export` take `boot_bin`.
* **`--xsa` with `--export-sim`** (`ImportOptions.export`, MCP `export_path`): the handoff maps
  the address map, interrupts, clocks and the Xilinx IP (vendor-ip models), and the user RTL and
  its testbench are taken from the export (`[rtl] export_dir`, the export's own testbench if one
  of its sources calls the `rp_*` DPI functions, else a generated one). This was needed because
  the export does not carry the Xilinx IP (below): an export alone cannot run this firmware.
  Combining it with `--rtl` or `--filelist` is refused.

## What the Vivado flow produced (observed, 2022.1)

* `export_simulation` of a project that has the block design lists only what the `sim_1`
  testbench reaches: the user RTL, the testbench and `glbl.v`, plus the Vivado-installed
  `xilinx_vip` and `xpm` library sources (`vlog.prj`, `vhdl.prj`) with `-L xilinx_vip -L xpm ...`;
  the block design's Xilinx IP (AXI GPIO, Timer, UART Lite, the PS7 model) are **not** in it. The
  Xilinx IP therefore always come from the XSA, as vendor-ip models.
* Vivado writes absolute paths (the install root, the scratch project's include directories, the
  source files). The only edits to its output are listed in `make_handoff.tcl`: sources relative
  to the export, the install root as `$XILINX_VIVADO`, scratch `--include` entries dropped.
* `rename clock` (the workaround for this machine's Vivado install lacking `clock.tcl`, used by
  `fixtures/export/make_export.tcl`) must not be done before the block design is built: the
  `ipintegrator` feature then fails to load. The script defines it just before
  `export_simulation`.
* The XSA is 327 KB, the export about 70 KB, `BOOT.BIN` 12 KB.

## Results (Windows, real QEMU + XSim; tests named in STATUS.md)

* The firmware boots from the `BOOT.BIN`, talks to the user RTL in XSim and to the three models,
  and prints `RESULT: ALL PASS`; the PL UART Lite line is `HANDOFF UART OK`; LED0 and LED3 are on.
* The export variant (RTL and testbench from the export, Xilinx IP from the XSA) prints the same
  UART text and has the same trace digest and record count. This holds because both variants run
  the same testbench: `rtl/cosim_top.sv` is the one `zynqsim init --ip` generates, which is also
  what `init --xsa` generates. The hand-written `fixtures/rtl/math/cosim_top.sv` in the export
  ran the same firmware to the same UART text and 186 bus accesses, but with 674 trace records
  instead of 675: the generated testbench reports a `pl finished` event at the end-of-test write,
  the hand-written one does not (observed by hand with the Perfetto export, not a test).
* Booting the same firmware as an ELF instead of from the `BOOT.BIN` gives the same UART text and
  the same records except the `qemu start elf=NAME` note, whose file name differs
  (`bootbin-app.elf` vs `rust_handoff_demo.elf`), so the digests differ; checked record by record.
* Three runs of the BOOT.BIN project give one trace digest.

## Exit criterion: status

**Met in the following restricted sense, not in the full one.** A Vivado 2022.1 hardware platform
and simulation export of one real block design, and a BOOT.BIN made by Vitis `bootgen` around
firmware for that design, run end to end in the simulator with the PS in QEMU, the user RTL in
XSim and the Xilinx IP as models, giving a deterministic result.

What is **not** the real Vivado/FSBL flow, and what the claim does not cover:

* **The FSBL is a stub** (four `b .` instructions) and is **never executed**. No FSBL was built,
  and prompt §12's "run the FSBL as software" is not done.
* **No bitstream is simulated.** The bitstream partition is a placeholder `.bit` container, never
  read as a design; the PL is the RTL plus models. Nothing checks that the RTL and the models are
  what the bitstream would contain, or that the design implements.
* **`ps7_init` is not applied**, neither the file in the XSA nor the register-init table in the
  image (listed, not applied). The firmware does the SLCR PL bring-up itself (`rt::pl_bringup`).
  The PS clock tree, MIO and peripheral enables of the XSA change nothing.
* **The Xilinx IP are register-level models** written from the product guides, never compared
  with a Xilinx simulation or a board (`VERIFY:` list in STATUS.md); timing is quantum-granular
  (interrupts at most 1 us late).
* **Nothing ran on a board.** One design only: other Vivado versions, SmartConnect, hierarchies,
  several user slaves, VHDL IP and DMA-class IP are untested or unmodelled.
* **The export is not a full-design simulation.** Vivado's own simulation of the block design (PS7
  VIP, IP libraries) is not run; the export is used for the user RTL and testbench only.

## Consequences

* The three P3 deliverables (XSA, export, BOOT.BIN) compose in one project and one command; the
  remaining gaps to the strong reading of the criterion are an FSBL/`ps7_init` replay, a real
  bitstream flow and cycle-accurate or vendor-supplied IP simulation models.
* `ImportOptions` and `InitOptions` gained fields (`boot_bin`, `export`); callers built with
  struct-update syntax are unaffected.
