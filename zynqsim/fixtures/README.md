# Test fixtures

`elf/` holds **user-supplied firmware images** built by the prototype (the C sources are in
`../../sw`, `../../dma_demo/sw*`, `../../board_demo/sw`; Vitis 2022.1, bare-metal, standalone BSP).
They are inputs to the simulator, not code of this project, and are only ever executed by the
simulated Cortex-A9.

| File | Source | Behaviour (from the source) |
|---|---|---|
| `math_test.elf` | `sw/main.c` | writes 7 vectors to `0x43C00000`, checks `0x43C00004 == 2 * value`, prints `RESULT: ALL PASS` then `COSIM_DONE` on UART1 |
| `dma_test.elf`, `dma_cpu1.elf` | `dma_demo/sw`, `dma_demo/sw_cpu1` | DMA accelerator regression, two cores (needs the `dma_xform` RTL and an RTL backend) |
| `board_test.elf` | `board_demo/sw` | GPIO / LED demo (needs the `gpio_ctrl` RTL) |

`rust_*.elf` are built from `../firmware` by `cargo xtask firmware`: `rust_hello`, `rust_audio`,
`rust_hdmi`, and `rust_net.elf` (`firmware/net_demo`: Cadence GEM driver + smoltcp DHCP/ping, see
`docs/adr/011-ethernet.md`).

## `xsa/`

Vivado 2022.1 hardware platforms written by `xsa/make_xsa.tcl` (batch mode, no bitstream; the
script documents how to run it): `zs_math_gpio.xsa` (PS7 with the Zybo Z7-20 board preset, the
`my_math_ip` module reference at `0x43C00000`, an AXI GPIO at `0x41200000` with its interrupt on
`IRQ_F2P[0]`, FCLK0 100 MHz) and `zs_math_only.xsa` (the same without the GPIO). Used by
`xsa-import`, `sim-api`, `zynqsim` and `zynqsim-mcp` tests. A module reference's RTL is not in
the archive: pass `rtl/math/my_math_ip.v` with `--rtl`.

## `export/`

Vivado 2022.1 simulation exports of `my_math_ip` + `cosim_top`, written by
`export/make_export.tcl` (batch mode; the script documents how to run it): `rtl/` (the two
sources the lists name), `behav/xsim/` (`export_simulation -simulator xsim`), `postsynth/`
(`launch_simulation -mode post-synthesis -type functional -scripts_only`: the funcsim netlist
`cosim_top_func_synth.v`, `cosim_top_vlog.prj`, the `.bat` scripts), `my_math_ip_funcsim.v`
(`write_verilog -mode funcsim`, UNISIM cells), and `vhdl/` (a hand-written VHDL twin of the math
IP and a `.prj` in Vivado's shape). The only edit to Vivado's output is that the source paths in
the lists were rewritten to `rtl/NAME` (relative to each list) so the folders resolve anywhere.
Used by `xsim-export`, `backend-rtl-xsim`, `sim-api`, `zynqsim` and `zynqsim-mcp` tests.

## `rtl/`

User-supplied RTL and testbench tops from the prototype, used as inputs to the XSim backend.
Each `cosim_top.sv` is a testbench that imports the `rp_*` DPI functions provided by
`xsim-dpi` (same names and signatures as the prototype's `rp_dpi.c`).

| Dir | Files | Notes |
|---|---|---|
| `math/` | `my_math_ip.v`, `cosim_top.sv` | AXI4-Lite slave, `reg1 = reg0 * 2`; used with `elf/math_test.elf` |
| `dma/` | `dma_xform.v`, `cosim_top.sv` | AXI4-Lite control + AXI4 master DMA (`dst = src*scale + offset`), IRQ; used with `elf/dma_test.elf` + `elf/dma_cpu1.elf` |
| `board/` | `gpio_ctrl.v`, `cosim_top.sv` (+ `dma/dma_xform.v`) | the "virtual Zybo" demo: LEDs, switches, buttons (IRQ), interactive UART shell, DMA image filter; used with `elf/board_test.elf`. The testbench reads switches/buttons with `tel_ctrl` and reports LED pins with `tel_event`, which `xsim-dpi` maps to the board |
