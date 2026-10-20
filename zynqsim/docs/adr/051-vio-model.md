# ADR-051: Virtual Input/Output model (`vendor-ip`)

Status: **Accepted**. Date: 2026-10-20.

## Context

The Xilinx Virtual Input/Output core (`vio` 3.0, PG159) is a hardware debugging engine used to monitor and drive internal FPGA signals in real time. In embedded systems, AXI-accessible debug hubs, or software-driven verification environments, the core is controlled via an AXI4-Lite slave interface to read live and latched probe input signals, update virtual probe output signals, detect activity or toggling on inputs, and trigger interrupts on state transitions.

Previously, `vio` was cataloged in `xsa-import::catalog` as a vendor IP, but lacked a behavioural model in `crates/vendor-ip`. Designs containing this IP were rejected as unmodelled.

## Decision

* **Register-level behavioural model in `crates/vendor-ip/src/vio.rs`** implementing `pl_native::RegisterBlock`:
  * 64 KiB address window (`0x10000` bytes):
    * `CR` (`0x000`, RW): Control Register (`CR_RESET` bit 0 self-clearing, `CR_UPDATE_OUT` bit 1 write-strobe, `CR_LATCH_IN` bit 2 write-strobe, `CR_AUTO_UPDATE` bit 3, `CR_LOOPBACK_EN` bit 4, `CR_STIM_EN` bit 5 autonomous input stimulus generator).
    * `SR` (`0x004`, RO): Status Register (`SR_OUT_UPDATED` bit 0, `SR_IN_LATCHED` bit 1, `SR_ACTIVITY_DET` bit 2, `C_NUM_PROBE_IN` in bits [15:8], `C_NUM_PROBE_OUT` in bits [23:16]).
    * `ISR` (`0x008`, RW1C): Interrupt Status Register reporting input state change (`IRQ_IN_CHANGE`), output commit acknowledge (`IRQ_OUT_ACK`), and input toggle activity (`IRQ_ACTIVITY`).
    * `IER` (`0x00C`, RW): Interrupt Enable Register matching `ISR` bits.
    * `PROBE_OUT0_VAL` (`0x010`, RW): Active value of Probe Out 0 (auto-commits when `CR_AUTO_UPDATE` is set).
    * `PROBE_OUT0_STAGING` (`0x014`, RW): Staging register for Probe Out 0 before atomic commit via `CR_UPDATE_OUT`.
    * `PROBE_OUT1_VAL` (`0x018`, RW): Active value of Probe Out 1.
    * `PROBE_OUT1_STAGING` (`0x01C`, RW): Staging register for Probe Out 1.
    * `PROBE_IN0_VAL` (`0x020`, RO): Latched value of Probe In 0.
    * `PROBE_IN0_LIVE` (`0x024`, RO): Current live value of Probe In 0.
    * `PROBE_IN1_VAL` (`0x028`, RO): Latched value of Probe In 1.
    * `PROBE_IN1_LIVE` (`0x02C`, RO): Current live value of Probe In 1.
    * `PROBE_IN_ACT` (`0x030`, RW1C): Activity and toggle detection flags per probe input.
    * `PROBE_OUT0_INIT` (`0x034`, RO): Reset/initial value of Probe Out 0.
    * `PROBE_OUT1_INIT` (`0x038`, RO): Reset/initial value of Probe Out 1.
    * `VER` (`0x03C`, RO): Core version reporting `0x0300_0000` (v3.0).
  * **Probing, Loopback, & Stimulus**:
    * Staged output updating ensures multi-word or multi-signal output probe state can be prepared in staging registers and committed synchronously to FPGA logic in a single cycle.
    * Input latching snapshots live probe inputs for coherent readback.
    * Autonomous loopback mode (`CR_LOOPBACK_EN`) routes output probes directly to corresponding input probes.
    * Autonomous stimulus generator (`CR_STIM_EN`) enables self-contained input toggling and interrupt generation on simulated time advance.
    * External logic driving and monitoring via [`AxiVioHandle`] (`set_probe_in`, `probe_out`, `probe_in_live`).
  * **Interrupt output (`irq`)**:
    * Level-sensitive active-high interrupt line asserting when `(isr & ier) != 0`.
* **Workspace Integration**:
  * Registered in `vendor_ip::MODELS`, `param_names`, `VendorPl::add`, and `VendorPl::vio`.
  * Registered in `sim-api::pl_attach::NATIVE_MODELS`.
  * Added to `crates/xsa-import/tests/ip2.rs` catalog vendor IP classification test.
  * HWH fixture `fixtures/xsa/zs_math_vio.hwh` and mapping tests in `crates/sim-api/tests/xsa_import.rs` and `crates/cli/tests/xsa.rs`.
* **Firmware and Example**:
  * Guest firmware `firmware/vio_demo/src/main.rs`, built to `fixtures/elf/rust_vio.elf` via `cargo xtask firmware`.
  * Example project `examples/vio-demo/project.toml`.
  * Determinism case in `crates/cli/tests/determinism.rs` verified 5/5 identical runs.
  * Integration test `the_axi_vio_model_runs_probes_and_interrupts` in `crates/sim-api/tests/vendor_ip2.rs`.

## Not verified (`VERIFY:`)

* More than 8 concurrent probe channels (probes 8..255).
* Probes wider than 32 bits mapped into multi-word consecutive register blocks.

## Consequences

* Hardware diagnostic and virtual probing flows can be simulated and tested with real guest firmware and host software in ZynqSim without requiring Vivado or physical hardware.
* `#![forbid(unsafe_code)]` remains strictly enforced across all workspace crates.

