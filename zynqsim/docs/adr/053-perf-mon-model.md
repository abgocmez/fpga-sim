# ADR-053: AXI Performance Monitor model (`vendor-ip`)

Status: **Accepted**. Date: 2026-10-20.

## Context

The Xilinx AXI Performance Monitor core (`axi_perf_mon` 5.0, PG037) measures real-time performance metrics on AXI interfaces including throughput, latency, transaction counts, and transfer sizes. In system-on-chip architectures, software configures sample intervals, selects metric types per monitor slot, and reads back metric counters either directly or as sampled snapshots upon interval expiration.

Previously, `axi_perf_mon` was cataloged in `xsa-import::catalog` as a vendor IP, but lacked a behavioural model in `crates/vendor-ip`. Designs containing this IP were rejected as unmodelled.

## Decision

* **Register-level behavioural model in `crates/vendor-ip/src/axi_perf_mon.rs`** implementing `pl_native::RegisterBlock`:
  * Standard address window (`0x10000` bytes):
    * `SRR` (`0x000`, WO): Software Reset Register; writing key `0x0000_000A` resets core state per PG037.
    * `SR` (`0x004`, RO): Status Register (`SR_METRIC_CNT_EN` bit 0, `SR_SAMPLE_CNT_EN` bit 1, `SR_SAMPLE_EXPIRED` bit 2).
    * `CR` (`0x024`, RW): Control Register (`CR_METRIC_CNT_EN` bit 0, `CR_METRIC_CNT_RESET` bit 1, `CR_SAMPLE_CNT_EN` bit 2, `CR_SAMPLE_CNT_RESET` bit 3, `CR_SAMPLE_LOAD` bit 4).
    * `SICR` (`0x028`, WO): Sample Interval Control Register; writing bit 0 latches a manual sample snapshot.
    * `IPISR` (`0x030`, RW1C): IP Interrupt Status Register reporting `IRQ_SAMPLE_EXPIRED` (bit 0) and `IRQ_METRIC_OVERFLOW` (bit 1).
    * `IPIER` (`0x034`, RW): IP Interrupt Enable Register matching `IPISR`.
    * `SI_REG` (`0x038`, RW): Sample Interval Register.
    * `SIC_REG` (`0x03C`, RO): Sample Interval Counter readout.
    * `MSR0` / `MSR1` (`0x044`, `0x048`, RW): Metric Selection Registers.
    * Active Metric Counters (`0x100..=0x17C`, RO): 8 counters, 64-bit wide (accessed via low 32-bit and high 32-bit words).
    * Sampled Metric Counters (`0x200..=0x27C`, RO): 8 sampled snapshot counters.
  * **Simulated Metrics & Progression**:
    * Virtual time advance (`advance`) decrements the sample interval counter and updates active metric counters.
    * Automatic sample snapshot latching into `SMC0`..`SMC7` on interval expiration, triggering `IRQ_SAMPLE_EXPIRED`.
    * External transaction recording and telemetry via [`AxiPerfMonHandle`] (`record_transaction`, `sample_snapshot`, `stats`).
  * **Interrupt output (`irq`)**:
    * Level-sensitive active-high interrupt line asserting when `(ipisr & ipier) != 0`.
* **Workspace Integration**:
  * Registered in `vendor_ip::MODELS`, `param_names`, `VendorPl::add`, and `VendorPl::perf_mon`.
  * Registered in `sim-api::pl_attach::NATIVE_MODELS`.
  * Added to `crates/xsa-import/tests/ip2.rs` catalog vendor IP classification test.
  * HWH fixture `fixtures/xsa/zs_math_perf_mon.hwh` and mapping tests in `crates/sim-api/tests/xsa_import.rs` and `crates/cli/tests/xsa.rs`.
* **Firmware and Example**:
  * Guest firmware `firmware/perf_mon_demo/src/main.rs`, built to `fixtures/elf/rust_perf_mon.elf` via `cargo xtask firmware`.
  * Example project `examples/perf-mon-demo/project.toml`.
  * Determinism case in `crates/cli/tests/determinism.rs` verified 5/5 identical runs.
  * Integration test `the_axi_perf_mon_model_runs_metrics_and_interrupts` in `crates/sim-api/tests/vendor_ip2.rs`.

## Not verified (`VERIFY:`)

* Cycle-accurate burst length distribution modeling for specialized custom streaming interconnect fabrics.
* Hardware profile streaming trace generation via external AXI Stream FIFO offload.

## Consequences

* Hardware diagnostic software, bus bandwidth profilers, and performance tuning tools can evaluate interconnect metrics in ZynqSim without requiring Vivado or physical hardware.
* `#![forbid(unsafe_code)]` remains strictly enforced across all workspace crates.

