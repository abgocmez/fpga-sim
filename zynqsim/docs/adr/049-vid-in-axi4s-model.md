# ADR-049: Video In to AXI4-Stream model (`vendor-ip`)

Status: **Accepted**. Date: 2026-10-20.

## Context

The Xilinx Video In to AXI4-Stream core (`v_vid_in_axi4s` 4.0, PG043) bridges incoming native parallel video interfaces (`vid_io_in` with clock, data, active video, horizontal/vertical blanking and sync) to an outgoing AXI4-Stream video interface (`m_axis_video`). In FPGA video capture and camera pipelines (e.g. HDMI RX, CMOS camera sensors such as the Omnivision OV5640 on the Pcam 5C), it synchronizes parallel video data with streaming clock domains, detects frame boundaries (`TUSER` Start of Frame) and line boundaries (`TLAST` End of Line), and absorbs interface throughput mismatches using an internal FIFO.

Previously, `v_vid_in_axi4s` was cataloged in `xsa-import::catalog` as a vendor IP, but lacked a behavioural model in `crates/vendor-ip`. Designs containing this IP were rejected as unmodelled.

## Decision

* **Register-level behavioural model in `crates/vendor-ip/src/v_vid_in_axi4s.rs`** implementing `pl_native::RegisterBlock`:
  * 64 KiB address window (`0x10000` bytes):
    * `CR` (`0x000`, RW): Control Register (`CR_EN` bit 0, `CR_RESET` bit 1 self-clearing, `CR_CAPTURE_MODE` bit 2 for continuous vs single-frame capture, `CR_PATTERN_EN` bit 3 for autonomous test pattern generation, `CR_LOOPBACK_EN` bit 4 for video frame loopback capture).
    * `SR` (`0x004`, RO): Status Register (`SR_LOCKED` bit 0, `SR_OVERFLOW` bit 1, `SR_UNDERFLOW` bit 2, `SR_SOF_DET` bit 3, `SR_EOL_DET` bit 4, `SR_FIFO_EMPTY` bit 5, `SR_FIFO_FULL` bit 6, `SR_AVIDEO` bit 7, `SR_VBLANK` bit 8, `SR_HBLANK` bit 9, and alignment status code in bits [31:16]: `0x0001` Unsynchronized, `0x0002` Coarse Alignment, `0x0004` Fine Alignment, `0x0008` Locked).
    * `ISR` (`0x008`, RW1C): Interrupt Status Register reporting lock achieved (`IRQ_LOCK`), lock lost (`IRQ_UNLOCK`), FIFO overflow (`IRQ_OVERFLOW`), FIFO underflow (`IRQ_UNDERFLOW`), Start of Frame (`IRQ_SOF`), End of Line (`IRQ_EOL`), and frame completion (`IRQ_FRAME_DONE`).
    * `IER` (`0x00C`, RW): Interrupt Enable Register matching `ISR` bits.
    * `ASIZE` (`0x010`, RW): Active video resolution (horizontal pixels in [15:0], vertical lines in [31:16]).
    * `TSIZE` (`0x014`, RW): Total video resolution including blanking intervals.
    * `FIFO_STATUS` (`0x018`, RO): Current FIFO fill level in [15:0] and total depth in [31:16].
    * `FIFO_THRESH` (`0x01C`, RW): Low/high watermarks for fine alignment and overflow protection.
    * `STREAM_DATA` (`0x020`, RO): Outgoing AXI4-Stream data port for software inspection and reading front pixel words.
    * `STREAM_CTRL` (`0x024`, RW): Stream control/status port (`TVALID` bit 0, `TUSER` bit 1, `TLAST` bit 2, `POP` write bit 3).
    * `LAST_PIXEL` (`0x028`, RO): Last pixel captured and pushed to the FIFO.
    * `FRAME_CNT` (`0x030`, RO): Cumulative captured frame counter.
    * `LINE_CNT` (`0x034`, RO): Current horizontal scanline coordinate.
    * `PIXEL_CNT` (`0x038`, RO): Current pixel coordinate.
    * `VER` (`0x03C`, RO): Core version reporting `0x0400_0000` (v4.0).
  * **Video Timing and Beam Progression (`advance`)**:
    * Advances pixel beam position based on simulated clock frequency (`CLK_FREQ_HZ`).
    * Autonomous test pattern generation (`CR_PATTERN_EN`) continuously supplies RGB test color gradient into the FIFO when enabled, allowing standalone verification without upstream camera sensors.
    * Synchronizer state machine tracks incoming parallel video timing markers across consecutive frames to achieve and maintain lock (`SR_LOCKED`).
    * Emits `MediaEvent::VideoFrameEnd` on each completed frame for host/GUI display capture.
  * **Interrupt output (`irq`)**:
    * Level-sensitive active-high interrupt line asserting when `(isr & ier) != 0`.
* **Workspace Integration**:
  * Registered in `vendor_ip::MODELS`, `param_names`, `VendorPl::add`, `VendorPl::vid_in`, and media draining.
  * Registered in `sim-api::pl_attach::NATIVE_MODELS`.
  * Added to `crates/xsa-import/tests/ip2.rs` catalog vendor IP classification test.
  * HWH fixture `fixtures/xsa/zs_math_vid_in.hwh` and mapping tests in `crates/sim-api/tests/xsa_import.rs` and `crates/cli/tests/xsa.rs`.
* **Firmware and Example**:
  * Guest firmware `firmware/vidin_demo/src/main.rs`, built to `fixtures/elf/rust_vidin.elf` via `cargo xtask firmware`.
  * Example project `examples/vidin-demo/project.toml`.
  * Determinism case in `crates/cli/tests/determinism.rs` verified 5/5 identical runs (`time: 6.000660 ms`).
  * Integration test `the_axi_vid_in_model_runs_capture_and_interrupts` in `crates/sim-api/tests/vendor_ip2.rs`.

## Not verified (`VERIFY:`)

* Hardware sub-pixel phase jitter between independent parallel pixel clocks and AXI stream clocks.
* Asymmetrical dual-pixel-per-clock native interface multiplexing.

## Consequences

* Video capture pipelines, camera sensors, and VDMA frame grabbing drivers can capture video data from Xilinx Video In to AXI4-Stream cores in ZynqSim without requiring external RTL tools.
* `#![forbid(unsafe_code)]` remains strictly enforced across all workspace crates.

