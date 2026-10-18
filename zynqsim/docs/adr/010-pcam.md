# ADR-010: Pcam 5C (OV5640) camera model: pin-level SCCB and register-gated pixel stream

Status: **Accepted**. Date: 2026-10-15 (documented 2026-10-18).

## Context

The Digilent Pcam 5C connects an OmniVision OV5640 image sensor to the board's MIPI CSI-2 camera
connector (`pcam` in the board definition, `cam` in `[devices]`). On hardware, firmware configures
the sensor over a 2-wire SCCB (I2C-compatible, 7-bit address `0x3C`, 16-bit register addresses) link
and enables power via `CAM_GPIO`, while high-speed pixel data arrives over a two-lane MIPI D-PHY /
CSI-2 serial link decoded in the PL by a D-PHY + CSI-2 RX + Bayer demosaic pipeline.

Simulating a multi-hundred-megabit differential D-PHY link and CSI-2 packetizer cycle-by-cycle in
RTL would be slow and would tie the camera model to a specific CSI-2 RX IP, whereas the firmware's
contract with the camera is:
1. Power-up and SCCB register reads/writes (`0x300A`/`0x300B` chip ID `0x5640`, `0x3008` reset and
   power-down, `0x4202` stream gate, `0x503D` test pattern selection, `0x3808..0x380B` output size).
2. Frames arriving in the PL frame buffer only when the sensor is powered and streaming, containing
   either the sensor's built-in test pattern or the host's video input (`[media] video_in`).

## Decision

* **Camera connector as pin group 5 (`CAMERA_PORT = 5`, `[devices] cam = "pcam"`).** Only a board
  whose definition includes a `pcam` connector accepts `cam = "pcam"`, and `"pcam"` is refused on
  ordinary Pmod ports (`JA..JE`, groups `0..=4`). The pin mapping in group 5 is bit 0 = `CAM_GPIO`
  (power enable), bit 2 = `SCL`, bit 3 = `SDA` (`VERIFY:` against the Zybo Z7 schematic).
* **Pin-level SCCB through `zs_pin_xchg` (ADR-009).** `devices::Ov5640` uses the shared `I2cSlave` +
  `PointerSlave` (2-byte big-endian register address) at `0x3C` on `SCL`/`SDA` of group 5.
  Reads and writes go through the synchronous pin exchange so an RTL I2C/SCCB master talks to the
  sensor at pin level.
* **Modelled OV5640 registers (`VERIFY:`).**
  * `0x300A` / `0x300B`: read-only chip ID `0x56` / `0x40`.
  * `0x3008` (`SYSTEM_CTRL0`): reset value `0x42` (software power-down bit 6 set, so the sensor
    starts stopped); writing bit 7 = 1 resets all registers to defaults and self-clears bit 7; bit 6
    controls software power-down.
  * `0x4202` (stream control): low nibble `0x0` streams, non-zero (e.g. `0x0F`) gates output off.
  * `0x503D` (test pattern): bit 7 enables the pattern; bits 1:0 select colour bars (`0`, with bit 6
    rolling the bars by one position per frame), deterministic pseudo-random noise (`1`), colour
    squares (`2`, or black-and-white squares when bit 4 is set), or solid black (`3`).
  * `0x3808..0x380B`: output width/height (default `2592x1944`, stored and reported in `describe()`).
  * All other 16-bit addresses (`0x0000..=0xFFFF`): 64 KiB read/write storage so full initialization
    tables (PLL, timing, MIPI lane control `0x300E`/`0x4800`) succeed and read back.
* **Video delivery through `BoardDevice::video_line` / `PinSink::video_line`.** No MIPI D-PHY or
  CSI-2 packets are simulated. Instead, when the PL testbench pulls a scanline from the video input
  DPI (`zs_hdmi_rx_line`, ADR-006), the adapter asks `PinSink::video_line(frame, y, w, h)` first:
  * If a `pcam` device is attached and **not** streaming (`CAM_GPIO` low, `0x3008[6]` set, or
    `0x4202[3:0] != 0`), it returns a solid black line (`0x000000`).
  * If it is streaming with test pattern enabled (`0x503D[7] == 1`), it returns the pattern line.
  * If it is streaming with test pattern disabled (`0x503D[7] == 0`), it returns `None`, falling
    through to the host's `[media] video_in` source.

## Consequences

* End-to-end camera firmware (`firmware/pcam_demo`, `examples/pcam-demo`, `fixtures/rtl/pcam/cosim_pcam.sv`)
  verifies the chip ID over SCCB, observes black frames while stopped, enables streaming to capture
  both colour-bar test patterns and the host video source into DDR over `S_AXI_HP0`, and is
  deterministic across runs (`pcam-demo` in `determinism.rs`).
* Designs that instantiate a real MIPI D-PHY / CSI-2 RX core at the bit level cannot be driven by
  this model without replacing the CSI-2 RX front end with the parallel pixel bridge.

