# ADR-029: PS timer rate, CAN0, and what still cannot be mapped (second pass on ADR-024)

Status: **Accepted** (2026-10-17). Measured on Vitis 2022.1 Xilinx QEMU 6.1.0 (`arm-generic-fdt-7series`,
`zynq-arm-cosim.dtb`), one machine. Everything about QEMU's internals below is inferred from the binary's
strings and from behaviour observed through ZynqSim, not from QEMU source (VERIFY where it matters).

## Context

ADR-024 left four open items in the PS-block work: the A9 timers tick at 100 MHz instead of 333 MHz (BSP
`sleep()` ~3.3x too long), the devcfg self test fails, the SCU watchdog never returns, and CAN is unmapped.
The reference clock tree is the Zynq one: PS_CLK 33.333 MHz, ARM PLL 1333.33 MHz (`ps7_init`, from the
`ps7_init.c` inside `fixtures/bsp/zs_ps_all.xsa`: `ARM_PLL_CTRL` FDIV 0x28), CPU_6x4x = 666.67 MHz, CPU_3x2x
(the timers' clock, `clocks = <&clkc 4>` in the tree) = 333.33 MHz.

## Experiments (probe firmware `firmware/timer_rate`, DTB variants through `ZYNQSIM_DTB`, not committed)

Probe: a Rust `no_std` loop that waits until the global timer (GT) advances by 1,000,000 ticks, then the private
timer (PT) by as much, printing `DONE_GT` / `DONE_PT`; the simulated time at which the line appears gives the rate.
Stock tree: GT done at about 10 ms (100 MHz).

| Change | GT | PT | Note |
|---|---|---|---|
| none | 100 MHz | 100 MHz | reset |
| SLCR: ARM PLL to FDIV 40, bypass off, CPU clock enabled (the `ps7_init` ARM sequence, from firmware) | 100 MHz | not measured | the SLCR model does not feed the timers |
| `clkc` `ps-clk-frequency` 33.3 -> 66.7 MHz | 100 MHz | not measured | ignored by the timers |
| `clkc` `clock-frequency` = 333.3 MHz (renamed property) | 100 MHz | not measured | ignored |
| timer `clocks` cell -> another clkc index (3 or 6) | 100 MHz | 100 MHz | index has no effect |
| `cpu@0` `clocks` (renamed to `clock-frequency`) = 333.3 MHz | - | 100 MHz | no effect on PT |
| **timer@f8f00200 `clock-frequency` = 333333333** (added, `clocks` kept) | **333 MHz** (done in 3.0-3.5 ms) | - | a9gtimer reads this property |
| same on `timer@f8f00600` (`arm_mptimer`) | - | **100 MHz** | `arm_mptimer` ignores it |

So the global timer takes its rate from a `clock-frequency` on its own node (QEMU's `a9gtimer` has the
property; the strings contain `clock-frequency` next to `num-cpu`). The private timer's `arm_mptimer` has no
rate property in this build (its strings contain only `num-cpu`, `timerblock`, `control`, `status`, `timer`);
nothing in the tree or the SLCR changed its rate.

Device mapping (probe reads of the registers, `RESET` line and after writes):

* **devcfg (0xF8007000)**: unmapped (reads 0, writes dropped) in the stock tree. Renaming its compatible to
  `xlnx,ps7-dev-cfg` (the ADR-024 guess, the QOM type `xlnx.ps7-dev-cfg` exists) still reads 0. The binary
  contains no FDT compatible string for the devcfg model at all (`xlnx,zynq-devcfg-1.0` is absent; the only
  `xlnx.ps7-dev-cfg` string is in the non-FDT `xilinx-zynq-a9` machine, `hw/arm/xilinx_zynq.c`).
* **CAN0 (0xE0008000)**: the table has `xlnx,ps7-can-1.00.a` -> QOM `xlnx.zynqmp-can`. The tree's
  `xlnx,zynq-can-1.0` is not in the table. Renaming it maps the model, and the BSP CAN examples then pass.
* **SCU watchdog (0xF8F00620)**: no model in this build. `arm_mptimer` has no watchdog block; `xlnx.swdt`
  (`hw/watchdog/xlnx-swdt.c`) is the ZynqMP/Versal SWDT with CONTROL/RESTART/STATUS registers, not the
  Zynq SCU watchdog's LOAD/COUNTER/CONTROL/ISR/RESET_STATUS/DISABLE. No Cadence WDT model is present either
  (no `cadence_wdt` string), so the CPU watchdog at 0xF8005000 is not mapped.

## Decision

1. **A9 global timer rate (default on).** `[ps] cpu_clock` (default `"666.667MHz"`, `"reset"` keeps QEMU's
   tree) sets `clock-frequency` = CPU/2 on `timer@f8f00200` (`dtb::set_u32_property`, applied in
   `backend-qemu::QemuMachine::spawn`). The BSP's `COUNTS_PER_SECOND` is CPU/2 as well. The default was
   changed only after the affected suites stayed green (see *Gate*).
2. **CAN0 (default on).** `dtb::DEFAULT_EDITS` renames the compatible of `can@e0008000` to
   `xlnx,ps7-can-1.00.a`, applied to every run (`dtb::patch_default_blocks`). CAN1 stays disabled as shipped.
   The model is the ZynqMP CAN (VERIFY: its register map equals the Zynq CAN's; the BSP examples pass).
3. **devcfg.** Not mapped. The `xlnx,ps7-dev-cfg` edit that ADR-024 added as a guess had no effect and was
   removed from `PS_BLOCK_EDITS`; nothing in this build can instantiate the model from the tree.
4. **SCU watchdog.** Not mapped; no model.

## Not fixed

* **Private timer at 100 MHz** (D3 in `docs/conformance/bsp-apps.md`). Pinned by
  `a9_timer_rate::the_private_timer_still_ticks_at_100_mhz_a_known_deviation`. Any BSP code that waits on the
  private timer (`XScuTimer`, the scutimer examples) still sees a 3.3x longer interval than on silicon.
* **devcfg**: `bsp_devcfg_selftest` and the peripheral test's `DcfgSelfTestExample FAILED` stay as recorded.
* **SCU watchdog**: `bsp_scuwdt_polled` and the peripheral test's interrupt test never return (recorded).

## Gate

Run after the change (default on): `conformance_gpio_timer` (2), `conformance_ttc_dma` (3),
`conformance_iic_spi_xadc` (1), `conformance_ps_blocks` (2), `timer_modes` (2), `bsp_apps` (4 + 1 ignored;
`bsp_canps_polled` moved from DEVIATIONS to PASS, the peripheral test now expects CAN polled and interrupt
PASSED), `a9_timer_rate` (3, new), and `determinism` (see the report in the commit).

## Consequences

* Every QEMU run now writes its patched tree to `<work dir>/zynq-cosim-patched.dtb` (the stock file is used
  only when no edit changes it). The edits are pure functions of the input tree.
* `[ps] cpu_clock` is the only project knob for the timer rate; a firmware that programs the SLCR itself
  does not change the timers (the SLCR model does not feed them in this build).
* ADR-024's measured numbers for GPIO and SPI are unchanged (opt-in, `extra_blocks`).
