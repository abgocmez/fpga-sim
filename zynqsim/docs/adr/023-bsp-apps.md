# ADR-023: Unmodified Xilinx BSP applications as the register-level conformance suite

Status: **Accepted** (milestone P4). Date: 2026-10-16.

## Context

Prompt §0 success criterion 2 and §14.2 ask for "unmodified Xilinx standalone BSP applications
(UART, GPIO, GIC, timers, QSPI, SD/FatFs, GEM/lwIP, I2C, SPI, XADC, PL330 DMA)" to behave as on
hardware at register level. Until now every ELF in `fixtures/` was either the prototype's own C
(`sw/main.c` and the DMA/board demos, which use only `Xil_In32/Out32` and `xil_printf`) or Rust
firmware written for this project, so the Xilinx drivers themselves had never run, and the Rust
drivers were written from memory of the same registers (`VERIFY:` in STATUS.md). The question was
whether Vitis 2022.1 on this machine can produce Xilinx applications without a GUI, offline, and what
the Xilinx QEMU does with them.

## Decision

* **Build with the Vitis batch tools, author no C.** `xsct` (`platform create -hw`, `platform
  generate`, `app create`, `app build`; the route the prototype's `build_app.tcl` used) generates a
  standalone BSP (standalone_v7_7 and the PS7 drivers) and builds:
  1. Xilinx's two application templates, **unmodified**: "Hello World" and "Peripheral Tests"
     (Vitis itself writes `testperiph.c` from each driver's `*_tapp.tcl`);
  2. one Xilinx **driver example file** per application (`drivers/<drv>/examples/*.c`). Each carries
     its own `main()`, so it is imported into an "Empty Application(C)" as it is. No wrapper main
     was needed, and none exists.
  `fixtures/bsp/build_bsp.tcl` is the whole recipe; the Xilinx sources are copied from the install at
  build time and are **not** committed. Only the scripts and the ELFs are.
* **A PS-only hardware platform** (`fixtures/bsp/zs_ps_all.xsa`, `make_ps_all_xsa.tcl`, Vivado
  2022.1 batch): the Zybo preset plus TTC0, I2C0, SPI0 and CAN0 on EMIO, so the BSP contains the
  driver of every PS peripheral. The earlier `zs_math_only.xsa` generated a BSP without TTC, I2C, SPI
  and CAN, because Vitis only includes drivers of enabled peripherals.
* **`-Os`, no `-g`, section GC**, so each ELF is < 300 KB (20-65 KB of code; the rest of the file is
  64 KB segment alignment).
* **Two kinds of expectation, never mixed** (`crates/cli/tests/bsp_apps.rs`): `PASS` (the application
  prints its own success line) and `DEVIATION` (it does not, and the cause is the QEMU machine). A
  deviation is asserted as the observed failure text, so a model that gains a device fails the test
  and forces `docs/conformance/bsp-apps.md` and STATUS.md to be updated. A deviating application is
  not hidden or skipped.
* **Slow ones are handled by the project, not the test harness.** A PS-only run's result does not
  depend on the PS/PL quantum, so `bsp_ttcps_intr` (5 s of simulated PWM period) uses
  `quantum_ns = 100000` and takes about 25 s of wall time instead of about 17 minutes.
  `bsp_emacps_intr_dma` needs about 40 simulated seconds and stays an `#[ignore]` test.
* **Examples** `examples/bsp-hello` and `examples/bsp-peripherals` (the two templates) join the
  five-process determinism test (`determinism.rs`).

## Consequences

* First real measurement of criterion 2: of the 22 driver examples, 16 ran to their own success
  line unmodified (one of them, the XADC, with zeros as data, and the GEM one only after about 40
  simulated seconds); the other 6 fail or hang because the QEMU machine lacks the peripheral (PS
  GPIO, SPI, CAN, device configuration, SCU watchdog). Hello World passes; Peripheral Tests reports
  seven PASSED and three FAILED and then hangs in the watchdog test. A third finding is not a failure: the A9
  MPCore timers run at 100 MHz, so BSP delays are 3.3x long. See the conformance document.
  Findings are recorded as findings.
* The suite is a regression net for any later change to the QEMU adapter or for a different PS
  engine (P5): the same ELFs, the same expected text.
* Not done: lwIP and FatFs applications (extra BSP libraries and a second platform generation),
  flash/EEPROM examples (need bus devices), anything behind a real FSBL/`ps7_init`, any comparison
  with hardware. Criterion 2 is therefore **partly** shown, not met: GPIO, SPI, CAN, USB and the
  watchdog fail because the Xilinx QEMU 2022.1 machine does not have them, which only a different
  PS model (or a QEMU with those devices) can change.
* The ELFs contain Xilinx code (MIT) plus newlib and libgcc; licences are in
  `fixtures/bsp/SOURCE.md`.
* The probe used to isolate the deviations (a throw-away Rust firmware reading registers, and
  `info mtree` of the same QEMU command line with a dummy remote-port peer) is not part of the
  repository; its findings are in the conformance document.
