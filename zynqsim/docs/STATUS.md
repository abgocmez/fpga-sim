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
| Rust firmware (`firmware/net_demo`, prebuilt `fixtures/elf/rust_net.elf`, `examples/net-demo`): MDIO PHY scan, reset and auto-negotiation, descriptor rings, polled RX/TX, **smoltcp** ARP + DHCP + ICMP echo; four pings with 32 bytes, payload echoed unchanged | verified | `backend-qemu::net::firmware_pings_the_gateway_and_every_frame_is_captured` (14 frames: 4 DHCP, ARP pair, 4 echo pairs), `zynqsim::net::net_demo_pings_the_gateway_and_exports_a_matching_pcap`. smoltcp stands in for the BSP's lwIP, which is C; the Xilinx lwIP/xemacps applications themselves were **not** run |
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
| Board SVG import, custom boards | not-planned / stub | layout is TOML-driven; only `zybo-z7-20` is selectable |
| SSM2603 and HDMI RX/TX | verified at pin level in the testbench | see *Audio, video and the project loader*. Not a model of the board's wiring: the testbench connects them |
| **Pmod pins** (JA..JE): the PL's pin levels reach the board state (pin colours in 2D and 3D, `pin_get JA1`), `Capability::Pmod`, trace signals `pmod.ja` | verified | `zynqsim::oled_demo_draws_text_pixel_exact`, `mcp::pmod_oled_through_mcp`, `xsim-dpi::core::tests` for the packets. Needs a testbench that calls `zs_pin_out`/is generated by `init --ip`. JF (a PS Pmod on MIO) and host-driven input pins are not exercised by a test |
| **Pmod OLED** (SSD1306, 128x32, SPI): commands, data, the three addressing modes with windows, on/off, inverse, segment remap, COM flip, contrast, reset | verified | `devices::ssd1306::tests` (9) and the end-to-end test: Rust firmware -> AXI-Lite -> SPI master RTL (`fixtures/rtl/pmod/oled_spi.v`) -> JA pins -> host model, **all 4096 pixels equal** the expected text in the shared font. Not modelled: scrolling, start line / display offset, charge-pump and supply sequencing; levels of VBATC/VDDC are recorded only. Pin order and the init values follow the Digilent reference as remembered: `VERIFY:` |
| **RGB LEDs**: six PWM pins averaged into duty cycles, shown as colours | verified | `zynqsim::rgb_demo_averages_the_pwm_pins_into_colours` (duties 64/128/255 and 255/0/32 within 4 of 255). Windows of >= 200 us, so a duty changes with up to that latency; the last duty is kept after the testbench ends |
| Ethernet PHY at pin level (RGMII/MDIO wires), Pmod modules not listed above (SSD, ...) | stub | drawn as inert parts; the GEM talks to QEMU's PHY model (ADR-011) |

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
| GUI: Pmod devices panel (the OLED picture and its state) | implemented-untested | not covered by a test; the model fetches the picture (10 Hz) |
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
| GUI: dockable panels, CPU/PL signal/network/timeline panels, DWARF views | stub | fixed layout; panels that need a backend say so |

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
| Every IP without a model is recorded in `[xsa.unmodelled]`; each entry is a validation error naming instance, VLNV, address, IRQ bits and reason; `run`/`project_open` stop (never a silent partial run) | verified | `zynqsim::xsa::an_unmodelled_ip_is_reported_and_stops_the_run` (exit 1, diagnostic on stderr; the unmodelled IP is an AXI DMA in the hand-written `fixtures/xsa/zs_math_dma.hwh` now that the AXI GPIO is modelled), `mcp::xsa::an_xsa_with_an_unmodelled_ip_creates_a_project_that_will_not_open`, `sim-api::xsa_import::{an_ip_without_a_model_is_recorded_and_stops_the_run, a_missing_rtl_source_is_unmodelled_with_the_flag_to_pass_it}` |
| `xsa-report.md`: PS7 table, MIO use, every instance with class/address/IRQ/clock/disposition, address map, mapped files, *Not supported*, *Not applied*; verdict says NOT RUNNABLE when anything is unmodelled and never claims correctness | verified (content) | `zynqsim::xsa`, `sim-api::xsa_import` (strings asserted) |
| **End to end**: an XSA whose only PL IP is the user's math IP runs on QEMU + XSim with the existing firmware: `RESULT: ALL PASS`, the same 21 bus accesses / 329 trace records as the native-model and `init --ip` routes | verified (Windows, needs QEMU + Vivado + DPI library) | `zynqsim::xsa::a_user_rtl_only_handoff_runs_the_real_firmware` |
| **Not done** | stub / not-planned | `ps7_init` data is read as file names only and not applied (QEMU boots the ELF; PS clock tree, MIO and peripheral enables change nothing); vendor IP other than AXI GPIO, Timer and UART Lite (see *Xilinx IP models*) is not modelled, so such designs are generated but do not run; one user AXI4-Lite slave per project (others are listed unmodelled; no multi-instance testbench or address decode); VHDL sources are listed, not compiled; `BOOT.BIN`; custom boards; GUI import; `--filelist` without `--xsa` |

## Xilinx IP models (`vendor-ip`, milestone P3)

`crates/vendor-ip`: register-level models of **AXI GPIO 2.0, AXI Timer 2.0 and AXI UART Lite 2.0**
as `pl-native` blocks, placed by `[native]` (`axi_gpio`, `axi_timer`, `axi_uartlite`) with
`[native_params.<instance>]`, composed with user RTL in XSim (ADR-016). `zynqsim init --xsa` maps
them from the block design. The example is `examples/gpio-timer-uart` (firmware
`firmware/gpio_timer_uart`, Rust `no_std`); its design is the **real Vivado 2022.1 XSA**
`fixtures/xsa/zs_vendor_ip.xsa` (`fixtures/xsa/make_vendor_xsa.tcl`: two AXI GPIO, a timer, a UART
Lite, interrupts through an `xlconcat`).

**Register maps and behaviour are written from the Xilinx product guides (PG144, PG079, PG142)
from memory and were never compared with a Xilinx simulation or a board.** "verified" below means
the model does what this repository's tests say it does, not that real hardware does.

| Feature | Status | Evidence / note |
|---|---|---|
| AXI GPIO: `GPIO_DATA/TRI`, `GPIO2_DATA/TRI`, `GIER`, `IP_ISR` (toggle on write), `IP_IER`; widths 1..32, dual channel, `C_ALL_INPUTS/OUTPUTS`, `C_DOUT_DEFAULT`, `C_TRI_DEFAULT`; interrupt on input change; reset values | verified (model) / VERIFY (register facts) | `vendor-ip::gpio::tests::*` (6) and, through QEMU and the real firmware, `sim-api::vendor_ip::the_firmware_drives_all_three_models_and_the_board_follows`. VERIFY: toggle-on-write of `IP_ISR`; status bit set by any change of the pad value (not masked by TRI); data read of a bidirectional channel (pin for inputs, register for outputs); `TRI` fixed under `C_ALL_*`; unmapped offsets read 0 without SLVERR |
| AXI Timer: `TCSR0/1`, `TLR0/1`, `TCR0/1`; generate mode, up/down, `LOAD`, auto reload, `TINT` write-1-to-clear, interrupt level, `ENALL`, `C_COUNT_WIDTH`, `C_ONE_TIMER_ONLY`, `generateout` pulses counted and PWM level | verified (model) / VERIFY (register facts) | `vendor-ip::timer::tests::*` (11), firmware: three periodic interrupts (`timer ticks=3`). VERIFY: period formulas (up `MAX-TLR+1`, down `TLR+1` clocks), no hold without `ARHT`, `LOAD` semantics, `ENALL` read-back, `TCR` exact (hardware is a few clocks stale) |
| AXI Timer capture mode, cascade mode (64 bit), external capture/generate pins | stub | a timer in capture mode just counts; `CASC` reads back and does nothing |
| AXI UART Lite: `RX_FIFO`, `TX_FIFO`, `STAT_REG`, `CTRL_REG`; 16-deep FIFOs, baud/frame paced shifter, overrun, interrupt, reset values | verified (model) / VERIFY (register facts) | `vendor-ip::uartlite::tests::*` (7), firmware greeting and echo (`sim-api::vendor_ip`, `zynqsim::xsa::a_vendor_ip_handoff_runs_the_real_firmware_on_the_models`). VERIFY: interrupt condition and that a `STAT_REG` read lowers it, "TX empty" means FIFO not shifter, TX write when full is dropped, overrun clears on `STAT_REG` read, no frame/parity errors (no pins) |
| UART Lite bytes to the host: `uart_read` port `2 + n`, `uart_write` port `2 + n` (deterministic, baud-paced), `uart<port>` trace records, `uart.pl` in `run --json` | verified | `sim-api::vendor_ip` (both tests, incl. determinism), `mcp::vendor_ip::vendor_ip_example_through_mcp`, `zynqsim::xsa` |
| GPIO bound to LEDs, switches, buttons and Pmod pins (`[connect] "<inst>.GPIO" = "board.leds"`); Pmod outputs go to the device bank | verified (LEDs, switches, buttons) / implemented-untested (Pmod binding beyond `vendor-ip::tests`) | `sim-api::vendor_ip`, `mcp::vendor_ip`; Pmod binding: `vendor-ip` unit tests only (no device attached on a GPIO-driven Pmod in an engine test) |
| Interrupts: a change caused by an access travels with the access, one caused by time or a board input at the next sync (at most one quantum, 1 us, late); level, to `IRQ_F2P[n]` | verified | `pl-native::regs::tests`, `vendor-ip::tests::{gpio_leds_switches_and_interrupt_through_the_pl, timer_interrupt_and_uart_bytes}`, firmware (3 timer interrupts, button interrupt count). The first version lowered lines only at the next sync and hung the guest in a re-firing level interrupt: see ADR-016 |
| Vendor models next to user RTL in XSim: vendor window answered by the model, everything else by the RTL; board outputs and pins merged | verified (Windows, QEMU + Vivado + DPI) | `zynqsim::xsa::user_rtl_in_xsim_and_a_vendor_model_share_one_pl` (math IP and GPIO in one PL, LEDs 0 and 3 lit), `vendor-ip::tests::unmapped_accesses_go_to_the_rtl_design` |
| `init --xsa` maps the three IP: parameters (`C_*`), `ADDRESS_SIZE`, `CLK_FREQ_HZ`, `IRQ_F2P` bit, GPIO board binding (by external port name, else guessed from direction and width, reported as a note) | verified | `sim-api::xsa_import::the_gpio_design_maps_the_user_ip_and_places_the_gpio_model_next_to_it`, `zynqsim::xsa::a_vendor_ip_handoff_runs_the_real_firmware_on_the_models` (the real XSA: `leds_4bits`, `sws_4bits`, `btns_4bits` recognised) |
| Snapshot, restore, reset and replay with the models | verified | `mcp::vendor_ip` (restore returns the trace digest, replay deterministic); models are rebuilt and the journal re-executed |
| Other Xilinx IP (DMA, VDMA, IIC, Quad SPI, BRAM controller, interrupt controller, Clocking Wizard, UART 16550, ...) | stub | still `[xsa.unmodelled]` and stop the run; `sim-api::xsa_import::an_ip_without_a_model_is_recorded_and_stops_the_run` with a hand-written AXI DMA handoff |
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
| `cargo xtask ci` (fmt, clippy, freestanding DPI clippy + build, tests, release build) | verified locally | exited 0 on 2026-10-15 (after the Pmod devices), 239 tests, 0 skipped, QEMU and XSim tests included |
| `cargo xtask dpi`, `third-party`, `golden`, `release` | verified | each run |
| `THIRD_PARTY.md` | verified | generated from `Cargo.lock`; licences as declared, not audited. Re-generate after dependency changes |
| Linux and macOS | verified (build, lints, tests without Xilinx tools) | GitHub Actions run 38731290203 on `ubuntu-latest`, `windows-latest` and `macos-latest`: fmt, clippy `-D warnings` (incl. the freestanding DPI crate), `cargo test --workspace --no-fail-fast` and the release build all pass. Hosted runners have no QEMU or Vivado, so every test that needs them **skips**; those are verified only on the Windows development machine. The first hosted run found three real problems (workflow in the wrong directory, `freestanding` feature broken off Windows, a CRLF-dependent test) and the Linux WSL run earlier found an ordering bug in the XSim adapter |
| `forbid(unsafe_code)` workspace lint | verified | `xsim-dpi` is the one exemption (FFI boundary, `win.rs` only) |
| Building and testing without QEMU/Vivado installed | implemented-untested | tests that need them skip with a message; not run on a machine without them |
| Git repository, stacked pull requests, CI at the repository root | verified | GitHub Actions on the pull requests (3 OS) |

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
