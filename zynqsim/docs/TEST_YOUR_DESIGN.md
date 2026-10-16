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

An **AXI4 master** on the IP (a 32-bit `*ARADDR` ... `*BREADY` interface, as on a DMA engine) is
bridged to the PS's memory through `S_AXI_HP0`, so the IP reads and writes the same DDR the
firmware sees, with a modelled 16-clock latency. Give the second core's firmware with `--elf1`.
For `dma_xform.v` the generated testbench reproduces the hand-written regression's trace digest.

Eight-bit ports named like a Pmod (`pmod_ja`, `jb_o`, `pmod_jc_i`...) and a six-bit `rgb_leds` are
forwarded as pin groups: outputs are reported to the host on every change, inputs follow the
levels the host sets. With `[devices] pmod_ja = "oled"` in the project (or `pmod_attach`), the
host decodes the SPI traffic of a Digilent Pmod OLED into a picture:
`zynqsim run ... --device-out JA=oled.png`. See `examples/oled-demo` and `examples/rgb-demo`.

What it does **not** generate: a full AXI4 slave or AXI-Stream, AXI masters that are not 32 bits
wide, board pins, audio/video. For those start from the hand-written testbenches:

| Need | Start from |
|---|---|
| Statistics, custom DDR latency, several masters | `fixtures/rtl/dma/cosim_top.sv` (`rp_mem_*`, `rp_irq`) |
| Switches, buttons, LEDs | `fixtures/rtl/board/cosim_top.sv` (`tel_ctrl`, `tel_event`) |
| Codec and HDMI | `fixtures/rtl/media/` |

### A Vivado simulation export or a post-synthesis netlist

If the design exists as a Vivado simulation export, point the generator at it instead of the
source files:

```
zynqsim init --export-sim export_sim/xsim --out mytest --elf fw/app.elf [--module my_ip]
```

The argument is an `export_simulation -simulator xsim` folder (the one with `vlog.prj`,
`elab.opt`, `xsim.ini`), a single `.prj` or `.f` list, or one `write_verilog -mode funcsim`
netlist file. `project.toml` gets `[rtl] export_dir = "..."` (the export is referenced, not copied:
keep it where it is); its files compile in list order into their libraries, with the `-L`
libraries (`unisims_ver`, `xpm`, `secureip`...), include directories, defines and `glbl`. The
testbench is generated for the AXI4-Lite slave in the export (`--module` picks it) unless some
source in the export already calls the `rp_*` DPI functions, in which case that is the top.
`mytest/export-report.md` lists the sources, libraries and everything that was ignored.

To get such an export from Vivado: `export_simulation -simulator xsim -directory DIR -force` for
the RTL, or `launch_simulation -mode post-synthesis -type functional -scripts_only` (and
`write_verilog -mode funcsim netlist.v`) after synthesis; `fixtures/export/make_export.tcl` does
both. Limits: no timing/SDF simulation, no compiled IP libraries tried, no encrypted IP tried,
the export's own DPI libraries are not loaded; unsupported options are printed as
`file:line: construct: reason`. Globs work in any project: `files = ["rtl/**/*.v", "rtl/*.sv"]`.

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
