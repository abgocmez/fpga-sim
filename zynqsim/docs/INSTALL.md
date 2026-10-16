# Installing ZynqSim

Two ways: unpack a release archive (no Rust needed), or build from source. The
Xilinx tools are optional; the parts that need them are listed under
[What needs what](#what-needs-what).

Status of each step is in [STATUS.md](STATUS.md) and [TESTPLAN.md](TESTPLAN.md).
Windows is the tested platform so far; Linux and macOS are built by CI and the
release workflow, which have not run yet.

## Prerequisites

| | Needed to | Windows 11 (tested) | Linux | macOS |
|---|---|---|---|---|
| Rust (stable, `rustup`) | build only | `winget install Rustlang.Rustup` | `rustup` from rustup.rs | `rustup` from rustup.rs |
| MinGW-w64 `gcc` + GNU toolchain | building the XSim DPI library (`libxsim_dpi.a`) | `stable-x86_64-pc-windows-gnu` plus a gcc (Qt's MinGW or MSYS2 `mingw-w64-x86_64-gcc`) | not needed | not needed |
| ALSA development files | the GUI sound device | not needed | `libasound2-dev` | not needed |
| X11/Wayland/GL dev files | the GUI | not needed | `libxkbcommon-dev libwayland-dev libx11-dev libxcb1-dev libgl1-mesa-dev` | not needed |
| Xcode command line tools | the C toolchain of Rust | not needed | not needed | `xcode-select --install` |

The release archive is built with the same command as the programs. On Windows
the MSVC toolchain builds the programs and the tests, but not the DPI library,
which is a GNU archive; `cargo xtask dist` therefore needs the GNU toolchain
there.

The Rust workspace needs no other program to build or test. Tests that need a
Xilinx tool print `SKIPPED:` and pass without it, so read the output of a run
that matters.

### Xilinx tools (optional)

| Tool | Used for | Where it comes from | What ZynqSim looks for |
|---|---|---|---|
| Xilinx QEMU (Vitis) | the PS engine: real firmware on the Cortex-A9 | AMD's Vitis installer (the version tested here is 2022.1, under `C:\Xilinx\Vitis\2022.1`) | `qemu-system-aarch64` on `PATH`, or `ZYNQSIM_QEMU`; the co-simulation device tree next to it (`data\emulation\dtbs\zynq\zynq-arm-cosim.dtb`), or `ZYNQSIM_DTB` |
| Vivado (XSim) | RTL in the PL | AMD's Vivado installer (tested: 2022.1 at `C:\Xilinx\Vivado\2022.1`; the tests pick the highest installed) | `XILINX_VIVADO` or `ZYNQSIM_VIVADO` (the install directory, the one with `settings64.bat`) |

Install them the way AMD documents for your platform, with a licence for each.
ZynqSim never bundles them and never downloads them.

### Verilator (optional, open source)

Verilator is the second RTL backend (`pl = "verilator"`, ADR-027). It is installed by unpacking
a portable build, with nothing written to the system:

1. Download the Windows x64 archive `oss-cad-suite-windows-x64-<date>.tgz` from
   `github.com/YosysHQ/oss-cad-suite-build/releases` (about 600 MB).
2. Unpack it into `%TEMP%\zs-tools\verilator` so that `...\verilator\oss-cad-suite\bin\verilator_bin.exe`
   exists. Do not run anything from it except the programs ZynqSim starts.
3. Verilator's build also needs three programs that the archive does not contain. Put them on
   `PATH`, or use the places ZynqSim looks in:
   * a C++ compiler: MinGW-w64 `g++` (for example the `mingw1310_64` of Qt Tools, or MSYS2);
   * GNU make: a copy of MinGW's `mingw32-make.exe` named `make.exe` in `%TEMP%\zs-tools\tools`
     (or `make` on `PATH`);
   * a POSIX shell `sh.exe`: Git for Windows (`C:\Program Files\Git\usr\bin` is searched).

`zynqsim inspect` shows `verilator` as found when the driver is located. The build needs the
DPI library (`cargo xtask dpi`), like XSim. Verilator's own `verilator` launcher is a Perl script;
ZynqSim uses `verilator_bin.exe` directly.

Environment variables (all optional):

| Variable | Value |
|---|---|
| `ZYNQSIM_QEMU` | the QEMU binary itself (`...\qemu\qemu-system-aarch64.exe`), not the `.bat` launcher that Vitis ships in `bin` |
| `ZYNQSIM_DTB` | the co-simulation device tree, if it is not next to QEMU |
| `ZYNQSIM_VIVADO` | the Vivado install directory, for XSim |
| `XILINX_VIVADO` | the same directory, read when `ZYNQSIM_VIVADO` is unset; also used for export file lists |
| `ZYNQSIM_VERILATOR` | `verilator_bin.exe` (or the folder with it), when it is not on `PATH` nor in the default folder above |
| `VERILATOR_ROOT` | Verilator's `share\verilator` folder, if it is not beside `bin` (read when set) |
| `ZYNQSIM_DPI_LIB` | the XSim DPI library file (`libxsim_dpi.a`), when it is not in `lib/` next to the programs or in `target/dpi/release` |
| `ZYNQSIM_WORK_DIR` | where run directories go (default: a `zynqsim` folder in the temporary directory) |
| `ZYNQSIM_NO_BUILD_CACHE` | `1` disables the XSim and Verilator build caches |

`zynqsim inspect` lists every backend, whether it was found and where.

## Install from an archive

Release archives are named `zynqsim-<version>-<os>-<arch>.zip` (Windows) or
`.tar.gz` (Linux, macOS), with `SHA256SUMS` next to them.

Windows (PowerShell):

```powershell
Get-FileHash -Algorithm SHA256 zynqsim-0.1.0-windows-x86_64.zip   # compare with SHA256SUMS
Expand-Archive zynqsim-0.1.0-windows-x86_64.zip -DestinationPath C:\Tools
C:\Tools\zynqsim-0.1.0-windows-x86_64\zynqsim.exe --version
```

Linux and macOS:

```sh
sha256sum -c SHA256SUMS            # macOS: shasum -a 256 -c SHA256SUMS
tar -xzf zynqsim-0.1.0-linux-x86_64.tar.gz
cd zynqsim-0.1.0-linux-x86_64
./zynqsim --version
```

The folder holds `zynqsim`, `zynqsim-gui`, `zynqsim-mcp`, `lib/libxsim_dpi.a`,
`examples/`, the `fixtures/` the examples use, `boards/`, `docs/`, `README.md`,
`THIRD_PARTY.md`, `LICENSE` and `INSTALL.txt`. The programs find the DPI
library and the files of the examples relative to their own place, so the folder
can be moved, but the examples are run from inside it (they use relative paths).
QEMU and XSim run directories go to the temporary directory (`ZYNQSIM_WORK_DIR`
changes that), not into the folder.

Put the folder on `PATH` if you want `zynqsim` from anywhere. Nothing else needs
installing.

## Build from source

```sh
git clone <repository> zynqsim && cd zynqsim
cargo build --release               # zynqsim, zynqsim-gui, zynqsim-mcp in target/release
cargo xtask dpi                     # the XSim DPI library, only for RTL in XSim
cargo xtask dist                    # the archive in dist/ (builds both, then checks it)
cargo xtask ci                      # the full gate: fmt, clippy, tests, release build (long)
```

`cargo xtask dist` ends with the same clean-directory check as the release
job. To run that check again on an archive: `cargo xtask dist-verify <archive>`.

## What needs what

| Works with Rust alone | Also needs Xilinx QEMU (`ps = "qemu"`) | Also needs Vivado XSim and the DPI library (`pl = "xsim"`) |
|---|---|---|
| The demo engine (`ps = "demo"`, `pl = "demo"`; the default of `zynqsim run`) | Firmware on the PS: UART, Ethernet, SD and QSPI images booted by firmware, the debugger, Rust firmware examples | RTL in the PL: the RTL examples, the DMA regression, board pins driven by RTL |
| Native PL models in Rust (`pl = "native"`, for example `dma-loopback`) | Those models with firmware | the same models next to RTL |
| Parsing, validation and writing projects: `init`, `init --xsa`, `init --export-sim`, `init --boot`, `boot-info`, `inspect`, trace, replay, board files | | |
| The GUI (`zynqsim-gui`): board, 3D view, waveforms, project loader, with the demo engine | Its firmware runs | Its RTL runs |
| The MCP server (`zynqsim-mcp`): the same tools, for the same engines | | |

Verilator needs the install above (`pl = "verilator"`, `examples/math-verilator`). Icarus Verilog
is not implemented and the native RTL engine is not built: `backends.pl = "icarus"` is refused
with a message, not a crash.

## Troubleshooting

**`zynqsim` says a backend "was not found".** Install the tool and set the
variable from the table above, then run `zynqsim inspect`. A Vitis
`qemu-system-aarch64.bat` is a launcher script, not the emulator; use the `.exe`.

**A test or run printed `SKIPPED:` and passed.** That is the design: the tests
for a missing Xilinx tool pass without running anything. Check the output with
`--nocapture` if the result matters.

**`cannot find libxsim_dpi.a` or "exports nothing".** A normal workspace build
compiles the same crate without the DPI exports and writes the same file name.
Build it with `cargo xtask dpi` (into `target/dpi`), or use the archive's `lib/`.
Any `rp_open` symbol in the file means it is the right one.

**The DPI build fails on Windows with a linker error.** Use the GNU toolchain
(`rustup toolchain install stable-x86_64-pc-windows-gnu`) with a MinGW gcc on
`PATH`. A `.drectve` linker warning is harmless.

**`cargo` is not found in a new shell.** On Windows, `~\.cargo\bin` must be on
`PATH`, which `winget install Rustlang.Rustup` does not always do for shells
started before it ran. In Git Bash:
`export PATH="$HOME/.cargo/bin:$PATH"`.

**A firmware run is much slower than the simulated time suggests.** A poll of a
PL register from the PS costs about 100 microseconds of wall time while the
simulated time moves about 10 ns. Firmware that polls in a tight loop makes a 50 ms
wait take minutes; pace the polls (`rt::delay_us` in `firmware/rt`).

**A run failed and left no obvious error.** Failed runs keep their logs in
`<temp>/zynqsim/run-*` (or `ZYNQSIM_WORK_DIR`): `qemu.err.log`,
`xsim/xsim.out.log`, `build.log`.

**XSim builds take ~11 s each time.** Builds are cached by content in
`<temp>/zynqsim/xsim-cache`; a repeat start takes about 4.6 s. `ZYNQSIM_NO_BUILD_CACHE=1`
turns the cache off.

**Board pins or audio/HDMI do not appear for my own RTL.** The RTL sources must
mention the test-bench ports ZynqSim drives: `tel_ctrl` and `tel_event` for
board switches, buttons and LEDs; `zs_ssm2603`, `zs_hdmi`, `zs_audio_` or
`zs_video_` for the media models. Without them those capabilities are not advertised.

**A define with a value does not reach xvlog.** It must be quoted for `cmd`:
`-d "NAME=VALUE"`. ZynqSim does this; the message is for hand-written XSim scripts.

**The examples that use `source_dirs = ["../../firmware"]` lack their sources.**
The archive ships the examples and fixtures, not the firmware source tree. Their
simulation runs; source-level debugging needs a source checkout.

**The MCP server answers nothing.** It speaks JSON-RPC, one message per line, on
stdin and stdout; start-up errors go to stderr. Start it with `--root DIR` to
allow file access under DIR (default: the current folder).

**Antivirus or SmartScreen blocks `zynqsim.exe` on first run.** The binaries are
not signed. Check the hash against `SHA256SUMS` before allowing them.
