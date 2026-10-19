# ADR-042: Clocking Wizard model (`vendor-ip`)

Status: **Accepted**. Date: 2026-10-19. (ADR-041 is not in the repository; this number follows ADR-040.)

## Context

The Clocking Wizard (`xilinx.com:ip:clk_wiz:6.0`, Xilinx PG065) was classed as passive IP since ADR-021: its clocks come from the XSA handoff and its registers were not modelled. A design whose firmware reconfigures its clocks through the AXI4-Lite interface (dynamic reconfiguration) was therefore reported as unmodelled: the importer treats a passive IP with an address window of its own as unmodelled (`sim-api::xsa`). This is the next candidate of the IP model list in HANDOVER section D.

## Decision

* **Register-level model in `crates/vendor-ip/src/clk_wiz.rs`** as a `pl_native::RegisterBlock`:
  * `SRR` (0x000): software reset, key `0xA`; resets the registers and drops `LOCKED`.
  * `SR` (0x004): bit 0 `LOCKED`. It rises `LOCK_CYCLES` (1000) ACLK cycles after a power-on, a reset or a reconfiguration.
  * `ISR` (0x00C) and `IER` (0x010): write-1-to-clear status and enable. Bit 0 `LOCK_CHANGED` is set on every `LOCKED` transition. Bit 1 `CLK_GLITCH` is set only by a host-side injection (`ClkWizHandle::inject_clock_glitch`). `irq = (ISR & IER) != 0`.
  * `CCR0` (0x200), `CCR1` (0x204), `CCR2` (0x208): clocking configuration words (DIVCLK and CLKFBOUT multiply, CLKFBOUT fraction, CLKOUT0 divide and phase). Stored as written.
  * `SADDR` (0x25C): writing bit 0 = 1 starts a dynamic reconfiguration: `LOCKED` falls and rises again after the lock time.
* **The model does not change any clock.** Output frequencies still come from the handoff. The configuration registers are stored, not applied, so a reconfiguration only affects `LOCKED` and the interrupt.
* **Integration:**
  * Listed in `vendor_ip::MODELS` and `param_names` (`PRIM_IN_FREQ`, `CLKOUT1_REQUESTED_OUT_FREQ`, `USE_DYN_RECONFIG`); `CLK_FREQ_HZ` (the ACLK) sets the lock time, as for the SPI model.
  * Re-exported in `sim-api::pl_attach::NATIVE_MODELS`.
  * XSA mapping: `xsa-import` keeps `classify()` unchanged (a clock wizard is `Infrastructure` by VLNV, as its test requires). After the address windows are assigned, a `clk_wiz` instance with an AXI4-Lite window from `M_AXI_GP0`/`M_AXI_GP1` is reclassified as `VendorIp { name: "clk_wiz" }`, so `sim-api::xsa` maps it to `[native]`. A clock wizard without a window stays passive.
* **Verification and guest firmware:**
  * Guest firmware `firmware/clk_wiz_demo/src/main.rs`, compiled to `fixtures/elf/rust_clk_wiz.elf` via `cargo xtask firmware`. It checks power-on lock, register retention, reconfiguration with relock, the lock-change interrupt on `IRQ_F2P[1]` (GIC 62), and the software reset.
  * Example project `examples/clk-wiz-demo/project.toml`.
  * Unit tests per register in `vendor-ip::clk_wiz::tests`.
  * End-to-end: `cli::native_pl::clocking_wizard_firmware_runs_on_the_native_ps_with_the_vendor_model`, `sim-api::vendor_ip2::the_clock_wizard_model_relocks_after_reconfiguration_and_raises_its_interrupt`, and `sim-api::xsa_import::a_clock_wizard_with_an_axi_lite_window_is_mapped_as_a_native_model`.
  * Added to `crates/cli/tests/determinism.rs` (5 identical runs).

## Not verified (`VERIFY:`)

Register facts were written from the product guide as remembered, and were not checked against hardware or a Xilinx simulation: the `SRR` key and the `LOCKED` bit (the convention of the AXI clocking cores), the `ISR`/`IER` bit assignment and interrupt output type, the `CCR` field layout and reset values, the `SADDR` load encoding (bit 0 is an assumption; the real core may use a different command value) and its read-back, the restart behaviour of a second load during a relock, and the lock time (a model constant, not a PG065 figure). The `interrupt` and `S_AXI_LITE` port names in the example `[connect]` table are also assumptions.

## Consequences

* A design whose firmware reaches a Clocking Wizard's AXI4-Lite interface runs its `LOCKED` polling and reconfiguration sequence natively. Before this ADR it stopped the run as unmodelled.
* The model is honest about what it omits: clock frequencies, MMCM/PLL behaviour, `CLKOUTn` outputs, `PRIM_IN_FREQ`-derived clocks, glitch and stop detection, and the dynamic reconfiguration sequence beyond the load bit.
* `#![forbid(unsafe_code)]` is preserved across all workspace crates.
