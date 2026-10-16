# ADR-026: AXI Timer capture and cascade modes, board-bound capture inputs

Status: **Accepted** (milestone P3). Date: 2026-10-16.

## Context

ADR-016 and ADR-021 modelled the AXI Timer 2.0 in generate mode only: counting, up and down,
load, auto reload, interrupts, `generateout` pulses and PWM level. STATUS listed capture mode and
cascade mode (64 bit) as stubs, where `CASC` read back and did nothing and a timer in capture mode
just counted. Capture needs an input the board can drive; no model had one.

## Decision

* **Capture mode** (`MDT` = 1) in `crates/vendor-ip/src/timer.rs`. A rising edge on
  `CAPTURETRIG0` (timer 0) or `CAPTURETRIG1` (timer 1) while `CAPT` and `ENT` are set copies the
  counter into `TLR` (the capture register), sets `TINT` and counts a capture. Rollovers set no
  interrupt in capture mode. With `ARHT` ("hold") the counter stops at the captured value until
  `TINT` is cleared by a write of 1.
* **Board binding of the capture inputs.** The same `[connect]` mechanism as the GPIO channels:
  `"axi_timer_0.CAPTURETRIG0" = "board.btn0"` (or `board.swN`, N = 0..7). `BoardSignal` gained
  single-bit variants `Switch(n)` and `Button(n)`; the whole-bus signals stay for GPIO channels,
  which refuse a single bit with a message, and the timer refuses a whole bus. The engine feeds
  the switch and button levels to the model on every board input change
  (`VendorPl::set_board_inputs` -> `TimerHandle::capture_input`), so an edge is captured at the
  simulated time of the input change (one quantum of host input latency at most, as for the
  other board inputs). `sim-api::project` checks the binding at validation, before any engine.
  `TimerHandle::capture_input` is the programmatic input for unit tests.
* **Cascade mode** (`CASC` in `TCSR0`): timers 0 and 1 form one counter of
  `2 * C_COUNT_WIDTH` bits, timer 1 the high word (`TCR1`, `TLR1`). Timer 0's `ENT`, `UDT`,
  `ARHT`, `MDT`, `GENT`, `LOAD` and `CAPT` control it; the carry from the low word is internal.
  A 64-bit rollover sets `TINT` of both timers; a capture on `CAPTURETRIG0` stores the 64-bit
  count in `TLR1:TLR0`.
* **Generate pins** (`GENERATEOUT0/1`) are unchanged: pulses counted, PWM level. No PWM duty
  readback was added.
* **Register facts are from memory.** Every item the product guide would have to confirm is a
  `VERIFY:` in the module documentation and in `docs/STATUS.md`: the capture edge and its
  `ENT` requirement, the meaning of `ARHT` in capture mode, which timer's `TINT` a cascade
  rollover sets, and whether `CAPTURETRIG1` and timer 1's control bits are ignored in cascade.

## Example and firmware

`examples/timer-modes` runs `firmware/timer_modes` on the real QEMU. Timer 0 is armed in capture
mode first; the host presses button 0 at 3 ms and releases it at 4 ms (`--at 3ms=btn0:down
--at 4ms=btn0:up`). The firmware checks that one capture happened (`TLR0` about 2.98 ms of clocks,
the release captured nothing), then that `TLR1:TLR0 = 0x0_FFFF_FFF0` carries into `TCR1`
without an interrupt, then that a 64-bit auto-reload counter with a 4096-clock period raises
three interrupts while `TCR1` stays at all ones. Every wait is bounded and paced with
`rt::delay_us`. `RESULT: ALL PASS` is the pass line. The CLI test
`crates/cli/tests/timer_modes.rs` runs it twice and compares the trace digests.

## Consequences

* The timer is now a board-facing device as well as a register block: `[connect]` entries for
  capture inputs count as board pins (`has_board_pins`), so the board input commands reach it.
* Capture and cascade are behavioural models. Captures happen at the input's simulated time, not at
  a PL clock edge. As in generate mode, register reads give the exact clock count, where hardware
  may be a few clocks stale.
* Not built: the PWM duty and period readback, capture on Pmod pins (only switches and buttons),
  and the `CAPTURETRIG1` input in cascade mode (ignored, `VERIFY:`).
