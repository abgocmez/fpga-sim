# ZynqSim

A deterministic simulator front end for **Zynq-7000 / Digilent Zybo Z7-20**: a virtual board
you can see, drive from a GUI, a CLI or an MCP server, and replay exactly.

> **Milestone P1 (locally verified).** Two kinds of engine sit behind the control plane:
> the built-in *demo* model (no CPU, no RTL, fidelity `demo`), and **Xilinx QEMU** (Cortex-A9,
> fidelity F0: functional with instruction-count timing) with a PL made of **native Rust models**
> or **RTL running in Vivado XSim**. The latter two need the Xilinx QEMU (Vitis) and, for RTL,
> Vivado, both located automatically. Read [docs/STATUS.md](docs/STATUS.md) before trusting any
> behaviour.

## Build and test

```
cargo build --release          # zynqsim, zynqsim-gui, zynqsim-mcp  (target/release)
cargo xtask ci                 # fmt --check, clippy -D warnings, tests, release build
```

Needs a Rust toolchain (stable). No external simulator is required to build or test. The GUI
links `cpal` (speaker; ALSA development files on Linux) and `rfd` (file dialogs); build with
`--no-default-features` to leave both out.

## Use

```
zynqsim run --for 1s --at 10ms=sw0:on --at 20ms=btn1:tap --record session.json --vcd run.vcd
zynqsim test session.json                  # replay and check the trace digest (exit 1 on mismatch)
zynqsim inspect                            # board, backends, what is implemented
zynqsim-gui --project examples/demo/project.toml
```

Keys in the GUI: `1`-`4` toggle switches, `Q` `W` `E` `R` hold buttons.

### Run real firmware

```
cargo xtask dpi                                    # once: the XSim DPI library
zynqsim run --project examples/math-qemu/project.toml --for 20ms        # QEMU + native PL
zynqsim run --project examples/dma-regression/project.toml --for 3ms    # QEMU + RTL in XSim
```

`examples/dma-regression` is the prototype's two-core DMA regression; expect six `PASS` lines and
`RESULT: ALL PASS`. Add `--json` for the trace digest, which is identical from run to run.

### The virtual Zybo (interactive shell, LEDs, switches, buttons, image filter)

```
zynqsim-gui --project examples/board-demo/project.toml
```

Type `help` in the console. `led 5`, `img 1`, `bright 60`, `contrast 3`, `run 2 -100 1`, `status`,
`end`. Click the switches and buttons on the board (or press `1`-`4`, `Q` `W` `E` `R`); the LEDs
are driven by the RTL. Open **Memory / image viewer** and press *source image* / *result image* to
see the pixels the PL wrote. The same thing without a window:

```
zynqsim run --project examples/board-demo/project.toml --for 40ms \
    --at "3ms=uart1:img 1\r" --at "8ms=uart1:contrast 3\r" --at 20ms=sw1:on --at 25ms=btn2:down \
    --dump-image 0x01100000,64x64,result.png --dump-mem 0x01000000,16384,src.bin
```

Verified by tests (`cargo test`): the 4096-pixel results equal a Rust golden model, a button press
raises the GPIO interrupt and the firmware toggles the LED.

### Audio and HDMI

```
cargo xtask firmware                     # (optional; needs `rustup target add armv7a-none-eabi`) rebuild the Rust firmware ELFs in fixtures/elf
zynqsim run --project examples/audio-demo/project.toml --for 90ms \
    --audio-in tone:440:0.5 --audio-out out.wav     # codec output as a WAV, LINE IN = 440 Hz
zynqsim run --project examples/hdmi-demo/project.toml --for 40ms \
    --video-in photo.png --video-out tx.png         # HDMI TX picture as PNG, RX = your image
```

The firmware (`firmware/audio_demo`, `firmware/hdmi_demo`, Rust `no_std` for the Cortex-A9)
brings the SSM2603 up over I2C, plays a 1 kHz tone then loops LINE IN back, and draws a test card
in DDR that the PL scans out to HDMI while capturing the picture you supply. In the GUI open the
same projects: the **HDMI** and **Audio codec** panels show the picture and level, *listen on the
speakers* plays the codec output, and the source pickers change what the board receives.

### Load your own PS firmware and PL design

In the GUI use **Open project…**, or **Project…** to set the Cortex-A9 ELF and the RTL files
separately (*Load PS firmware…* keeps the PL, *Load PL design…* keeps the firmware; the compiled
RTL is cached so a firmware change does not recompile it). **Save project.toml…** writes the
result. The **3D board** tab shows the board in perspective: drag to rotate, wheel to zoom, click
the switches and buttons.

### MCP

```json
{ "mcpServers": { "zynqsim": {
    "command": "zynqsim-mcp",
    "args": ["--root", "C:/path/to/project"] } } }
```

Call `status_get` first and read `fidelity` and `capabilities`. Tools that need a CPU or RTL
backend are listed but return `unsupported` until those backends exist. `video_frame` and
`audio_capture` return the HDMI picture (PNG) and codec output (WAV); `media_source_set` chooses
what the board receives.

## Layout

| Path | Contents |
|---|---|
| `crates/` | `sim-core`, `trace`, `board-zybo`, `sim-api`, `mcp`, `cli`, `gui`, `xtask`, `interop`, `pl-api`, `pl-native`, `backend-qemu`, `backend-rtl-xsim`, `xsim-dpi` |
| `boards/` | board descriptions (TOML) |
| `docs/` | `DESIGN.md`, `STATUS.md`, `TESTPLAN.md`, `adr/` |
| `examples/` | `demo`, `math-qemu`, `dma-regression`, `board-demo`, `audio-demo`, `hdmi-demo`, `rust-hello` projects |
| `firmware/` | Rust `no_std` Cortex-A9 firmware (`hello`, `audio_demo`, `hdmi_demo`); its own workspace, built by `cargo xtask firmware` |
| `fixtures/` | prototype firmware ELFs, the prebuilt Rust ELFs (`rust_*.elf`) and the RTL used by the examples and tests |

Requirements: [../docs/RUST_SOC_SIM_PROMPT.md](../docs/RUST_SOC_SIM_PROMPT.md).
Licence: Apache-2.0. Third-party crates: [THIRD_PARTY.md](THIRD_PARTY.md).
