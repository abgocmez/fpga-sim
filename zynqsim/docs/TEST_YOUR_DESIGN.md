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

### From a Vivado project

If the design is a Vivado block design (Zynq PS7 + your RTL as a module reference + Xilinx IP),
hand over what Vivado writes instead of the sources. `examples/vivado-handoff` is a complete,
tested instance of this walk-through (`fixtures/handoff/make_handoff.tcl` builds the design):

1. **Hardware hand-off.** In Vivado: `write_hw_platform -fixed -file design.xsa`. A pre-synthesis
   write leaves the block-design handoff (HWH) out, so synthesise once first. A module
   reference's RTL is not in the archive: keep the source file.
2. **Simulation hand-off** (optional, for the user RTL and its testbench): `export_simulation
   -simulator xsim -directory export -force`. Put your ZynqSim testbench (see section 2) in the
   `sim_1` set so that the lists name it. Vivado lists only what the testbench reaches: the Xilinx
   IP of the block design are NOT in the export, they come from the XSA.
3. **Firmware and `BOOT.BIN`.** Build the ELF as in section 1, then pack it with
   `bootgen -arch zynq -image boot.bif -o BOOT.BIN` (`fixtures/handoff/boot.bif` is a working
   recipe). A stub FSBL and a placeholder bitstream are fine: ZynqSim runs neither (the FSBL is
   not executed, the bitstream is not simulated, `ps7_init` is not applied). With a real FSBL
   and bitstream the image still boots the same way; only the application partition runs.
4. **One `init`** for the project:

   ```
   zynqsim init --xsa design.xsa --rtl rtl/my_ip.v --boot-bin BOOT.BIN --out myproj
   zynqsim init --xsa design.xsa --export-sim export/xsim --boot-bin BOOT.BIN --out myproj2
   ```

   `--xsa` maps the address map, interrupts and clocks and the Xilinx IP (AXI GPIO, Timer, UART
   Lite become `vendor-ip` models bound to the board, `[native]`); the user RTL becomes `[rtl]`
   with a generated testbench (first form) or is taken from the export with its own testbench
   (second form, `[rtl] export_dir`); `--boot-bin` writes `[boot] mode = "boot_bin"` and copies the
   image next to `project.toml` (no `--elf`). `xsa-report.md` lists what was mapped and what was
   not; any IP without a model is listed under `[xsa.unmodelled]` and stops the run.
5. **Run and test.** `zynqsim run --project myproj/project.toml --for 8ms --json`; the UART text
   is `uart["1"]`, the PL's UART Lite lines `uart["pl"]["2"]`, the LEDs `leds`. Run it three times
   and compare `trace_hash` (section 4). `crates/cli/tests/handoff.rs` does exactly this.

What this does not prove: it is not Vivado's flow on a board. The FSBL, `ps7_init`, the PS clock
tree and MIO routing, and the bitstream are not simulated; the Xilinx IP are register-level
models from the product guides (`VERIFY:` items in STATUS.md); one user AXI4-Lite slave per
project.

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

## 4a. AXI protocol checkers

A testbench from `zynqsim init --ip` (or `--export-sim`) watches both AXI interfaces with
protocol checkers (ADR-025). They drive nothing. Each finding is a trace record, and the run
prints it with its simulation time, port, channel and rule:

```
protocol  : 1 warning(s), 0 error(s), 0 info (warn)
  protocol check warning at 45635 ns: AXI4 master W LAST_EARLY detail=0x1
```

* Rules: VALID_DROP (VALID fell before READY), PAYLOAD_UNSTABLE (address, data, strobe or
  length changed while waiting), RESET_VALID (VALID high while ARESETN is low), CROSS_4KB (an
  INCR burst crosses 4 KB), LAST_EARLY / LAST_MISSING (WLAST or RLAST on the wrong beat), WRAP_ALIGN
  and WRAP_LEN, SIZE (AxSIZE wider than the bus), and RESP (SLVERR or DECERR, reported as info).
* `[rtl] protocol_checkers` in `project.toml`: `true` (the default) reports warnings; `"strict"`
  makes each violation an `error` record and `zynqsim run` exits 1; `false` leaves the checkers out.
  `--json` has the same counts under `protocol_checks`.
* A hand-written testbench (your own `cosim_top.sv`) has no checkers. Copy the checker in from
  `fixtures/rtl/common/zs_axi_checker.sv` and instantiate it if you need them.
* Limits: a W beat that arrives before its AW turns the WLAST check off for that port; an INCR
  burst with an unaligned start is legal and not reported.

## 5. When it fails

* The run directory is `%TEMP%\zynqsim\run-*`: `qemu.err.log`, `xsim/build.log`,
  `xsim/xsim.out.log`.
* `[rtl] waves = true` (or `ZYNQSIM_KEEP_RUN=1`) keeps that directory and writes
  `xsim/zs_snap.wdb`.
* `--vcd run.vcd` exports the simulator-level trace (UART bytes, signals, not RTL waveforms).
* Stop the CPU: `[debug] enabled = true`, `breakpoints = ["main"]`, then through the GUI or
  MCP `sim_run` / `cpu_regs` / `cpu_disasm` / `symbol_lookup`. Registers exist at stops, and
  timing under the debugger is not reproducible.
* To see the registers in the GUI without editing `project.toml`: in the CPU registers section
  press **Restart under debugger** (or tick Simulation > Debugger (restart)); `--debugger` does it
  at start-up. The project then runs to `main` and stops there.
* `zynqsim-gui --project mytest/project.toml` shows the console, the PL bus accesses, memory and
  the board.

## Limits worth knowing

* PL timing is exact only at the 1 us quantum; CPU timing is instruction counting (fidelity F0).
* XSim costs roughly 0.3-0.6 s of wall time per simulated millisecond; the first build of a design
  takes ~11 s (cached afterwards).
* The AXI protocol checkers (section 4a) cover the generated testbenches only; a hand-written
  testbench checks nothing about the IP's handshakes beyond completing them.
* Typed UART input arrives on the host's schedule, not virtual time's, so tests that type
  into the firmware may see different orderings under load.
