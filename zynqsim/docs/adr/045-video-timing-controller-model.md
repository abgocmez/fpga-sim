# ADR-045: Video Timing Controller model (`vendor-ip`)

Status: **Accepted**. Date: 2026-10-19.

## Context

The Xilinx Video Timing Controller core (`v_tc` 6.1/6.2, PG016) is a key building block in FPGA video processing systems. It generates and detects horizontal and vertical video timing signals (HBLANK, VBLANK, HSYNC, VSYNC, AVIDEO, and field ID) for display interfaces (HDMI, DVI, VGA, DisplayPort) and camera sensors. It commonly coordinates frame timing alongside AXI VDMA (ADR-043) and AXI4-Stream to Video Out cores.

Previously, `v_tc` was classified in `xsa-import::catalog` as a vendor IP, but lacked a behavioural model in `crates/vendor-ip`. Designs using `v_tc` were rejected as unmodelled.

## Decision

* **Register-level behavioural model in `crates/vendor-ip/src/v_tc.rs`** as a `pl_native::RegisterBlock`:
  * 64 KiB address window (configurable, minimum 4 KiB `0x1000`):
    * Control register `XVTC_CTL_OFFSET` (`0x000`): bit 0 `CORE_ENABLE`, bit 2 `GEN_ENABLE`, bit 3 `DET_ENABLE`, bit 31 `SW_RESET` (restores default state, self-clearing).
    * Interrupt Status register `XVTC_ISR_OFFSET` (`0x004`): write-1-to-clear (W1C), reporting frame syncs (`FSYNC00`..`FSYNC15` bits 16..31), generator active video `G_AV` (bit 13), generator vertical blanking `G_VBLANK` (bit 12), and detector lock.
    * Error register `XVTC_ERROR_OFFSET` (`0x008`): write-1-to-clear signal lock and tracking error flags.
    * Interrupt Enable register `XVTC_IER_OFFSET` (`0x00C`): interrupt mask.
    * Version register `XVTC_VER_OFFSET` (`0x010`): reads `0x0602_0000` (VTC v6.2).
    * Generator timing registers:
      * `GASIZE` (`0x060`): horizontal active size [13:0] and vertical active size [29:16].
      * `GTSTAT` (`0x064`): timing status bits for active video and vertical blanking.
      * `GFENC` (`0x068`): progressive/interlaced encoding.
      * `GPOL` (`0x06C`): output polarity control.
      * `GHSIZE` (`0x070`): total frame horizontal size (pixels/clocks).
      * `GVSIZE` (`0x074`): total frame vertical size (lines).
      * `GHSYNC` (`0x078`): horizontal sync start and end cycles.
      * `GVSYNC` (`0x080`): vertical sync start and end lines.
    * Frame sync configuration registers `FS00`..`FS15` (`0x100`..`0x13C`) and global delay `GGD` (`0x140`).
  * Time progression (`advance`):
    * Video beam and pixel accumulator advance with simulated time based on `CLK_FREQ_HZ` (nominal pixel clock).
    * Emits frame sync and VBLANK interrupts and updates status flags as frames complete.
  * Interrupt signaling:
    * Single active-high level interrupt line `irq` asserting when `(isr & ier) != 0`.
* **Integration across crates:**
  * Registered in `vendor_ip::MODELS`, `param_names`, `VendorPl::add`, and `VendorPl::v_tc`.
  * Registered in `sim-api::pl_attach::NATIVE_MODELS`.
  * HWH fixture `fixtures/xsa/zs_math_vtc.hwh` and import tests in `crates/sim-api/tests/xsa_import.rs` and `crates/cli/tests/xsa.rs`.
* **Firmware and example:**
  * Guest firmware `firmware/vtc_demo/src/main.rs`, built to `fixtures/elf/rust_vtc.elf` via `cargo xtask firmware`.
  * Example project `examples/vtc-demo/project.toml`.
  * Determinism test case in `crates/cli/tests/determinism.rs` passing 5 runs identically.
  * Unit tests in `crates/vendor-ip/src/v_tc.rs` and integration test in `crates/sim-api/tests/vendor_ip2.rs`.

## Not verified (`VERIFY:`)

* Hardware sub-pixel timing jitter and external frame sync pulse width tolerances.
* Dual-detector interlace field parity tracking.

## Consequences

* Video processing software and drivers (such as `xvtc.c`) can configure timing modes and receive VTC frame sync interrupts in ZynqSim without external RTL tools.
* `#![forbid(unsafe_code)]` remains strictly enforced across all workspace crates.

