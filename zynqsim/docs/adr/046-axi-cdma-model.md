# ADR-046: AXI Central Direct Memory Access (CDMA) model (`vendor-ip`)

Status: **Accepted**. Date: 2026-10-19.

## Context

The Xilinx AXI Central Direct Memory Access core (`axi_cdma` 4.1, PG034) is a high-bandwidth direct memory access engine designed for memory-mapped to memory-mapped transfers between system memory, on-chip BRAM, and peripheral memories. Unlike the stream-oriented AXI DMA (PG021) and AXI VDMA (PG020), CDMA performs direct transfers from an AXI-MM read master to an AXI-MM write master without requiring intermediate streaming channels or software intervention.

Previously, `axi_cdma` was recognised in the catalog (`xsa-import::catalog`) as a vendor IP, but lacked a behavioural model in `crates/vendor-ip`. Designs targeting memory-to-memory block transfers stopped with an unmodelled IP diagnostic.

## Decision

* **Register-level behavioural model in `crates/vendor-ip/src/cdma.rs`** as a `pl_native::RegisterBlock`:
  * 64 KiB address window (configurable, standard 64 KiB `0x10000` allocated by Vivado):
    * Control register `CDMACR` (`0x00`): bit 2 `Reset` (software reset, self-clears, restores default values), bit 3 `SGMode` (1 = Scatter Gather, 0 = Simple mode), bit 4 `Keyhole_Read`, bit 5 `Keyhole_Write`, bit 12 `IOC_IrqEn`, bit 13 `Dly_IrqEn`, bit 14 `Err_IrqEn`, bits 23:16 `IRQThreshold`, bits 31:24 `IRQDelay`.
    * Status register `CDMASR` (`0x04`): bit 1 `Idle` (1 = Idle, 0 = Busy), bit 3 `SGIncld` (reports hardware scatter-gather capability), bits 4..6 error flags (`DMAIntErr`, `DMASlvErr`, `DMADecErr`), bits 8..10 SG error flags (`SGIntErr`, `SGSlvErr`, `SGDecErr`), bit 12 `IOC_Irq` (write-1-to-clear completion interrupt), bit 13 `Dly_Irq`, bit 14 `Err_Irq`.
    * Current descriptor pointer registers `CURDESC_PNTR` (`0x08`) and `CURDESC_PNTR_MSB` (`0x0C`).
    * Tail descriptor pointer registers `TAILDESC_PNTR` (`0x10`) and `TAILDESC_PNTR_MSB` (`0x14`). In SG mode, writing `TAILDESC_PNTR` initiates descriptor chain traversal.
    * Source address registers `SA` (`0x18`) and `SA_MSB` (`0x1C`).
    * Destination address registers `DA` (`0x20`) and `DA_MSB` (`0x24`).
    * Bytes to transfer register `BTT` (`0x28`): writing a non-zero length in Simple mode clears `Idle` and queues a transfer.
  * **Memory access and execution (`service`)**:
    * Transfers are performed via `PsPort::mem_read` and `PsPort::mem_write` on the selected PS port (`PS_PORT`, defaulting to `S_AXI_HP0`).
    * Supports both Simple transfer and Scatter-Gather chain execution (reading 32-byte BDs, writing back BD status with completed bit `0x8000_0000` and transferred length).
    * Collects performance statistics (`CdmaStats`: `transfers`, `bytes`, `descriptors`) and access records (`MasterAccess`).
  * **Interrupt signaling**:
    * Output 0 is `cdma_introut`. Asserted whenever `((CR & SR) & 0x7000) != 0`.
* **Integration across crates**:
  * Registered in `vendor_ip::MODELS`, `param_names`, `VendorPl::add`, and `VendorPl::cdma`.
  * Registered in `sim-api::pl_attach::NATIVE_MODELS`.
  * Classified as vendor IP in `xsa-import::catalog` and verified in `crates/xsa-import/tests/ip2.rs`.
  * Fixture `fixtures/xsa/zs_math_cdma.hwh` and import verification in `crates/sim-api/tests/xsa_import.rs` and `crates/cli/tests/xsa.rs`.
* **Firmware and example**:
  * Guest firmware `firmware/cdma_demo/src/main.rs`, built to `fixtures/elf/rust_cdma.elf` via `cargo xtask firmware`.
  * Example project `examples/cdma-demo/project.toml`.
  * Determinism case in `crates/cli/tests/determinism.rs` verified 5/5 identical runs.
  * Integration test `the_axi_cdma_model_runs_transfers_and_interrupts` in `crates/sim-api/tests/vendor_ip2.rs`.

## Not verified (`VERIFY:`)

* AXI burst boundary splits and data width conversions on physical AXI crossbars.
* Delay interrupt timers in scatter-gather mode under hardware clock variations.

## Consequences

* Bare-metal applications using Xilinx's `xaxicdma` driver or direct memory DMA transfers can execute in co-simulation without Vivado or external RTL simulators.
* `#![forbid(unsafe_code)]` remains strictly enforced across all workspace crates.

