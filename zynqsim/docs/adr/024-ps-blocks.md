# ADR-024: PS blocks the co-sim tree names but this QEMU does not map

Status: **Accepted** (2026-10-16). Opt-in. Date of the measurements: 2026-10-16, Vitis 2022.1 Xilinx QEMU
6.1.0 (`arm-generic-fdt-7series`, `zynq-arm-cosim.dtb`), one machine.

## Context

The conformance work (ADR-023, `docs/conformance/*.md`) measured that the PS GPIO (0xE000A000), SPI0/1
(0xE0006000/0xE0007000), devcfg (0xF8007000) and the SCU watchdog are not mapped in the QEMU machine: reads
return 0 and writes are dropped. `zynq-arm-cosim.dtb` does name them: `gpio@e000a000` (`xlnx,zynq-gpio-1.0`,
enabled), `spi@e0006000`/`spi@e0007000` (`xlnx,zynq-spi-r1p6`, `status = "disabled"`), `devcfg@f8007000`
(`xlnx,zynq-devcfg-1.0`). The question was whether the binary has a model for them and how the tree gets
attached to it.

## What the binary contains (VERIFY: from `strings` and the device list, not from source)

* `qemu-system-aarch64 -device help` lists `xlnx.zynq-pll`, `xlnx.efuse`, `xilinx_spi`; the Zynq models
  are not user-creatable. The QOM type names in the binary include `xlnx.ps7-spi` (the Zynq SPI controller),
  `xlnx.ps7-dev-cfg` (devcfg), `xlnx.zynqmp-gpio` (a ZynqMP GPIO model, `xlnx-zynqmp-gpio.c`), `cdns.spi-r1p6`,
  `cdns.i2c-r1p10`, `cdns.ttc`, `cdns.uart`, `xlnx.zynq-xadc`.
* No Zynq GPIO model (`xlnx,zynq-gpio-1.0`) exists in the build.
* The FDT compatible table of this build recognises a short list of `compatible` strings (for example
  `xlnx,zynq-qspi-1.0`, `cdns,zynq-gem`, `xlnx,zynq-efuse`, `silabs,si570`). `cdns,i2c-r1p10` is not among
  them, yet the I2C controller is mapped, so the binary also resolves a compatible string through its QOM
  name (`cdns,i2c-r1p10` -> `cdns.i2c-r1p10`). This is the working hypothesis for why `xlnx,zynq-spi-r1p6`
  (no `xlnx.zynq-spi-r1p6` type) is not mapped.

## Decision

1. **Rename, do not add.** `dtb::patch_ps_blocks` rewrites the `compatible` (and `status`) of the nodes in
   `PS_BLOCK_EDITS` to a string this build resolves: GPIO -> `xlnx,zynqmp-gpio` (the ZynqMP GPIO model),
   SPI0/1 -> `cdns,spi-r1p6` (the Xilinx SPI model, `status = "okay"`), devcfg -> `xlnx,ps7-dev-cfg` (a
   guess). The tree is re-encoded through the same structure-block editor as `patch_qspi` (ADR-012).
2. **Opt-in.** `[ps] extra_blocks = true` in `project.toml` (`PsSection` in `sim-api::project`) turns it on.
   Off by default, because the mapped models FAIL some checks of our conformance firmware, and the pinned
   tests assert `RESULT: ALL PASS` for the default. The default run is byte-identical to before.
3. **Recorded findings, not hidden.** `crates/cli/tests/conformance_ps_blocks.rs` pins the opt-in per-check
   statuses. A FAIL is a difference between the mapped model and the firmware's expectation; it is either a
   model deviation or a wrong remembered value (VERIFY), and this ADR does not decide which.

## Measured results with `extra_blocks = true`

| Block | Firmware | Result | Test |
|---|---|---|---|
| GPIO (ZynqMP model under the Zynq name) | `conf-gpiops` | 9 PASS (DIRM/OEN readback, masked writes, INT enable/disable, INT type, INT status, DATA_RO), **2 FAIL**: check 1 reset (`0x0000ffff`, want 0), check 7 bank 2 width (`0x03ffffff`, want `0x003fffff`: the ZynqMP bank 2 has 26 pins, the Zynq one 22), 1 N/A (no pin source) | `gpio_mapped_model_fails_bank_geometry_and_reset_checks` |
| SPI0 (Xilinx SPI model) | `conf-spips` | 20 PASS, **3 FAIL** (check 11 SS/CS read back `0x7800`, want `0x17800`; checks 19 and 20: TX FIFO full at 128 entries), 4 N/A | `spi_mapped_model_passes_most_checks_and_fails_three` |
| SPI Xilinx selftest | `bsp_spips_selftest` | **passes** (default: fails, `Delay` readback 0) | manual run, not pinned |
| devcfg | `bsp_devcfg_selftest` | **still fails** with `xlnx,ps7-dev-cfg` (the guess); whether that name is wrong or the model is unusable is not known | manual run, not pinned |

The default run keeps the N/A lists of `conformance_gpio_timer`, `conformance_iic_spi_xadc` and `bsp_apps`
unchanged (they pass unmodified).

## What is not done

* **A9 timer rate (333 MHz vs 100 MHz).** Not changed. The timers reference clock 4 of the SLCR clock
  controller (`clocks = <&clkc 4>` on `timer@f8f00200` and `timer@f8f00600`); the rate is therefore set by
  the SLCR clock model (`zynq_slcr.c` computes it from the PLL and divider registers), not by a DTB property
  on the CPU or the timer node. The CPU node has no `clock-frequency`. VERIFY: whether programming the SLCR
  PLL registers from firmware (a partial `ps7_init`) moves the rate; not tried. No `-icount` option changes
  the frequency, only the instruction-to-time mapping.
* **devcfg** (see the table): unmapped in practice. The QOM type exists; the compatible string does not.
* **SCU watchdog** (`0xF8F00620`): not touched.
* **GPIO geometry**: the ZynqMP model is not the Zynq GPIO (different bank widths). Its checks are kept
  as FAILs in the pinned test rather than adjusted.
* **Hardware**: nothing here is compared with a board. The mapped models are QEMU's, not Zynq silicon.

## Consequences

* `[ps] extra_blocks = true` is the only way to get these models; the default stays as measured in ADR-023.
* A future QEMU build may register the compatible strings directly; then `PS_BLOCK_EDITS` should be
  reduced to what is still needed, and the pinned statuses re-measured.

## Appendix (2026-10-17): superseded in part by ADR-029

* The A9 timer rate is **not** set by the SLCR (the statement under "What is not done" above is wrong): the
  global timer takes a `clock-frequency` on its own node, now set by `[ps] cpu_clock` (default on). The
  private timer is still at 100 MHz.
* The `devcfg@f8007000` -> `xlnx,ps7-dev-cfg` edit had no effect (the unmapped devcfg stays unmapped) and
  was removed from `PS_BLOCK_EDITS`.
* CAN0 is mapped by default through `dtb::DEFAULT_EDITS` (`xlnx,ps7-can-1.00.a`).
* The SCU watchdog remains without a model.
