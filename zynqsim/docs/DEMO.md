# ZynqSim demo: test everything by hand and by script

This is the document to read while testing ZynqSim. It has three parts: a scripted tour
(`cargo xtask demo`) that checks each capability through the real binaries, a manual GUI tour with
exact commands and what must be seen, and the list of things that are expected NOT to work.
Status words (verified, implemented-untested, stub) mean what docs/STATUS.md says they mean.

## Prerequisites

| Needed for | What | Where it is found |
|---|---|---|
| building everything | Rust stable with the `stable-x86_64-pc-windows-gnu` toolchain (Windows) | `winget install Rustlang.Rustup` |
| the XSim DPI library | MinGW-w64 `gcc` (Qt Tools `mingw1310_64` or MSYS2) | `PATH`; the demo builds `target/dpi/release/libxsim_dpi.a` |
| PS on real firmware | Xilinx QEMU | Vitis install, `C:\Xilinx\Vitis\2022.1`, or `ZYNQSIM_QEMU` |
| RTL in the PL (XSim) | Vivado | `C:\Xilinx\Vivado\2022.1` (or 2023.1), or `ZYNQSIM_VIVADO` |
| Verilator | optional | `%TEMP%\zs-tools\verilator\oss-cad-suite\bin`, or `ZYNQSIM_VERILATOR` |
| Icarus Verilog | optional | the same archive (`iverilog.exe`, `vvp.exe`), or `ZYNQSIM_ICARUS`; the demo builds the `zynqsim.vpi` module (`cargo xtask vpi`) itself when the compiler is found |

The firmware ELFs are committed in `fixtures/elf`; the demo rebuilds them only if one is missing.
Run `zynqsim inspect` to see which backends were found. A step that needs a missing tool is reported
as `SKIPPED` with the reason in the table, never as a pass.

## Start

```
cargo xtask demo --quick       # about 1.5 minutes on the test machine (plus the first release build): 20 steps
demo.cmd --quick               # the same from a plain Windows terminal (a double-click also works)
cargo xtask demo               # the full tour: about 4 minutes 20 seconds on the test machine
```

Output goes to `demo-out/` in the `zynqsim/` folder (git-ignored): the OLED PNG, the HDMI PNG, the
WAV, the pcap, the Vivado hand-off project regenerated under `demo-out/handoff-project`, the GUI
screenshots under `demo-out/gui/` (with `--gui`), and `summary.txt` with the final table.

Options: `--quick` (the subset described below), `--only name,name` (run named steps, for example
`--only irq-demo,dma-sg`), `--gui` (also take the four GUI screenshots of section k; with `--only`
you can pick them by name, for example `--gui --only gui-board`).

Exit code: 0 when nothing FAILed, 1 when a step FAILed. SKIPPED is not a failure.

## The tour

Each step prints one line (`[b] irq-demo ... PASS (8.4s)`). A step PASSes only when its exit code is
0 and every check holds: the UART text contains the expected text, the `--json` fields have the
expected values, and each artifact file exists with the right magic number (PNG, WAV or pcap).
Steps in a hash group must give one trace hash: the math design through XSim, Verilator and Icarus
(same hash as the QEMU native PL run), and the Vivado hand-off example against the project the tour
regenerates with `zynqsim init --xsa`.

| Section | Steps | What is checked |
|---|---|---|
| a. PS only | rust-hello, native-hello, native-thumb, dual-core-native, dual-core-qemu, conf-scutimer, conf-ttc, bsp-hello, timer-rate, conf-iicps, conf-spips, conf-xadcps, conf-gpiops, conf-dmaps | UART1 text, `RESULT: ALL PASS` where the firmware prints it. Conformance checks that QEMU does not model are N/A, not PASS. bsp-hello is Xilinx's own Hello World, built unmodified |
| b. PL through QEMU and XSim | math-qemu (native PL model), dma-regression, irq-demo, board-demo, oled-demo, rgb-demo, tmp2-demo, als-demo, acl-demo, pmod-simple-demo, pcam-demo | UART results; board-demo LED duty values after scripted switch and shell input; the OLED picture (PNG); RGB duty values in `--json`; the temperature, light and acceleration you set come back in the firmware's readings |
| c. Other RTL backends | math-verilator, math-icarus, native-gpio-timer-uart, native-math, rtl-check | same trace hash for the math design through XSim, Verilator and Icarus (printed side by side); native-math passes all 7 vectors natively without external tools; `rtl-check` on `fixtures/rtl/math/my_math_ip.v` finds no diagnostics |
| d. Media | hdmi-demo (frame PNG), audio-demo (WAV) | UART results; PNG and RIFF/WAVE headers |
| e. Network and storage | net-demo (pcap), sd-demo, qspi-demo | 4/4 ping replies; pcap header; `HELLO.TXT` read back from the FAT card; QSPI erase and program read-back |
| f. Vendor IP | gpio-timer-uart, dma-loopback, dma-sg, iic-bram-demo, intc-demo, timer-modes | UART results of the AXI GPIO, timer, UART Lite, DMA (simple and scatter-gather), IIC and BRAM, interrupt controller, capture mode with a scripted button press |
| g. Debugger | rust-debug (`--break main.rs:19`), rust-debug-regs (MCP) | the run stops at line 19 of `main.rs` in `collatz_steps`; the MCP `cpu_regs` reply carries the registers |
| h. Vivado hand-off | vivado-handoff, handoff-init, handoff-run, boot-info | the shipped example runs; a project is created again with `zynqsim init --xsa ... --boot ...` and runs with the SAME trace hash; `boot-info` reports the BOOT.BIN as runnable |
| i. Determinism | determinism | `rust-hello` run three times gives one trace hash |
| j. MCP | mcp-tour | `zynqsim-mcp` over stdio on `board-demo`: initialize, tools/list (prints the tool count), project_open, sim_run 5 ms, uart_read (the shell banner), board_get_leds, waveform_get. Every reply is printed compactly |
| k. GUI (only with `--gui`) | gui-board, gui-rust-debug, gui-oled, gui-net | the window opens, writes its screenshot to `demo-out/gui/` and exits; the file has the PNG header |

`--quick` runs section a, `math-qemu`, `math-verilator`, `rust-debug`, `rust-debug-regs`,
`determinism` and `mcp-tour`: the PS checks, one PL design, one alternative RTL backend, the
debugger, determinism and the MCP server.

## Manual GUI tour

Start the GUI from the release build:

```
target\release\zynqsim-gui.exe --project examples\board-demo\project.toml
```

The window has a 2D board, a 3D board and a Waveforms tab across the top; the UART consoles at the
bottom (UART0 and UART1; type into UART1 with the Send line); the side panel on the right (speed,
System, CPU registers, Source, PL bus accesses, Pmod devices, Ethernet, the trace strip with its
filter and **copy visible**). The menus are **File** (Open project, Recent projects, Examples, Project
window), **Simulation** (Run or Pause with Space, Step with F6, Advance 100 ms, Reset with F5, Save
snapshot, Restore snapshot, Debugger), **View** (Waveforms, Side panel, Console, Larger with Ctrl+=,
Smaller with Ctrl+-, Normal size with Ctrl+0, Dark theme) and **Help** (Keyboard shortcuts with F1).

The screenshots below were taken with `zynqsim-gui --project P --screenshot FILE.png
--exit-after-ms N` (with `--run-ms` before the window, where the scenario needs simulated time). Every
one listed here was viewed when this document was written. Each caption says what the picture
shows, including what it does NOT show.

### 1. board-demo: switches, buttons, LEDs, shell, 2D and 3D

```
target\release\zynqsim-gui.exe --project examples\board-demo\project.toml
```

1. Press Space. The UART1 console prints `virtual Zybo Z7-20 ready (type help)` and a `>` prompt.
2. In the UART1 line at the bottom type `led 5` and press Enter. The reply is `LED=5`; LED0 and LED2
   light.
3. Click SW1 on the 2D board; type `status`. The reply is `LED=5 SW=2 BTN=0 ...`.
4. Click BTN0; `status` then shows `button events=1`.
5. Click the **3D board** tab: the same LEDs and switches are drawn in 3D. Drag to rotate.

Screenshot: `docs/screenshots/board-demo.png`, taken after 15 ms with no typed input. You see the
2D board with all LEDs off, the UART1 banner and prompt, and the CPU-registers panel saying that
registers need the debugger ("Restart under debugger" is offered). Typing `led 5` lights LED0 and LED2.

### 2. oled-demo: the Pmod devices panel and the display picture

```
target\release\zynqsim-gui.exe --project examples\oled-demo\project.toml --run-ms 30
```

The **Pmod devices** section of the side panel lists `JA oled`, the SSD1306 on Pmod JA, with its
picture: `ZYNQSIM OLED`, `HELLO FROM RUST`, `PS PL 100 MHZ`, `RESULT: PASS`. The picture is drawn
once the firmware has written it, so take it after about 30 ms; at 3 ms the picture is still blank.
The UART1 console shows `oled demo: SSD1306 128x32 on Pmod JA through oled_spi`, `sent 512 data bytes`,
`RESULT: ALL PASS`. The Pmod JA connector LEDs light on the board picture.

The picture is also written to a PNG with
`zynqsim run --project examples/oled-demo/project.toml --for 3ms --device-out JA=oled.png` (the tour
step oled-demo does this and checks the PNG header).

Screenshot: `docs/screenshots/oled-demo.png` (after 30 ms, viewed).

### 3. tmp2-demo: the temperature the firmware reads

```
target\release\zynqsim-gui.exe --project examples\tmp2-demo\project.toml --run-ms 8
```

The UART1 console prints `T13 raw=0x0BC0 mC=23500` and `T16 ...` (23.5 degrees C, the default). The
ADT7420 model answers the firmware's I2C reads.

The **Pmod devices** panel lists `JA tmp2` with the sensor's readout picture, its state line and a
slider `temperature (degC)`. Moving the slider sends the value to the running simulation; the firmware
sees it at its next read. To start at another temperature, use
`--device-set JA.temperature=30` (before the run), or the MCP tool `pmod_set`. The CLI flag of the same
name is checked by `tmp2_demo_reads_the_host_set_temperature_over_i2c` (`-12.25` reads back as
`mC=-12250`). The panel shows the slider below
the other sections, so with `--sections cpu=0,source=0,pl=0,pmod=1 --side-scroll 150` it is in view.

Screenshot: `docs/screenshots/tmp2-demo.png` (viewed, 2026-10-17): the sensor at 30 degC set with
`--device-set`, its readout picture, the slider at 30 and the console `T13 raw=0x0BC0 mC=30000`,
`T16 raw=0x0BC0 mC=30000`, the 2D board with the Pmod JA LEDs.

### 4. Waveforms tab

```
target\release\zynqsim-gui.exe --project examples\board-demo\project.toml --run-ms 15 --waves-tab
```

1. The Waveforms tab has a signal list on the left (bus.M_AXI_GP0.addr, data, write, uart1.tx; the
   search box and the groups board, uart, bus, pmod, other), a **follow live time** checkbox, the
   buttons **zoom to fit**, **+** and **-**, **clear cursor** and **export VCD**.
2. The default shown signal is `uart1.tx`; tick a signal in the list to add it.
3. The mouse wheel zooms, drag pans, a click sets the cursor, a right drag zooms to a selection.
4. **export VCD** saves the trace (dialog "Export trace as VCD"); the same signals open in any VCD
   viewer.

Screenshot: `docs/screenshots/waveforms.png`: the tab after 15 ms with `uart1.tx` shown; the bus
signals are listed but not shown.

### 5. rust-debug: registers, source, Continue

```
target\release\zynqsim-gui.exe --project examples\rust-debug\project.toml --debugger
```

The project starts under the debugger and stops at `main.rs:19` (the breakpoint in the project file).
You must see:

- **CPU registers**: the flags N Z C V, the PC as `_RNvCsdzHk0pLIqSu_10debug_demo13collatz_steps+0x1c`,
  r0 to r12, sp, lr, pc and cpsr;
- **Source (stopped)**: `core 0 debug_demo/src/main.rs:19` with the surrounding lines;
- the status bar: `running under the debugger: timing is not reproducible` and `halted at breakpoint`;
- the toolbar **Continue** button (the run goes on to the next stop or to the end) and **Step**.

On `rust-hello` (`examples\rust-hello\project.toml`) the same panels say "Restart under debugger";
click it to start that project under the debugger.

Screenshot: `docs/screenshots/rust-debug-registers.png` (this one, viewed).

### 6. net-demo: Ethernet counters and pcap

```
target\release\zynqsim-gui.exe --project examples\net-demo\project.toml --run-ms 10
```

The side panel shows **Ethernet (GEM0)** with the MAC, `10.0.2.2/24` and the gateway. The UART1 console
ends with `ping reply seq=2 ...`, `ping reply seq=3 ...`, `net demo: 4/4 replies, frames tx=7 rx=7` and
`RESULT: ALL PASS`. Write the frames to a file with `zynqsim run --project examples/net-demo/project.toml
--for 10ms --pcap net.pcap`; the file opens in Wireshark with DHCP, ARP and four ICMP echo pairs.

Screenshot: `docs/screenshots/net-demo.png`.

### 7. sd-demo: the card's files

```
target\release\zynqsim-gui.exe --project examples\sd-demo\project.toml --run-ms 60
```

The UART1 console prints the file `HELLO.TXT` read back from the card (`FILE HELLO.TXT size=178`) and
`RESULT: ALL PASS`. The side panel lists the storage device; the card is read only unless the project
says `writable = true`. This screen was not viewed in this run; the tour checks the UART text.

### 8. hdmi-demo and audio-demo

```
target\release\zynqsim-gui.exe --project examples\hdmi-demo\project.toml --run-ms 40
target\release\zynqsim-gui.exe --project examples\audio-demo\project.toml --run-ms 160
```

hdmi-demo: the UART1 console ends with `CAPTURE frames=1 checksum ...` and `RESULT: ALL PASS`; the
frame it scans out is written with `--video-out` (the tour checks the PNG). audio-demo: the UART1
console ends with `LOOPBACK frames=1000 ...`; the tone is written with `--audio-out` (WAV). Neither
screen was viewed in this run; check the sound with the speaker on, in real time, by hand.

### 9. Project window, menus and shortcuts

1. File > Examples lists the examples; pick `board-demo`. File > Recent projects lists the last ones.
2. View > Larger (Ctrl+=) and View > Dark theme switch the UI scale and theme.
3. Help > Keyboard shortcuts (F1) lists the shortcuts.
4. File > Project window opens the project window.

### 10. Snapshot, restore, reset

1. Run board-demo to 5 ms. Simulation > Save snapshot.
2. Run on to 15 ms and type `led 5` in UART1.
3. Simulation > Restore snapshot. The time goes back to 5 ms and the LEDs return to the saved state.
4. Simulation > Reset (F5). The project starts again from 0.

A restore re-runs the recorded session up to the snapshot and is refused if the trace differs. It is
done by re-execution, not a memory copy (docs/STATUS.md).

### 11. Trace: filter and copy

The trace strip has a filter field (text, no regex) and the checkboxes uart, board, bus, marker,
protocol. Type `uart` to show only those lines; **copy visible** puts the shown lines on the clipboard.

## The big one: the showcase

A Vivado block design (the user's FIR in Verilog + five Xilinx IP) and Rust firmware, run from a `BOOT.BIN`
on the Xilinx QEMU + XSim, or natively with no Xilinx tool: see [SHOWCASE.md](SHOWCASE.md).

## Try your own design

1. A Vivado hardware handoff (`design.xsa` exported from Vivado):

   ```
   zynqsim init --xsa my_design.xsa --rtl my_ip.v --boot BOOT.BIN --out my-project
   zynqsim run --project my-project\project.toml --for 15ms
   ```

   `xsa-report.md` in the project lists what was mapped and what was not. An IP without a model stops
   the run with a diagnostic; that is by design.

2. Vitis firmware: `zynqsim boot-info BOOT.BIN` tells you whether the image is runnable, then a project
   with `init --boot` runs it on the PS.

3. A single RTL block with its testbench generated: `zynqsim init --ip my_ip.v --elf fw.elf`.

## What is expected NOT to work

From docs/STATUS.md and the example headers. Each item is either not run by the tour, or shown as
SKIPPED or N/A.

- **bsp-peripherals** (Xilinx's peripheral test application): on this QEMU machine the CAN and
  device-configuration self tests FAIL (those peripherals are not in the model), and the SCU watchdog
  interrupt test never returns. Not in the tour; see docs/conformance/bsp-apps.md.
- **board-demo across processes**: its shell input over the UART TCP path arrives at wall-clock
  dependent virtual times, so its trace hash differs between runs (STATUS.md FINDING). The tour checks
  its LED values, not its hash.
- **math-debug** (QEMU debugger timing): the guest state at each stop is reproducible, the timing is
  not. Not in the tour.
- **Unmodelled IP** in a Vivado hand-off stops the run with a diagnostic (validation), by design.
- **SPI and PS GPIO conformance**: SPI is not modelled by default and QEMU maps no PS GPIO at the
  standard address, so those checks are N/A, not PASS (docs/conformance/iic-spi-xadc.md).
- **Verilator and Icarus**: Verilator is 2-state. Icarus is verified on Windows only; the test machine
  has Icarus Verilog in the portable oss-cad-suite, and the demo builds its VPI module when the compiler
  is found. Neither was run on Linux or macOS.
  Questa, VCS and Xcelium loading the DPI library are not tested.
- **Board comparisons**: nothing here is compared with a real Zybo board; pin locations are unverified.
- **Pcam**: the MIPI CSI-2 link is not simulated; the sensor hands the PL a parallel pixel bus.

## If something fails

1. Read the `why:` line printed under the failing step. The table at the end gives the wall time and
   the artifact path.
2. Run the step alone with its run directory kept:

   ```
   set ZYNQSIM_KEEP_RUN=1
   cargo xtask demo --only irq-demo
   ```

   The run directories are in `%TEMP%\zynqsim` (or `ZYNQSIM_WORK_DIR`): `qemu.err.log`,
   `xsim\xsim.out.log`, `build.log`. Open the newest `run-*` folder.
3. Diagnose in this order: (a) is the tool found (`zynqsim inspect`)? (b) does the UART text show how
   far the firmware got? (c) does the end of the run log say what XSim or QEMU reported?

Known flaky checks: tests that use wall-clock input over the UART TCP path (board-demo and the shell
tests) can miss a 5 ms window under load; re-run them alone before judging. The MCP scenario tests are
sensitive to machine load when several test gates run at once.

Stray processes: if a run hangs for more than 10 minutes, kill the leftovers before re-running:

```
taskkill /F /IM qemu-system-aarch64.exe
taskkill /F /IM xsim.exe
```

## Last verified run

Date: 2026-10-17. Machine: Windows 11 Pro 10.0.26300, the test machine of this repository (Vitis and
Vivado 2022.1 installed, Vivado 2023.1 also installed, Verilator from oss-cad-suite, no Icarus).
Commit: the commit that adds this tour (see `git log -1 -- zynqsim/crates/xtask/src/demo.rs`).
Build: `cargo xtask demo` builds the release binaries first; the wall times below are the tour
steps only, after the build.

Full tour (`cargo xtask demo`), wall time of the whole tour **4m17s**:

```
sec step                   example                result       wall
a   rust-hello             rust-hello             PASS         1.5s
a   native-hello           native-hello           PASS         0.4s
a   native-thumb           native-thumb           PASS         0.8s
a   dual-core-native       dual-core-native       PASS         1.8s
a   dual-core-qemu         dual-core-qemu         PASS        11.3s
a   conf-scutimer          conf-scutimer          PASS        18.9s
a   conf-ttc               conf-ttc               PASS         4.8s
a   bsp-hello             bsp-hello              PASS         1.5s
a   timer-rate             timer-rate             PASS         1.6s
a   conf-iicps             conf-iicps             PASS         5.0s
a   conf-spips             conf-spips             PASS         5.1s
a   conf-xadcps            conf-xadcps            PASS         4.9s
a   conf-gpiops            conf-gpiops            PASS         2.6s
a   conf-dmaps             conf-dmaps             PASS         5.0s
a   math-qemu              math-qemu              PASS         1.1s
b   dma-regression         dma-regression         PASS         8.7s
b   irq-demo               irq-demo               PASS         8.4s
b   board-demo             board-demo             PASS         8.6s
b   oled-demo              oled-demo              PASS         8.7s
b   rgb-demo               rgb-demo               PASS         8.1s
b   tmp2-demo              tmp2-demo              PASS         8.8s
b   als-demo               als-demo               PASS         8.3s
b   acl-demo               acl-demo               PASS         4.7s
b   pmod-simple-demo       pmod-simple-demo       PASS        11.0s
b   pcam-demo              pcam-demo              PASS        13.6s
c   math-verilator         math-verilator         PASS         1.7s
c   math-icarus            math-icarus            SKIPPED (Icarus Verilog not found)   0.0s
c   native-gpio-timer-uart native-gpio-timer-uart PASS         1.7s
c   rtl-check                                     PASS         0.0s
d   hdmi-demo              hdmi-demo              PASS        16.6s
d   audio-demo             audio-demo             PASS        23.1s
e   net-demo               net-demo               PASS         1.7s
e   sd-demo                sd-demo                PASS         5.9s
e   qspi-demo              qspi-demo              PASS         6.6s
f   gpio-timer-uart        gpio-timer-uart        PASS         1.3s
f   dma-loopback           dma-loopback           PASS         1.2s
f   dma-sg                 dma-sg                 PASS         1.1s
f   iic-bram-demo          iic-bram-demo          PASS         1.4s
f   intc-demo              intc-demo              PASS         1.1s
f   timer-modes            timer-modes            PASS         1.7s
g   rust-debug             rust-debug             PASS         1.2s
g   rust-debug-regs        rust-debug             PASS         1.2s
h   vivado-handoff         vivado-handoff         PASS         9.1s
h   handoff-init                                  PASS         0.0s
h   handoff-run            (demo-out/handoff-project)  PASS   9.4s
h   boot-info                                     PASS         0.0s
i   determinism            rust-hello             PASS         4.6s
j   mcp-tour               board-demo             PASS        11.2s
total: 47 PASS, 0 FAIL, 1 SKIPPED in 4m17s
hash group math: identical: math-qemu=fnv1a64:c801168503080d41  math-verilator=fnv1a64:c801168503080d41
hash group handoff: identical: vivado-handoff=fnv1a64:a0040ac376267dd2  handoff-run=fnv1a64:a0040ac376267dd2
```

The Icarus step is SKIPPED because `iverilog`/`vvp` are not installed here. Installing them is expected
to make the same step run and join the `math` hash group; that has not been run on this machine.

Quick tour (`cargo xtask demo --quick`, same machine and commit): **20 PASS, 0 FAIL, 0 SKIPPED in 1m27s**
(the tour steps; the release build comes first and is not in that figure).

Known issues found while verifying: none open in the tour. Two MCP steps failed in the first quick run
(a wrong check on the debugger's `tools/list` reply, and `board_get_leds` on the native PS, which has
no board I/O); both were fixed in the tour and pass in the runs above. The other example headers still
say `--for 5ms` for native-hello and native-thumb; those firmwares print their last line after 20 ms,
so the tour uses 20 ms.

Icarus after the fix (2026-10-17, same machine): the tour printed `math-icarus SKIPPED (Icarus Verilog
not found)` because `inspect` reports Icarus found only with its `zynqsim.vpi` module, which the tour
never built. `cargo xtask demo --only math-icarus,math-verilator` now builds the module and gives
`math-verilator PASS (1.8s)`, `math-icarus PASS (2.7s)`, one hash group `fnv1a64:c801168503080d41`.
The full tour table above is from before that fix; re-run the tour to refresh it.

Screenshots in `docs/screenshots/` (each viewed when this document was written): board-demo.png,
oled-demo.png (2026-10-17, after 30 ms), waveforms.png, net-demo.png, tmp2-demo.png (2026-10-17, the
slider at 30 degC), rust-debug-registers.png.
