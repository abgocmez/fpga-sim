# Prompt: ZynqSim — a universal PS+PL simulator for Zynq-7000 / Zybo Z7-20 (Rust-first, pragmatic)

> Paste this whole document as the task for a coding agent. Every section is a requirement
> unless it says "stretch" or "phase 3".
> Revision 3: **Rust is the default and the goal, not a dogma.** Where a Rust rewrite would
> make the project much longer without helping the user, use a proven external engine behind
> an adapter and keep everything else in Rust (§2).

---

## 0. Role and mission

You are a senior systems engineer building **ZynqSim**: a deterministic, full-system
simulator of AMD/Xilinx **Zynq-7000** devices — reference board **Digilent Zybo Z7-20**
(**XC7Z020-1CLG400C**) — with a **visual model of the board**, a headless CLI and an
**MCP server**, so developers and AI agents can build, run, observe and debug PS+PL designs
with no physical board.

At the end a user must be able to bring **any** Zynq PS+PL design (their software and their
RTL, or a Vivado/Vitis build output) and simulate it, see it on the virtual board, debug
it, and automate it.

### Success definition (all must hold)
1. **Universality**: given ELF/`BOOT.BIN` software and RTL (Verilog/SystemVerilog sources,
   post-synthesis netlists, or Vivado simulation exports) the system elaborates the design,
   connects it to the PS model and runs it, within the documented supported subset (§9).
   Anything outside fails with a precise diagnostic, never with silently wrong behaviour.
2. Unmodified Xilinx **standalone BSP** applications (UART, GPIO, GIC, timers, QSPI, SD/FatFs,
   GEM/lwIP, I2C, SPI, XADC, PL330 DMA) behave as on hardware at register level.
3. PL logic is **cycle-exact at its own ports** (RTL semantics) and attached to the PS ports
   (`M_AXI_GP0/1`, `S_AXI_GP0/1`, `S_AXI_HP0-3`, `S_AXI_ACP`, `IRQ_F2P`, `FCLK/RESET`, EMIO)
   and board pins.
4. The GUI shows the board as a picture with live LEDs, switches, buttons, Pmods, HDMI video,
   audio, Ethernet, SD, QSPI and UART, and lets the user drive the inputs.
5. Runs are **deterministic** (same inputs → same trace hash on the same backend set).
6. One control plane serves the GUI, the CLI and an **MCP server**.
7. Documentation states, per subsystem, what is and is not modelled.

---

## 1. Honesty contract (non-negotiable)

* **Do not claim cycle-accuracy where it does not exist.** Cortex-A9, PL310, SCU and the DDR
  controller are closed IP. Label every result with the fidelity level of §5 and the
  backend that produced it.
* PL logic is where exactness is achievable (RTL semantics); that is the strongest guarantee.
* **Do not claim to run what you cannot.** Vendor IP delivered as encrypted RTL (IEEE 1735)
  cannot be simulated; detect it and substitute a library model (§9.5) or stop with a
  diagnostic that names the IP.
* Every feature carries a status in `docs/STATUS.md`: `verified` (test names), `implemented-
  untested`, `stub`, `not-planned`. Never write "works" without a test.
* Where a hardware fact is uncertain write `VERIFY:` plus the document to check (UG585,
  Zybo Z7 Reference Manual, schematic) instead of guessing.

---

## 2. Rust-first policy (read carefully)

### 2.1 What must be Rust, from day 1
The **orchestrator and everything the user touches**: `sim-api` control plane, GUI
(egui/eframe), CLI, **MCP server**, board model and board devices (Pmod modules, audio,
HDMI, camera, PHY…), host backends (serial, network, audio, video, files), tracing and
waveform export, protocol bridges (remote-port client/server, GDB stub), project/XSA
ingestion, test drivers and build automation (`xtask`), example firmware (`no_std`).

**No C, C++, Python, PowerShell, shell, JavaScript or HTML source is written or committed
for any of the above.** (RTL, TOML, SVG and linker scripts are data/design inputs.)

### 2.2 What may be an external engine (heavy, closed-world components)
Rewriting these in Rust is a large effort with little user value at the start:

| Component | Phase 1–2 default | Allowed alternatives | Native-Rust option |
|---|---|---|---|
| **PS: Cortex-A9 cores + PS peripherals** | Xilinx QEMU (Zynq-7000 machine) driven as a child process by a Rust adapter (`backend-qemu`) over remote-port, monitor and chardevs | another ISS | `cpu-a9` + `ps-periph` (phase 3) |
| **PL: RTL execution** | external simulators behind `backend-rtl-*` adapters: **Verilator** (fastest; needs a C++ toolchain on the host), **Icarus Verilog** (easy install, slower, full 4-state), **Vivado XSim** (when the user has Vivado) | any simulator exposing a co-sim interface | `rtl-frontend` + `rtl-engine` (subset interpreter, phase 3) |
| PL native models | Rust (`pl-native`) | — | — |

The user chooses a backend per project (`[backends]` in `project.toml`); the adapter
interfaces (`PsBackend`, `PlBackend`) are the same for external and native engines, so a
native replacement is a drop-in.

### 2.3 Decision rule for any further component
Default to Rust. Use an external engine only if **all** hold:
1. a Rust implementation is estimated to be **> 4 engineer-weeks** or needs a closed spec,
2. a proven engine exists with a licence compatible with Apache-2.0 distribution or as an
   *optional, user-installed* tool,
3. it can be isolated behind a trait/IPC boundary so it can be replaced later,
4. the dependency is recorded as an ADR in `docs/adr/NNN-*.md` (context, options, decision,
   consequences, exit plan).
External tools are **never vendored as source** into the repository, are located at run
time (PATH/config), and their absence yields a clear message and a graceful fallback.

### 2.4 What the policy costs (be explicit in the docs)
* With the QEMU PS backend, fidelity is **F0 + instruction-count timing** only; the F1/F2
  models of §5 (caches, DDR timing, non-coherent-DMA strictness) need the **native PS
  backend**. State this in the UI (fidelity badge).
* Snapshot/rewind of the PS is limited to what the external engine offers; full-machine
  snapshots need the native backend.
* Determinism requires the lock-step techniques listed in the appendix (§19); they are part of
  the QEMU adapter's contract, not optional.

### 2.5 Other hard constraints
* `#![forbid(unsafe_code)]` except crates that exist for OS/FFI/JIT reasons (isolated,
  documented, fuzzed).
* `#![deny(missing_docs)]` on libraries; `cargo clippy -- -D warnings`; `cargo fmt --check`;
  CI on Windows, Linux, macOS; Apache-2.0 with `THIRD_PARTY.md`.
* Single `cargo build --release` produces `zynqsim`, `zynqsim-gui`, `zynqsim-mcp`.
* The repository must build and its tests must pass **without** QEMU, Verilator, Vivado or
  Vitis installed (tests needing them are skipped and reported as skipped).

---

## 3. Workspace layout

```
zynqsim/
  crates/
    sim-api/         control plane: typed commands, queries, event stream (serde)
    sim-core/        time base, scheduler, clock domains, deterministic replay
    bus/             address map, AXI-style transactions, interconnect/latency models
    ps-backend/      PsBackend trait + common types (regs, memory, IRQ, UART, GPIO, ...)
    backend-qemu/    adapter: spawns/controls Xilinx QEMU, remote-port, HMP, serial TCP
    pl-api/          PlBackend/PlModule traits, AXI4/Lite/Stream ports, clocking, checkers
    backend-rtl-verilator/ backend-rtl-icarus/ backend-rtl-xsim/    external RTL adapters
    pl-native/       native Rust PL models + RTL-style Rust DSL + reference IP library
    vendor-ip/       behavioural Rust models of common Xilinx IP (same register maps)
    xsa-import/      XSA/HWH parser, Vivado export_simulation / file-list ingestion
    boot/            BOOT.BIN parser, synthetic boot, ps7_init replay
    board-zybo/      board definition: pins, connectors, components, layout
    devices/         board-side devices: PHY, SSM2603, flash, SD, Pmod modules, HDMI, camera
    host/            serial, user-mode network, audio, video, files
    trace/           VCD/FST, Perfetto, AXI/IRQ monitors
    gdb-stub/        GDB RSP server
    interop/         remote-port client/server; Rust cdylib exposing a DPI-compatible ABI
    mcp/             MCP server on sim-api
    gui/             egui application
    cli/             headless runner
    xtask/           automation
  # phase 3 (optional native engines, same traits):
    cpu-a9/ mem-sys/ gic/ ps-periph/        native PS backend
    rtl-frontend/ rtl-engine/ unisim/       native RTL backend (Verilog subset)
  firmware/  examples/  boards/  docs/
```
Rules: front-ends (`gui`, `cli`, `mcp`) depend on `sim-api` only; engines plug in through
`PsBackend`/`PlBackend`; nothing depends on `gui`.

---

## 4. Time, scheduling, determinism

* One global timeline in **picoseconds**; deterministic event scheduling; no component reads
  the host clock.
* **Clock domains** are first-class (CPU, FCLK0–3 following SLCR at run time, pixel, I2S,
  user PL clocks).
* PL is evaluated per clock edge with RTL scheduling semantics; module ordering never
  changes results.
* Ties by `(time, priority, creation id)`; randomness only from a seeded, snapshotted PRNG.
* GUI/MCP/CLI inputs are timestamped into the timeline → **deterministic replay of recorded
  sessions**.
* **Lock-step coupling between PS and PL backends** at a bounded quantum (default 1 µs):
  the PL advances to the PS timestamp before the PS continues; interrupts are applied
  acknowledged; PL→memory accesses cost zero simulated time plus a fixed modelled latency
  (appendix §19). This is the adapter contract for every external backend.

---

## 5. Fidelity levels (recorded in every trace)

| Level | Available with | CPU timing | Memory timing |
|---|---|---|---|
| **F0 functional + icount** | QEMU or native PS | 1 instruction = fixed ps | zero latency (+ fixed PL port latency) |
| **F1 annotated** | native PS only | in-order latency table, branch penalty, cache model, TLB cost | L1/L2, SCU, DDR bank/row/refresh, interconnect arbitration |
| **F2 detailed** (stretch) | native PS only | dual-issue A9 pipeline model | + write buffers, prefetch |

A `calib.toml` format (per-class CPI, cache and DDR parameters) and a documented procedure
to fit it from PMU/global-timer measurements ship with the native backend; defaults are
datasheet-derived and labelled `uncalibrated`. Non-coherent HP-DMA vs cached CPU data is
modelled (switchable strictness) in the native backend so missing cache maintenance
reproduces the hardware bug.

---

## 6. PS subsystem

**Phase 1–2 (`backend-qemu`)**: launch Xilinx QEMU with the Zynq-7000 machine and the
Xilinx co-sim device tree; connect the PL through a remote-port chardev; read CPU state and
memory through the monitor; carry UART over TCP; load per-core ELFs; control icount, quantum
and snapshot features; expose everything through `PsBackend`. The adapter must provide:
CPU registers/PC/mode/WFI-WFE state of both cores, physical-memory read/write, breakpoints
(via the GDB stub QEMU provides), UART RX/TX, GPIO/EMIO, interrupt injection from PL,
SD/QSPI image attachment, network backend (`-nic user`) with `.pcap` export.

**Phase 3 (native)**: ARMv7-A ISS (ARM + Thumb-2, VFPv3-D32, NEON stretch), CP15, MMU/TLB,
exceptions, dual core with SCU, GIC PL390, private/global timers, L1/L2 models, PS
peripherals with register-level fidelity from UG585 (see §8), boot-ROM CPU1 parking
behaviour. Differential tests against the QEMU backend instruction-by-instruction are the
primary correctness oracle.

---

## 7. Memory map and interconnect

Implement/describe the Zynq-7000 physical map from **UG585** (VERIFY every address):
DDR from `0x0` (Zybo Z7-20: **1 GB DDR3L**), `M_AXI_GP0` at `0x4000_0000` (1 GB),
`M_AXI_GP1` at `0x8000_0000`, IOP peripherals from `0xE000_0000`, SMC `0xE100_0000`,
SLCR/TTC/DMAC/SWDT/DDRC/DevC from `0xF800_0000`, SCU/GIC/timers/L2 from `0xF8F0_0000`,
QSPI linear `0xFC00_0000`, OCM high `0xFFFC_0000`.
Transactions carry address, size, burst, AXI attributes, data, byte enables, response and
a timestamp. The `bus` crate owns interconnect latency/bandwidth models usable by every
backend (PL ports always; PS side in the native backend). Unmapped access → DECERR.

---

## 8. PS peripherals

Required behaviour per block (register-level fidelity from UG585; Xilinx BSP drivers are
the executable specification): **SLCR** (clocks/resets/MIO), **UART0/1**, **GPIO+EMIO**,
**TTC0/1**, **SWDT**, **PL330 DMAC**, **GEM0/1** (+PHY, MDIO), **SDIO0/1**, **QSPI**,
**SPI0/1**, **I2C0/1**, **CAN0/1**, **USB0/1** (minimum probe), **XADC/DevC**, OCM/DDRC/SMC
registers. In phase 1–2 these come from QEMU's model and are *verified* by the conformance
tests of §14 (record which blocks QEMU models only partially); in phase 3 they are native.
Every block has a host backend and a UI representation as in the table of §11.

---

## 9. PL subsystem — the universal path

### 9.1 Supported inputs
1. **Verilog-2001** and the common **synthesizable SystemVerilog** subset;
2. **behavioural test-bench constructs** (`initial`, delays, event controls, `$display`,
   `$finish`, `$readmemh`);
3. **post-synthesis functional netlists** (`write_verilog -mode funcsim`) with **UNISIM**
   primitives;
4. **Vivado simulation exports** (`export_simulation` file lists, `.prj`, `.f`, `glbl.v`);
   **XSA/HWH** for address map, clocks, interrupts, MIO;
5. native Rust models and the Rust RTL DSL (§9.6);
6. VHDL subset (stretch; depends on the chosen backend).
Which subset is supported depends on the backend; `docs/RTL_SUBSET.md` states it per backend
and the tool prints **file/line/construct** for anything unsupported.

### 9.2 Backend selection and capabilities
| Backend | Strengths | Requirements | Limits |
|---|---|---|---|
| Verilator | speed, wide SV support (2-state) | C++ toolchain on the host; model build step | no 4-state X/Z, test-bench timing constructs limited |
| Icarus Verilog | easy install, 4-state, behavioural test-benches | `iverilog`/`vvp` | slow, partial SV |
| Vivado XSim | exact vendor semantics, encrypted IP, VHDL | Vivado install | slower start-up, licence terms |
| native subset engine (phase 3) | no external tools, deterministic snapshots | none | only the documented subset |

Each adapter implements `PlBackend` with the same AXI/IRQ/clock/pin binding model (§9.4)
and must report capabilities (`supports_x`, `supports_snapshot`, `supports_vhdl`).

### 9.3 Semantics the adapters must honour
* PL clocks and reset from the SLCR model (FCLK0–3, FPGA resets), per-edge sampling.
* Lock-step quantum with the PS (§4); PL outputs observable at picosecond timestamps.
* Waveforms: VCD/FST for every bound signal; signal queries via `sim-api`.

### 9.4 Binding to the PS and the board
Via a `connect` table (§12): `M_AXI_GP0/1`, `S_AXI_GP0/1`, `S_AXI_HP0–3`, `S_AXI_ACP`
(AXI4 / AXI4-Lite / AXI-Stream naming and `X_INTERFACE_INFO` recognised), `IRQ_F2P[15:0]`,
`IRQ_P2F`, `FCLK_CLK0–3`, `FCLK_RESET0–3_N`, EMIO[63:0], board pins. From an HWH, wire the
design as the block design did. **Protocol checkers** (VALID/READY rules, 4 KB crossing,
WLAST, alignment) are on by default and report with timestamps.

### 9.5 Library models of vendor IP (`vendor-ip`)
Rust behavioural models with the **same register maps and interrupt behaviour** as: AXI
GPIO/Timer/DMA (simple + SG)/VDMA/IIC/Quad SPI/UART Lite/BRAM controller/Interconnect and
SmartConnect, Clocking Wizard, Processor System Reset, AXI4-Stream FIFO/switch/width
converter, ILA/VIO (→ waveform viewer), Video Timing Controller, AXI4-Stream-to-Video-Out,
RGB2DVI/DVI2RGB. Encrypted vendor IP is recognised by VLNV/module name and mapped to
these models, or the run stops with a diagnostic naming the IP.

### 9.6 Authoring PL logic in Rust (`pl-native`)
Embedded RTL-style DSL (registers with `next/commit`, clock enable, reset, bit-vector
helpers, FSM macro, memories, AXI-Lite register-bank `derive`). Trait:
```rust
pub trait PlModule {
    fn ports(&self) -> PortList;
    fn clocks(&self) -> &[ClockId];
    fn reset(&mut self);
    fn eval(&mut self, ctx: &mut PlCtx);       // once per active clock edge
    fn snapshot(&self) -> Vec<u8>; fn restore(&mut self, s: &[u8]);
}
```
Native modules and external RTL backends can coexist in one design.

### 9.7 Interop (`interop`, Rust only)
Remote-port client **and** server (hello/cfg/read/write/interrupt/sync/ats; capabilities
`BUSACCESS_EXT_BASE`, `WIRE_POSTED_UPDATES`), used by `backend-qemu`; plus a **Rust
`cdylib` exporting a DPI-compatible C ABI** so XSim/Questa test-benches can import it.
Device numbering for the Xilinx Zynq-7000 co-sim tree: `M_AXI_GP0 = 7`, `M_AXI_GP1 = 8`,
wires-in `= 9`, wires-out `= 10`, interrupt `= 11`, `S_AXI_HP0 = 2`, `S_AXI_ACP = 6`.

---

## 10. Board model: Digilent Zybo Z7-20 (`board-zybo`, `devices`)

Describe the board in `boards/zybo-z7-20.toml` (components, pins, connections, layout) and
implement device models. *PL* = PL pins, *PS* = MIO/EMIO. VERIFY every pin/MIO assignment in
the Zybo Z7 Reference Manual and schematic.

| Component | Side | Model |
|---|---|---|
| 4 slide switches, 4 push buttons, 4 LEDs | PL | logic levels, UI controls |
| 2 RGB LEDs | PL | PWM duty → colour |
| Pmod ports (VERIFY count; XADC dual-purpose; one on MIO) | PL/PS | pin bundles; pluggable modules: OLED (SSD1306-class), ALS, temperature (ADT7420-class), 8LD, SSD, accelerometer, buttons/switches, logic analyser |
| HDMI **sink** (RX) and **source** (TX) — *no VGA on this board* | PL | pixel-level RGB + HS/VS/DE, EDID over DDC; RX from images/patterns/video; TX to a frame window and `.png` |
| Pcam 5C / MIPI CSI-2 | PL | sensor model at the pixel interface the camera IP expects + SCCB model (lane-level CSI-2 out of scope) |
| Audio codec **SSM2603** (I2C + I2S + MCLK) | PL | register model, I2S TX/RX, line-in from WAV/tone, host audio and `.wav` |
| Gigabit Ethernet PHY (RTL8211E-class, RGMII) | PS | MDIO registers, link/auto-neg → GEM0 (note: the QEMU reference tree models a Marvell PHY; handle ID differences) |
| microSD, QSPI flash (16 MB class), USB-UART, USB OTG PHY | PS | §8 models |
| DDR3L, XADC header, clocks (125 MHz PL, 33.33 MHz PS), reset/boot jumpers | — | §7, §8, §4 |

**`BoardDevice` API** (attach to I2C/SPI/GPIO/UART/Pmod pins, clocked callbacks, UI widget
description) for user-added hardware. **Custom boards** are a TOML + SVG pair (Zybo Z7-10,
ZedBoard, PYNQ-Z2 as examples). Board devices run in Rust and are independent of the
PS/PL backends.

---

## 11. Visual board model (`gui`)

egui/eframe, dockable, persistent panels:

1. **Board view**: scaled vector drawing (`boards/*.svg` + layout TOML) with live LED glow
   (PWM colour/brightness), clickable/holdable switches and buttons (keyboard shortcuts),
   Pmod pins (0/1/Z, activity strobes), Ethernet link/activity LEDs, DONE/power LEDs;
   hover tooltips; click a connector to open its tool; the SoC shows per-core state,
   clocks, AXI traffic and the **fidelity/backend badge**.
2. **CPU panels (×2)**: registers, CPSR, disassembly with ELF symbols and DWARF lines
   (`gimli`), breakpoints/watchpoints, step, call stack.
3. **PL panel**: hierarchy, signals, waveform viewer, AXI monitors with latency histograms,
   checker violations, elaboration diagnostics.
4. **Memory panel**: hex/ASCII, typed views, **image viewer** (Gray8, RGB565, RGB888,
   YUV422; width/height/stride), watch expressions.
5. **Console**: UART0/UART1 terminals with input and scroll-back.
6. **Video/audio**: HDMI window with statistics, audio meter/spectrum.
7. **Network**: link, counters, packet list, `.pcap` export, port forwarding.
8. **Timeline**: IRQs, DMA, AXI bursts, UART bytes, frames; Perfetto export.
9. **Control bar**: run/pause/step (quantum/clock/instruction), run-until, speed,
   snapshot/rewind (where the backend supports it), seed, reset.
10. **Clock tree, interrupt map, MIO/EMIO map, SLCR register browser.**

The GUI never blocks the simulation and the simulation never depends on frame rate.

---

## 12. Projects, ingestion, boot (`xsa-import`, `boot`, `cli`)

```toml
schema_version = 1
[board]    name = "zybo-z7-20"; ddr_mb = 1024
[backends] ps = "qemu"; pl = "verilator"          # or "icarus" | "xsim" | "native"
[sim]      quantum_ns = 1000; seed = 1; icount_shift = 1
[boot]     mode = "elf"                            # "elf" | "boot_bin"
[cpu0]     elf = "app0.elf"
[cpu1]     elf = "app1.elf"                        # optional
[rtl]      files = ["rtl/*.v", "rtl/*.sv"]; top = "design_1_wrapper"; defines = ["SIM"]
[connect]  "u_accel.s_axi" = "M_AXI_GP0@0x43C00000"
           "u_accel.m_axi" = "S_AXI_HP0"
           "u_accel.irq"   = "IRQ_F2P[0]"
[devices]  pmod_ja = "oled"; sd = "card.img"; qspi = "flash.bin"
[host]     uart1 = "gui"; net = "user"; audio = "default"
```
Precise validation messages; `zynqsim init --xsa design.xsa` or `--filelist files.f`
scaffolds a project. **XSA/HWH ingestion**: clocks, address map, IRQ numbers, MIO and
peripheral configuration, `ps7_init` data. **`BOOT.BIN`**: parse header and partitions,
run the FSBL as software (its `ps7_init` writes executed by the PS model), map the bitstream
partition to the user's RTL project; document the difference from real boot-ROM timing.

---

## 13. Control plane and MCP server (`sim-api`, `mcp`, `cli`)

`sim-api` defines typed commands, queries and an async event stream; GUI, CLI and MCP are
thin front-ends. Commands are timestamped; sessions can be recorded and replayed.

### MCP server (`zynqsim-mcp`, Rust; stdio, optional streamable HTTP)
| Tool | Purpose |
|---|---|
| `project_open`, `project_create_from_xsa`, `project_validate` | load/create, report elaboration diagnostics and unsupported constructs |
| `sim_run`, `sim_pause`, `sim_step`, `sim_run_until`, `sim_reset` | run control (time, clocks, instructions, event) |
| `snapshot_save`, `snapshot_restore`, `session_replay` | state management (if backend supports) |
| `cpu_regs`, `cpu_disasm`, `mem_read`, `mem_write`, `symbol_lookup` | PS inspection |
| `breakpoint_set`, `watchpoint_set`, `breakpoint_list` | debugging |
| `uart_write`, `uart_read` | console I/O |
| `board_set_switch`, `board_press_button`, `board_get_leds`, `pmod_attach`, `pin_get` | board interaction |
| `pl_signal_get`, `pl_signal_trace`, `pl_axi_log`, `waveform_export` | PL observation |
| `video_frame`, `board_screenshot`, `audio_capture` | PNG/WAV results |
| `net_send`, `net_capture` | network |
| `config_set`, `status_get`, `backend_list` | fidelity/quantum/seed, capabilities, installed backends |

Also MCP **resources** (project, logs, STATUS, waveform files) and **prompts** ("debug a
hang", "explain this AXI violation"). Rules: JSON schemas with descriptions and examples,
bounded/paginated outputs, structured errors, file access limited to the project directory
and allow-listed paths, **no arbitrary command execution**, cancellable long operations
with progress, and an in-repo Rust MCP-client scenario test.

CLI: `zynqsim run|test|init|inspect|trace|mcp|gui|ctl` with `--json` output and CI-friendly
exit codes.

---

## 14. Testing and acceptance criteria

`docs/TESTPLAN.md` maps requirements to tests; **all test drivers are Rust**. Tests that need
an external engine are skipped (and listed) when it is absent.

1. **Prototype regression** (first acceptance test of phase 1): a Zybo-style design with
   a DMA image accelerator (`out = clamp(in*scale+offset)`, 16-beat INCR bursts, IRQ) and a
   GPIO block, firmware doing 7 vector tests + 6-job two-core pipeline + a 64×64 filter;
   results must equal a Rust golden model; **8/8 identical traces**.
2. **Peripheral conformance**: Rust `no_std` drivers and (when supplied) Xilinx BSP
   examples for xuartps, xgpiops, xscugic, xscutimer, xttcps, xdmaps, xqspips, xsdps+FatFs,
   xemacps+lwIP, xiicps, xspips, xadcps — self-report PASS; record per block whether the PS
   backend models it fully, partly or not.
3. **Multicore**: WFE/SEV ping-pong, LDREX/STREX locks, SGIs, producer/consumer.
4. **PL**: AXI protocol-checker tests (valid + deliberately illegal behaviour); bursts
   crossing 4 KB; interrupt-driven completion; cache-maintenance dependence (native PS).
5. **Universality corpus** (`tests/rtl`): ≥ 10 permissively licensed third-party designs
   (record name/licence/commit) plus Vivado funcsim netlists; per backend a table of
   pass/unsupported with diagnostics; every unsupported construct has a negative test.
6. **Determinism**: every integration test runs 5×; trace hashes identical per backend set.
7. **GUI**: headless board render compared with golden PNGs.
8. **MCP**: schema validation of every tool, scenario test (load, run, press button, read
   UART, screenshot, trace a signal), fuzzed inputs.
9. **Performance**: report MIPS/clock-rate per backend; no hard targets for external
   engines, targets for native backends (F0 ≥ 50 MIPS/core, RTL fast path ≥ 1 M cycles/s on
   a 5 k-LUT design).
10. **Fuzzing**: `cargo fuzz` for project parser, MCP requests, remote-port packets and (when
    present) the native decoder/RTL parser.

---

## 15. Milestones (value first; each ends with a runnable demo and `STATUS.md` updated)

| M | Deliverable | Exit criterion |
|---|---|---|
| **P0** | workspace, `sim-api`, scheduler/replay, `trace`, GUI shell with board view, MCP shell, CLI | board drawn; MCP lists tools; CI green |
| **P1** | `backend-qemu` (spawn, remote-port in Rust, monitor, serial, ELF load, icount/quantum lock-step) + one RTL backend (Verilator **or** Icarus **or** XSim) + `pl-api` bindings + protocol checkers | prototype regression §14.1 passes with the virtual board, UART console and MCP control |
| **P2** | board devices (RGB LED, Pmod modules, SSM2603 + host audio, HDMI TX/RX pixel models, Pcam sensor), network/SD/QSPI attachment through QEMU, waveform viewer, GDB, DWARF source view | frame displayed, tone heard, OLED text, lwIP ping, FatFs read |
| **P3** | XSA/HWH + `export_simulation` ingestion, UNISIM netlists via the RTL backend, `vendor-ip` library, `BOOT.BIN` boot, custom boards | a Vivado-exported design and its `BOOT.BIN` run end to end; corpus §14.5 table published |
| **P4** | docs, packaging, 3-OS CI, second and third RTL backends, interop `cdylib` | clean-machine install; all §14 tests green or skipped-with-reason |
| **P5 (optional)** | native PS backend (`cpu-a9`, `mem-sys`, `gic`, `ps-periph`) with F1, cache/DMA strictness, snapshots; differential tests vs QEMU | native passes the P1 regression and §14.2–3 |
| **P6 (optional)** | native RTL subset engine, VHDL subset, NEON, Linux boot, WASM GUI | no external tools needed for the supported subset |

Do not start a phase before the previous exit criterion is met and recorded.

---

## 16. Migration of the existing prototype (inventory → Rust)

A working prototype exists (QEMU + Vivado XSim co-simulation). It is reference material;
non-Rust parts are re-implemented in Rust:

| Prototype piece | Today | In this project |
|---|---|---|
| remote-port client (`rp_dpi.c`) | C (DPI) | `interop` (Rust) + Rust `cdylib` DPI surface |
| winsock shim, socket/safeio helpers | C | `std::net` |
| telemetry/event files (`tel_dpi.c`) | C | `trace` + `sim-api` events |
| UI server (`ui/server.py`), page (`index.html`) | Python, HTML/JS | `gui` (egui) over `sim-api` |
| runner (`run_cosim_win.ps1`, `build_app.tcl`) | PowerShell/Tcl | `zynqsim` CLI + `xtask` |
| driver (`drive_board.py`) | Python | Rust integration tests + MCP scenario |
| testbench (`cosim_top.sv`) | SystemVerilog | generated binding from `project.toml` (+ Rust harness) |
| `dma_xform.v`, `gpio_ctrl.v` | Verilog | **kept as RTL inputs** and regression designs |
| firmware (`main.c`, CPU1 app) | C | Rust `no_std` firmware (C only as user input ELFs) |

---

## 17. Working rules for the implementing agent

1. **Read first**: UG585, Zybo Z7 Reference Manual and schematic, AMBA AXI spec, Xilinx
   standalone BSP driver sources, QEMU Zynq machine docs, the remote-port protocol sources,
   the chosen RTL simulator's co-simulation interface.
2. Write `docs/DESIGN.md` and ADRs (§2.3) before large implementation; keep them current.
3. **Test-first** for adapters, peripherals, RTL features and MCP tools.
4. No placeholders in delivered code; `todo!()` only behind a feature flag with a
   `STATUS.md` entry.
5. Prefer the behaviour the Xilinx driver or vendor tool relies on; document it and pin it
   with a test.
6. Report honestly after each phase: `verified` / `implemented-untested` / `stub`, and
   deviations from this prompt.
7. Choose sensible defaults and keep going; ask the user only when a decision changes the
   architecture or the licence situation.

---

## 18. Non-goals

* Gate-level timing (SDF) simulation, analog/electrical behaviour, real USB/HDMI sinks or
  network adapters beyond user-mode/TAP.
* Running encrypted vendor RTL without a library model (it must be *detected*).
* Reproducing Vivado/Vitis flows (importing their outputs is in scope).
* Proving cycle-accuracy of the Cortex-A9 or PS interconnect without a measured reference.

---

## 19. Appendix — facts established by the prototype (reuse, re-verify)

* Xilinx Zynq-7000 co-sim device tree: `M_AXI_GP0` at `0x40000000` (1 GB) = remote-port
  device 7, `M_AXI_GP1` at `0x80000000` = 8, `S_AXI_GP0/1` = 0/1, `S_AXI_HP0–3` = 2–5,
  `S_AXI_ACP` = 6, PL→PS wires = 9 (16 lines → GIC SPI 29–36 and 52–59, i.e. IDs 61–68
  and 84–91), PS→PL wires = 10 (17 resets), PL→PS interrupt = 11.
* That tree has **no `chrdev-id`**: give the remote-port device its chardev with
  `-global remote-port.chardev=<id>` and `-chardev socket,id=<id>,...,wait=on`; Windows QEMU
  has **no unix sockets** — use TCP.
* No FSBL ⇒ bare-metal code must set SLCR `LVL_SHFTR_EN` (`0xF8000900 = 0xF`) and
  `FPGA_RST_CTRL` (`0xF8000240 = 0`) after unlocking SLCR (`0xF8000008 = 0xDF0D`).
* Determinism of a PS/PL co-simulation required: `-icount shift=N,align=off,sleep=off`;
  time-locked `sync` (answer each sync only after the PL reached that time); PS access time
  stamps honoured and PL-side latency reported back; **acknowledged (non-posted) interrupt
  updates** (`CAP_WIRE_POSTED_UPDATES`); **blocking, zero-simulated-time** PL→DDR response
  waits with a fixed modelled latency; end the run from the firmware with a marker write,
  not from wall-clock timing. Without these, traces diverged between runs.
* `pmemsave` through the monitor needs a unique file per request on Windows.
* Firmware: never wait with a counted spin loop (WFI + interrupt); the simulator can be
  slower than the CPU model.
* Windows/XSim DPI quirks (relevant only to the `xsim` backend): link the DPI code as a
  static `dpi.a`, avoid static `ws2_32` imports in the DPI objects (resolve winsock at run
  time), do not pass link options to `xsc`.
* The Zybo Z7 has **no VGA connector** (HDMI sink and source only).
* Reference results: 64×64 word-per-pixel image, `out = clamp(in*scale+offset)`, 16-beat
  INCR bursts never crossing 4 KB: 256 read + 256 write bursts per frame, ≈ 18 000 PL
  clocks at 100 MHz with a 16-clock modelled DDR latency; golden-model mismatches 0/4096.
