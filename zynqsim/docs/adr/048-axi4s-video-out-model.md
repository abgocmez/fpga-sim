# ADR-048: AXI4-Stream to Video Out model (`vendor-ip`)

Status: **Accepted**. Date: 2026-10-20.

## Context

The Xilinx AXI4-Stream to Video Out core (`v_axi4s_vid_out` 4.0, PG044) bridges incoming AXI4-Stream video data (`s_axis_video`) to native parallel video interfaces (`vid_io_out`). It synchronizes video streaming data with video timing generator signals from companion cores such as the Video Timing Controller (`v_tc`, PG016, ADR-045). In FPGA video display pipelines (e.g. HDMI TX, DVI, VGA), it aligns frame boundaries via `TUSER` (Start of Frame) and scanline boundaries via `TLAST` (End of Line), providing FIFO buffering across clock domains.

Previously, `v_axi4s_vid_out` was cataloged in `xsa-import::catalog` as a vendor IP, but lacked a behavioural model in `crates/vendor-ip`. Designs containing this IP were rejected as unmodelled.

## Decision

* **Register-level behavioural model in `crates/vendor-ip/src/v_axi4s_vid_out.rs`** implementing `pl_native::RegisterBlock`:
  * 64 KiB address window (`0x10000` bytes):
    * `CR` (`0x000`, RW): Control Register (`CR_EN` bit 0, `CR_RESET` bit 1 self-clearing, `CR_MODE_MASTER` bit 2, `CR_PATTERN_EN` bit 3 for autonomous test pattern generation, `CR_LOOPBACK_EN` bit 4 for video frame loopback capture).
    * `SR` (`0x004`, RO): Status Register (`SR_LOCKED` bit 0, `SR_UNDERFLOW` bit 1, `SR_OVERFLOW` bit 2, `SR_SOF_DET` bit 3, `SR_EOL_DET` bit 4, `SR_FIFO_EMPTY` bit 5, `SR_FIFO_FULL` bit 6, `SR_AVIDEO` bit 7, `SR_VBLANK` bit 8, `SR_HBLANK` bit 9, and alignment status code in bits [31:16]: `0x0001` Unsynchronized, `0x0002` Coarse Alignment, `0x0004` Fine Alignment, `0x0008` Locked).
    * `ISR` (`0x008`, RW1C): Interrupt Status Register reporting lock achieved (`IRQ_LOCK`), lock lost (`IRQ_UNLOCK`), FIFO underflow (`IRQ_UNDERFLOW`), FIFO overflow (`IRQ_OVERFLOW`), Start of Frame (`IRQ_SOF`), End of Line (`IRQ_EOL`), and frame completion (`IRQ_FRAME_DONE`).
    * `IER` (`0x00C`, RW): Interrupt Enable Register matching `ISR` bits.
    * `ASIZE` (`0x010`, RW): Active video resolution (horizontal pixels in [15:0], vertical lines in [31:16]).
    * `TSIZE` (`0x014`, RW): Total video resolution including blanking intervals.
    * `FIFO_STATUS` (`0x018`, RO): Current FIFO fill level in [15:0] and total depth in [31:16].
    * `FIFO_THRESH` (`0x01C`, RW): Low/high watermarks for fine alignment and overflow protection.
    * `STREAM_DATA` (`0x020`, RW): AXI4-Stream data port for manual/polled software injection.
    * `STREAM_CTRL` (`0x024`, RW): Stream control port (`TUSER` bit 0, `TLAST` bit 1, `PUSH` bit 2).
    * `LAST_PIXEL` (`0x028`, RO): Last pixel popped from FIFO and driven to the video interface.
    * `FRAME_CNT` (`0x030`, RO): Cumulative completed frame counter.
    * `LINE_CNT` (`0x034`, RO): Current horizontal scanline coordinate.
    * `PIXEL_CNT` (`0x038`, RO): Current pixel coordinate.
    * `VER` (`0x03C`, RO): Core version reporting `0x0400_0000` (v4.0).
  * **Video Timing and Beam Progression (`advance`)**:
    * Advances pixel beam position based on simulated clock frequency (`CLK_FREQ_HZ`).
    * Autonomous test pattern generation (`CR_PATTERN_EN`) continuously supplies RGB test color bars into the FIFO when enabled, allowing standalone verification without upstream DMA engines.
    * Synchronizer state machine tracks `TUSER` SOF and `TLAST` EOL markers across consecutive frames to achieve and maintain lock.
    * Emits `MediaEvent::VideoFrameEnd` on each completed frame for host/GUI display capture.
  * **Interrupt output (`irq`)**:
    * Level-sensitive active-high interrupt line asserting when `(isr & ier) != 0`.
* **Workspace Integration**:
  * Registered in `vendor_ip::MODELS`, `param_names`, `VendorPl::add`, `VendorPl::vid_out`, and media draining.
  * Registered in `sim-api::pl_attach::NATIVE_MODELS`.
  * Added to `crates/xsa-import/tests/ip2.rs` catalog vendor IP classification test.
  * HWH fixture `fixtures/xsa/zs_math_vid_out.hwh` and mapping tests in `crates/sim-api/tests/xsa_import.rs` and `crates/cli/tests/xsa.rs`.
* **Firmware and Example**:
  * Guest firmware `firmware/vidout_demo/src/main.rs`, built to `fixtures/elf/rust_vidout.elf` via `cargo xtask firmware`.
  * Example project `examples/vidout-demo/project.toml`.
  * Determinism case in `crates/cli/tests/determinism.rs` verified 5/5 identical runs (`time: 6.000238 ms`).
  * Integration test `the_axi_vid_out_model_runs_loopback_and_interrupts` in `crates/sim-api/tests/vendor_ip2.rs`.

## Not verified (`VERIFY:`)

* Hardware sub-pixel phase jitter between independent pixel clock synthesizers and AXI stream clocks.
* Asymmetrical dual-pixel-per-clock native interface multiplexing.

## Consequences

* Video processing software and drivers can configure and stream video output through Xilinx AXI4-Stream to Video Out bridges in ZynqSim without requiring external RTL tools.
* `#![forbid(unsafe_code)]` remains strictly enforced across all workspace crates.

