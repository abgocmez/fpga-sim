# Testing your own PS firmware against your own PL design

This is the shortest path from "I have a Cortex-A9 program and an AXI4-Lite IP" to a repeatable
test. Everything here is verified by the test suite unless it says otherwise; see
[STATUS.md](STATUS.md) for the details and limits.

## Requirements

* Xilinx QEMU (from Vitis) and Vivado XSim, both found automatically (or set `ZYNQSIM_QEMU`,
  `ZYNQSIM_VIVADO`). XSim runs on Windows only.
* The DPI library, once: `cargo xtask dpi`.

## 1. The firmware (PS)

An ELF for core 0 (core 1 optional), built with Vitis/xsct, or Rust `no_std` from `firmware/`
(`cargo xtask firmware`). QEMU runs no FSBL, so the firmware must open the PL itself before it
touches it:

```
SLCR_UNLOCK   0xF8000008 <- 0xDF0D
LVL_SHFTR_EN  0xF8000900 <- 0xF      (PS-PL level shifters)
FPGA_RST_CTRL 0xF8000240 <- 0        (release the FPGA resets)
SLCR_LOCK     0xF8000004 <- 0x767B
```

(`rt::pl_bringup()` in `firmware/rt`.) Then use the IP's address, for example `0x43C00000`, and
print a result line such as `RESULT: ALL PASS` on UART1 (`0xE0001000`) for the test to look for.
When you poll a PL register, put a short delay in the loop (`rt::delay_us`): back-to-back reads
cost ~100 us of wall time each and advance simulated time by almost nothing.

## 2. The IP (PL) and its testbench

For an **AXI4-Lite slave with a 32-bit data bus** the testbench is generated:

```
zynqsim init mytest --ip rtl/my_ip.v --elf fw/app.elf [--module my_ip] [--base 0x43C00000]
```

(`--ip` may be repeated for the other source files.) It reads the module header, finds the
`*_AWADDR` ... `*_RREADY` interface, the clock and the reset by name (any prefix, any case,
active-low or high), and writes:

* `mytest/tb/cosim_top.sv`: the stand-in for the PS7. It drives the IP from the remote-port link in
  lock-step, ties the other inputs low (other clocks to FCLK0), leaves outputs open, and ends the
  run when the firmware writes `0xC051D0DE` to offset `0x3C`.
* `mytest/project.toml`: QEMU + XSim, your ELF, your RTL plus the testbench, `top = "cosim_top"`.

The testbench ignores the upper address bits: the IP sees the offset inside its window.

One-bit outputs whose name contains `irq`, `interrupt` or `intr` (up to 16, in declaration order)
are forwarded to the PS as `IRQ_F2P[0..]` (level, on change): the first one is GIC interrupt ID 61.
In Rust firmware, `rt::irq` does the GIC setup and vector table
(`irq::install(); irq::set_handler(61, isr); irq::enable_spi(61); irq::enable();`); see
`firmware/irq_demo`. Clear the interrupt source in your IP from the handler, or the level stays high.

What it does **not** generate: IPs with an AXI master (DMA), other AXI flavours, DDR access, board
pins, audio/video. For those start from the hand-written testbenches:

| Need | Start from |
|---|---|
| Master ports into DDR, interrupt lines | `fixtures/rtl/dma/cosim_top.sv` (`rp_mem_*`, `rp_irq`) |
| Switches, buttons, LEDs | `fixtures/rtl/board/cosim_top.sv` (`tel_ctrl`, `tel_event`) |
| Codec and HDMI | `fixtures/rtl/media/` |

## 3. Run it

```
zynqsim run --project mytest/project.toml --for 20ms --json
```

The JSON has the UART text (`uart["1"]`), the trace digest, the PL bus access count, and the time
reached. Inject inputs with `--at 5ms=uart1:status\r --at 8ms=sw1:on` (UART input needs
`[host] uart1 = "tcp"`), read memory back with `--dump-mem 0x01100000,16384,out.bin`.

## 4. Make it a test

Pattern used by the repository's own tests (`crates/cli/tests/cli.rs`):

1. Run `zynqsim run --json` and assert the result line is in the UART text.
2. Compare data you dumped with `--dump-mem` against a golden model written in Rust
   (`backend-rtl-xsim/tests/dma_golden.rs`).
3. Determinism: run three times and require the same `trace_hash`.
4. Replay: `zynqsim run ... --record s.json` then `zynqsim test s.json`.

## 5. When it fails

* The run directory is `%TEMP%\zynqsim\run-*`: `qemu.err.log`, `xsim/build.log`,
  `xsim/xsim.out.log`.
* `[rtl] waves = true` (or `ZYNQSIM_KEEP_RUN=1`) keeps that directory and writes
  `xsim/zs_snap.wdb`.
* `--vcd run.vcd` exports the simulator-level trace (UART bytes, signals, not RTL waveforms).
* Stop the CPU: `[debug] enabled = true`, `breakpoints = ["main"]`, then through the GUI or
  MCP `sim_run` / `cpu_regs` / `cpu_disasm` / `symbol_lookup`. Registers exist at stops, and
  timing under the debugger is not reproducible.
* `zynqsim-gui --project mytest/project.toml` shows the console, the PL bus accesses, memory and
  the board.

## Limits worth knowing

* PL timing is exact only at the 1 us quantum; CPU timing is instruction counting (fidelity F0).
* XSim costs roughly 0.3-0.6 s of wall time per simulated millisecond; the first build of a design
  takes ~11 s (cached afterwards).
* There is no AXI protocol checker; the generated testbench does not assert anything about your
  IP's handshakes beyond completing them.
* Typed UART input arrives on the host's schedule, not virtual time's, so tests that type
  into the firmware may see different orderings under load.
