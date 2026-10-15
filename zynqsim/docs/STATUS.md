# ZynqSim feature status

Status values (prompt §1): **verified** (a named test proves it), **implemented-untested**,
**stub** (present but returns a structured `unsupported` error), **not-planned**.
"Verified" never means "matches hardware": nothing here has been compared with a board.

Current milestone: **P1 reached locally** (see the exit criterion below). P0 is done. Parts of
P2 (board devices) exist: the audio codec and HDMI connectors as pin-level models in the RTL
testbench, with host audio/video, and a 3D board view. The remaining P2 devices are still drawn
only.

* `demo` engine (fidelity `demo`): built-in scripted model, no CPU, no RTL. Control-plane and
  front-end test double.
* `qemu` PS engine (fidelity **F0**: functional, instruction-count timing) with either **native
  Rust PL models** (transaction level) or **RTL in Vivado XSim** (RTL-exact at its ports,
  quantum-granular coupling to the PS).

**P1 exit criterion** (prompt §15: "prototype regression §14.1 passes with the virtual board, UART
console and MCP control"): the prototype DMA regression (two Cortex-A9 cores, RTL DMA engine, IRQ)
passes, its results equal a Rust golden model, and 5 runs give one trace digest (CLI, MCP and the
control plane drive it). Caveats are listed under *Known limits of the P1 engines*.

## Kernel and control plane

| Feature | Status | Evidence / note |
|---|---|---|
| Picosecond time base, deterministic `(time, priority, id)` scheduler | verified | `sim-core::sched::tests::*` |
| Seeded, snapshot-able PRNG | verified | `sim-core::prng::tests::*` |
| Clock domains (period, phase, enable, edge counting) | verified | `sim-core::clock::tests::*`. SLCR-driven FCLK changes: not yet |
| Typed commands / queries / event stream, threaded client, any `Engine` | verified | `sim-api::server::tests::*` |
| Input recording and deterministic replay | verified | demo: `sim-api::sim::tests::replay_reproduces_a_session_exactly`. QEMU: journal of every state-changing request with its run targets, `sim-api::qemu_journal::a_recorded_session_replays_to_the_same_digest`, `zynqsim::qemu_sessions_record_and_replay_and_detect_tampering`, RTL board demo replayed by hand |
| Snapshot / restore / reset on QEMU engines | verified, **by re-execution** | `sim-api::qemu_journal::snapshot_restore_and_reset_reproduce_the_session`, `mcp::qemu_project_through_mcp`. A restore starts a fresh QEMU (and XSim, from the build cache) and re-applies the journal up to the snapshot; the result must reproduce the snapshot's trace digest, time and record count or the restore is refused. Cost = the simulated time so far, not a memory copy |
| Continuous run (`Play`/`Pause`), engine faults stop it and are reported | verified | `sim-api::server::tests::play_advances_time_and_pause_stops_it` |
| `project.toml` parse + validation incl. `[native]`, `[rtl]`, `[host]` | verified | `sim-api::project::tests::*`, CLI/MCP tests |
| Backend inventory; tool detection on PATH and Xilinx install dirs (binaries, never `.bat` wrappers, for QEMU) | verified | `sim-api::tools::tests::*` |

## Remote-port, QEMU, PL engines

| Feature | Status | Evidence / note |
|---|---|---|
| Remote-port protocol 4.3 codec (hello, busaccess ext/old, interrupt, sync) | verified | `interop::packet::tests::*`: byte-exact against the C reference encoder; fuzzed decode never panics |
| TCP session with capability negotiation | verified | `interop::session::tests::*` |
| QEMU adapter: spawn, ELF0/ELF1 load, lock-step by holding syncs, `-icount`, GP0/GP1 bus, PS-to-PL wires, acknowledged PL-to-PS IRQs | verified | `backend-qemu` e2e tests with the real Xilinx QEMU: firmware passes, two runs identical in UART, PL access times/data and finish time |
| UART1 `file` mode (deterministic output) / `tcp` mode (input, best-effort timing) | verified | `backend-qemu::tests::e2e::*` |
| Native PL models (`my_math_ip`) with lanes/strobes, reset wire | verified | `pl-native::*::tests`, CLI/MCP QEMU tests. **Transaction level, no cycle timing** |
| XSim backend: build (xvlog/xelab), run, lock-step, PS access, DMA master, IRQ forwarding | verified | `backend-rtl-xsim::tests::math::*`, `dma_golden` against real XSim |
| `xsim-dpi`: `no_std` Rust static library XSim links (`<name>.a`), Winsock bound at run time, prototype `rp_*` ABI | verified | `xsim-dpi::core::tests::*` (protocol, host) and the XSim runs above (link + run) |
| Board pins: switches/buttons into the RTL (`tel_ctrl`), LED pins out of it (`tel_event`), prototype testbench unchanged | verified | `backend-rtl-xsim::tests::board::shell_leds_switches_and_buttons`, `zynqsim::board_demo_shell_leds_switches_and_button_interrupt`, `mcp::board_demo_through_mcp` |
| Interactive UART shell on the Cortex-A9 (`help`, `status`, `led`, `img`, `run`, `bright`, `contrast`, `end`) | verified | same tests; `[host] uart1 = "tcp"` |
| Button -> GPIO event -> `IRQ_F2P[1]` -> ISR -> LED toggle | verified | `board::shell_leds_switches_and_buttons`. The handler can run twice for one press (level IRQ, PL falling edge arrives one quantum later); the test accepts 1 or 2 |
| DMA image filters equal a Rust golden model pixel for pixel (3 filters, 3 patterns, 4096 px) | verified | `board::image_filters_match_the_golden_model_pixel_for_pixel`, `zynqsim::board_demo_image_filter_matches_golden_through_the_cli` (via `--dump-mem`) |
| Guest memory read **and write** while paused, through the remote-port memory path (not the QEMU monitor) | verified | `backend-qemu::memory_is_readable_and_writable_while_the_machine_is_paused`, MCP `mem_read`/`mem_write`, CLI `--dump-mem`/`--dump-image` |
| `Play`/`Pause` on a QEMU engine, queries stay responsive | verified | `sim-api::tests::qemu_play` |
| Prototype DMA regression: 6 cases PASS, 2 cores, IRQ | verified | `zynqsim::dma_regression_passes_and_is_deterministic_across_processes` |
| DMA results equal a Rust golden model (4,337 words) | verified | `backend-rtl-xsim::dma_golden`; mutation-checked (a deliberately wrong model fails) |
| 5 identical trace digests of the DMA regression | verified | run manually 5x (`fnv1a64:4137e81608225a53`, 3.000836 ms, 687 bus accesses); CLI test runs 3 processes |
| Verilator, Icarus, native RTL engine, Questa DPI cdylib | stub | not built; `backends.pl` rejects them with a precise message |
| XSim build cache: a design with the same sources, defines, top, DPI library and Vivado is not compiled again (cached start ~4.6 s vs ~11 s for the math design) | verified | `backend-rtl-xsim::math::compiled_snapshots_are_cached_by_content` |
| `-d NAME=VALUE` defines reach xvlog (quoted for `cmd`) | verified | same test (a define with a value builds) |
| A request stamped after the run target is held, not served: a firmware that polls a PL register cannot make one `run` call overshoot its target | verified | found by the audio demo (a poll loop ran 9.4 ms of virtual time inside a 0.6 ms call); `backend-qemu::e2e` and `sim-api::qemu_play` still pass; `zynqsim::audio_demo_*` |

### Known limits of the P1 engines

* **Fidelity is F0.** No caches, no DDR timing, no non-coherent-DMA strictness. QEMU is functional.
* **XSim is Windows-only and needs Vivado**; the DPI library must be built (`cargo xtask dpi`, into
  `target/dpi`: a plain workspace build compiles the same crate without exports and would
  replace it, which the adapter detects and reports).
  Tests that need it print `SKIPPED:` and pass vacuously when it is absent, so check the output.
* **The RTL testbench top is user-supplied**, except that `zynqsim init --ip` generates one for an
  AXI4-Lite slave. Generation from `[connect]`, `[connect]` for RTL ports, XSA/HWH ingestion and
  globs in `rtl.files` are not implemented (milestone P3).
* **QEMU sessions cannot** change the quantum, and cannot read registers *at an arbitrary
  time*. A machine that is merely paused sits in QEMU's sync wait holding the global lock, so
  neither the monitor nor the gdb stub can answer (measured: the `-gdb` port accepts the TCP
  connection and never answers). What works is a stop caused by the guest itself: with
  `[debug] enabled = true` QEMU starts under its gdb stub, a breakpoint or watchpoint stops the
  vCPUs, the lock is released and registers can be read (see *Debugger*). Registers at an
  arbitrary simulated time would need a change inside QEMU's remote-port device or the native PS
  backend (P5). (Memory works because it uses the remote-port path.)
* **Snapshots and replays re-execute**, so they cost wall time proportional to the simulated time
  so far (XSim runs at 0.3-0.6 s per simulated millisecond). They are exact only when the session
  is deterministic: interactive (TCP) UART *input* is not, and then a restore is refused with the
  digest mismatch. The journal is bounded (2 million requests or 64 MiB of inputs); beyond that
  snapshots and recording report why. A QEMU recording needs the `project.toml` it came from.
* **Board pins** (switches, buttons, LEDs) are wired only when the PL is an RTL design whose
  testbench uses `tel_ctrl` / `tel_event` (the prototype's convention). With native PL models or no
  PL they are refused with an explanation. Only 4 LEDs / 4 switches / 4 buttons are mapped; RGB
  LEDs and Pmod pins are not driven.
* UART input needs `[host] uart1 = "tcp"`, whose delivery time is not tied to virtual time, and
  after each run the engine waits briefly (tens of ms) for in-flight output to settle. Scripted
  shell sessions are therefore reproducible in outcome, not guaranteed identical in trace timing.
* The prototype README's "about 25 s per simulated ms" does not apply: this stack ran the board
  demo at roughly 1 s of wall time per simulated millisecond with RTL in XSim, mostly one sync
  round-trip per 1 us quantum.
* UART bytes have no per-byte timestamp (file chardev): they are attributed to the 100 us chunk in
  which they were observed. The trace is identical for identical run-request sequences, not for
  arbitrary re-slicing of the same total time.
* PL-to-PS interrupt and DMA timing is exact only at quantum granularity (1 us default).
* GIC SPI mapping for `IRQ_F2P` is whatever QEMU's co-sim tree does; not independently checked.
* Only `M_AXI_GP0/1` (PS to PL) and `S_AXI_*` (PL to PS) bus paths are forwarded; ACP/cache
  coherency is not modelled.

## Trace

| Feature | Status | Evidence / note |
|---|---|---|
| Event log with canonical FNV-1a-64 digest, `Bus` events for PS and PL-master accesses | verified | `trace::log::tests::*` |
| VCD export (signals and UART bytes) | verified | `trace::export::tests::*` (not opened in a third-party viewer). Bus events are not in the VCD |
| Perfetto (Chrome trace-event JSON) export | verified | `trace::export::tests::perfetto_is_valid_json_with_all_events` |
| Bounded trace for QEMU engines: the newest 1,000,000 records are kept, the digest and record count cover all of them, `trace_get`/events use absolute indices | verified | `trace::log::tests::a_bounded_log_keeps_the_digest_and_absolute_indices`. VCD/Perfetto exports of a long QEMU run cover only the retained tail (`trace_dropped` in `run --json` says how many were dropped) |
| Waveforms from XSim (`.wdb`) | verified by hand | `[rtl] waves = true` records `xsim/zs_snap.wdb` in the run directory (`%TEMP%\zynqsimun-*`), which is **kept** (also with `ZYNQSIM_KEEP_RUN=1`; the path is printed on stderr); XSim is ended cleanly so the database is closed. Run on the board demo: 11 MB file written. No automated test, and the file was not opened in the Vivado viewer |
| FST export | not-planned | VCD is sufficient for now |
| AXI protocol checkers, latency histograms | stub | not implemented |

## Debugger (`[debug] enabled = true`)

Needs the Xilinx QEMU. The CPUs run under QEMU's gdb stub: `-gdb` and `-S`, a debugger client in
`backend-qemu::gdb`, breakpoints planted before the first instruction.

| Feature | Status | Evidence / note |
|---|---|---|
| Execution breakpoints (`[debug] breakpoints = ["main", "0x100614"]`, `breakpoint_set`), run stops there, next run continues (steps off the breakpoint first) | verified | `backend-qemu::e2e::a_breakpoint_stops_the_guest_and_its_registers_are_exact`, `sim-api::qemu_debug::*`, `mcp::debugger_through_mcp` |
| `cpu_regs`: r0-r15, CPSR, mode, flags, pc symbol, while stopped; refused (with the reason) while not stopped | verified | same tests; `gui::model::registers_appear_in_the_model_when_the_guest_stops` |
| Guest state at every stop is reproducible between sessions (4 consecutive stops, 2 sessions: identical registers) | verified | `backend-qemu::e2e::registers_are_the_same_in_every_debug_session` |
| `watchpoint_set` (write/read/access), `breakpoint_list`, `symbol_lookup` (ELF symbols, both directions) | verified for write and read watchpoints on PL registers | `sim-api::qemu_debug::a_write_watchpoint_stops_the_guest_on_the_access` (stops inside `Xil_Out32`, reason `watchpoint at 0x43c00000`) and `a_read_watchpoint_stops_the_guest_on_the_load` (inside `Xil_In32`). Access watchpoints (`Z4`) use the same path but no test makes one fire; RAM watchpoints were not tried |
| `Play` stops at a breakpoint and reports why instead of running past it | verified | `sim-api::qemu_debug::continuous_running_stops_at_a_breakpoint_instead_of_running_past_it` |
| ELF symbol reader (ELF32 LE `.symtab`, no DWARF) | verified | `sim-api::elf::tests::*` on the C and the Rust firmware |
| Core 1 stops and registers (`cpu_regs core=1`) | verified | `sim-api::qemu_debug::core_one_stops_at_its_own_breakpoint`: the DMA regression's second core stops at its `main` (0x040005e4), core 0 is readable at the same time. Breakpoint addresses are global, so a symbol of core 0's ELF resolves only core 0's code (core 1 needs a numeric address) |
| `cpu_disasm`: ARM and Thumb/Thumb-2 from guest memory, symbol-annotated, works on a paused machine | verified | `sim-api::disasm::tests::*` (encodings, sizes, a real function without undecodable lines), `mcp::debugger_through_mcp`. Pure Rust (`yaxpeax-arm`, 0BSD). `thumb` defaults to the CPSR T bit of a stopped core 0, else ARM; the decoder is permissive and is not compared with an independent disassembler (no objdump on this machine) |
| Single-step, reverse debugging, DWARF source lines | stub / not-planned | the stub supports `s`, which resume uses internally; there is no step command or line table |

Limits that matter:

* **Timing under the debugger is not reproducible.** Attaching shifts virtual time by up to a
  few microseconds and by a few hundred nanoseconds between two sessions (measured: first PL
  access at 13.9 vs 14.9 us, finish 114.98 vs 115.16 us). The guest's *state* at each stop is
  identical, the trace digest is not: snapshots and replays of a debug session may be refused
  for a digest mismatch.
* Breakpoints set while the machine is only paused are queued in the stub and take effect
  within the next quantum at a wall-clock-dependent instant; the project's `[debug] breakpoints`
  are exact.
* A stop is only observed when no remote-port traffic arrives for 100 ms of wall time, so a stop
  is reported with up to that much latency.

## Audio, video and the project loader

Everything here needs the Xilinx QEMU, Vivado XSim and a design that instantiates the media models
(`fixtures/rtl/media/zs_media.sv`). Without them the tools return `unsupported` naming what is
missing. See ADR-006.

| Feature | Status | Evidence / note |
|---|---|---|
| SSM2603 pin model: I2C register writes (0x1A), I2S DAC receive, DAC mute and headphone volume applied, I2S ADC transmit from a host source | verified | `backend-rtl-xsim::media::codec_receives_a_tone_obeys_registers_and_feeds_the_adc` (1 kHz recovered within 25 Hz, -6 dB halves the peak, DACMU gives zeros, 440 Hz loopback) |
| HDMI TX sink: frame-buffer scan-out reaches the host as RGB frames, pixel-exact | verified | `media::framebuffer_scans_out_and_hdmi_input_is_captured` |
| HDMI RX source: a host image is captured into DDR through the PL, pixel-exact | verified | same test |
| Host sources: colour bars, gradient, PNG (`--video-in`), silence, tone, WAV resampled to 48 kHz (`--audio-in`) | verified | `pl-api::media::tests::*`, `sim-api::project` parsers, CLI/MCP tests below |
| Rust bare-metal firmware: I2C codec bring-up, tone, ADC loopback, frame-buffer drawing, scan-out and capture (`firmware/audio_demo`, `firmware/hdmi_demo`) | verified | `zynqsim::audio_demo_plays_a_tone_and_loops_line_in_back`, `zynqsim::hdmi_demo_scans_out_a_frame_and_captures_the_host_image` (WAV frequency and peak; picture and capture checksum against Rust models) |
| CLI `--video-in/--audio-in/--video-out/--audio-out`, `[media]` in `project.toml` | verified | same CLI tests |
| MCP `video_frame` (PNG image content), `audio_capture` (WAV audio content, peak/RMS/frequency), `media_source_set` (path-confined) | verified | `mcp::scenario::audio_and_hdmi_demos_through_mcp` |
| Polling firmware paced with `rt::delay_us` | verified | needed: back-to-back register polls advance simulated time by ~10 ns per ~100 us of wall time |
| GUI: project window (PS ELF cpu0/cpu1, PL RTL files and top, UART mode, media, quantum), *Load PS firmware…* / *Load PL design…*, *Open project…*, *Save project.toml…*, background build with banner | verified (model) / implemented-untested (window) | `gui::loader::tests::*`, `gui::model::tests::a_project_loads_in_the_background_and_its_audio_reaches_the_model`; window inspected by screenshot only |
| GUI: HDMI TX picture, RX source picker, audio level, LINE IN source picker | verified (model) / implemented-untested (panels) | `gui::model::tests::hdmi_frames_reach_the_model_as_rgba`; panels inspected by screenshot |
| GUI: speaker output (cpal), 48 kHz queue with linear resampling and a 0.5 s latency cap | verified up to the device; **audibility not verified** | `gui::audio_out::tests::*`, including `the_sound_device_consumes_frames_in_real_time` (opens the default output device, 0.25 s of frames drain in 0.15-2 s, nothing dropped; skips without a device, so hosted CI skips it). Whether a person hears the right sound has not been checked |
| Scratch directories live in the system temp directory (`ZYNQSIM_WORK_DIR`), stale ones are removed after a day | verified | examples no longer grow a `.zynqsim/` folder |

### Known limits of the media models

* The codec model has a write-only I2C path, 16-bit I2S only, one sample rate (48 kHz); ADC gain
  and mute, the digital audio path and sample-rate registers are stored but have no effect. Volume
  uses 1 dB per step from 0x79 and mutes below 0x30: a reading of the datasheet, **not checked
  against hardware** (`VERIFY:`).
* The HDMI models carry pixels only: no EDID/DDC, no hot-plug, no HDMI audio, no deep colour. The
  example PL uses a fixed 320x240 raster (25 MHz pixel clock derived from the 100 MHz clock).
  The host side accepts up to 4096x4096.
* Audio is delivered in 64-frame blocks; the last partial block is lost at the end of a run, and
  10 s is retained for capture.
* Listening is best effort: below real-time speed the speaker has gaps, above it old audio is
  dropped. XSim runs at roughly 0.3-0.6 s of wall time per simulated millisecond with the media
  design, so real time is not reachable.
* The media DPI/remote-port device numbers (14-18) are this project's convention.

## Board (`board-zybo`)

| Feature | Status | Evidence / note |
|---|---|---|
| Board description (TOML), validation with every problem listed | verified | `board-zybo::def::tests::*` |
| **Pin assignments** | implemented-untested | `VERIFY:` transcribed from memory of Zybo-Z7-Master.xdc; not checked against the reference manual or schematic. `pin_status = "unverified"` |
| **Pmod pin locations, port count, XADC/MIO sharing** | stub | `VERIFY:` pins intentionally absent; 6 ports (JA-JE PL, JF PS) assumed |
| Software renderer, golden PNGs, hit-testing | verified | `board-zybo::render::tests::*` (layout is schematic, not to scale) |
| **3D board view**: layout extruded to boxes, orbit camera, perspective, lit LEDs glow, switch levers and button plungers move, picking by drawn face | verified (scene) / implemented-untested (mouse handling) | `board-zybo::scene3d::tests::*` (deterministic, camera changes pixels, every switch/button/LED is found where it is drawn at three cameras, state changes pixels). Heights are illustrative, not measurements |
| Board SVG import, custom boards | not-planned / stub | layout is TOML-driven; only `zybo-z7-20` is selectable |
| SSM2603 and HDMI RX/TX | verified at pin level in the testbench | see *Audio, video and the project loader*. Not a model of the board's wiring: the testbench connects them |
| Pcam, PHY, SD, QSPI, Pmod modules | stub | drawn as inert parts; milestone P2 |

## Demo engine

| Feature | Status | Evidence / note |
|---|---|---|
| Switch -> LED0-2, heartbeat LED3 (seeded), RGB PWM, button -> UART1 text, UART echo, 115200 8N1 timing | verified | `sim-api::sim::tests::*` |

## Front ends

| Feature | Status | Evidence / note |
|---|---|---|
| MCP server over stdio (tools, resources, prompts, schemas, validation) | verified | `zynqsim-mcp` tests |
| MCP: path allow-list, no command execution, bounded output, progress, cancellation | verified | `zynqsim-mcp` tests |
| MCP: QEMU + PL projects (open, run, UART, trace with bus events, gating) | verified | `mcp::tests::scenario::{qemu_project_through_mcp, dma_project_through_mcp}` |
| MCP streamable HTTP transport | stub | stdio only |
| MCP `mem_read`, `mem_write` on a QEMU engine; board tools, `uart_*` with an RTL board design | verified | `mcp::board_demo_through_mcp` |
| MCP `cpu_regs`, `breakpoint_set`, `watchpoint_set`, `breakpoint_list`, `symbol_lookup` | verified for QEMU projects with `[debug]` | `mcp::scenario::debugger_through_mcp`; without `[debug]` they say how to enable it |
| MCP `cpu_disasm`, `symbol_lookup` | verified for QEMU projects (no `[debug]` needed) | `mcp::scenario::debugger_through_mcp` |
| MCP tools needing PL signals/network | stub | `unsupported` naming the missing capability |
| `zynqsim init --ip FILE`: testbench + `project.toml` generated from the module header of an AXI4-Lite slave IP (32-bit data, any prefix/case, active-low or high reset, extra clocks tied to FCLK0, other inputs tied low) | verified | `sim-api::tbgen::tests::*` (header parsing incl. attributes/comments/parameters/non-ANSI, detection, rejections with reasons), `zynqsim::init_ip_generates_a_project_that_passes_with_the_real_firmware` (generated for `my_math_ip.v`: the real firmware passes with the same 21 bus accesses and 329 trace records as the native model). The parser reads module headers only, it is not a Verilog parser; IPs with AXI masters, interrupts or other bus flavours are refused (use a hand-written testbench). Interrupt outputs and AXI4 full are not generated |
| `project_create_from_xsa`, `pmod_attach` | stub | milestones P3 / P2 |
| CLI: `run` (any engine, `--at` inputs incl. `uartN:`/`swN:`/`btnN:`, `--dump-mem`, `--dump-image`), `inspect`, `init`, `trace`, `test` (exit codes 0/1/2/3, `--json`) | verified | `crates/cli/tests/cli.rs` |
| CLI: `mcp`, `gui` launchers | implemented-untested | not covered by a test |
| CLI: `ctl` | stub | needs an IPC listener |
| GUI behaviour layer (`Model`) incl. engine faults | verified | `zynqsim-gui::model::tests::*`, `app::tests::*` |
| GUI window incl. Memory / image viewer | verified (input handling) / implemented-untested (appearance) | `zynqsim-gui::app::tests`: the whole UI is driven frame by frame with synthetic egui input and no window: 2D click toggles a switch, a button is held while the pointer is down, keyboard shortcuts, 3D click picks the switch, drag orbits, wheel zooms, the project window builds and swaps the simulation and refuses invalid drafts. Appearance is still judged only from `--screenshot` images; native file dialogs cannot be driven by tests |
| GUI start-up flags for automated checks (`--uart`, `--mem`, `--toggle`, `--run-ms`, `--screenshot`) | implemented-untested | used by hand |
| GUI: 2D/3D board tabs, PL bus-access list | verified (interaction) | `app::tests` (above) and screenshots |
| GUI: dockable panels, CPU/PL signal/network/timeline panels, waveform/DWARF views | stub | fixed layout; panels that need a backend say so |

## Backends

| Backend | Status |
|---|---|
| `demo` (PS and PL) | verified, fidelity `demo` |
| `qemu` (PS) | verified, F0 |
| `native` (PL) | verified, transaction level |
| `xsim` (PL) | verified, Windows + Vivado (build cache) |
| `verilator`, `icarus` (PL) | stub, P4 |
| `native` PS / RTL engine | stub, P5 / P6 (optional) |

## Build, tooling and process

| Item | Status | Note |
|---|---|---|
| `cargo fmt --check`, `clippy --all-targets -D warnings`, `cargo test --workspace` | verified locally | Windows, Rust stable GNU toolchain (MinGW from Qt). Harmless `.drectve` linker warning |
| `cargo xtask ci` (fmt, clippy, freestanding DPI clippy + build, tests, release build) | verified locally | exited 0 on 2026-10-15 (after the debugger), 202 tests, 0 skipped, QEMU and XSim tests included |
| `cargo xtask dpi`, `third-party`, `golden`, `release` | verified | each run |
| `THIRD_PARTY.md` | verified | generated from `Cargo.lock`; licences as declared, not audited. Re-generate after dependency changes |
| Linux and macOS | verified (build, lints, tests without Xilinx tools) | GitHub Actions run 38731290203 on `ubuntu-latest`, `windows-latest` and `macos-latest`: fmt, clippy `-D warnings` (incl. the freestanding DPI crate), `cargo test --workspace --no-fail-fast` and the release build all pass. Hosted runners have no QEMU or Vivado, so every test that needs them **skips**; those are verified only on the Windows development machine. The first hosted run found three real problems (workflow in the wrong directory, `freestanding` feature broken off Windows, a CRLF-dependent test) and the Linux WSL run earlier found an ordering bug in the XSim adapter |
| `forbid(unsafe_code)` workspace lint | verified | `xsim-dpi` is the one exemption (FFI boundary, `win.rs` only) |
| Building and testing without QEMU/Vivado installed | implemented-untested | tests that need them skip with a message; not run on a machine without them |
| Git repository | not-planned for now | the directory is not under version control |

## Deviations from the prompt (so far)

* `boards/*.svg` is not used; the board is drawn from the TOML layout.
* There is no `ps-backend` trait crate: the engine abstraction is `sim_api::Engine`, and
  `backend-qemu` is a concrete adapter. The `PsBackend` trait is extracted when a second PS
  engine exists (P5).
* The RTL adapter's simulator-side library is a static archive (`xsim-dpi`), not the prompt's
  `cdylib`, because XSim on Windows links DPI statically (ADR-002). A DPI `cdylib` for Questa /
  Verilator remains future work.
* The workspace sets `panic = "abort"` for `dev` and `release` because the freestanding DPI
  library cannot unwind. Tests still unwind (cargo ignores the setting for them).
* Dockable GUI panels (egui_dock) are not used; the layout is fixed.
* `cargo fuzz` targets do not exist; a seeded property test covers the MCP surface and the codec
  decoder is fuzzed with random bytes.
* The workspace lives in `zynqsim/` inside the prototype directory; the prototype files are
  untouched (prompt §16 migration is partly done: remote-port client, runner and driver logic now
  exist in Rust; the web UI and PowerShell scripts are not yet retired).
