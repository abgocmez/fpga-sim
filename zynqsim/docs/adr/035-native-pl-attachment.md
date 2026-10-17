# ADR-035: PL attachment for the native PS, and `backends.pl = "rtl-native"`

Status: **Accepted, partly done** (milestone P9). Date: 2026-10-17.

## Context

ADR-033 gave the native PS no PL: `backends.pl` had to be `demo`. ADR-034 built the native RTL
engine but did not wire it as a PL. The P6 goal is a run with no external tool, which needs both.

## Decision

* **One PL for both PS engines** (`crates/sim-api/src/pl_attach.rs`). The native model builder
  (`build_pl`, moved out of the QEMU engine), the access recorder (`TracedPl`) and the trace
  helpers (bus accesses, LEDs, Pmod pins, UART Lite bytes, board inputs) are shared. The QEMU
  engine calls them in the same order as before, so its trace is unchanged in kind.
* **The GP windows on the native system bus** (`crates/sim-api/src/native_pl.rs`). `M_AXI_GP0`
  is `0x4000_0000`..`0x8000_0000` and `M_AXI_GP1` is `0x8000_0000`..`0xC000_0000`, each a device
  on `SystemBus`. Only the GP0 window ticks the PL and reports interrupts, so each is reported
  once. A PL access is stamped with the cycle of the instruction that made it (the `Watch` of
  `native_ps.rs`); the PL catches up on each bus tick (1500 ps per CPU cycle).
* **Interrupts.** `IRQ_F2P[0..7]` drive SPIs 61..68 and `IRQ_F2P[8..15]` drive SPIs 84..91, as on
  the Zynq. The GIC receives them through the bus propagation after each tick or device write.
* **`backends.pl = "rtl-native"`**: the project's `[rtl] files/top` elaborated by `rtl-frontend`,
  simulated by `rtl-engine`, served as an AXI4-Lite slave (`RtlModule`) on the GP port of its
  `[connect]` entry `"<top>.S_AXI" = "M_AXI_GP<n>@<addr>"`. The interrupt output is the one a
  `[connect]` entry names (`"<top>.irq" = "IRQ_F2P[n]"`), or, with none, the first output the
  testbench generator's name rule finds (`irq`, `interrupt`, `intr`), on `IRQ_F2P[0]`.
  Works on both engines (QEMU through `QemuSpec.rtl_native`).
* **Fixes to `rtl-engine`** found on the way: the adapter's clock had a zero-width high phase
  (rising edges every 5 ns, i.e. 200 MHz in engine time); it is now 100 MHz (the FCLK0 default).
  `RegisterBlock::advance` runs idle cycles to PS time and `irq_level` reads the named output.
  The XSim differential (`xsim_diff`) still agrees.
* **Validation.** `ps = "native"` accepts `pl = "native"`, `pl = "rtl-native"` and `pl = "demo"`.
  `pl = "xsim"`, `"verilator"` and `"icarus"` stay refused with the reason: they need remote-port,
  which the native PS does not speak (out of scope here). `axi_dma` (needs an HP/ACP master) and
  Pmod `[devices]` are refused with their reasons.

## Limits

* **No HP/ACP masters.** `PsPort::mem_read`/`mem_write` answer decode errors, so a PL DMA cannot
  move data. `axi_dma` is refused on the native PS.
* **Pmod devices are not attached.** The pins are traced; no board device answers them.
  `[devices]` is refused.
* **RGB LED PWM is not modelled** on the native PS (pin group 6 is ignored).
* **RTL is 2-state** (ADR-034). An access takes its simulated AXI cycles on the RTL clock; PS time
  does not wait for them, so the RTL clock may lead PS time by a few cycles per access.
* **FCLK0 is fixed at 100 MHz**; no `FCLK` setting or PS clock change reaches the PL.
* **PL state is not in the native state hash**: the snapshot check covers the bus RAM, devices'
  registers and the core, not the RTL's internal state (the trace digest still covers every
  access).
* **Stalled firmware is silent.** A data abort on an unmapped PS address enters the firmware's
  abort vector; the engine reports nothing (see the status of `native-math` below).

## Status of the examples

* `examples/native-gpio-timer-uart` runs on the native PS with the vendor-ip models. The UART text
  equals the QEMU example's apart from `tcr=` (a timer value that depends on the PS clock). It
  finishes in about 60 ms of virtual time, which is longer than the QEMU run takes to print the
  same text. The LEDs and the greeting on UART Lite match.
* `examples/native-math` does **not** run yet. Its firmware (`math_test.elf`) reads `0xF800D010` in
  `boot.S` before `main`. The native PS maps no device there, so the read is a data abort, and the
  abort vector loops. QEMU's zynq model reads that address without a fault. Making the native PS
  return zero there would be a modelled guess about a register this project does not document, so
  it is not done. The example stays in the repository, marked as not running.

## Consequences

* A PS-only native project with a PL runs with no tool. The GPIO/timer/UART proof does.
* `docs/STATUS.md` rows and `docs/TESTPLAN.md` name the tests in `crates/cli/tests/native_pl.rs`
  and the unit tests in `pl_attach`, `native_pl` and `rtl-engine`.

## Alternatives considered

* **Routing PL accesses through the bus `Device` of each window** (chosen) rather than a PL check
  in `Watch` alone: it keeps the address map the single source of truth (ADR-031).
* **Stubbing `0xF800D010` to zero** to make the math example run: rejected, see above.
