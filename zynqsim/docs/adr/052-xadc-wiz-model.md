# ADR-052: XADC Wizard model (`vendor-ip`)

Status: **Accepted**. Date: 2026-10-20.

## Context

The Xilinx 7 Series FPGAs and Zynq-7000 XADC Wizard core (`xadc_wiz` 3.3, PG091) provides an AXI4-Lite slave interface to the on-chip dual 12-bit 1 MSPS Analog-to-Digital Converter (XADC). It allows embedded firmware and fabric logic to monitor internal environmental conditions (die temperature, internal power rails `VCCINT`, `VCCAUX`, `VCCBRAM`), dedicated analog input channels (`VP`/`VN`), and external auxiliary analog input pins (`VAUXP`/`VAUXN`), with automatic alarm generation and interrupt notification.

Previously, `xadc_wiz` was cataloged in `xsa-import::catalog` as a vendor IP, but lacked a behavioural model in `crates/vendor-ip`. Designs containing this IP were rejected as unmodelled.

## Decision

* **Register-level behavioural model in `crates/vendor-ip/src/xadc_wiz.rs`** implementing `pl_native::RegisterBlock`:
  * 64 KiB address window (`0x10000` bytes):
    * `SRR` (`0x000`, WO): Software Reset Register; writing key `0x0000_000A` resets the core per PG091.
    * `SR` (`0x004`, RO): Status Register (`SR_DONE` bit 0, `SR_EOC` bit 1, `SR_EOS` bit 2, `SR_BUSY` bit 3, channel index in [12:8], alarm flags `ALM[3:0]` in [19:16], `OT` over-temperature flag in bit 24).
    * `AIFR` (`0x008`, RO): Alarm Output Status Register.
    * `CONVST` (`0x00C`, WO): Conversion start trigger register.
    * `IPISR` (`0x05C`, RW1C): IP Interrupt Status Register reporting temperature alarm (`IRQ_ALM0`), supply alarms (`IRQ_ALM1`..`IRQ_ALM3`), over-temperature alarm (`IRQ_OT`), End of Sequence (`IRQ_EOS`), and End of Conversion (`IRQ_EOC`).
    * `IPIER` (`0x060`, RW): IP Interrupt Enable Register matching `IPISR`.
    * `GIER` (`0x068`, RW): Global Interrupt Enable Register (`GIER_ENABLE` in bit 31).
    * Direct Memory-Mapped DRP Window (`0x200..=0x3FC`):
      * `0x200` (DRP 0x00): On-chip Temperature (`TEMP`).
      * `0x204` (DRP 0x01): Internal Core Voltage (`VCCINT`).
      * `0x208` (DRP 0x02): Auxiliary Supply Voltage (`VCCAUX`).
      * `0x218` (DRP 0x06): Block RAM Supply Voltage (`VCCBRAM`).
      * `0x240..=0x27C` (DRP 0x10..=0x1F): Auxiliary analog input channels `VAUX[0..15]`.
      * `0x280..=0x29C` (DRP 0x20..=0x27): Maximum and minimum tracked sensor values.
      * `0x300..=0x308` (DRP 0x40..=0x42): Configuration registers (`CONF0`, `CONF1`, `CONF2`).
      * `0x340..=0x37C` (DRP 0x50..=0x5F): Alarm upper and lower threshold registers for temperature, OT, VCCINT, VCCAUX, and VCCBRAM.
  * **Conversions & Transfer Functions**:
    * Implements official UG480 / PG091 transfer functions:
      * Temperature: \(T = \text{code} \times 503.975 / 4096 - 273.15\)
      * Voltage: \(V = \text{code} \times 3.0 / 4096\)
    * Simulated conversion sequencer advances with virtual time (`advance`), completing conversions and sequences and updating `SR`, `EOC`, `EOS`, and `IPISR`.
    * Automatic threshold checking trips `ALM[3:0]` and `OT` flags when sensor values exceed configured limits.
    * External host injection via [`AxiXadcHandle`] (`set_temperature`, `set_vccint`, `set_vccaux`, `set_vccbram`, `set_vaux`).
  * **Interrupt output (`irq`)**:
    * Level-sensitive active-high interrupt line asserting when `(gier & 0x8000_0000) != 0` and `(ipisr & ipier) != 0`.
* **Workspace Integration**:
  * Registered in `vendor_ip::MODELS`, `param_names`, `VendorPl::add`, and `VendorPl::xadc`.
  * Registered in `sim-api::pl_attach::NATIVE_MODELS`.
  * Added to `crates/xsa-import/tests/ip2.rs` catalog vendor IP classification test.
  * HWH fixture `fixtures/xsa/zs_math_xadc.hwh` and mapping tests in `crates/sim-api/tests/xsa_import.rs` and `crates/cli/tests/xsa.rs`.
* **Firmware and Example**:
  * Guest firmware `firmware/xadc_demo/src/main.rs`, built to `fixtures/elf/rust_xadc.elf` via `cargo xtask firmware`.
  * Example project `examples/xadc-demo/project.toml`.
  * Determinism case in `crates/cli/tests/determinism.rs` verified 5/5 identical runs.
  * Integration test `the_axi_xadc_model_runs_readings_and_interrupts` in `crates/sim-api/tests/vendor_ip2.rs`.

## Not verified (`VERIFY:`)

* External analog bipolar differential mode voltage noise modeling.
* Dynamic event-driven sampling timing jitter under variable clock dividers.

## Consequences

* Hardware diagnostic software, thermal monitoring loops, and system telemetry drivers can query on-chip XADC status in ZynqSim without requiring Vivado or physical hardware.
* `#![forbid(unsafe_code)]` remains strictly enforced across all workspace crates.

