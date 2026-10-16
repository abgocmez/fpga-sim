# ADR-016: Xilinx IP as native models (`vendor-ip`), composed with user RTL

Status: **Accepted** (milestone P3). Date: 2026-10-16.

## Context

ADR-015 made `init --xsa` report every PL IP that has no model and refuse to run. Most real
designs carry Xilinx IP next to the user's own RTL (an AXI GPIO for the LEDs, a timer, a UART
Lite), and the user's RTL is what XSim simulates. Two things were missing: models of the common
IP (prompt section 9.5), and a way to have them in the same PL as an XSim design. Until now a
project had either `[rtl]` (XSim) or `[native]` models, never both.

## Decision

* **A new crate `vendor-ip`** with register-level behavioural models, each a
  `pl_native::RegisterBlock`: `AxiGpio` (axi_gpio 2.0, PG144), `AxiTimer` (axi_timer 2.0, PG079)
  and `AxiUartLite` (axi_uartlite 2.0, PG142). They are written from the product guides as
  remembered; every register fact that has not been checked against hardware or a Xilinx
  simulation is marked `VERIFY:` in the module documentation and in `docs/STATUS.md`.
* **Time and interrupts are part of the block interface.** `RegisterBlock` gained two defaulted
  methods, `advance(now_ns)` (PS time, called before every access and on every lock-step sync)
  and `irq_level()`. `NativePl::map_irq` binds a block's interrupt output to an `IRQ_F2P` line.
  A model is a pure function of simulated time: a timer's counter is computed from the elapsed
  AXI clock cycles, never from a host thread, so runs are deterministic and replayable.
* **Interrupt delivery.** A level change caused by a bus access is sent with that access (the
  way an RTL design's interrupt arrives while an access is outstanding); one caused by time
  (timer rollover, UART character time) or by a board input is sent at the next lock-step
  `advance`, at most one quantum (1 us by default) late. Falling edges *must* travel with the
  clearing access: an early version lowered the line only at the next sync and the level-high
  GIC interrupt re-fired in a tight loop that starved the sync (found by the end-to-end test; the
  guest never left its handler).
* **Composition with XSim: a `VendorPl` in front of the RTL adapter.** `vendor_ip::VendorPl`
  implements `PlTarget`, holds a `NativePl` with the vendor models and optionally the XSim
  adapter. An access that lands in a vendor window is served by the model; every other access
  goes to the RTL. Everything else is merged: wires and `advance` go to both, board outputs and
  pin events of both are returned, the Pmod pin sink is shared. This was chosen over binding the
  models "on the PS-side AXI address ranges in the QEMU engine" because the router then lives
  with the PL abstraction (`PlTarget`) and the engine is unchanged except for building it. The
  limit is the one the generated testbench already has: the RTL is not told that part of the
  address space went elsewhere, and a vendor window that the RTL would also decode is simply
  shadowed.
* **Board binding.** A GPIO channel is bound to the board with a `[connect]` entry
  `"<inst>.GPIO" = "board.leds"` (also `board.sw`, `board.btn`, `board.pmod_ja`..`board.pmod_je`;
  `GPIO2` for channel 2). Outputs reach the LEDs / Pmod pins through the same `BoardOutput` and
  `PinEvent` channels an RTL design uses; switches, buttons and Pmod inputs drive the channel's
  input pins, and an input change sets the channel's interrupt status. `init --xsa` binds by the
  name of the external port (`leds_4bits`, `sws_4bits`, `btns_4bits`) and, failing that, guesses
  from direction and width (four outputs: LEDs; four inputs: switches), saying in the report and
  as a note which one it did.
* **UART Lite bytes reach the host as `uart_read` / `uart_write` port `2 + n`** (the PL's UART
  Lite instances in instance-name order; ports 0 and 1 remain the PS UARTs). Transmit bytes are
  stamped with their completion time and appear in the trace as `uart<port>` records; host input
  is delivered at the current simulated time and paced at the baud rate, so it is deterministic,
  unlike the TCP-mode PS UART input.
* **Parameters** come from the block design by their Vivado names (`C_GPIO_WIDTH`,
  `C_ALL_OUTPUTS`, `C_BAUDRATE`, ...) into `[native_params.<instance>]`, plus `ADDRESS_SIZE`
  (default 64 KiB) and `CLK_FREQ_HZ` (default 100 MHz) that the importer fills from the address
  map and the clock.

## Consequences

* `zs_math_gpio.xsa` (user RTL + AXI GPIO) is now a runnable design: the RTL in XSim, the GPIO as
  a model, one PL. The unmodelled-IP path is exercised with a hand-written handoff containing an
  AXI DMA (`fixtures/xsa/zs_math_dma.hwh`).
* Models are register-level, not cycle-accurate at the AXI ports: a register access takes no
  simulated PL time, an interrupt is at most one quantum late. Nothing about AXI protocol
  checking is exercised for them.
* Not built: DMA, VDMA, IIC, Quad SPI, BRAM controller, interrupt controller, Clocking Wizard,
  timer capture/cascade, UART 16550. They stay in `[xsa.unmodelled]`.
* The register maps are the single biggest risk: a firmware written against the real IP that
  works here may rely on a behaviour that is guessed. Check the `VERIFY:` list in STATUS.md.
