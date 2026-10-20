# The showcase: a Vivado design and Rust firmware, end to end

The biggest example in the repository, built the way a user would build one: a Vivado block design
(written in Tcl, built by Vivado 2022.1 in batch mode) with the user's own RTL, Rust firmware, a
`BOOT.BIN` from `bootgen`, then **one `zynqsim init --xsa`** and **one `zynqsim run`**.

Status of every claim here is in [STATUS.md](STATUS.md); nothing was run on a board.

## The design (Zybo Z7-20, FCLK0 = 100 MHz, one `M_AXI_GP0`)

| Block | What | Address | `IRQ_F2P` (GIC id) |
|---|---|---|---|
| `zs_fir` | **the user's Verilog**: an 8-tap FIR filter (AXI4-Lite slave, `irq`); module reference | `0x43C00000` | [0] (61) |
| `axi_gpio_0` | AXI GPIO, dual: channel 1 = the 4 LEDs, channel 2 = the 4 switches | `0x41200000` | [1] (62) |
| `axi_gpio_1` | AXI GPIO: the 4 buttons | `0x41210000` | [2] (63) |
| `axi_timer_0` | AXI Timer | `0x42800000` | [3] (64) |
| `axi_uartlite_0` | AXI UART Lite, 115200 8N1 | `0x40600000` | [4] (65) |
| `axi_quad_spi_0` | AXI Quad SPI (loopback in the model) | `0x41E00000` | [5] (66) |

`fixtures/showcase/rtl/zs_fir.v` is the only RTL; the Xilinx IP run as `vendor-ip` models. The FIR's
registers are in `firmware/showcase_demo/src/main.rs` (CTRL, STATUS, COUNT, ID, COEF0..7, DIN, DOUT);
`y[n] = sum COEF[k] * x[n-k]`, nine clock cycles per sample.

## What the firmware does (`firmware/showcase_demo`, Rust `no_std`)

1. Opens the PL (`rt::pl_bringup`), checks the FIR's ID register.
2. Builds a 64-sample test signal (a square wave of +-1000 with small noise from a fixed LCG) and
   filters it **twice in the hardware FIR**: a low-pass pass **driven by the FIR interrupt**
   (64 interrupts) and a high-pass pass **polled**. Every hardware result is compared with a software
   reference in Rust; one mismatch stops the program.
3. Walks the LEDs, reads the switches and the buttons, takes three **timer** interrupts.
4. Sends 16 bytes through the **Quad SPI** (loopback) and checks them; sends a line through the
   **UART Lite**.
5. Prints `RESULT: ALL PASS`, leaves LED0 and LED3 on and ends the run.

## Run it

Two projects, the same design and firmware:

```
# Native: no Xilinx tool at all (native Cortex-A9, the FIR in the pure-Rust RTL engine)
zynqsim run --project examples/showcase-native/project.toml --for 40ms

# Real tools: the Xilinx QEMU boots the BOOT.BIN, the FIR runs in Vivado XSim with the AXI protocol checkers
zynqsim run --project examples/showcase/project.toml --for 40ms
```

Expected UART1 (both): `ZYNQSIM SHOWCASE`, `fir id ok`, `fir lowpass: 64 samples hw==sw`,
`fir irq count: 64`, `fir highpass: 64 samples hw==sw`, `leds walked`, `switches: 0x0`,
`buttons: 0x0`, `timer irq count: 3`, `spi loopback: 16 bytes ok`, `uartlite sent`,
`hw fir: N global-timer ticks per sample (polled)`, `RESULT: ALL PASS`. The PL UART shows
`SHOWCASE PL UART OK`. Open either project in `zynqsim gui` to watch the LEDs, flip the switches and
look at the waveforms.

Also: `cargo xtask demo --only showcase,showcase-native` (the tour), `cargo test -p zynqsim --test
showcase`, and the determinism table (`showcase`, `showcase-native`: five runs, one trace digest).

## Rebuild everything from scratch

```
cargo xtask firmware                              # firmware/showcase_demo -> fixtures/elf/rust_showcase.elf
# Vivado 2022.1 (needs a SHORT path, see the header of the script): design.xsa and export/xsim
vivado -mode batch -source C:\zsh\make_showcase.tcl -tclargs C:\zsh\w <out>\fixtures\showcase <rtl dir>
cd fixtures/showcase && bootgen -arch zynq -image boot.bif -w on -o BOOT.BIN
zynqsim init --xsa fixtures/showcase/design.xsa --rtl fixtures/showcase/rtl/zs_fir.v \
             --boot fixtures/showcase/BOOT.BIN --out examples/showcase
```

## Limits (what this does not prove)

* The FSBL in `BOOT.BIN` is a stub that is never executed and the bitstream is a placeholder: the PL is the
  user RTL in XSim (or the RTL engine) plus the models, never a bitstream.
* The Xilinx IP are register-level models from the product guides (`VERIFY:` items in STATUS.md), not compared
  with hardware. The FIR is not timing-checked.
* The native PS does not boot a `BOOT.BIN` yet (it runs the ELF), which is why `showcase-native` uses the ELF.
* Switch and button values are read once, at their reset value; scripted input is possible
  (`zynqsim run --at 1ms=sw1:on`) but this firmware does not act on it.
