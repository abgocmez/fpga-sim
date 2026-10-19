# ADR-050: Integrated Logic Analyzer model (`vendor-ip`)

Status: **Accepted**. Date: 2026-10-20.

## Context

The Xilinx Integrated Logic Analyzer core (`ila` 6.2, PG172) is a hardware debugging engine used to capture internal FPGA signals in real-time. In embedded systems, AXI-accessible debug hubs, or software-driven verification environments, the core is controlled via an AXI4-Lite slave interface to configure trigger comparison conditions, arm/disarm the acquisition state machine, monitor live probe signals, and read out captured trace sample buffers.

Previously, `ila` was cataloged in `xsa-import::catalog` as a vendor IP, but lacked a behavioural model in `crates/vendor-ip`. Designs containing this IP were rejected as unmodelled.

## Decision

* **Register-level behavioural model in `crates/vendor-ip/src/ila.rs`** implementing `pl_native::RegisterBlock`:
  * 64 KiB address window (`0x10000` bytes):
    * `CR` (`0x000`, RW): Control Register (`CR_ARM` bit 0, `CR_DISARM` bit 1, `CR_TRIG_NOW` bit 2, `CR_AUTO_REARM` bit 3, `CR_RESET` bit 4 self-clearing, `CR_STIM_EN` bit 5 autonomous probe stimulus generator).
    * `SR` (`0x004`, RO): Status Register (`SR_ARMED` bit 0, `SR_TRIGGERED` bit 1, `SR_FULL` bit 2, `SR_IDLE` bit 3, and acquisition state code in bits [31:16]: `0x0000` Idle, `0x0001` Armed, `0x0002` Triggered, `0x0004` Full).
    * `ISR` (`0x008`, RW1C): Interrupt Status Register reporting trigger match (`IRQ_TRIGGERED`) and sample buffer full (`IRQ_DONE`).
    * `IER` (`0x00C`, RW): Interrupt Enable Register matching `ISR` bits.
    * `PROBE0_VAL` (`0x010`, RW): Trigger comparison target value for Probe 0.
    * `PROBE0_MASK` (`0x014`, RW): Trigger comparison bitmask for Probe 0.
    * `PROBE0_MODE` (`0x018`, RW): Trigger condition mode (0: Equal, 1: NotEqual, 2: GreaterThan, 3: LessThan).
    * `SAMPLE_DEPTH` (`0x020`, RO): Trace sample buffer depth (configurable, default 1024 words).
    * `SAMPLE_CNT` (`0x024`, RO): Number of samples currently captured in the buffer.
    * `TRIG_POS` (`0x028`, RO): Index within the trace buffer where trigger match occurred.
    * `VER` (`0x02C`, RO): Core version reporting `0x0602_0000` (v6.2).
    * `TRACE_RADDR` (`0x030`, RW): Trace buffer read address pointer.
    * `TRACE_RDATA` (`0x034`, RO): Sequential trace sample readout port with automatic post-read pointer increment.
    * `PROBE0_LIVE` (`0x038`, RO): Current live value of probe 0.
    * Direct Memory-Mapped Buffer Window (`0x1000..=0x1000 + 4*depth`): Direct random-access window to trace buffer memory.
  * **Acquisition State Machine & Stimulus**:
    * Transitions across `Idle` -> `Armed` -> `Triggered` -> `Full`.
    * Autonomous probe stimulus generator (`CR_STIM_EN`) enables standalone hardware-in-the-loop and software verification without external FPGA logic nets.
    * External probe sample feeding via `AxiIlaHandle::feed_probe0`.
  * **Interrupt output (`irq`)**:
    * Level-sensitive active-high interrupt line asserting when `(isr & ier) != 0`.
* **Workspace Integration**:
  * Registered in `vendor_ip::MODELS`, `param_names`, `VendorPl::add`, and `VendorPl::ila`.
  * Registered in `sim-api::pl_attach::NATIVE_MODELS`.
  * Added to `crates/xsa-import/tests/ip2.rs` catalog vendor IP classification test.
  * HWH fixture `fixtures/xsa/zs_math_ila.hwh` and mapping tests in `crates/sim-api/tests/xsa_import.rs` and `crates/cli/tests/xsa.rs`.
* **Firmware and Example**:
  * Guest firmware `firmware/ila_demo/src/main.rs`, built to `fixtures/elf/rust_ila.elf` via `cargo xtask firmware`.
  * Example project `examples/ila-demo/project.toml`.
  * Determinism case in `crates/cli/tests/determinism.rs` verified 5/5 identical runs (`time: 6.000874 ms`).
  * Integration test `the_axi_ila_model_runs_capture_and_interrupts` in `crates/sim-api/tests/vendor_ip2.rs`.

## Not verified (`VERIFY:`)

* Multi-probe (probes 1..63) combined trigger equation parsing.
* Dynamic storage qualification filters (when `C_EN_STRG_QUAL` is set).

## Consequences

* Hardware diagnostic and logic analyzer flows can be simulated and tested with real guest firmware and debug software in ZynqSim without requiring Vivado or physical hardware.
* `#![forbid(unsafe_code)]` remains strictly enforced across all workspace crates.

