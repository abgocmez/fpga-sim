# ADR-043: AXI VDMA model (`vendor-ip`)

Status: **Accepted**. Date: 2026-10-19.

## Context

The Xilinx AXI Video Direct Memory Access (AXI VDMA 6.3, Xilinx PG020) provides high-bandwidth 2D video frame transfers between memory (such as DDR via PS HP ports) and AXI4-Stream video interfaces. Previously, `axi_vdma` was cataloged in `xsa-import` but unmodelled in `vendor-ip`, serving as the representative unmodelled IP in test fixture `fixtures/xsa/zs_math_vdma.hwh`. Implementing a behavioural register-level model enables simulation of video frame pipelines, park and circular buffer rotations, and video DMA drivers.

## Decision

* **Register-level behavioural model in `crates/vendor-ip/src/vdma.rs`** as a `pl_native::RegisterBlock`:
  * Core version register `VDMA_VERSION` (0x2C) reports `0x0603_0000` (PG020 v6.3).
  * Two independent channels: MM2S (read from memory, transmit stream) and S2MM (receive stream, write to memory).
  * Channel control (`MM2S_VDMACR` at 0x00, `S2MM_VDMACR` at 0x30): Run/Stop (`RS`, bit 0), Circular/Park mode selection (`Circular_Park`, bit 1), software reset (`Reset`, bit 2), interrupt enables (`IOC_IrqEn` bit 12, `Dly_IrqEn` bit 13, `Err_IrqEn` bit 14).
  * Channel status (`MM2S_VDMASR` at 0x04, `S2MM_VDMASR` at 0x34): `Halted` (bit 0), `Idle` (bit 1), error flags (`DMAIntErr` bit 4, `DMASlvErr` bit 5, `DMADecErr` bit 6, `FSize_Err` bit 7), write-1-to-clear interrupt status (`IOC_Irq` bit 12, `Dly_Irq` bit 13, `Err_Irq` bit 14), and active frame pointer (`FrmPntr`, bits 28:24).
  * 2D stride and dimension registers: `VSIZE` (0x50 / 0xA0), `HSIZE` (0x54 / 0xA4), and `FRMDLY_STRIDE` (0x58 / 0xA8). Writing `VSIZE` initiates or arms the frame transfer when `RS = 1`.
  * Frame buffer start addresses: `MM2S_START_ADDRESS_1..16` (0x5C..0x98) and `S2MM_START_ADDRESS_1..16` (0xAC..0xE8). Bank selection via `MM2S_REG_INDEX` (0x14) and `S2MM_REG_INDEX` (0x44) enables addressing up to 32 frame stores (1..32).
  * Buffer cycling: in park mode, the buffer selected in `PARK_PTR_REG` (0x28) is reused; in circular mode, the active buffer pointer advances modulo `C_NUM_FSTORES`.
  * Stream loopback: when `STREAM_LOOPBACK = 1`, transmitted MM2S frames route directly into S2MM; when `0`, frames queue into host-accessible FIFO buffers (`take_mm2s` and `push_s2mm`).
  * Interrupts: two outputs, output 0 for `mm2s_introut`, output 1 for `s2mm_introut`.
* **Integration across crates:**
  * Registered in `vendor_ip::MODELS`, `param_names`, `irq_output_index`, `VendorPl::add`, and `VendorPl::vdma`.
  * Registered in `sim-api::pl_attach::NATIVE_MODELS`.
  * `sim-api::xsa` maps `axi_vdma` memory master windows to HP/GP/ACP ports and detects AXI4-Stream loopback matching `dma_streams`.
  * `sim-api::project` validates that VDMA instances require QEMU backend because master memory transfers require PS HP/ACP ports.
  * Preserved unmodelled IP verification: introduced `fixtures/xsa/zs_math_ethlite.hwh` (`xilinx.com:ip:axi_ethernetlite:3.0`) to replace `zs_math_vdma.hwh` in tests verifying unmodelled IP rejection (`cli::tests::xsa`, `mcp::tests::xsa`, `sim-api::tests::xsa_import`).
  * Added handoff import test verifying `zs_math_vdma.hwh` is now mapped cleanly to the native `axi_vdma` model.
* **Firmware and example:**
  * Guest firmware `firmware/vdma_demo/src/main.rs`, built to `fixtures/elf/rust_vdma.elf` via `cargo xtask firmware`.
  * Example project `examples/vdma-demo/project.toml`.
  * Determinism test case in `crates/cli/tests/determinism.rs` passing 5 runs identically.
  * Unit tests in `crates/vendor-ip/src/vdma.rs` and end-to-end integration test `sim-api::vendor_ip2::the_axi_vdma_model_runs_2d_frame_transfers_and_interrupts`.

## Not verified (`VERIFY:`)

* Hardware genlock synchronization with external video timing controllers (`fsync` signals).
* Delay interrupt timers based on horizontal/vertical blanking intervals.
* Video timing controller integration registers.

## Consequences

* Video processing firmware that configures and uses Xilinx AXI VDMA 6.3 can now run natively under ZynqSim with full 2D memory transfers and interrupt completion.
* `#![forbid(unsafe_code)]` remains strictly enforced across all workspace crates.

