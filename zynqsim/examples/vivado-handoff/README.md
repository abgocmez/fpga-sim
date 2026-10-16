# vivado-handoff: the P3 exit demonstration

A Vivado-exported design and its `BOOT.BIN`, run end to end (milestone P3, ADR-019).

```
fixtures/handoff/
  make_handoff.tcl   Vivado 2022.1 batch flow: ONE block design -> design.xsa AND export/
  design.xsa         write_hw_platform -fixed of that design
  export/xsim/       export_simulation -simulator xsim of the same Vivado project
  rtl/               the user RTL (my_math_ip.v) and the ZynqSim testbench the export names
  boot.bif, BOOT.BIN bootgen 2022.1: stub FSBL + placeholder bitstream + the firmware ELF
firmware/handoff_demo   the firmware (Rust, no_std); prebuilt: fixtures/elf/rust_handoff_demo.elf
```

The block design (Zybo Z7-20, FCLK0 100 MHz): the Zynq PS, the user's RTL `my_math_ip` as a
module reference (`0x43C00000`, in XSim), an AXI GPIO on the LEDs (`0x41200000`), an AXI Timer
(`0x42800000`, `IRQ_F2P[0]`) and an AXI UART Lite (`0x40600000`, `IRQ_F2P[1]`; the Xilinx IP run
as `vendor-ip` models). The firmware checks seven math vectors, walks and sets the LEDs, takes
three timer interrupts, sends `HANDOFF UART OK` through the UART Lite and prints
`RESULT: ALL PASS`.

## Run it

One `init` makes the project (XSA-derived PL mapping and `[boot] mode = "boot_bin"`):

```
zynqsim init --xsa fixtures/handoff/design.xsa --rtl fixtures/handoff/rtl/my_math_ip.v \
             --boot-bin fixtures/handoff/BOOT.BIN --out handoff-xsa
zynqsim run --project handoff-xsa/project.toml --for 8ms
```

The second variant takes the user RTL and its testbench from the `export_simulation` folder
(the Xilinx IP still come from the XSA: the export does not carry them):

```
zynqsim init --xsa fixtures/handoff/design.xsa --export-sim fixtures/handoff/export/xsim \
             --boot-bin fixtures/handoff/BOOT.BIN --out handoff-export
zynqsim run --project handoff-export/project.toml --for 8ms
```

Both print the same UART text and have the same trace digest (the test
`crates/cli/tests/handoff.rs`). MCP: `project_create_from_xsa` (arguments `boot_bin`,
optionally `export_path`) then `project_open` and `sim_run`.

## What this is not

* The FSBL in `BOOT.BIN` is a **stub** (four `b .` instructions): it is not executed, and no
  real FSBL was built or run. Its register-init table is listed and **not applied**; `ps7_init`
  is not applied either.
* The bitstream partition is a **placeholder** (`.bit` container, not a design). No bitstream is
  simulated; the PL is the user RTL in XSim plus the vendor models.
* The register maps of the Xilinx IP models are from the product guides, not checked against
  hardware (`VERIFY:` items in `docs/STATUS.md`). Nothing here ran on a board.

Rebuild everything: `cargo xtask firmware`, then in `fixtures/handoff` run `bootgen -arch zynq
-image boot.bif -w on -o BOOT.BIN`; the Vivado part is documented at the top of
`make_handoff.tcl`.
