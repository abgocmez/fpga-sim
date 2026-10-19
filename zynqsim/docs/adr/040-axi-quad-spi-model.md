# ADR-040: AXI Quad SPI v3.2 model (`vendor-ip`)

Status: **Accepted**. Date: 2026-10-19.

## Context

Following the multi-slave and master routing integration (ADR-039), expanding the library of built-in Xilinx IP models (`crates/vendor-ip`, ADR-016, ADR-021) is needed for designs connecting off-chip SPI devices or flash memories. The AXI Quad SPI core (`xilinx.com:ip:axi_quad_spi:3.2`, Xilinx PG153) is one of the most widely used peripherals in Vivado block designs. Until now, XSA designs containing an AXI Quad SPI instance were reported as unmodelled IP.

## Decision

* **Register-level model in `crates/vendor-ip/src/spi.rs`** as a `pl_native::RegisterBlock`:
  * `DGIER` (0x1C): Global Interrupt Enable register; bit 31 (`GIE`) gates device interrupt generation.
  * `IPISR` (0x20) and `IPIER` (0x28): IP Interrupt Status and Enable registers with toggle-on-write (write-1-to-clear) behavior for IPISR. Interrupt conditions modelled include `DTR_EMPTY` (bit 2), `DRR_FULL` (bit 4), `DRR_OVERRUN` (bit 5), `TX_HALF_EMPTY` (bit 6), `DRR_NOT_EMPTY` (bit 8), and `DTR_UNDERRUN` (bit 3).
  * `SRR` (0x40): Software Reset register; writing the key `0x0000000A` resets the core, restoring initial register values and clearing FIFOs.
  * `SPICR` (0x60): SPI Control register with loopback mode (`LOOP`, bit 0), core enable (`SPE`, bit 1), master mode (`MASTER`, bit 2), TX/RX FIFO resets (bits 5 and 6, self-clearing), manual slave select assertion (`MANUAL_SS`, bit 7), and master transaction inhibit (`MASTER_INHIBIT`, bit 8).
  * `SPISR` (0x64): Status register reflecting RX FIFO empty/full and TX FIFO empty/full flags.
  * `SPIDTR` (0x68) and `SPIDRR` (0x6C): 8-bit transmit and receive data FIFO registers.
  * `SPISSR` (0x70): Slave select register (active-low bitmask) masked according to `C_NUM_SS_BITS`.
  * `TX_FIFO_OCY` (0x74) and `RX_FIFO_OCY` (0x78): Occupancy counters for FIFOs.
* **Loopback and Transfer Mode:**
  * In loopback mode (`SPICR[0] = 1`), writes to `SPIDTR` automatically transfer the data byte to `SPIDRR`, asserting `INTR_DRR_NOT_EMPTY` and `INTR_DTR_EMPTY`, matching the behavior expected by the standard Xilinx standalone driver self-test (`xspi_selftest.c`).
* **Integration into `sim-api` and `xsa-import`:**
  * Added to `MODELS` and `param_names` (`C_FIFO_DEPTH`, `C_SCK_RATIO`, `C_NUM_SS_BITS`, `C_NUM_TRANSFER_BITS`, `C_SPI_MODE`, `C_USE_STARTUP`) in `crates/vendor-ip/src/lib.rs`.
  * Re-exported in `crates/sim-api/src/pl_attach.rs` under `NATIVE_MODELS`.
  * Classified as `VendorIp` in `xsa-import::catalog` so Vivado XSA imports automatically map it to `[native]` without manual configuration.
* **Verification and Guest Firmware:**
  * Guest firmware in `firmware/spi_demo/src/main.rs`, compiled to `fixtures/elf/rust_spi.elf` via `cargo xtask firmware`.
  * Example project in `examples/spi-demo/project.toml`.
  * Tested on the Native PS engine in `crates/cli/tests/native_pl.rs` and `crates/sim-api/tests/vendor_ip2.rs`.
  * Added to `crates/cli/tests/determinism.rs` with 5/5 identical trace hashes verified.

## Consequences

* Vivado XSA handoffs containing `axi_quad_spi` instances are fully recognized and simulated out-of-the-box.
* Standalone Xilinx drivers and bare-metal applications can perform SPI initialization, self-tests, loopback transfers, and interrupt handling.
* `#![forbid(unsafe_code)]` is preserved across all workspace crates.

