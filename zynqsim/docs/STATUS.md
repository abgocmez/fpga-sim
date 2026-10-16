# ZynqSim feature status

Status values (prompt §1): **verified** (a named test proves it), **implemented-untested**,
**stub** (present but returns a structured `unsupported` error), **not-planned**.
"Verified" never means "matches hardware": nothing here has been compared with a board.

Current milestone: **P1 reached locally** (see the exit criterion below). P0 is done. Parts of
P2 (board devices) exist: the audio codec and HDMI connectors as pin-level models in the RTL
testbench, with host audio/video, a 3D board view, and the PS Ethernet (GEM0) on QEMU's
user-mode network with a Rust ping firmware and `.pcap` export (see *Ethernet*). The remaining P2 devices are still drawn
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
| PS I2C, SPI and XADC conformance (`firmware/conf_*`): I2C and XADC partly modelled, SPI not modelled by default (mapped with `[ps] extra_blocks = true`, see ADR-024) | verified (partly / not modelled) | `conformance_iic_spi_xadc::iic_spi_xadc_firmware_passes_with_the_listed_na_checks`; opt-in: `conformance_ps_blocks::spi_mapped_model_passes_most_checks_and_fails_three`; table in `docs/conformance/iic-spi-xadc.md` |
| PS GPIO, SPI0/1, devcfg: mapped by renaming their compatible strings in the co-sim tree (`[ps] extra_blocks = true`, default off) | verified (opt-in; findings recorded) | `conformance_ps_blocks` (GPIO: 9 PASS, 2 FAIL, 1 N/A; SPI: 20 PASS, 3 FAIL, 4 N/A); `dtb::the_ps_block_edits_are_applied_to_the_real_cosim_tree`; devcfg not mapped (`xlnx,ps7-dev-cfg` guess fails its selftest). ADR-024 |
| A9 timers' rate (333 MHz CPU clock / 2 vs measured 100 MHz) | not fixed | ADR-024: the rate comes from the SLCR clock tree (clkc index 4); no DTB property or QEMU option was found that changes it |
| `Play`/`Pause` on a QEMU engine, queries stay responsive | verified | `sim-api::tests::qemu_play` |
| Prototype DMA regression: 6 cases PASS, 2 cores, IRQ | verified | `zynqsim::dma_regression_passes_and_is_deterministic_across_processes` |
| DMA results equal a Rust golden model (4,337 words) | verified | `backend-rtl-xsim::dma_golden`; mutation-checked (a deliberately wrong model fails) |
| Every example, 5 separate processes each: trace hash, end time and UART text identical (§14.6). 26 examples run (25 identical; `board-demo` is known non-deterministic, see FINDING below); `hdmi-demo` not run | verified for 25 examples; board-demo **not** deterministic | `zynqsim::every_example_runs_five_times_with_identical_trace_hashes` (`crates/cli/tests/determinism.rs`). FINDING: `board-demo` drives its shell over UART1 TCP (`[host] uart1 = "tcp"`), best-effort timing: 3 distinct hashes in 5 runs in one full run of the test and 5 in 5 in another, end times 15.000116..15.000652 ms, the firmware's PL tick count differs (300145..328234). Not fixed; the scripted inputs are not reproducible on this path. `rust-debug` and `math-debug` stop at their first breakpoint, so they compare the stop state only (rust-debug at 0 ps); math-debug was identical in 5 runs although its project header warns that debugger timing can vary. `hdmi-demo` needs a host video image and is covered by `zynqsim::hdmi_demo_scans_out_a_frame_and_captures_the_host_image` |
| `zynqsim bench --project P --for T`: wall time, simulated time, sim/wall ratio, PL cycles per wall second (XSim, nominal 100 MHz), approximate guest instruction rate (QEMU) | verified for the numbers in `docs/PERFORMANCE.md` (single runs, 2026-10-16); no test asserts them | `docs/PERFORMANCE.md` |
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
  AXI4-Lite slave, or `init --xsa` from a Vivado handoff (see *XSA / HWH ingestion*). Generation
  from `[connect]` and `[connect]` for RTL ports are not implemented (milestone P3). Globs in
  `rtl.files` and Vivado simulation exports are (see *Vivado simulation exports and netlists*).
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
| Waveform queries: signal catalogue (name, width, change count, times, derived flag) and value changes of chosen signals in `[from_ps, to_ps]` with the value in effect at the start; stable time ordering; `prefix*` names; bounded (per-signal limit with `truncated` and `covered_to_ps`, 128 signals); `bus.<port>.addr/data/write` derived from bus accesses (not in the VCD); both engines | verified | `trace::waveform::tests::*` (windowing incl. inclusive bounds and initial value, truncation, ordering, wildcards, unknown names, dropped records), `sim-api::wave::tests::*` (demo engine through `Query`) |
| Waveform picture (one scene drawn by the GUI, as SVG and as PNG; no diagonals, sub-pixel changes folded) | verified (geometry and content) / implemented-untested (colours, font) | `trace::wave_scene::tests::*`; a PNG was viewed once by hand |
| MCP `waveform_get`: catalogue, window, truncation, errors, SVG/PNG render and `path` inside the allowed roots | verified | `mcp::tests::waveform::{waveform_get_on_the_demo_engine, waveform_get_renders_svg_and_png, waveform_get_shows_the_bus_of_a_qemu_project}` (the last needs Xilinx QEMU and ran here: the 21 PL accesses of `math-qemu` are 21 samples of `bus.M_AXI_GP0.addr`) |
| PL-internal nets, IRQ lines and AXI handshakes as waveforms | not-planned | only signals recorded at the boundary exist (ADR-014); the XSim `.wdb` below covers internals |
| Waveforms from XSim (`.wdb`) | verified by hand | `[rtl] waves = true` records `xsim/zs_snap.wdb` in the run directory (`%TEMP%\zynqsim
un-*`), which is **kept** (also with `ZYNQSIM_KEEP_RUN=1`; the path is printed on stderr); XSim is ended cleanly so the database is closed. Run on the board demo: 11 MB file written. No automated test, and the file was not opened in the Vivado viewer |
| FST export | not-planned | VCD is sufficient for now |
| AXI protocol checkers in the `init --ip` testbenches (ADR-025): VALID/READY and payload stability per channel, VALID high in reset, INCR 4 KB crossing, WLAST/RLAST, WRAP alignment and length, AxSIZE vs bus width, SLVERR/DECERR as info. `[rtl] protocol_checkers` = true (warnings, default), false (compiled out), `"strict"` (errors, `zynqsim run` exits 1) | verified: the legal math, DMA, IRQ and PMOD OLED designs report nothing; five deliberately illegal fixtures (`fixtures/rtl/illegal/`) report their rule at their time; strict fails the run; off is silent. Real XSim. **Limits:** hand-written testbenches have no checkers; a W beat before its AW turns WLAST checking off for the port; unaligned INCR starts are not reported | `zynqsim::protocol_checks::*` (9 tests), `sim-api::tbgen::protocol_checkers_*`, `sim-api::project::protocol_checkers_*`, `pl-api::protocol::tests::*`, `xsim-dpi::core::tests::protocol_*` |
| AXI latency histograms | stub | not implemented |

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
| ELF symbol reader (ELF32 LE `.symtab`; DWARF lines are a separate reader, below) | verified | `sim-api::elf::tests::*` on the C and the Rust firmware |
| Core 1 stops and registers (`cpu_regs core=1`) | verified | `sim-api::qemu_debug::core_one_stops_at_its_own_breakpoint`: the DMA regression's second core stops at its `main` (0x040005e4), core 0 is readable at the same time. Breakpoint addresses are global, so a symbol of core 0's ELF resolves only core 0's code (core 1 needs a numeric address) |
| `cpu_disasm`: ARM and Thumb/Thumb-2 from guest memory, symbol-annotated, works on a paused machine | verified | `sim-api::disasm::tests::*` (encodings, sizes, a real function without undecodable lines), `mcp::debugger_through_mcp`. Pure Rust (`yaxpeax-arm`, 0BSD). `thumb` defaults to the CPSR T bit of a stopped core 0, else ARM; the decoder is permissive and is not compared with an independent disassembler (no objdump on this machine) |
| Single-step, reverse debugging | stub / not-planned | the stub supports `s`, which resume uses internally; there is no step command |
| DWARF line table: address to `file:line:column` and function, and `file:line` to addresses (`.debug_line` + subprogram DIEs via `gimli`/`object`; ELF32 LE, DWARF 4/5) | verified | `sim-api::source::tests::*`: a hand-built ELF (adjacent sequences, discarded address-0 sequences, loop blocks, ambiguous names, case/separator-insensitive suffix match, file lookup limited to line-table files) and the real `rust_debug.elf` (every `MARK:` line of `firmware/debug_demo/src/main.rs` maps both ways, function names match). An ELF without debug info gives an empty map and a clear error, never a failure to start |
| `Query::SourceLine` / `SourceFile` / `SourceAddrs`, `cpu_regs.pc_source`, stop reason and `breakpoint_list` carry the line | verified | `sim-api::qemu_source::a_stop_is_reported_as_a_line_of_the_rust_source` (real QEMU: stop at `main.rs:LINE`, `pc_source`, function `collatz_steps`, file text found through `[debug] source_dirs`, second stop on the same line), `bad_source_locations_are_refused_with_the_reason` |
| Breakpoints at `file.rs:LINE`: `[debug] breakpoints`, MCP `breakpoint_set location=`, CLI `run --break` | verified | `qemu_source::a_source_line_breakpoint_can_be_set_at_run_time`, `mcp::source::source_lines_through_mcp`, `zynqsim::source::break_at_a_source_line_reports_file_and_line`. A line that starts code in several places (a loop, a generic) gets a breakpoint at each; a line without code resolves to the next one that has some |
| MCP `source_line` (addr, symbol or `file:LINE`, with context) and the source lines in `cpu_regs` | verified | `mcp::source::source_lines_through_mcp`; tool list: `mcp::tools::tests::covers_every_tool_in_the_spec_exactly_once` |
| GUI Source panel (file around the stopped pc, current line marked, +/- context) | verified | `gui::source_view::tests::the_model_follows_a_stop_to_its_source_line` (real QEMU, model plus drawn text), `the_panel_draws_the_window_and_the_buttons_change_it` (synthetic pointer click on "+") |
| Source *level* debugging beyond lines: variables, types, inlined-call chains, step over/into by line, a core 1 ELF's lines, PIE/relocated images, split DWARF | stub / not-planned | not read; only core 0's ELF has a `SourceMap`. Only `rust_debug.elf` carries line info of the committed fixtures (the other firmware ELFs are built without debug info) |

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

## Ethernet (PS GEM0)

The Zybo's PS Gigabit Ethernet on QEMU's user-mode network, with every frame visible to the host.
Needs the Xilinx QEMU; no PL, no XSim. Design, alternatives and limits: ADR-011. This section
supersedes the "PHY" mention in the Board table's stub row and the "network" mention in the
front-end table's `unsupported` row.

| Feature | Status | Evidence / note |
|---|---|---|
| `[network]` (`enable`, `mac`, `subnet`, `gateway`, `restrict`) attaches GEM0 to a simulated LAN (`-netdev user` + `filter-dump`); without it the NIC is unplugged (`-net none`) instead of QEMU's implicit user-mode network | verified | `backend-qemu::net::{qemu_arguments_bind_gem0_and_mirror_to_the_capture_file, an_unplugged_nic_gets_no_answers_and_captures_nothing}`. **Behaviour change**: projects without `[network]` used to get that implicit network, invisible to the host |
| Rust firmware (`firmware/net_demo`, prebuilt `fixtures/elf/rust_net.elf`, `examples/net-demo`): MDIO PHY scan, reset and auto-negotiation, descriptor rings, polled RX/TX, **smoltcp** ARP + DHCP + ICMP echo; four pings with 32 bytes, payload echoed unchanged | verified | `backend-qemu::net::firmware_pings_the_gateway_and_every_frame_is_captured` (14 frames: 4 DHCP, ARP pair, 4 echo pairs), `zynqsim::net::net_demo_pings_the_gateway_and_exports_a_matching_pcap`. smoltcp stands in for the BSP's lwIP, which is C; the Xilinx lwIP applications were **not** run (the xemacps DMA example was, see *Unmodified Xilinx BSP applications*) |
| Frames visible to the host: classified (ARP/ICMP/UDP/TCP), IPv4 and ICMP checksums verified, summarised, counted, 1 us virtual-time stamps; trace markers and `net.*_frames` signals; `.pcap` export | verified | `backend-qemu::net::tests::*` (decoder, malformed frames, partial reads, time recovery, pcap writer), `zynqsim::net::{net_demo_*, network_frames_are_in_the_trace_and_the_vcd}` |
| **Determinism**: same project, same frames, same bytes, same virtual times, same measured round trips, same `.pcap` bytes, same trace digest | verified | `backend-qemu::net::{two_runs_produce_identical_frames_times_and_uart, frame_times_do_not_depend_on_how_the_run_is_sliced}` (7 us vs 1 ms slices give identical frames and times), `zynqsim::net::two_processes_agree_on_digest_pcap_and_uart`. Holds because slirp answers inside the transmitting instruction |
| Record/replay, snapshot/restore (by re-execution) of a network session | verified | `zynqsim::net::a_network_session_records_and_replays_to_the_same_digest`, `mcp::net::network_project_through_mcp` (snapshot, restore, `session_replay`) |
| `[network]` validation: MAC, subnet 8..=27, gateway inside it and not .3/.15/network/broadcast, needs `ps = "qemu"`, unknown keys, `restrict = false` warns | verified | `sim-api::project::tests::network_*`, `zynqsim::net::a_bad_network_section_is_rejected_with_every_field_listed` |
| `mac`, `subnet`, `gateway` reach the firmware and the wire (DHCP lease, ARP, source MAC) | verified | `backend-qemu::net::a_custom_subnet_and_mac_reach_the_firmware_and_the_wire` |
| CLI `run --pcap FILE`, counters in `run` output (`network`) | verified | `zynqsim::net::*`; `--pcap` without `[network]` exits 3 |
| MCP `net_capture` (paged frames, counters), counters in `status_get`, gated by `Capability::Network` | verified | `mcp::net::{network_project_through_mcp, net_tools_are_refused_with_the_reason_without_a_network}` |
| MCP `net_send` (inject a frame) | stub | answers `unsupported` with the reason: slirp has no injection point (ADR-011) |
| GUI side panel "Ethernet (GEM0)": MAC, LAN, frame and byte counters | verified (model) / implemented-untested (panel) | `gui::model::tests::ethernet_counters_reach_the_model_for_a_network_project`; the panel was inspected in a `--screenshot` run only |
| `restrict = false` (guest may reach the host's network) | implemented-untested | only "DHCP then offers a router and the gateway still answers" is tested (`backend-qemu::net::an_open_network_offers_a_router_and_the_gateway_still_answers`); traffic that leaves the simulation is not reproducible |
| GEM driver register/descriptor layouts | implemented-untested against hardware | `VERIFY:` from memory of `xemacps_hw.h` / UG585; what is verified is that QEMU's `cadence_gem` model accepts them |

### Known limits of the Ethernet model

* **QEMU's GEM and PHY models, not the board's.** The firmware talks to the `cadence_gem` model
  and its Marvell-like PHY (MDIO address 7, ID `0141:0e50`); the RTL8211E, RGMII timing and the
  MIO pin mux are not simulated. The PHY reports link up with or without a network (no cable
  model), so link status says nothing about the cable. GEM1 exists in QEMU and is wired to
  nothing (its "no peer" warning in `qemu.err.log` is expected).
* **No host-side injection and no custom peer** (UDP echo, loss, latency): the network is
  slirp's (ARP, ICMP echo, DHCP, DNS, NAT); a Rust peer would need a socket backend behind a hub
  and would make only frame content/order, not arrival instruction, reproducible.
* Not exercised: interrupt-driven RX/TX, jumbo frames, checksum offload, priority queues, TCP
  and UDP application traffic, port forwarding (`hostfwd`), the second GEM, IPv6.
* The A9 global timer ticks at 100 MHz in this QEMU (silicon: 333 MHz at stock clocking,
  `VERIFY:` UG585); the demo firmware's microsecond figures use the QEMU value (`GT_HZ`).
* Timestamps have QEMU's 1 us resolution. The capture retains the newest 8192 frames (counters
  and the trace digest cover all of them).
* The Ethernet link/activity LEDs on the board picture and the 3D view are not driven.

## Board (`board-zybo`)

| Feature | Status | Evidence / note |
|---|---|---|
| Board description (TOML), validation with every problem listed | verified | `board-zybo::def::tests::*` |
| **Pin assignments** | implemented-untested | `VERIFY:` transcribed from memory of Zybo-Z7-Master.xdc; not checked against the reference manual or schematic. `pin_status = "unverified"` |
| **Pmod pin locations, port count, XADC/MIO sharing** | stub | `VERIFY:` pins intentionally absent; 6 ports (JA-JE PL, JF PS) assumed |
| Software renderer, golden PNGs, hit-testing | verified | `board-zybo::render::tests::*` (layout is schematic, not to scale) |
| **3D board view**: layout extruded to boxes, orbit camera, perspective, lit LEDs glow, switch levers and button plungers move, picking by drawn face | verified (scene) / implemented-untested (mouse handling) | `board-zybo::scene3d::tests::*` (deterministic, camera changes pixels, every switch/button/LED is found where it is drawn at three cameras, state changes pixels). Heights are illustrative, not measurements |
| **Custom boards at run time** (ADR-022): `[board] name = "<built-in>"` or `file = "board.toml"` (relative to the project); `BoardDef::load`, `Project::board_def`; recordings carry a non-built-in board | verified | `sim-api::boards::{every_builtin_board_name_validates_and_unknown_names_list_the_choices, a_board_file_resolves_next_to_the_project_and_its_mistakes_are_precise, a_recording_carries_a_custom_board_and_replays_without_the_file}`, `board-zybo::def::tests::a_board_file_loads_with_its_svg_and_reports_a_bad_one` |
| Board validation with precise errors: unknown connector kinds, unknown tables/fields/colours, controls outside or overhanging the layout, duplicate ids / package pins / keys, bad ids and names, derived-name collisions, more than 8 switches/buttons/LEDs | verified | `board-zybo::def::tests::{each_class_of_mistake_has_its_own_precise_message, more_than_eight_bits_do_not_fit_the_testbench_interface, validation_reports_every_problem}`; through the project: `sim-api::boards::a_board_file_resolves...`, `zynqsim::boards::inspect_board_summarises_built_in_boards_and_board_files`, `mcp::boards::a_board_file_with_mistakes_is_refused_with_every_one_of_them` |
| **SVG artwork** behind the controls (`svg = "art.svg"`): `rect circle ellipse line polyline polygon path(M L H V C Q Z) g`, fill/stroke/opacity, view box stretched over the layout; transforms, `url()` paints, percentages and other path commands are errors; text, images, gradients and other elements are skipped | verified (parser, drawing, golden) / not-planned (a general SVG renderer; the 3D view ignores the art) | `board-zybo::svg::tests::*` (6), `board-zybo::render::tests::a_custom_board_with_svg_art_matches_its_golden_and_the_art_shows` (`lab_board_active.png`, one pixel of the art asserted) |
| **Built-in boards Zybo Z7-10, ZedBoard (8 switches, 7 buttons, 8 LEDs, OLED, Pmods JA-JD, HDMI TX, VGA, ADAU1761), PYNQ-Z2 (2 switches, 4 buttons, 4 LEDs, 2 RGB LEDs, Pmods A/B, Arduino and RPi headers, HDMI in/out, ADAU1761): structure** (loading, state sized by the file, 2D render, hit-testing, 3D extrusion and picking) | verified | `board-zybo::def::tests::{every_builtin_board_loads_with_its_component_counts, the_z7_10_is_the_z7_20_with_a_smaller_device, the_new_boards_keep_their_controls_apart}`, `render::tests::{each_builtin_board_matches_its_golden_pictures, every_control_of_every_board_hit_tests_to_itself}`, `scene3d::tests::every_board_extrudes_without_panicking_and_its_controls_are_pickable` |
| **Facts of those three boards** (every package pin, MIO number, LED/switch/button count, PS/PL clock, DDR size, FPGA resources, which Pmods/connectors exist, that the Z7-10 equals the Z7-20 apart from the device, the ZedBoard's "7 push buttons" = 5 PL + 2 MIO, its OLED on PL pins, the PYNQ-Z2 audio jacks) | implemented-untested | `VERIFY:` in each `boards/*.toml`: written from memory of the vendors' documents, none checked; `pin_status = "unverified"`. Only duplicate pins within a file are checked |
| Component counts reach every layer: `BoardState`, 2D/3D, GUI keys and status hint, CLI `run --json` (`switches`, `buttons`, `leds`, `board_info`), `run --board`, `inspect --board`, MCP `board_set_switch`/`board_press_button` ranges, `pin_get` names and hints, `status_get.board_info`, `zynqsim://board`, demo engine (LED i follows SW i, last LED = heartbeat) | verified | `sim-api::boards::{the_demo_engine_sizes_its_state_and_validates_inputs_from_the_board, zedboard_leds_follow_the_switches_and_pin_names_come_from_the_board}`, `zynqsim::boards::{run_json_reports_the_boards_own_controls, a_project_with_a_board_file_runs_and_inspects}`, `mcp::boards::*` (5) |
| GUI board picker (project window): built-in names or a custom file, Build swaps the engine; clicks, held buttons and key shortcuts work on the new board | verified (logic, synthetic input) / implemented-untested (appearance of the combo box) | `zynqsim-gui::app::tests::picking_a_board_in_the_project_window_changes_the_controls_and_they_work`, `loader::tests::the_board_picker_edits_only_the_board` |
| Testbench interface widths (ADR-022): `tel_ctrl` switches/buttons and `tel_event` LED mask are one byte, bit n = component n of the board file; Zybo encoding (bits 3..0) unchanged | verified | `xsim-dpi::core::tests::board_pins_are_eight_bits_wide`, `vendor-ip::tests::eight_bit_boards_reach_the_gpio_in_full`, and end to end: `sim-api::boards::zedboard_drives_eight_leds_from_firmware_and_takes_eight_switches_and_seven_buttons` (Rust firmware `board8_demo` -> 8-bit RTL GPIO in XSim: LEDs 0xA5, then SW=0x90 and BTN=0x40 mirrored; `examples/zedboard-demo`). Needs QEMU + XSim + DPI (`SKIPPED:` otherwise) |
| Devices vs. the board's ports: `pmod_ja`.. = the board's 1st, 2nd.. Pmod; `cam` needs a `pcam` connector | verified | `sim-api::boards::devices_must_fit_the_boards_ports` |
| **Not done for other boards**: PS-side parts (ZedBoard MIO buttons/LED, PS Pmod JE; the OLED has no model), more than 8 bits per group, `init --xsa/--ip/--boot` (they write `zybo-z7-20`), Pmod keys are still spelled `pmod_ja`..`pmod_je` whatever the Pmod is called, media/HDMI/audio models are the Zybo's SSM2603 testbench (the ADAU1761 is not modelled), SVG in 3D | stub / not-planned | ADR-022 |
| SSM2603 and HDMI RX/TX | verified at pin level in the testbench | see *Audio, video and the project loader*. Not a model of the board's wiring: the testbench connects them |
| **Pmod pins** (JA..JE): the PL's pin levels reach the board state (pin colours in 2D and 3D, `pin_get JA1`), `Capability::Pmod`, trace signals `pmod.ja` | verified | `zynqsim::oled_demo_draws_text_pixel_exact`, `mcp::pmod_oled_through_mcp`, `xsim-dpi::core::tests` for the packets. Needs a testbench that calls `zs_pin_out`/is generated by `init --ip`. JF (a PS Pmod on MIO) is not exercised by a test; host-driven input pins are, on JB (`pmod_simple_demo_lights_the_8ld_and_reads_the_buttons`) |
| **Pmod OLED** (SSD1306, 128x32, SPI): commands, data, the three addressing modes with windows, on/off, inverse, segment remap, COM flip, contrast, reset | verified | `devices::ssd1306::tests` (9) and the end-to-end test: Rust firmware -> AXI-Lite -> SPI master RTL (`fixtures/rtl/pmod/oled_spi.v`) -> JA pins -> host model, **all 4096 pixels equal** the expected text in the shared font. Not modelled: scrolling, start line / display offset, charge-pump and supply sequencing; levels of VBATC/VDDC are recorded only. Pin order and the init values follow the Digilent reference as remembered: `VERIFY:` |
| **RGB LEDs**: six PWM pins averaged into duty cycles, shown as colours | verified | `zynqsim::rgb_demo_averages_the_pwm_pins_into_colours` (duties 64/128/255 and 255/0/32 within 4 of 255). Windows of >= 200 us, so a duty changes with up to that latency; the last duty is kept after the testbench ends |
| **Pmod 8LD** (eight active-high LEDs on the port's eight pins): the state and picture follow the pins the PL drives | verified | `zynqsim::pmod_simple_demo_lights_the_8ld_and_reads_the_buttons` (Rust firmware -> AXI-Lite register block -> JA: `0xA5` at 2 ms, `0x3C` at the end, the same two pin changes), `devices::led8::tests` (3). Pin order and polarity as remembered: `VERIFY:` |
| **Pmod BTN** (four buttons and four switches, host-driven inputs `btn0`..`btn3`, `sw0`..`sw3`): the values the host sets reach the PL's input pins (`zs_pin_in`, sampled every microsecond) and the firmware reads them | verified | the same end-to-end test: the firmware prints `BTN 0x00` then `BTN 0x41` after `--device-set 3ms@JB.btn0=1` and `3ms@JB.sw2=1`; two runs give one trace hash; `devices::buttons::tests` (5). The switches on the BTN module follow the task's module list, not a datasheet: `VERIFY:` |
| **Pmod SWT** (four host-driven switches) | verified (model unit tests only; no PL test) | `devices::buttons::tests` (`swt_has_switches_only` and the others) |
| **Pmod SSD** (two seven-segment digits, segments and digit select on the pins, multiplexed by the PL; a deselected digit persists for 4 ms; hex digits 0-9, A-F) | verified (model unit tests only; no PL test, no firmware) | `devices::ssd::tests` (5): all 16 digit patterns, multiplexed digits, the persistence time, a static display, the picture. Pin order (AA..AG, C) and the polarity of the segments are `VERIFY:` |
| Ethernet PHY at pin level (RGMII/MDIO wires), other Pmod modules (no kind in `devices::KINDS`) | stub | drawn as inert parts; the GEM talks to QEMU's PHY model (ADR-011) |

## Storage (SD card, QSPI flash; ADR-012)

| Feature | Status | Evidence / note |
|---|---|---|
| `[storage]` SD card from a raw image or a folder (FAT16/FAT32, reproducible), read-only by default | verified | `sim-api::storage::tests::*`; `zynqsim` `sd_demo_reads_a_file_from_the_card_through_sdhci_and_fat`; `sim-api` `storage_session` |
| Image tools: `sd-image`, `qspi-image`; size, label and missing-folder rules | verified | `sim-api::storage::tests::*`, `zynqsim` `image_tools_build_card_and_flash_images_and_refuse_bad_sizes` |
| Writable storage: guest writes change the host image; snapshot, reset and recording refused with the reason | verified | `sim-api` `storage_session::writable_storage_drops_snapshot_and_replay_and_says_why`; `zynqsim` `writable_storage_changes_the_image_and_refuses_what_cannot_be_repeated` |
| Read-only storage keeps snapshots, reset and replay exact | verified (QEMU) | `sim-api` `storage_session::read_only_storage_keeps_snapshots_reset_and_replay_exact` |
| QSPI flash, default `s25fl129p0`; other models report their own JEDEC ID | verified | `zynqsim` `qspi_demo_reads_the_jedec_id_*`, `the_qspi_demo_example_and_each_flash_model_report_their_own_id` |
| Writable flash is programmed in the image file | verified | `zynqsim` `writable_flash_is_programmed_in_the_image_file` |
| `sd_demo` (polled SDHCI, PIO, 1-bit) and `qspi_demo` (FIFO and linear reads) firmware | implemented-untested | run against QEMU's SDHCI and QSPI models only; register offsets are from the TRM, marked `VERIFY:` |
| Storage through MCP (`status_get`, `sim_run`, snapshots over the example projects) | verified (QEMU) | `mcp` `tests/storage.rs` |
| SD high-speed, 4-bit bus, DMA, card-detect; SDHC above 1 GiB | not implemented | ADR-012 |
| QSPI dual/quad modes, DMA, interrupts, dual-flash | not implemented | ADR-012 |
| Storage on the PL (RTL) engines | not implemented | storage is attached to the QEMU (PS) engine only |

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
| MCP `pmod_attach`, `pmod_frame` (extension), CLI `--device-out JA=FILE.png`, `[devices] pmod_ja = "oled"` with validation | verified | `mcp::scenario::pmod_oled_through_mcp`, `sim-api::project::tests::devices_are_validated_by_connector_and_kind` |
| GUI: Pmod devices panel (the OLED picture and its state) | implemented-untested (behaviour) / verified (appearance) | not covered by a test; the model fetches the picture (10 Hz). Appearance: *GUI panel appearance* below |
| MCP tools needing PL signals/network | stub | `unsupported` naming the missing capability |
| `zynqsim init --ip FILE`: testbench + `project.toml` generated from the module header of an AXI4-Lite slave IP (32-bit data, any prefix/case, active-low or high reset, extra clocks tied to FCLK0, other inputs tied low) | verified | `sim-api::tbgen::tests::*` (header parsing incl. attributes/comments/parameters/non-ANSI, detection, rejections with reasons), `zynqsim::init_ip_generates_a_project_that_passes_with_the_real_firmware` (generated for `my_math_ip.v`: the real firmware passes with the same 21 bus accesses and 329 trace records as the native model). The parser reads module headers only, it is not a Verilog parser; IPs with AXI masters, interrupts or other bus flavours are refused (use a hand-written testbench). An AXI4 **master** interface (32-bit, `*ARADDR`... `*BREADY`, 25 signals) is bridged to PS memory through S_AXI_HP0 with a modelled 16-clock DDR latency: `zynqsim::init_ip_generates_the_dma_testbench_with_a_memory_bridge` generates the testbench for `dma_xform.v` and the two-core DMA regression gives **the same trace digest and 687 bus accesses** as the hand-written testbench. Other AXI4 slave flavours (full slave, AXI-Stream) are not generated; a master with other than 32-bit data is refused. One-bit outputs named `*irq*`/`*interrupt*`/`*intr*` (up to 16) become `IRQ_F2P[0..]`: `zynqsim::init_ip_forwards_interrupts_to_a_firmware_handler` (a generated testbench for `fixtures/rtl/irq/irq_ip.v`, level-high; the Rust firmware's GIC setup and handler took 3 of 3 events, 0 spurious). Only level-high is exercised; edge-style outputs are forwarded as levels |
| `pmod_attach` | verified | `mcp::scenario::pmod_oled_through_mcp` (OLED), `pmod_sensors_through_mcp` |
| `project_create_from_export` | verified | `mcp::export::*`: a real Vivado 2022.1 export folder and a bare funcsim netlist become valid projects, a list with a missing file is refused with `file:line`; see *Vivado simulation exports and netlists* |
| `project_create_from_xsa` | verified | `mcp::xsa::*`: reads a real Vivado 2022.1 XSA, writes `project.toml` + report + testbench, lists the unmodelled IP, `project_open` then refuses to start; see *XSA / HWH ingestion* |
| CLI: `run` (any engine, `--at` inputs incl. `uartN:`/`swN:`/`btnN:`, `--dump-mem`, `--dump-image`), `inspect`, `init`, `trace`, `test` (exit codes 0/1/2/3, `--json`) | verified | `crates/cli/tests/cli.rs` |
| CLI: `mcp`, `gui` launchers | implemented-untested | not covered by a test |
| CLI: `ctl` | stub | needs an IPC listener |
| GUI behaviour layer (`Model`) incl. engine faults | verified | `zynqsim-gui::model::tests::*`, `app::tests::*` |
| GUI window incl. Memory / image viewer | verified (input handling) / implemented-untested (appearance) | `zynqsim-gui::app::tests`: the whole UI is driven frame by frame with synthetic egui input and no window: 2D click toggles a switch, a button is held while the pointer is down, keyboard shortcuts, 3D click picks the switch, drag orbits, wheel zooms, the project window builds and swaps the simulation and refuses invalid drafts. Appearance is still judged only from `--screenshot` images; native file dialogs cannot be driven by tests |
| GUI start-up flags for automated checks (`--uart`, `--mem`, `--toggle`, `--run-ms`, `--screenshot`) | implemented-untested | used by hand |
| GUI: 2D/3D board tabs, PL bus-access list | verified (interaction) | `app::tests` (above) and screenshots |
| GUI: waveform viewer tab (signal picker with search, time ruler, digital and hex bus traces, wheel zoom around the pointer, drag pan, click cursor with value readout, follow live time, Export VCD) | verified (interaction, content) / implemented-untested (appearance) | `zynqsim-gui::app::tests::the_waveform_tab_zooms_pans_and_places_a_cursor` (synthetic wheel/drag/click, readouts at two cursor positions, window re-fetched to match the view), `zynqsim-gui::waves::tests::*` (zoom anchoring and clamps, search filter, fetch only while active, follow, selection kept across an engine swap, VCD export and its error). Look and feel judged by eye only; the native Save dialog of Export VCD cannot be driven by a test (the export itself is). The chosen signals live for the session and across project loads; they are not written to the project file |
| GUI: waveform defaults on load (LEDs, switches, buttons, UART, Pmod, RGB, a bus address; at most 12), group picker (board/uart/bus/pmod/other), select all matches / none, "Pick default signals", zoom to fit, right-drag zoom to selection, coloured names, value column at the cursor, time-unit label, elided long names with the full name on hover | verified (behaviour) / verified (appearance: `waves-defaults.png`) | `zynqsim-gui::waves::tests::default_signals_cover_each_kind_and_stay_within_the_cap`, `groups_and_the_picker_actions`, `a_new_project_opens_with_the_default_signals` (demo engine), `zoom_to_selection_sets_the_window_and_leaves_follow`, `long_symbols_are_elided_in_the_middle_and_short_text_is_kept`. Right-button drag itself is not driven by a test |
| GUI: trace strip filter (substring, case-insensitive, no regex), type toggles (uart/board/bus/marker/protocol), copy visible lines, pause on scroll with "jump to latest", text size 10-18 pt shared with the console | verified (filter logic, pause state machine) / **not verified in the window layout** (see the appearance table) | `zynqsim-gui::model::tests::trace_filter_shows_by_kind_and_substring`, `trace_kinds_follow_the_record_type`, `the_trace_strip_stops_following_when_the_user_scrolls_up`. The egui wiring (copy, scroll, slider) is not tested |
| GUI: dockable panels, CPU/PL signal/network/timeline panels, DWARF views | stub | fixed layout; panels that need a backend say so |
| GUI menu bar (File, Simulation, View, Help) and the Ctrl/F-key shortcuts (Ctrl+O, Ctrl+Q, Ctrl+= / Ctrl+- / Ctrl+0, Space, F5, F6, F1); the menu entries call the same actions | verified (model, synthetic input) / implemented-untested (dropdowns) | `zynqsim-gui::app::shell::tests::{menu_actions_change_the_view_theme_and_scale, keyboard_shortcuts_drive_the_shell}`. No test opens a dropdown; the `--screenshot` path cannot open one |
| GUI recent projects (last 8, no duplicates, saved to `%APPDATA%\zynqsim\gui.json` or `~/.config/zynqsim/gui.json`) | verified (logic, file round trip in a temp dir) / implemented-untested (menu) | `zynqsim-gui::prefs::tests::{recent_projects_dedupe_move_to_front_and_cap_at_eight, settings_survive_a_save_and_load_in_a_temp_dir}`, `app::shell::tests::recent_projects_open_and_missing_ones_are_dropped`. Automated runs (`--screenshot`, `--exit-after-ms`, `--defaults`, `--theme`) save nothing |
| GUI examples browser: File > Examples lists every `examples/*/project.toml` (found next to the executable, above it, above the working directory, or in the source tree) and opens one through the background build | verified (discovery, opening) / implemented-untested (menu) | `zynqsim-gui::prefs::tests::examples_discovery_lists_the_repos_examples`, `app::shell::tests::examples_from_the_repo_are_offered_and_open_in_the_background` |
| GUI theme (dark, light), UI scale (0.8 to 2.0 in steps), readable floors for small, body and monospace text, remembered between runs | verified (logic) / verified (appearance, light theme screenshot) | `app::shell::tests::menu_actions_change_the_view_theme_and_scale`, `prefs::tests::scale_steps_move_one_notch_and_stop_at_the_ends`. Window size is remembered by `main.rs` and was not tested |
| GUI status bar: simulated time, running / paused / halted at breakpoint, measured speed (sim time per wall time), project and backends, last error (click clears) | verified (appearance: paused state, shown) / implemented-untested (running and halted text, speed) | `app::shell::tests::the_speed_meter_measures_simulated_per_wall_time`; `default-demo.png` shows the paused state; the halted text is in `rust-debug-source.png`; the running text was not looked at |

| GUI side panel: play-speed selector (0.01x, 0.1x, 1x, 10x, max) and measured sim/wall ratio | verified (model, interaction) / implemented-untested (appearance) | `zynqsim-gui::model::tests::speeds_map_to_ps_per_second_and_start_at_real_time`, `the_measured_ratio_is_simulated_over_wall_time`, `app::tests::the_speed_selector_reaches_the_engine_and_max_runs`. The measured ratio of a real run was not compared with `docs/PERFORMANCE.md` |
| GUI side panel: collapsible sections (CPU registers, Source, Pmod, HDMI, Audio, Ethernet, Storage, Memory) with one-line summaries when closed; open state saved through eframe storage (`persistence` feature) and read back at start-up | verified (state round trip) / implemented-untested (saving between runs) | `zynqsim-gui::app::sections::tests::*`, `app::tests::open_sections_are_saved_and_restored_through_storage` (save into an in-memory store, restore into a new app). That a closed panel stays closed after a real restart was not checked by hand; `--sections` overrides the saved state for one run |
| GUI storage table (device, image or folder, size, model, access; writable in a warning colour, read-only tooltip "writes discarded") | verified (rows from the status) / looked at (appearance) | `zynqsim-gui::model::tests::storage_rows_come_from_the_status`, `sizes_are_shown_in_the_largest_exact_unit`; `sd-demo-storage-table.png` (read-only row only: no writable device was photographed) |
| GUI memory viewer: go-to address (number, or ELF symbol with `+offset` through the symbol query), 8/16/32-bit cells, ASCII column, bytes changed since the last read in orange, follow PC / SP while stopped, copy as hex, clear message without memory access | verified (parsing, cells, change marks, copy text) / looked at (appearance, `--mem-hex`) | `zynqsim-gui::app::memory::tests::*`, `app::tests::the_memory_viewer_says_why_it_has_no_memory`; `rust-debug-source.png`. The symbol path needs a stopped qemu engine and was not run by a test |
| GUI CPU registers: N Z C V chips, mode chip, core 1 only when the project declares `[cpu1]`, registers changed since the previous stop in orange | verified (flags, mode, diff) / looked at (core 0 only) | `zynqsim-gui::app::cpu::tests::*` (the diff is `model::changed_registers`); `rust-debug-source.png` (first stop only, so no change marks shown). No dual-core project was photographed |

### GUI panel appearance (screenshots, 2026-10-17, merged p6 GUI)

Appearance is judged by eye from `--screenshot` PNGs. "verified" here means that a screenshot was
viewed in the merge review (`docs/screenshots/` keeps five of them); a later change can break a
layout without any test failing. The smoke test `crates/gui/tests/screenshot.rs` only checks that
the screenshot path produces a non-uniform PNG. The screenshots taken before the merge (2026-10-16)
were removed because they no longer match the UI; rows that depend on them are marked *not
re-checked*.

| Panel | Status | Evidence / defects found |
|---|---|---|
| 2D board (switches, buttons, LEDs, Pmod pins, labels) | verified (appearance) | `default-demo.png`, and the `examples/board-demo` run (not kept). No defect seen |
| 3D board (drag, zoom, reset / top-down buttons) | not re-checked | no screenshot in this review |
| Waveforms tab (default signals) | verified (appearance) | `waves-defaults.png` (`--waves-tab`): 12 signals, the 8-bit LEDs and switches first. **Fixed**: the default pick skips signals wider than 8 bits, so `led5`/`led6` (24-bit) are no longer chosen. Side effect: the bus-address default (32-bit) is no longer picked either; it can still be chosen by hand. The text and the zoom hint were fixed earlier |
| Trace strip (filter, toggles, copy, count) | verified (appearance) | `default-demo.png`, `rust-debug-source.png`: about six lines visible in a 1600x1025 window with the default layout (was one line at ~40 px). **Fixed**: the strip takes 35% of the side panel (30-70%, min 120 px), and a grip between sections and trace resizes it. The filtered view itself (`--trace-filter`) was not re-photographed |
| Source panel at a breakpoint | verified (appearance) | `rust-debug-source.png` (opened with `--sections source=1,memory=1`). Long source lines scroll sideways (earlier fix). Not changed: the `pc in` line shows the mangled Rust symbol over several lines |
| CPU registers | verified (appearance) | `rust-debug-source.png`: N Z C V chips, mode chip, core 0 only. Earlier: CPU 1 is shown only when the project declares `[cpu1]` |
| Status line | verified (appearance) | `default-demo.png` (paused), `rust-debug-source.png` (halted at breakpoint). **Fixed**: the status text was drawn under the "F1: keyboard shortcuts" hint; it is now cut with "..." to the room the hint leaves, and the full text is on hover. Not changed: the "speed -" field while paused |
| Speed selector | verified (appearance) | `default-demo.png`: one copy, in the side panel. **Fixed**: the duplicate Simulation > Speed submenu was removed |
| Menu bar and top control row | verified (appearance, menus closed) | `default-demo.png`. Play, Step, +100 ms, Reset, snapshot buttons appear in both the Simulation menu and the top row; this is intended (the menu lists every command). The dropdowns were not photographed |
| Light theme | verified (appearance) | `sd-demo` run with `--theme light` (not kept); the 2D board, side panel, trace and status line were checked, no defect seen |
| Keyboard shortcuts window (F1) | verified (appearance) | `shell-shortcuts.png`. The list matches the bindings in `shell_keys` and `handle_keys`; no key is bound twice (board keys are letters and digits and ignore Ctrl; shell keys are Ctrl combinations, Space, F1, F5, F6) |
| Storage table (SD card row in the side panel) | verified (appearance) | `sd-demo-storage-table.png`: read-only SD card row (only read-only was photographed) |
| Memory / image viewer | not re-checked | the viewer in the Rust-debug run shows "no data read yet" and the controls; the image views were not photographed in this review |
| Pmod devices, Ethernet, HDMI, Audio, Project window | not re-checked | the screenshots of these panels were removed with the pre-merge set; the earlier clipping fix of the side panel applies to them, but they were not looked at again |
| Clock tree, IRQ map, SLCR | stub | the panel says it is not available; no appearance check beyond that |

Caveats: the screenshots were taken at the window default size and scale, with the default layout.
Fonts, colours and contrast were judged by eye, not measured.

## XSA / HWH ingestion (start of milestone P3)

`zynqsim init --xsa design.xsa --out DIR [--rtl FILE...] [--elf FILE]` and MCP
`project_create_from_xsa` (ADR-015). The fixtures are **real Vivado 2022.1 output**
(`fixtures/xsa/make_xsa.tcl`: Zybo Z7-20 board preset, `my_math_ip` as a module reference at
`0x43C00000`, optionally an AXI GPIO with its interrupt on `IRQ_F2P[0]`); they are one design,
so other Vivado versions and richer designs (SmartConnect, hierarchies, DMA) are `VERIFY:`.
Gate on this branch: `cargo xtask ci` exited 0 on 2026-10-16 with 376 tests (356 + 20 new), none
failed; the end-to-end row below was also run alone with QEMU and XSim present (it did not skip).

| Feature | Status | Evidence / note |
|---|---|---|
| Reader: `.xsa` (zip) and bare `.hwh`; PS7 FCLK0-3 (enable, MHz), enabled `M_AXI_GP`/`S_AXI_GP`/`S_AXI_HP`/ACP, `IRQ_F2P`, PS peripheral enables and IO, MIO use per pin, EMIO GPIO; every PL instance with VLNV, parameters, ports, bus interfaces; PS address map; interrupts to `IRQ_F2P` bits; clock and reset sources | verified | `xsa-import::tests::the_vivado_2022_1_fixture_is_read` (the real XSA: VLNVs, `0x43C00000`/`0x41200000`, ranges, IRQ bit 0 level-high, FCLK0 100 MHz, UART1 on MIO 48..49, 54-pin MIO tree) |
| `xlconcat` resolution, HP/ACP/GP1, EMIO, third-party/unknown/vendor classification, GIC ID of a bit | verified against a **hand-written** HWH in Vivado's element names (not Vivado output) | `xsa-import::tests::a_synthetic_handoff_covers_concat_hp_acp_emio_and_classes`, `sim-api::xsa_import::irq_bit_and_clock_frequency_reach_the_testbench` |
| Archive handling: sources and file lists collected, `ps7_init`/bitstream noted, entries with `..`/absolute paths refused, clear message for a zip without a HWH or non-XML input | verified | `xsa-import::tests::{archive_sources_and_filelists_are_collected, zip_entries_with_parent_components_are_refused, a_zip_without_a_hwh_says_how_to_fix_it, bad_inputs_are_explained}` |
| `export_simulation` `.prj` and `.f` file lists parsed | verified (parser) / implemented-untested (`--filelist` with `--xsa` end to end) | `xsa-import::tests::{archive_sources_and_filelists_are_collected, plain_file_lists_skip_options_and_comments}` |
| User RTL (module reference or packaged IP, AXI4-Lite slave, source in the archive or `--rtl`) becomes `[rtl]` + generated testbench; address map, IRQ bits and FCLK clocks go into `[connect]`; interrupt outputs are forwarded on the bit the design uses; testbench clock follows the design's frequency | verified | `sim-api::xsa_import::{the_gpio_design_maps_the_user_ip_and_places_the_gpio_model_next_to_it, a_user_rtl_only_design_validates_clean, irq_bit_and_clock_frequency_reach_the_testbench}`. The `[connect]` entries are informational on the RTL path: the generated testbench answers the whole `M_AXI_GP0` window with the one module (ignores upper address bits) |
| Every IP without a model is recorded in `[xsa.unmodelled]`; each entry is a validation error naming instance, VLNV, address, IRQ bits and reason; `run`/`project_open` stop (never a silent partial run) | verified | `zynqsim::xsa::an_unmodelled_ip_is_reported_and_stops_the_run` (exit 1, diagnostic on stderr; the unmodelled IP is an AXI VDMA in the hand-written `fixtures/xsa/zs_math_vdma.hwh`, the DMA being modelled since ADR-021), `mcp::xsa::an_xsa_with_an_unmodelled_ip_creates_a_project_that_will_not_open`, `sim-api::xsa_import::{an_ip_without_a_model_is_recorded_and_stops_the_run, a_missing_rtl_source_is_unmodelled_with_the_flag_to_pass_it}` |
| `xsa-report.md`: PS7 table, MIO use, every instance with class/address/IRQ/clock/disposition, address map, mapped files, *Not supported*, *Not applied*; verdict says NOT RUNNABLE when anything is unmodelled and never claims correctness | verified (content) | `zynqsim::xsa`, `sim-api::xsa_import` (strings asserted) |
| **End to end**: an XSA whose only PL IP is the user's math IP runs on QEMU + XSim with the existing firmware: `RESULT: ALL PASS`, the same 21 bus accesses / 329 trace records as the native-model and `init --ip` routes | verified (Windows, needs QEMU + Vivado + DPI library) | `zynqsim::xsa::a_user_rtl_only_handoff_runs_the_real_firmware` |
| **Not done** | stub / not-planned | `ps7_init` data is read as file names only and not applied (QEMU boots the ELF; PS clock tree, MIO and peripheral enables change nothing); vendor IP other than the models listed in *Xilinx IP models* (e.g. VDMA, Quad SPI) is not modelled, so such designs are generated but do not run; one user AXI4-Lite slave per project (others are listed unmodelled; no multi-instance testbench or address decode); VHDL sources are listed, not compiled; `BOOT.BIN`; custom boards; GUI import; `--filelist` without `--xsa` |

## Xilinx IP models (`vendor-ip`, milestone P3)

`crates/vendor-ip`: register-level models of **AXI GPIO 2.0, AXI Timer 2.0, AXI UART Lite 2.0
(ADR-016), AXI DMA 7.1 (direct and scatter-gather), AXI IIC 2.1, AXI BRAM Controller 4.1 and AXI
Interrupt Controller 4.1 (ADR-021)** as `pl-native` blocks, placed by `[native]` (`axi_gpio`,
`axi_timer`, `axi_uartlite`, `axi_dma`, `axi_iic`, `axi_bram_ctrl`, `axi_intc`) with
`[native_params.<instance>]`, composed with user RTL in XSim (ADR-016). `zynqsim init --xsa` maps
them from the block design. The example is `examples/gpio-timer-uart` (firmware
`firmware/gpio_timer_uart`, Rust `no_std`); its design is the **real Vivado 2022.1 XSA**
`fixtures/xsa/zs_vendor_ip.xsa` (`fixtures/xsa/make_vendor_xsa.tcl`: two AXI GPIO, a timer, a UART
Lite, interrupts through an `xlconcat`).

**Register maps and behaviour are written from the Xilinx product guides (PG144, PG079, PG142,
PG021, PG090, PG078, PG099) from memory and were never compared with a Xilinx simulation or a board.** "verified" below means
the model does what this repository's tests say it does, not that real hardware does.

| Feature | Status | Evidence / note |
|---|---|---|
| AXI GPIO: `GPIO_DATA/TRI`, `GPIO2_DATA/TRI`, `GIER`, `IP_ISR` (toggle on write), `IP_IER`; widths 1..32, dual channel, `C_ALL_INPUTS/OUTPUTS`, `C_DOUT_DEFAULT`, `C_TRI_DEFAULT`; interrupt on input change; reset values | verified (model) / VERIFY (register facts) | `vendor-ip::gpio::tests::*` (6) and, through QEMU and the real firmware, `sim-api::vendor_ip::the_firmware_drives_all_three_models_and_the_board_follows`. VERIFY: toggle-on-write of `IP_ISR`; status bit set by any change of the pad value (not masked by TRI); data read of a bidirectional channel (pin for inputs, register for outputs); `TRI` fixed under `C_ALL_*`; unmapped offsets read 0 without SLVERR |
| AXI Timer: `TCSR0/1`, `TLR0/1`, `TCR0/1`; generate mode, up/down, `LOAD`, auto reload, `TINT` write-1-to-clear, interrupt level, `ENALL`, `C_COUNT_WIDTH`, `C_ONE_TIMER_ONLY`, `generateout` pulses counted and PWM level | verified (model) / VERIFY (register facts) | `vendor-ip::timer::tests::*` (11), firmware: three periodic interrupts (`timer ticks=3`). VERIFY: period formulas (up `MAX-TLR+1`, down `TLR+1` clocks), no hold without `ARHT`, `LOAD` semantics, `ENALL` read-back, `TCR` exact (hardware is a few clocks stale) |
| AXI Timer capture mode (`MDT`=1): rising edge on `CAPTURETRIG0/1` (`CAPT`, `ENT`) copies the counter to `TLR`, sets `TINT`, `ENIT` interrupt; `ARHT` holds after a capture until `TINT` is cleared; bound to the board with `[connect] "<inst>.CAPTURETRIG0" = "board.btn0"` (or `board.swN`) and as a programmatic input | verified (model) / VERIFY (register facts) | `vendor-ip::timer::tests::{capture_on_a_rising_edge_copies_the_counter_into_tlr, capture_needs_a_rising_edge_not_a_level, capture_is_ignored_when_capt_or_ent_or_mdt_is_clear, timer1_has_its_own_capture_trigger, capture_with_arht_holds_the_counter_until_tint_is_cleared, reset_clears_captures_and_the_trigger_level}`, `vendor-ip::tests::timer_capture_inputs_are_bound_to_board_bits`, `sim-api::project::tests::a_timer_capture_input_takes_one_bit_and_only_the_capture_pins`, `crates/cli/tests/timer_modes.rs::capture_and_cascade_modes_run_on_the_real_firmware` (real QEMU; button 0 pressed at 3 ms, one capture, the release captures nothing). VERIFY: the rising edge and the `ENT` requirement, what `ARHT` (hold) means in capture mode, that rollovers set no interrupt in capture mode |
| AXI Timer cascade mode (`CASC` in `TCSR0`): timers 0/1 form one `2 * C_COUNT_WIDTH`-bit counter, `TCR1`/`TLR1` high word, carry from timer 0, load, `ENT`/`UDT`/`ARHT`/`GENT` of timer 0, capture of the 64-bit count into `TLR1:TLR0`, 64-bit rollover interrupt | verified (model) / VERIFY (register facts) | `vendor-ip::timer::tests::{cascade_carries_from_timer0_into_timer1_across_the_32_bit_boundary, cascade_rollover_is_a_64_bit_rollover_with_a_periodic_interrupt, cascade_capture_stores_the_64_bit_count_in_tlr1_tlr0}`, `crates/cli/tests/timer_modes.rs::capture_and_cascade_modes_run_on_the_real_firmware` (carry across 2^32 with no interrupt, three periodic 64-bit interrupts, `TCR1` stays all ones). VERIFY: which timer's `TINT` a 64-bit rollover sets (both are set here), that `CAPTURETRIG1` and timer 1's own `ENT`/`UDT`/`ARHT` are ignored in cascade mode |
| AXI Timer external generate pins `GENERATEOUT0/1` (counted pulses, PWM level) | verified (model) | `vendor-ip::timer::tests::{generate_pulses_and_pwm_level}`; PWM duty/period readback not modelled |
| AXI UART Lite: `RX_FIFO`, `TX_FIFO`, `STAT_REG`, `CTRL_REG`; 16-deep FIFOs, baud/frame paced shifter, overrun, interrupt, reset values | verified (model) / VERIFY (register facts) | `vendor-ip::uartlite::tests::*` (7), firmware greeting and echo (`sim-api::vendor_ip`, `zynqsim::xsa::a_vendor_ip_handoff_runs_the_real_firmware_on_the_models`). VERIFY: interrupt condition and that a `STAT_REG` read lowers it, "TX empty" means FIFO not shifter, TX write when full is dropped, overrun clears on `STAT_REG` read, no frame/parity errors (no pins) |
| UART Lite bytes to the host: `uart_read` port `2 + n`, `uart_write` port `2 + n` (deterministic, baud-paced), `uart<port>` trace records, `uart.pl` in `run --json` | verified | `sim-api::vendor_ip` (both tests, incl. determinism), `mcp::vendor_ip::vendor_ip_example_through_mcp`, `zynqsim::xsa` |
| GPIO bound to LEDs, switches, buttons and Pmod pins (`[connect] "<inst>.GPIO" = "board.leds"`); Pmod outputs go to the device bank | verified (LEDs, switches, buttons) / implemented-untested (Pmod binding beyond `vendor-ip::tests`) | `sim-api::vendor_ip`, `mcp::vendor_ip`; Pmod binding: `vendor-ip` unit tests only (no device attached on a GPIO-driven Pmod in an engine test) |
| Interrupts: a change caused by an access travels with the access, one caused by time or a board input at the next sync (at most one quantum, 1 us, late); level, to `IRQ_F2P[n]` | verified | `pl-native::regs::tests`, `vendor-ip::tests::{gpio_leds_switches_and_interrupt_through_the_pl, timer_interrupt_and_uart_bytes}`, firmware (3 timer interrupts, button interrupt count). The first version lowered lines only at the next sync and hung the guest in a re-firing level interrupt: see ADR-016 |
| Vendor models next to user RTL in XSim: vendor window answered by the model, everything else by the RTL; board outputs and pins merged | verified (Windows, QEMU + Vivado + DPI) | `zynqsim::xsa::user_rtl_in_xsim_and_a_vendor_model_share_one_pl` (math IP and GPIO in one PL, LEDs 0 and 3 lit), `vendor-ip::tests::unmapped_accesses_go_to_the_rtl_design` |
| `init --xsa` maps the three IP: parameters (`C_*`), `ADDRESS_SIZE`, `CLK_FREQ_HZ`, `IRQ_F2P` bit, GPIO board binding (by external port name, else guessed from direction and width, reported as a note) | verified | `sim-api::xsa_import::the_gpio_design_maps_the_user_ip_and_places_the_gpio_model_next_to_it`, `zynqsim::xsa::a_vendor_ip_handoff_runs_the_real_firmware_on_the_models` (the real XSA: `leds_4bits`, `sws_4bits`, `btns_4bits` recognised) |
| Snapshot, restore, reset and replay with the models | verified | `mcp::vendor_ip` (restore returns the trace digest, replay deterministic); models are rebuilt and the journal re-executed |
| AXI DMA, direct register mode: `MM2S_*`/`S2MM_*` registers, `DMACR`/`DMASR` bits, a `LENGTH` write starts a transfer that reads/writes PS memory through the HP port, `STREAM_LOOPBACK` (default, set by the importer when the design loops M_AXIS_MM2S to S_AXIS_S2MM), IOC/Dly/Err interrupts with threshold and delay, error bits, soft reset, halted/idle | verified (model) / VERIFY (register facts) | `vendor-ip::dma::tests::{reset_values_and_config, run_stop_halted_and_idle, simple_loopback_copy_with_interrupts, status_bits_follow_the_interrupt_enables, s2mm_waits_for_the_stream_and_a_short_packet_sets_length, long_packet_continues_in_the_next_transfer, mm2s_without_loopback_leaves_packets_for_the_host, decode_slave_and_internal_errors_halt_the_channel, length_is_limited_by_the_length_width_and_requires_run, interrupt_threshold_and_delay, registers_of_the_wrong_mode_read_zero}`, `sim-api::vendor_ip2::the_dma_copies_buffers_through_the_stream_loopback_and_interrupts` (`examples/dma-loopback`, the Vivado handoff `zs_dma_loopback.xsa`). VERIFY: `DMACR`/`DMASR` reset values (`Halted`), `Idle` timing, zero-length `LENGTH` handling, S2MM `LENGTH` read-back. Limits: multi-channel and micro DMA rejected; keyhole, cyclic BDs, 64-bit addresses, AXI bursts and 4 KiB crossing not modelled |
| AXI DMA scatter-gather (`C_INCLUDE_SG = 1`): linear descriptor chain (`NXTDESC`, `BUFFER_ADDRESS`, `CONTROL` with SOF/EOF, `STATUS` with `Cmplt` and the byte count), tail-pointer protocol, a packet over two descriptors each way | verified (model) / VERIFY (descriptor status bits) | `vendor-ip::dma::tests::{scatter_gather_two_descriptor_packet_loopback, sg_descriptor_errors}`, `sim-api::vendor_ip2::scatter_gather_descriptors_move_a_packet_through_the_loopback` (`examples/dma-sg`, firmware `firmware/dma_sg` checks the status words). **Limits:** linear chains only; no cyclic descriptors, no status/control streams, no SG Vivado handoff exists (the example is hand-written); not checked against silicon or a Xilinx simulation |
| AXI DMA determinism: the DMA session gives one trace | verified | `sim-api::vendor_ip2::the_dma_session_is_deterministic` |
| AXI IIC, dynamic controller master: `TX_FIFO` words with START/STOP, address byte, read count word, `RX_FIFO`, repeated START, NACK on an unknown address (error bit, STOP, rest of FIFO discarded), interrupts (`ISR` toggle on write, `RX_FIFO_PIRQ`, TX half empty). Bit-banged on a Pmod port (`"axi_iic_0.IIC" = "board.pmod_ja"`), so the ADT7420 (TMP2) device model answers as it does for RTL designs | verified (model) / VERIFY (word layout, NACK reaction) | `vendor-ip::iic::tests::{reset_state_and_registers, isr_toggles_on_write, dynamic_write_then_repeated_start_read, nack_on_an_unknown_address_sets_the_error_and_stops, no_bus_means_every_address_is_nacked, words_wait_for_enable_and_the_fifo_can_be_flushed, rx_threshold_interrupt_and_tx_half_empty}`, `sim-api::vendor_ip2::{the_iic_master_reads_the_tmp2_model_and_the_bram_holds_data, the_sensor_value_the_host_sets_is_what_the_iic_master_reads}` (`examples/iic-bram-demo`). **Limits:** standard controller mode (`MSMS`/`TX` stored, no transfer), slave mode and general call, 10-bit addresses, arbitration (`ISR` bit 0 never set), clock stretching, glitch filter, timing registers and `GPO` stored only |
| AXI BRAM Controller with its block memory as one window: 32-bit reads and writes with byte strobes, little-endian, zero at power-on; host preload and inspect | verified (model) / VERIFY (narrow write with zero strobe) | `vendor-ip::bram::tests::{word_and_byte_access, host_preload_and_inspect, configuration_errors}`, `sim-api::vendor_ip2::the_iic_master_reads_the_tmp2_model_and_the_bram_holds_data` (`examples/iic-bram-demo`). **Limits:** ECC registers (`C_ECC = 1` refused), the block memory's second port, `.coe` init, read latency and AXI4 bursts |
| AXI Interrupt Controller: `ISR`, `IPR`, `IER`, `IAR`, `SIE`, `CIE`, `IVR`, `MER`; edge and level inputs; `irq = ME & pending` to `IRQ_F2P`; software interrupts until `HIE` | verified (model) / VERIFY (`HIE` latch, `IVR` reset value, edge-output level) | `vendor-ip::intc::tests::{reset_state, edge_input_latches_until_acknowledged, disabled_inputs_are_pending_in_isr_but_not_in_ipr, master_enable_gates_the_output, level_inputs_follow_the_line, software_interrupts_until_hie_is_set, fast_interrupt_registers_exist_only_with_c_has_fast, inputs_beyond_the_width_and_vector_registers}`, `sim-api::vendor_ip2::the_interrupt_controller_serves_the_timer_and_the_gpio_through_one_irq_line` (`examples/intc-demo`: software interrupt, three timer interrupts, GPIO interrupt). **Limits:** fast interrupt mode (`IMR`, `ILR`, `IVAR` stored only), cascade, processor vector output |
| XSA import of the four models and the passive classes: DMA with and without FIFO plumbing (stream connection detected), INTC behind an `xlconcat`, IIC pins on a Pmod, BRAM controller with its `blk_mem_gen` | verified against the Vivado 2022.1 handoffs `fixtures/xsa/zs_dma_loopback.xsa`, `zs_dma_fifo.xsa`, `zs_intc.xsa`, `zs_iic_bram.xsa` (and the hand-written `zs_math_vdma.hwh`) | `xsa-import::ip2::{the_dma_fixture_has_its_hp_window_stream_loopback_and_two_interrupts, the_fifo_variant_puts_stream_plumbing_between_the_two_stream_interfaces, the_intc_fixture_traces_interrupt_sources_through_the_concat_to_controller_inputs, the_iic_bram_fixture_has_the_controller_and_its_block_memory, plumbing_clock_wizards_and_block_memory_are_passive_ip}` |
| Passive IP: interconnects, reset generators, concatenations, clock wizards and block memories with no reachable register are classed as infrastructure, reported as "Passive IP", mapped to nothing, and do not stop a run; an IP with an address window of its own that is not passive stays unmodelled | verified | `xsa-import::ip2::plumbing_clock_wizards_and_block_memory_are_passive_ip`, `sim-api::xsa_import` (unmodelled-IP tests) |
| Other Xilinx IP (VDMA, Quad SPI, UART 16550, ...) | stub | still `[xsa.unmodelled]` and stop the run; `zynqsim::xsa::an_unmodelled_ip_is_reported_and_stops_the_run` (the hand-written `fixtures/xsa/zs_math_vdma.hwh` has an AXI VDMA) |
| Cycle-accurate AXI timing of the models | not-planned | register level only; no AXI protocol checking for them |

Gate on this branch: `cargo xtask ci` exited 0 on 2026-10-16 with 457 tests (421 + 36 new), none
failed or ignored, no `SKIPPED:` line (QEMU, XSim and the DPI library present).

The export-simulation file-list flow and `BOOT.BIN` boot of milestone P3 are other work and are
not covered here.

## BOOT.BIN boot (milestone P3)

`[boot] mode = "boot_bin"` (alias `bootbin`) + `file = "BOOT.BIN"`, `zynqsim boot-info BOOT.BIN
[--json]`, `zynqsim init --boot BOOT.BIN --out DIR`, MCP `boot_inspect` (ADR-018). The fixture
`fixtures/boot/BOOT.BIN` is **real `bootgen` 2022.1 output** (`-arch zynq`, `boot.bif`): a stub FSBL
ELF (16 bytes of code; no FSBL is built on this machine), a **dummy** `.bit` container (not a real
design) and `rust_hello.elf` as the application, with a 4-pair register-init file and `[checksum=md5]`.
Field offsets were cross-checked with `bootgen -read`; other Vivado/bootgen versions and images with
`[load=]`/`[offset=]`/`[alignment=]`/flash partitions are `VERIFY:`.
Gate on this branch: `cargo xtask ci` exited 0 on 2026-10-16 with 446 tests (421 + 25 new), 0
failed, 0 skipped (`SKIPPED:` count 0 in the output). A first run failed one unrelated timing-sensitive
test (`mcp::scenario::board_demo_through_mcp`, shell not ready within 6 ms while other agents loaded
the machine); it passed alone and in the second full run.

| Feature | Status | Evidence / note |
|---|---|---|
| Parser (`boot` crate, pure Rust, no unsafe): boot header incl. checksum, register-init table, image header table, image headers (names), partition headers (load/exec address, length, data offset, attributes, destination PS/PL, checksum type, header checksum), MD5 partition checksums, FSBL / bitstream / application classification, payload extraction | verified | `boot::tests::a_real_bootgen_image_is_read_completely` (values equal `bootgen -read`), `application_payloads_equal_the_elf_segments_bootgen_was_given`, `boot::md5::tests::rfc1321_test_suite` |
| Corruption is reported precisely (field, file offset, stored vs expected): boot-header checksum, partition-header checksum, partition MD5, truncation at every level, wrong width-detect word / signature, an ELF handed in by mistake; never panics | verified | `boot::tests::{a_damaged_boot_header_is_reported_with_its_checksum, a_damaged_partition_header_names_the_partition, a_damaged_partition_payload_fails_its_md5, truncation_is_reported_at_every_level, a_file_that_is_not_a_boot_image_says_so, damaged_images_never_panic}` (the last: 3000 seeded mutations) |
| Encrypted (boot-header key source, partition encrypted length) and RSA-authenticated (certificate offsets) images are refused with a message; no decryption, no key store, no signature check | verified (refusal) / not-planned (support) | `boot::tests::{encrypted_images_are_refused_with_a_clear_message, authenticated_images_are_refused}`, `zynqsim::bootbin::boot_info_names_what_is_wrong_and_exits_2`. The key-source values `0xA5C3C5A3` / `0x3A5C3C5A` come from memory of UG585 (`VERIFY:`): no encrypted image could be produced here |
| Boot: the application partitions are written back as one ELF in the run directory (one `PT_LOAD` per partition at its load address, entry = first application partition with a non-zero execution address) and loaded by QEMU like `[cpu0] elf`; UART output equals the ELF boot of the same firmware | verified | `zynqsim::bootbin::a_bootbin_project_prints_what_the_elf_prints` (real QEMU, no skip: `hello from Rust ... RESULT: ALL PASS` both ways), `zynqsim-mcp::bootbin::a_bootbin_project_boots_through_mcp`, `boot::tests::the_load_plan_becomes_an_elf_with_the_same_segments` |
| Project validation: `boot.file` required / missing / damaged (finding text carries field and offset), `cpu0.elf` ignored with a warning, no `cpu0.elf` needed, `run` stops with exit 1 | verified | `zynqsim::bootbin::project_validation_reports_each_boot_problem_with_its_field` |
| `boot-info` (text, `--json`; exit 2 for an image ZynqSim cannot run), `init --boot` (copies the image, writes and validates the project, never overwrites, refuses damaged images), MCP `boot_inspect` | verified | `zynqsim::bootbin::{boot_info_lists_the_partitions_of_a_real_image, init_boot_writes_a_valid_project_and_never_overwrites}`, `zynqsim-mcp::bootbin::boot_inspect_lists_partitions_and_findings`, `sim-api::bootbin::tests::*`, `mcp::tools::tests::*` |
| PL bitstream partition | recognised and reported only (**not-planned** to simulate) | its destination PL and the `0xAA995566` sync word are checked; the PL is the project's RTL/native models (`init --boot` writes `pl = "demo"` and a comment on how to add `[rtl]`) |
| FSBL execution, `ps7_init` replay | **not-planned** (stub) | the FSBL partition is never run; its register-init pairs are listed (`register_init_applied: false`) and **not applied**: PS clocks, MIO, DDR and peripheral enables change nothing (QEMU's machine has everything configured). Prompt §12's "run the FSBL as software" is not done |
| Boot-ROM behaviour: timing, QSPI/SD image load, boot-mode pins, FSBL handoff logic, a second application image started on core 1 | not-planned | core 0 starts at the first application partition's execution address (which partition the real FSBL hands off to is FSBL logic, `VERIFY:`); further application images are loaded into RAM but not started; symbols are absent (bootgen strips the ELF), so breakpoints need addresses |
| ZynqMP / Versal images | not-planned | only the Zynq-7000 layout is read; a ZynqMP image shares the magic words and is not told apart reliably |

## Vivado simulation exports and netlists (milestone P3)

`zynqsim init --export-sim PATH --out DIR [--elf FILE] [--module M]`, MCP
`project_create_from_export`, and `[rtl] export_dir = "..."` / `filelist = "..."` (ADR-017,
per-backend subset: `docs/RTL_SUBSET.md`). The exports are **real Vivado 2022.1 output**
(`fixtures/export/make_export.tcl`: `export_simulation`, `write_verilog -mode funcsim`,
`launch_simulation -mode post-synthesis -type functional -scripts_only`, on `my_math_ip` with its
testbench; ~150 KB). The VHDL design file in `fixtures/export/vhdl` is hand-written.

| Feature | Status | Evidence / note |
|---|---|---|
| Reader: folder, `.prj` (`verilog`/`sv`/`vhdl`/`vhdl2008 LIB [-i] [-d] [-L] "file"`, continuations, comments, quoted paths, relative `..`, `nosort`), `.f` (`+incdir+`, `+define+`, `-f`, `-work`, `-sv`, comments), `elab.opt` (`-L`, tops, `-timescale`, `-generic_top`), `xsim.ini` mappings, `.sh`/`.bat` scripts (`xvlog`/`xvhdl`/`xelab` lines, script variables), a single `.v/.sv/.vhd`; `$VAR`/`%VAR%` expansion | verified | `xsim-export::tests` (14), including the two real Vivado exports (`the_real_vivado_2022_1_behavioural_export_is_read`, `the_real_post_synthesis_export_names_the_netlist_and_unisim`) |
| Unsupported or unusable input is reported with **file, line and construct** (unknown option or directive: warning; `-y`/`-sourcelibdir`, `-sv_lib`: warning; missing source, unresolved variable, unreadable list: error); several flows in one folder are refused with the candidates | verified | `xsim-export::tests::{unknown_options_and_missing_files_name_file_line_and_construct, dpi_libraries_and_search_directories_..., environment_variables_expand_and_an_unknown_one_is_an_error_with_its_line, a_folder_with_two_flows_asks_for_one}`, `zynqsim::export_sim::an_unreadable_export_names_file_line_and_construct`, `mcp::export::an_export_that_cannot_be_read_...` |
| XSim adapter builds an export prj-style: one `.prj` line per file with its library, `-i`/`-d`, `xvlog -prj` then `xvhdl -prj`, `xelab -L <libraries> -L unisims_ver ... LIB.top LIB.glbl`, `glbl` from the netlist/sources or Vivado's copy, `-timescale`, `-generic_top` quoted, `--relax` | verified | command-line construction `backend-rtl-xsim::export::tests` (5); real XSim: `backend-rtl-xsim::export::{the_rtl_export_and_the_funcsim_netlist_give_the_same_answers, prj_includes_and_defines_reach_xvlog_and_the_build_is_cached}` (a testbench that fails to compile without the include directory and the define builds) |
| **Post-synthesis functional netlist** (UNISIM `LUT`/`FDRE`, `-L unisims_ver`, `glbl`) behaves like the RTL: same 7 writes, 14 reads, same answers | verified | `backend-rtl-xsim::export::the_rtl_export_and_the_funcsim_netlist_give_the_same_answers` (Vivado 2023.1 on this machine ran the 2022.1-generated export) |
| **End to end**: the real firmware (`math_test.elf`) on QEMU against the RTL export, the post-synthesis netlist flow and a bare funcsim netlist with a generated testbench: `RESULT: ALL PASS`, 21 bus accesses, 329 trace records and **one trace digest** (`fnv1a64:c801168503080d41`, also the digest of the native-model and `init --ip` routes) | verified (Windows, needs QEMU + Vivado + DPI library) | `zynqsim::export_sim::rtl_export_and_funcsim_netlist_pass_with_the_real_firmware_and_agree` |
| VHDL design file (`xvhdl`) mixed with the Verilog testbench; `.vhd` in `rtl.files` | verified for one hand-written design / implemented-untested for Vivado-generated VHDL IP | `backend-rtl-xsim::export::a_vhdl_design_file_is_compiled_with_xvhdl_and_answers_like_the_verilog` |
| Snapshot build cache keyed by the project files, elaboration arguments and every source and header | verified | `backend-rtl-xsim::export::prj_includes_and_defines_reach_xvlog_and_the_build_is_cached` (hit on the second start, miss after a header changes) |
| Glob patterns in `rtl.files` (`*`, `?`, `[a-z]`, `[!x]`, `**`; sorted per pattern, relative to the project, absolute allowed, empty match is an error naming the pattern) | verified | `sim-api::exportsim::tests::*` (4), `zynqsim::export_sim::a_project_with_globs_and_an_export_validates_and_says_what_is_missing` |
| `[rtl] export_dir` / `filelist` validated (`export_dir` and `filelist` are alternatives, diagnostics carry the list's `file:line`), `top` taken from the export's single top | verified | `sim-api::exportsim::tests::export_and_filelist_are_alternatives`, `zynqsim::export_sim::*` |
| `init --export-sim` (export's own testbench, or generated for the AXI4-Lite slave in it; `export-report.md`; refuses to overwrite) | verified | `zynqsim::export_sim::*`, `mcp::export::*` |
| Compiled Vivado IP libraries (`xsim.ini` mappings of `blk_mem_gen...`, `-lib_map_path`) passed as `-L name=dir` | implemented-untested | unit-tested command line only (`backend-rtl-xsim::export::tests::a_compiled_ip_library_with_a_directory_is_mapped_...`); no IP library was compiled or run here |
| Encrypted IP / `secureip`, `xpm` and other Xilinx simulation libraries | implemented-untested | named in `-L` and left to Vivado's own `xsim.ini`; no design using them was run |
| Timing / SDF simulation (`simprims_ver`, `write_verilog -mode timesim`) | stub | not read specially; would compile without SDF and say nothing |
| The export's own DPI libraries (`-sv_lib`), library search (`-y`, `-v`), simulator scripts of other simulators (`.do`, Questa/VCS) | stub | reported as unsupported, never executed |
| Verilator / Icarus on an export | stub | no such backend (P4) |
| GUI import of an export | stub | the project window lists `files` only |

## P3 exit demonstration: a Vivado design and its BOOT.BIN (milestone P3)

`examples/vivado-handoff`, `fixtures/handoff`, `firmware/handoff_demo`, ADR-019. **One real Vivado
2022.1 block design** (`fixtures/handoff/make_handoff.tcl`: Zynq PS7, the user RTL `my_math_ip` as
a module reference, AXI GPIO on the LEDs, AXI Timer, AXI UART Lite) written as `design.xsa`
(`write_hw_platform -fixed`) and as `export/xsim` (`export_simulation`) by the same project, a Rust
`no_std` firmware for it, and a `BOOT.BIN` that Vitis `bootgen` packed around that firmware.
`zynqsim init --xsa design.xsa --rtl ... --boot-bin BOOT.BIN` (or `--export-sim export/xsim`
instead of `--rtl`) makes one project with the XSA-derived PL and `[boot] mode = "boot_bin"`;
`zynqsim run` boots the firmware from the image.

| Feature | Status | Evidence / note |
|---|---|---|
| `init --boot` / `--boot-bin` next to `--xsa` or `--export-sim` (and MCP `boot_bin`): image validated, copied to `BOOT.BIN`, `[boot] mode = "boot_bin"`, no `[cpu0]` (`--elf` ignored with a warning), report lists what a BOOT.BIN boot does not do; damaged image creates nothing; `init --boot` alone and `init --xsa --elf` unchanged | verified | `sim-api::xsa_import::a_boot_bin_next_to_an_xsa_gives_one_project_with_both`, `zynqsim::handoff::{one_init_gives_the_xsa_mapping_and_the_bootbin_boot, the_export_variant_references_the_export_and_keeps_the_xilinx_ip, init_boot_and_init_xsa_keep_working_separately}`, `zynqsim-mcp::handoff::the_handoff_project_is_created_with_the_bootbin_through_mcp` |
| `init --xsa` with `--export-sim` (MCP `export_path`): the XSA maps the Xilinx IP, the user RTL and testbench come from the export (`[rtl] export_dir`, the export's own testbench or a generated one); refused with `--rtl`/`--filelist` | verified (one design) | same tests; the export of this design holds no Xilinx IP, so only this combination runs the firmware |
| **End to end, XSA variant**: the firmware boots FROM the BOOT.BIN on real QEMU, seven math vectors pass on the user RTL in XSim, the GPIO drives LED0 and LED3, the timer takes 3 interrupts, the UART Lite sends `HANDOFF UART OK` (host `uart_read` port 2), `RESULT: ALL PASS`; 675 trace records, 186 bus accesses | verified (Windows, QEMU + Vivado XSim + DPI library) | `zynqsim::handoff::the_exit_design_runs_end_to_end_from_the_bootbin_in_both_variants` |
| **Export variant**: PL from the `export_simulation` folder gives the identical UART text and trace digest (`fnv1a64:f70fca029ca96cf6`) | verified (same design; the same generated testbench in both, see ADR-019) | same test |
| Determinism: three runs give one trace digest and text; the committed `examples/vivado-handoff` project gives the same digest | verified | same test |
| BOOT.BIN boot equals the ELF boot of the same firmware: same UART text, trace length and bus accesses; the digests differ only by the file name in the `qemu start elf=...` note (checked record by record) | verified | same test |
| MCP route: `project_create_from_xsa` (with `boot_bin`, and with `export_path`) + `project_open` + `sim_run`; both variants give one trace digest | verified | `zynqsim-mcp::handoff::the_exit_design_runs_end_to_end_through_mcp` |
| Vivado 2022.1 batch flow producing the XSA and the export from one project, run twice on this machine | verified (this machine) | `fixtures/handoff/make_handoff.tcl`; other Vivado versions and machines `VERIFY:` |
| **FSBL**: the partition in the image is a **stub** (four `b .` instructions); it is never executed; no FSBL was built or run | stub / not-planned | ADR-018, ADR-019 |
| **Bitstream**: the partition is a **placeholder** `.bit` container; no bitstream is simulated or even compared with the RTL | not-planned | the PL is the user RTL in XSim plus the vendor-ip models |
| **`ps7_init`** (the XSA's files and the image's register-init table) | not applied | the firmware does the SLCR PL bring-up itself (`rt::pl_bringup`); PS clocks, MIO and peripheral enables change nothing |
| Xilinx IP behaviour vs the real IP; anything on a board; other designs, Vivado versions, SmartConnect, several user slaves | `VERIFY:` / not covered | register-level models from the product guides (see *Xilinx IP models*); this is one design |

**P3 exit criterion** (prompt §15: "a Vivado-exported design and its `BOOT.BIN` run end to end"):
demonstrated for this one design in the restricted sense above (Vivado-written XSA and
`export_simulation`, `bootgen`-written image, PS in QEMU, user RTL in XSim, Xilinx IP as models,
deterministic result). It is **not** the real Vivado/FSBL flow: stub FSBL not executed, no
bitstream simulated, `ps7_init` not applied, IP models unverified against hardware. ADR-019 has
the full statement.

## Universality corpus (milestone P3, Vivado XSim)

Table with licence, commit and result per design: `docs/CORPUS.md`. Decision: `docs/adr/020-corpus.md`.
Every status below is verified by the named test in `backend-rtl-xsim::corpus`; `unsupported`
means a construct the backend does not run, listed with the diagnostic the test asserts.

| Design or construct | Status | Evidence / note |
|---|---|---|
| secworks sha256, sha1, md5, aes, chacha (BSD-2-Clause) | verified | `corpus_secworks_sha256`, `corpus_secworks_sha1`, `corpus_secworks_md5`, `corpus_secworks_aes`, `corpus_secworks_chacha` |
| forencich lfsr, uart, axis (MIT) | verified | `corpus_forencich_lfsr`, `corpus_forencich_uart`, `corpus_forencich_axis` |
| picorv32 (ISC), serv (ISC), uriscv (Apache-2.0) CPUs | verified | `corpus_picorv32`, `corpus_serv`, `corpus_uriscv` |
| pulp common_cells (SHL-0.51): package, `parameter type`, `always_comb`, assertion macros | verified | `corpus_pulp_common_cells` |
| dpretet async FIFO (MIT), dual clock | verified | `corpus_dpretet_async_fifo` |
| Vivado post-synthesis funcsim netlist in this repository | verified | `corpus_vivado_funcsim_netlist` |
| SystemVerilog `bind`, `config` block, `covergroup`, class `rand`/`constraint`, concurrent assertions, `virtual` interface and modport, UVM 1.2 with `-L uvm` | verified (run) | `probe_bind`, `probe_config_block`, `probe_covergroup`, `probe_constrained_random`, `probe_concurrent_assertion`, `probe_virtual_modport`, `probe_uvm_with_library` |
| Missing include, define, UNISIM library, module, UVM library, DPI library; IEEE-1735 envelope; VHDL-2008 under VHDL-93; `nettype`; `interconnect`; VPI system task; syntax errors; undeclared identifier; `$readmemh` of a missing file; `$fatal` | unsupported (the failure is the verified behaviour) | `negative_*` (18), each with file:line and construct asserted |
| Third-party designs that do not pass | none in the corpus | `corpus_*` all pass |

Known limits: the corpus runs each design once with its own testbench or the stated check; no
timing (SDF) simulation; UVM is only probed with the library, not a full UVM testbench.

## Unmodified Xilinx BSP applications (milestone P4, success criterion 2)

`fixtures/bsp` (`SOURCE.md`, `build_bsp.tcl`, `zs_ps_all.xsa`, 24 ELFs), `examples/bsp-hello`,
`examples/bsp-peripherals`, `crates/cli/tests/bsp_apps.rs`, ADR-023, results and every deviation in
`docs/conformance/bsp-apps.md`. Xilinx's own standalone BSP (Vitis 2022.1, `-Os`) and its own
application templates and driver examples, **built without any C of ours** and run on the Xilinx
QEMU (PS only). "Passes" means the application printed its own success line; nothing was compared
with a board. Not run behind an FSBL/`ps7_init`.

| Feature | Status | Evidence / note |
|---|---|---|
| Vitis 2022.1 `xsct` builds a standalone BSP and the Hello World / Peripheral Tests templates and Xilinx driver examples from the command line, offline, from a PS-only XSA | verified (this machine) | `fixtures/bsp/build_bsp.tcl` run twice with different XSAs; `bsp_apps::every_bsp_elf_is_small_and_documented` checks the committed ELFs (24, each < 300 KB, each in `SOURCE.md`) |
| Hello World template: `xil_printf` over xuartps (UART1) | verified | `bsp_apps::bsp_hello_world_runs_unmodified_and_deterministically` (two runs, one digest). `bsp-hello` and `bsp-peripherals` are also in the `determinism.rs` table; five separate processes each gave one digest by hand (the full determinism test was not re-run for this entry) |
| xuartps polled / interrupt / selftest examples (local loopback mode) | verified | `bsp_apps::bsp_driver_examples_match_the_recorded_results` |
| GIC examples (`xscugic_example`, low level / software-generated interrupts) | verified | same test |
| SCU timer polled and interrupt examples | verified (the timer runs at 100 MHz, see below) | same test |
| TTC0 examples: tapp, low level, interrupt with PWM (about 5 s of simulated time for the last) | verified | same test (`bsp_ttcps_intr` uses `quantum_ns = 100000`) |
| PL330 DMA example with done interrupt and data compare | verified | same test (`bsp_dmaps_intr`) |
| QSPI and I2C self tests | verified (register self tests only; no flash or I2C device traffic) | same test |
| SD raw read/write example (SDHCI, ADMA, 16 sectors at the 100 MiB mark) | verified on a 128 MiB card; fails on the default 8 MiB `sd_dir` card because of the sector it addresses | same test (`bsp_sdps_raw`, `sd_size_mb = 128`) |
| GEM0 DMA/interrupt/PHY-loopback example (`xemacps_example_intr_dma`) | verified, slow | `bsp_apps::bsp_emacps_intr_dma_prints_its_success_line` (`#[ignore]`, about 13 min, 40 s of simulated time). lwIP applications: **not built** |
| Peripheral Tests template: GIC, PL330 DMA, I2C, QSPI and SCU timer tests PASSED; CAN, CAN interrupt and devcfg tests FAILED; the SCU watchdog interrupt test never returns | verified (as the recorded behaviour of this machine, not as hardware behaviour) | `bsp_apps::bsp_peripheral_tests_report_what_the_model_implements` |
| **DEVIATION D1**: PS GPIO, SPI0/1, CAN0/1, USB0/1, device configuration, DDR controller, OCM controller, eFuse, L2C (PL310) and the SCU watchdog are not in the QEMU machine (reads 0, writes dropped, no abort). `xgpiops` (2 examples), `xspips`, `xcanps`, `xdevcfg` examples print their failure; `xscuwdt` and the Peripheral Tests' watchdog test spin forever | verified as observed behaviour; the criterion-2 applications for GPIO, SPI, CAN, USB **fail** | `bsp_apps` `DEVIATIONS` table, asserted; probe + `info mtree` in the conformance document (the probe is not committed) |
| **DEVIATION D2**: XADC returns zero data (temperature -273.149 C, rails 0.000 V) although the driver runs to its success line | verified | `bsp_apps` `DEVIATIONS` (`bsp_xadcps_polled`) |
| **DEVIATION D3**: the A9 global and private timers tick at 100 MHz (measured: 1,000,094 ticks in 10 ms); the BSP's `COUNTS_PER_SECOND` is 333,333,343, so `sleep()`/`usleep()`/SCU timer deadlines are 3.3x too long in simulated time | measured with a throw-away probe, **no test** | conformance document, D3 |
| lwIP/FatFs applications, flash and EEPROM examples, FSBL/`ps7_init`, multicore, any comparison with hardware | not built / not covered | ADR-023, conformance document |

**Criterion 2 is partly shown, not met**: UART, GIC, timers (SCU, TTC), QSPI (self test), SD
(raw), GEM (DMA example), I2C (self test), XADC (driver runs, data wrong) and PL330 DMA applications
run unmodified; GPIO, SPI, CAN, USB, devcfg and the watchdog fail because the machine lacks them.

## Backends

| Backend | Status |
|---|---|
| `demo` (PS and PL) | verified, fidelity `demo` |
| `qemu` (PS) | verified, F0 |
| `native` (PL) | verified, transaction level |
| `xsim` (PL) | verified, Windows + Vivado (build cache) |
| `verilator`, `icarus` (PL) | stub, P4 |
| `native` PS / RTL engine | stub, P5 / P6 (optional) |

## Robustness fuzzing (spec §14.8, §14.10)

Seeded, deterministic tests run in `cargo test` on stable Rust (no nightly, no new dependency).
They show that inputs do not panic or hang. They do not show that the answers are right.

| Feature | Status | Evidence / note |
|---|---|---|
| Project parser and validator: 2000 seeded mutations per example (27) and handwritten project (5): byte flips, truncation, garbage tokens, duplicated and deleted lines, huge numbers, unicode, NUL; every answer is `Ok` or an `invalid_argument` error with a message; a sample also goes through `Project::load` | verified | `sim-api::fuzz_project::mutated_projects_never_panic_and_are_answered_structurally` |
| MCP: every tool from `tools/list` (47) gets 200 random argument objects (wrong types, missing and extra keys, huge strings, negative and nested values, nulls), without a project and with `examples/demo` open. Each answer is a JSON-RPC result with boolean `isError` and a known error code, or a JSON-RPC error; 5 s per call (60 s for rendering tools: `board_screenshot` at scale 4 takes about 8 s in the debug build) | verified | `mcp::fuzz_requests::every_tool_survives_random_arguments_without_a_project`, `..._with_the_demo_open` |
| MCP raw transport: invalid JSON, non-object lines, batch arrays, wrong `method`, huge and odd ids, mutated request lines; every output line is JSON-RPC, and invalid JSON gets `-32700` | verified | `mcp::fuzz_requests::raw_json_rpc_lines_are_always_answered_or_ignored` |
| Remote-port decoder: 20000 cases, half random bytes and half mutations of valid hello, read/write (ext and old layout), read response, interrupt, sync and nop packets, with hot offset and length words; no panic, and decoded data never longer than its input | verified | `interop::fuzz_packets::random_and_mutated_packets_never_panic_or_over_allocate` |
| Session length cap: a declared payload above 16 MiB (and `0x7fffffff`, `u32::MAX`) is refused before the payload is allocated; a short payload then a closed peer is `Closed` | verified | `interop::fuzz_packets::a_declared_length_above_the_cap_is_refused_before_allocating`, `..._closed_peer` |
| `cargo-fuzz` targets `project_parse`, `mcp_request`, `interop_packet` in `fuzz/` (outside the workspace) | implemented-untested | type-checked against a stub `libfuzzer-sys`; never run under libFuzzer (needs Linux and nightly). Run instructions in `fuzz/README.md` |

## Build, tooling and process

| Item | Status | Note |
|---|---|---|
| `cargo fmt --check`, `clippy --all-targets -D warnings`, `cargo test --workspace` | verified locally | Windows, Rust stable GNU toolchain (MinGW from Qt). Harmless `.drectve` linker warning |
| `cargo xtask ci` (fmt, clippy, freestanding DPI clippy + build, tests, release build) | verified locally | exited 0 on 2026-10-15 (after the Pmod devices), 239 tests, 0 skipped, QEMU and XSim tests included |
| `cargo xtask dpi`, `third-party`, `golden`, `release` | verified | each run |
| `THIRD_PARTY.md` | verified | generated from `Cargo.lock`; licences as declared, not audited. Re-generate after dependency changes |
| Linux and macOS | verified (build, lints, tests without Xilinx tools) | GitHub Actions run 38731290203 on `ubuntu-latest`, `windows-latest` and `macos-latest`: fmt, clippy `-D warnings` (incl. the freestanding DPI crate), `cargo test --workspace --no-fail-fast` and the release build all pass. Hosted runners have no QEMU or Vivado, so every test that needs them **skips**; those are verified only on the Windows development machine. The first hosted run found three real problems (workflow in the wrong directory, `freestanding` feature broken off Windows, a CRLF-dependent test) and the Linux WSL run earlier found an ordering bug in the XSim adapter | **CI minutes**: the 2000-minute free quota ran out (macOS bills 10x, Windows 2x, and every push of the stacked pull requests ran all three). Since then pull requests run Linux only, the three-OS matrix runs on pushes to main and on demand, superseded runs are cancelled and documentation-only changes skip CI; the last three-OS results are those of PRs #5-#14. Pushes made after the quota ran out were not verified by GitHub CI, only by the local `cargo xtask ci`.
| `forbid(unsafe_code)` workspace lint | verified | `xsim-dpi` is the one exemption (FFI boundary, `win.rs` only) |
| Building and testing without QEMU/Vivado installed | implemented-untested | tests that need them skip with a message; not run on a machine without them |
| Host-written UART input arrives at a wall-clock dependent virtual time | open limitation (tests adapted) | `board::shell_leds_switches_and_buttons` and `mcp::scenario::board_demo_through_mcp` failed in about 1 run of 3 with a 5-6 ms virtual-time window for the firmware's answer; the answer was only late, not lost (10 of 10 runs pass with 50 ms windows / waiting in 6 ms steps). Cause: QEMU reads the TCP UART in its main loop, which runs only between lock-step holds, so when the byte is consumed is not a function of virtual time. Consequence for users: a host-driven shell is not trace-deterministic (`board-demo` in the determinism test). Whether scripted `--at uartN:` input is deterministic was not tested. Earlier note about "root cause not found" is superseded |
| Git repository, stacked pull requests, CI at the repository root | verified | GitHub Actions on the pull requests (3 OS) |

## Packaging and distribution (milestone P4)

| Feature | Status | Evidence / note |
|---|---|---|
| `cargo xtask dist` on Windows 11 x86_64: release binaries, `lib/libxsim_dpi.a`, `examples/`, the fixtures they refer to, `boards/`, `docs/` (md and adr), README, THIRD_PARTY, LICENSE, `INSTALL.txt`, a `.zip` and `SHA256SUMS` | verified | run 2026-10-16: archive 13,974,744 bytes, 89 fixture files for 63 references. The command the release workflow runs: `cargo xtask dist` |
| Clean-directory check of the archive: unpack into an empty temp directory, PATH holds only System32, then `zynqsim --version`, `zynqsim run --project examples/demo/project.toml --for 1ms --json` (`now_ps` 1000000000) and the `zynqsim-mcp` initialize reply | verified (Windows) | `xtask::clean_unpack::the_archive_unpacks_and_runs_from_a_clean_directory` (`cargo test -p xtask -- --ignored` after `cargo xtask dist`); the same four checks end `cargo xtask dist` |
| DPI library found next to the executable in `lib/` (the unpacked archive), before `target/dpi` | verified | `sim-api::tools::tests::the_dpi_library_is_found_in_an_unpacked_release_archive`. End to end with XSim not run (needs Xilinx QEMU and Vivado) |
| Built-in boards are compiled into the binaries (`include_str!`), so `boards/` in the archive is for custom boards and reference | verified | `board-zybo::def` (`BoardDef::builtin`), `sim-api::boards` |
| Fixtures in the archive are those the examples mention (`fixture_refs_in`), plus `fixtures/README.md` | verified | `xtask::dist::tests::fixture_mentions_in_project_files_and_comments_are_found`. A mention in a comment also copies its file or directory |
| `cargo xtask dist` on Linux and macOS (`.tar.gz`, `sha256sum` or `shasum -a 256`) | implemented-untested | not run here; the archive and checksum code paths differ from Windows only in the archiver and hash tool |
| `.github/workflows/release.yml`: tag `v*`, `cargo xtask dist` on ubuntu, windows and macos, one GitHub release with the archives and a combined `SHA256SUMS` | implemented-untested | never run on GitHub. Windows uses the GNU toolchain and MinGW gcc (the DPI archive is a GNU `.a`) |
| `.github/workflows/ci.yml` runs `dist` | not-planned | left alone: on Windows the default MSVC toolchain cannot build `libxsim_dpi.a`, so `dist` would fail the Windows job, and the DPI build adds minutes to every push |
| Installation documented: prerequisites per OS, install from archive, build from source, Xilinx variables, what needs what, troubleshooting | implemented-untested | `docs/INSTALL.md`, linked from `README.md`. Not followed step by step on a clean machine; the archive steps are what the clean-directory check runs on Windows; the Linux and macOS commands are not run |
| Every crate inherits `version`, `license` and `edition` from the workspace; root `LICENSE` (Apache-2.0) is in the archive | verified | `Cargo.toml` `[workspace.package]`. Gap: the workspace declares `Apache-2.0` only, so no `LICENSE-MIT`/`LICENSE-APACHE` pair was added; `repository` is still the placeholder `https://example.invalid/zynqsim` |
| Peripheral conformance, TTC (xttcps) and PL330 DMAC (xdmaps), Rust firmware self-checks on QEMU (spec §14.2): 13 of 14 and 12 of 12 checks pass; TTC is partly modelled (match flag read-to-clear, one N/A) | verified | `docs/conformance/ttc-dma.md`; test `conformance_ttc_dma` |

## Deviations from the prompt (so far)

* Board SVG art is an optional per-board file in a small subset (ADR-022); the built-in boards
  have none and are drawn from the TOML layout.
* There is no `ps-backend` trait crate: the engine abstraction is `sim_api::Engine`, and
  `backend-qemu` is a concrete adapter. The `PsBackend` trait is extracted when a second PS
  engine exists (P5).
* The RTL adapter's simulator-side library is a static archive (`xsim-dpi`), not the prompt's
  `cdylib`, because XSim on Windows links DPI statically (ADR-002). A DPI `cdylib` for Questa /
  Verilator remains future work.
* The workspace sets `panic = "abort"` for `dev` and `release` because the freestanding DPI
  library cannot unwind. Tests still unwind (cargo ignores the setting for them).
* Dockable GUI panels (egui_dock) are not used; the layout is fixed.
* `cargo fuzz` targets are in `fuzz/` and were only type-checked (no nightly or libFuzzer here). The
  seeded fuzz tests in the normal suite are the verified part (see *Robustness fuzzing*).
* The workspace lives in `zynqsim/` inside the prototype directory; the prototype files are
  untouched (prompt §16 migration is partly done: remote-port client, runner and driver logic now
  exist in Rust; the web UI and PowerShell scripts are not yet retired).

## Peripheral conformance (spec §14.2)

| Block | Status | Evidence / note |
|---|---|---|
| xgpiops (PS GPIO), xscutimer and global timer | private timer fully modelled (checked features); global timer partly; PS GPIO not modelled | `docs/conformance/gpio-timer.md`; tests `conf_gpiops_checks_match_the_expected_na_list`, `conf_scutimer_checks_match_the_expected_na_list` |

## Native PS (P5, optional)

The native ARMv7-A core `crates/cpu-a9` (ADR-030). Integer A32 only; the MMU, Thumb, VFP and NEON are
`stub`. Every row below names its evidence; the firmware rows are per image in ADR-030.

| Feature | Status | Evidence / note |
|---|---|---|
| Condition codes (all 15) on every instruction | verified | `cpu-a9/tests/isa.rs::all_fifteen_condition_codes_follow_the_flags` |
| Data processing, 16 opcodes, all shifters, S-bit flags with the carry rules | verified | `cpu-a9/tests/dp_props.rs` (10000 seeded cases per opcode against an independent reference, plus random immediate shifts) |
| Loads and stores: word, byte, halfword, signed, doubleword, all addressing modes | verified | `cpu-a9/tests/isa.rs::loads_and_stores_cover_offset_index_and_sign_modes`, `ldm_and_stm_all_four_modes_with_writeback` |
| Multiplies, long multiplies, halfword multiplies (SMULxy, SMLAxy) | verified (SMULWy, SMLAWy, SMMUL, SMMLA: implemented-untested) | `isa::multiplies_and_long_accumulates`, `isa::halfword_multiplies_select_halves` |
| Bit operations: CLZ, REV, REV16, REVSH, RBIT, UBFX, SBFX, BFI, BFC | verified | `isa::reversal_bit_and_field_instructions` |
| Extends (SXT/UXT, accumulate), saturation (SSAT, USAT, QADD), PKH | verified for the listed forms; the rest implemented-untested | `isa::extend_instructions_including_the_accumulate_forms`, `isa::saturating_instructions_set_q`, `isa::pack_halfword_instructions` |
| Parallel add and subtract (SIMD in GPRs) | verified for four forms; ASX, SAX and other Q/SH forms implemented-untested | `isa::parallel_add_subtract_sets_ge_for_the_plain_forms_only` |
| Exclusive monitor (LDREX, STREX, CLREX, word) | verified; byte, half and doubleword implemented-untested | `isa::exclusive_monitor_succeeds_once_and_fails_after_a_clear` |
| Branches: B, BL, BX; BLX register and immediate | verified (B, BL, BX); BLX implemented-untested | `isa::branches_bl_bx_and_b_reach_their_targets` |
| MRS, MSR (CPSR and SPSR, field masks), CPS | verified (MRS, MSR); CPS implemented-untested | `isa::msr_and_mrs_read_and_write_the_flags_and_the_mode`, `isa::msr_spsr_and_mrs_spsr` |
| Banked registers: SP and LR per mode, FIQ R8-R12 | verified (SP across IRQ mode) | `isa::banked_stack_pointers_are_per_mode` |
| Exceptions: SVC, undefined, data abort, prefetch abort, IRQ, FIQ, return by `subs pc, lr, #4` | verified (reset and BKPT entry implemented-untested) | `isa::svc_takes_the_exception_and_records_the_return`, `isa::undefined_instruction_takes_the_undef_exception`, `isa::data_abort_*`, `isa::prefetch_abort_*`, `isa::irq_is_taken_between_instructions_and_returns_with_subs_pc_lr_4`, `isa::fiq_masks_fiq_and_irq_and_saves_the_return` |
| WFI, WFE, SEV, interrupt wake from WFI while masked | verified | `isa::wfi_waits_then_resumes_after_the_wakeup_even_when_masked`, `isa::sev_wakes_wfe_and_the_event_register_is_consumed` |
| CP15: MIDR, MPIDR, ACTLR, cache and TLB no-ops | verified (the other registers implemented-untested) | `isa::cp15_identification_control_and_maintenance_registers` |
| UDIV, SDIV are undefined (no integer divide on Cortex-A9) | verified for the permanently undefined encoding | `isa::undefined_instruction_takes_the_undef_exception` |
| MMU: setting SCTLR.M stops the core, never runs untranslated | verified (no translation exists) | `isa::enabling_the_mmu_stops_the_core_rather_than_running_untranslated` |
| Thumb (T=1, BX to an odd address): reported, not run | stub | `isa::bx_to_an_odd_address_enters_thumb_which_is_reported` |
| VFP, NEON, coprocessor 10/11 | stub (`Unimplemented`) | `isa::vfp_is_reported_unimplemented_and_nothing_changes` |
| MMU walk, TLB, the `Translate` hook beyond identity | stub (identity translation only) | `Translate` / `Identity` in `cpu-a9::xlate` |
| GIC, timers, SCU, caches, peripherals | not in this crate | `mem-sys`, `ps-periph` (other crates) |
| Rust firmware: `rust_hello.elf` runs to completion, UART text equals QEMU | verified | `cpu-a9/tests/rust_firmware.rs::rust_hello_runs_to_completion_and_prints_its_message`, `cli/tests/cpu_a9_diff.rs::rust_hello_prints_the_same_text_in_qemu_and_in_the_interpreter` (skips without QEMU) |
| Every distinct instruction word of the 31 Rust firmware images decodes to an implemented class | verified (decode, not behaviour) | `cpu-a9/tests/rust_firmware.rs::every_instruction_word_of_the_firmware_decodes` |
| Other 30 Rust firmware images run to their end | stub: they need the GIC, timer, PL and peripheral models; ADR-030 lists each | `cpu-a9/tests/rust_firmware.rs::every_rust_firmware_image_is_classified` (report) |
| Instruction-by-instruction differential against QEMU | stub (only the UART text of one image is compared) | ADR-030, TESTPLAN |
