# ADR-015: Hardware handoff (XSA/HWH) ingestion

Status: **Accepted** (start of milestone P3). Date: 2026-10-16.

## Context

Prompt §9.1/§12 wants `zynqsim init --xsa design.xsa` to turn what Vivado hands off into a
project: address map, clocks, interrupts, PS configuration, the user's RTL. Until now a project
was written by hand or generated from one IP module header (`init --ip`, ADR in
`tbgen.rs`). The block design's own description (the HWH inside the XSA) was ignored.

Two things make this risky to over-promise: the HWH is Vivado's internal exchange format (no
published schema), and a typical design contains Xilinx IP (AXI GPIO, timers, DMA, ...) for which
ZynqSim has no model yet (`vendor-ip`, milestone P3, not built).

## Decision

* **A new pure-Rust crate `xsa-import`** reads an `.xsa` (zip, via the `zip` crate) or a bare
  `.hwh` (XML, via `roxmltree`) into a `Design`: the PS7 configuration, every other module with
  VLNV, parameters, ports, bus interfaces, address windows, the interrupts resolved to
  `IRQ_F2P` bits (through `xlconcat`), the clock and reset sources, and the HDL/file-list/
  `ps7_init` files in the archive. It knows nothing about projects. `sim-api` re-exports it as
  `sim_api::xsa`, so front ends keep depending on `sim-api` only.
* **`sim_api::xsa::plan` maps a `Design` to a `Project`** and renders `xsa-report.md`; nothing is
  written until `XsaImport::write` (which refuses to overwrite). The mapping:
  * user RTL (module references `xilinx.com:module_ref:*`, packaged `user` IP) whose source is
    in the archive or passed with `--rtl` and is an AXI4-Lite slave the existing testbench
    generator (`tbgen`) understands becomes `[rtl]` plus a generated `tb/cosim_top.sv`; its
    interrupt outputs are forwarded on the `IRQ_F2P` bit the block design uses, and the
    testbench clock follows the design's clock frequency;
  * the address map, interrupts and FCLK clocks of every non-wiring instance go into `[connect]`
    (informational for the RTL path: the generated testbench answers the whole `M_AXI_GP0`
    window with the one module, as `init --ip` always did);
  * every other IP is **recorded, never silently dropped**, in `[xsa.unmodelled]`:
    recognised Xilinx IP (`needs a vendor-ip model (milestone P3, not built yet)`), unknown
    Xilinx IP, third-party IP, user RTL without sources or outside the generator's subset, and a
    second user slave. Each entry is a validation **error**, so `run`/`project_open` stop with a
    diagnostic that names the instance, VLNV, address and interrupt bits.
  * wiring IP (interconnects, `proc_sys_reset`, `xlconcat`, ...) is classified and reported as
    "not simulated", not as missing.
* **The project is generated even when it cannot run.** The report's verdict says so in words; a
  design with no unmodelled IP is *not* declared correct either, only "nothing unmodelled".
* **The PS7 configuration is read and reported, not applied.** QEMU's Zynq machine has every PS
  peripheral and runs the ELF directly; MIO routing, peripheral enables, the PS clock tree and
  `ps7_init` have no effect and the report's *Not applied* section says so.

## HWH fields read (Vivado 2022.1; `VERIFY:` = seen in one Vivado version only)

| Element / attribute | Used for |
|---|---|
| `EDKSYSTEM@VIVADOVERSION`, `@TIMESTAMP` | report |
| `SYSTEMINFO@NAME, ARCH, DEVICE, PACKAGE, SPEEDGRADE, BOARD` | report |
| `MODULE@INSTANCE, MODTYPE, VLNV, FULLNAME` | identity and classification (`xilinx.com:module_ref:` = module reference, `:ip:` = Xilinx IP, `processing_system7` = PS) |
| `MODULE/PARAMETERS/PARAMETER@NAME,VALUE` | all parameters kept per IP; PS7 `PCW_*` read as below |
| `MODULE/PORTS/PORT@NAME, DIR, LEFT, RIGHT, SIGIS, SENSITIVITY, CLKFREQUENCY, POLARITY` | widths, interrupt/clock/reset ports, frequencies, edge/level |
| `PORT/CONNECTIONS/CONNECTION@INSTANCE, PORT` | where an interrupt/clock/reset comes from; `IRQ_F2P` resolution through `xlconcat` (`In<k>`, widths, consecutive slices) |
| `MODULE/BUSINTERFACES/BUSINTERFACE@NAME, TYPE, DATAWIDTH, BUSNAME, VLNV` + `PARAMETER PROTOCOL` | bus interface list, AXI flavour; PS7 `M_AXI_GP*`/`S_AXI_*` presence |
| `MODULE/MEMORYMAP/MEMRANGE@INSTANCE, MASTERBUSINTERFACE, SLAVEBUSINTERFACE, ADDRESSBLOCK, BASEVALUE, HIGHVALUE, MEMTYPE` | the PS-to-PL address map (PS7's map), PL masters' windows (for example `S_AXI_HP0`) |
| PS7 `PCW_EN_CLKn_PORT`, `PCW_FPGAn_PERIPHERAL_FREQMHZ`, `PCW_ACT_FPGAn_PERIPHERAL_FREQMHZ`, port `FCLK_CLKn@CLKFREQUENCY`, `PCW_EN_RSTn_PORT` | FCLK0-3 enable and frequency, reset ports |
| PS7 `PCW_USE_M_AXI_GPn`, `PCW_USE_S_AXI_GPn`, `PCW_USE_S_AXI_HPn`, `PCW_USE_S_AXI_ACP` | enabled AXI ports |
| PS7 `PCW_USE_FABRIC_INTERRUPT`, `PCW_IRQ_F2P_INTR`, port `IRQ_F2P@LEFT/RIGHT`, `PCW_NUM_F2P_INTR_INPUTS` | IRQ_F2P use and width |
| PS7 `PCW_<P>_PERIPHERAL_ENABLE` or `PCW_EN_<P>`, `PCW_<P>_<P>_IO` | UART0/1, ENET0/1, SD0/1 (`PCW_EN_SDIO0`), USB0/1, QSPI, I2C0/1, SPI0/1, CAN0/1, TTC0/1, WDT enablement and IO |
| PS7 `PCW_GPIO_MIO_GPIO_ENABLE`, `PCW_GPIO_EMIO_GPIO_ENABLE`, `PCW_GPIO_EMIO_GPIO_WIDTH` | GPIO and EMIO GPIO |
| PS7 `PCW_MIO_TREE_PERIPHERALS`, `PCW_MIO_TREE_SIGNALS` | MIO 0..53 use, by peripheral and signal (`VERIFY:` present only with a board preset or a configured MIO) |

Zip entries read: `*.hwh`, `*.v/.vh/.sv/.svh/.vhd/.vhdl`, `*.prj`, `*.f`; names only for
`ps7_init.*` and `*.bit`. Entries with absolute or `..` paths are refused. The `.prj` list
(`export_simulation`) and `.f` lists are parsed (`parse_filelist`) and, with `--filelist`, their
Verilog/SystemVerilog files join the RTL; VHDL is listed but not compiled (XSim here is driven
with `xvlog -sv` only).

## What was observed

* A real Vivado 2022.1 XSA (`fixtures/xsa/zs_math_gpio.xsa`, made by `fixtures/xsa/make_xsa.tcl`)
  holds `design_1.hwh`, `hwdef.xml`, `xsa.xml`, `xsa.json`, `ps7_init.{c,h,tcl,html}`,
  `ps7_init_gpl.{c,h}`; a module reference's RTL is **not** in it (so `--rtl` is needed).
  `write_hw_platform -fixed` before synthesis writes no HWH ("no IPI block design hardware
  handoff files present"), and with only `generate_target all` it still fails to include it; the
  script runs a small synthesis first.
* In 2022.1 the PS-to-PL address map is in the PS7 module's `MEMORYMAP`, and each slave also
  carries `C_BASEADDR`/`C_HIGHADDR` parameters (only the former is used).

## Consequences

* The honest limits are written down: nothing validates the address map against the RTL (the
  generated testbench ignores upper address bits), vendor IP is refused rather than approximated,
  one user AXI4-Lite slave per project, no VHDL, PS configuration not applied.
* Follow-ups in P3: `vendor-ip` models (each removes entries from the unmodelled list), a
  multi-instance testbench with real address decode, `ps7_init` data applied to QEMU, `BOOT.BIN`.
