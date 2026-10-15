# Test plan

Maps the requirements of prompt §14 to tests. All test drivers are Rust. Tests that need an
external engine are skipped and listed here; **at P0 no test needs one**, so none are skipped.

Run everything: `cargo xtask ci` (fmt check, clippy `-D warnings`, `cargo test --workspace`,
release build). Regenerate golden images: `cargo xtask golden` (review the diff).

## Status by §14 item

| §14 item | P0 status | Where |
|---|---|---|
| 1. Prototype regression (DMA accelerator, two-core pipeline) | **done** for the prototype's `dma_xform` design (6 cases, 2 cores, IRQ), vs a Rust golden model. The "64x64 image filter" and "7 vector" cases of the prompt's wording are the prototype's `math` and DMA firmware; there is no separate 64x64 image design | `backend-rtl-xsim::dma_golden`, `zynqsim::dma_regression_*`, `zynqsim-mcp::dma_project_through_mcp` |
| 2. Peripheral conformance (xuartps, xgpiops, ...) | not started (P1+) | n/a |
| 3. Multicore (WFE/SEV, LDREX/STREX, SGI) | not started (P1+) | n/a |
| 4. PL protocol checkers, 4 KB crossing, IRQ completion | not started (P1) | n/a |
| 5. Universality corpus (>= 10 third-party designs) | not started (P3) | n/a |
| 6. Determinism, 5 runs, identical hashes | **done**: demo model (5 runs, in-process and 5 processes), QEMU+native PL (3 processes), QEMU+XSim DMA (3 processes in the test, 5 runs manually) | `sim-api::sim::tests::five_runs_one_digest_and_replay_agrees`, `zynqsim::five_separate_processes_produce_one_trace_hash`, `zynqsim::qemu_project_runs_the_firmware_and_is_deterministic`, `zynqsim::dma_regression_passes_and_is_deterministic_across_processes`, `backend-qemu::two_runs_are_identical_in_time_and_traffic` |
| 7. GUI headless render vs golden PNGs | **done for the board renderer** | `board-zybo::render::tests::default_board_matches_golden`, `active_board_matches_golden` |
| 8. MCP schema validation, scenario, fuzzed inputs | **done** | `crates/mcp/tests/{scenario,fuzz,stdio}.rs`, `tools::tests::*`, `schema::tests::*` |
| 9. Performance report | not started | n/a |
| 10. `cargo fuzz` for parsers and packets | **not set up**; a seeded property test stands in for the MCP surface | `mcp/tests/fuzz.rs` |

## Test inventory (P0)

| Area | Tests | What they establish |
|---|---|---|
| `sim-core` | scheduler ordering, past-scheduling rejection, cancel, serde round trip; PRNG reference vector and snapshot; clock edges and enable | the kernel is ordered, restartable and deterministic |
| `trace` | FNV-1a reference vectors, length-prefix aliasing, hash sensitivity, serde restore, VCD structure and id codes, Perfetto JSON; waveform windows (inclusive bounds, initial value, truncation and `covered_to_ps`, stable ordering, wildcards, signal cap, dropped records) and the waveform scene (pixel positions of edges, hex when it fits, cursor, truncation marker, folded dense changes, SVG escaping, raster) | the digest means what it claims; exports are well-formed |
| `board-zybo` | board file parses and has the expected parts, validation lists every problem, unknown fields rejected, pin lookup, reset, golden renders, determinism, scale, PNG signature, hit-testing | the board is consistent and renders reproducibly |
| `sim-api` | UART timing, switch/LED/RGB mapping, button text, echo, heartbeat and seed, input validation, snapshot rewind and equivalence, replay and tamper detection, offset reads, project validation (every field), backend inventory, server round trip, events, play/pause, restore | the control plane and the demo model behave as documented |
| `zynqsim-mcp` | handshake and version negotiation, tool/resource/prompt listing, full scenario, gating messages, argument validation, path confinement, diagnostics, parse errors, progress and cancellation, real binary over pipes, 1500 random tool calls + 500 garbage messages | the MCP surface is safe, bounded and honest |
| `zynqsim` (CLI) | run output and JSON, fidelity statement, record/test/trace round trip and tamper (exit 1), init, unsupported (exit 3), usage errors (exit 2), inspect, 5-process determinism, `--at` grammar | the CLI is scriptable with stable exit codes |
| `zynqsim-gui` | model actions reach the simulation, console accumulation and restore, error surfacing, reset seed, key bindings, trace line formatting; waveform viewer state (zoom anchoring, pan clamps, search, fetch only while active, follow, selection kept across engine swap, VCD export) and a synthetic wheel/drag/click test of the Waveforms tab; project draft round trip, PS-only and PL-only edits, validation messages; speaker queue, resampler and (with a device) real-time drain; synthetic-input tests of clicks, holds, keys, 3D orbit/zoom/pick and the project window | GUI behaviour is correct without a window; the sound device is exercised when present |
| `sim-api` `storage` | SD image round trip of files and directories, reproducible bytes, FAT16/FAT32 choice, card size and label rules, QSPI packing at an offset, image checks naming the failed rule | the SD and QSPI images are valid and reproducible (host only) |
| `board-zybo` (3D) | deterministic render, camera clamps, picking at three cameras, state shows in pixels | the 3D view draws and picks what the state says |
| `backend-qemu` `net::tests` (13) | RFC 1071 checksum vectors, ARP/ICMP/UDP summaries and direction, corrupted checksums reported, malformed/truncated/random frames never panic, Ethernet padding ignored, pcap reader across partial writes and garbage, virtual-time recovery, pcap writer, config validation, QEMU arguments, MAC/CIDR parsers | the Ethernet frame decoder and capture reader are right without QEMU |
| `xsa-import` (9), `sim-api` `xsa_import` (5), `zynqsim` `xsa` (4), `zynqsim-mcp` `xsa` (2) | real Vivado 2022.1 XSA parsed (VLNV, address map, IRQ, clocks, PS7 and MIO); hand-written HWH for `xlconcat`/HP/ACP/EMIO and the class catalogue; zip safety and message quality; user RTL mapped to a validating project; unmodelled IP stops `run` and `project_open` with a diagnostic; report content; MCP arguments and no-overwrite | XSA/HWH ingestion (§9.1, §12, start of P3) is right on the one design Vivado produced here and honest about everything else |
| `sim-api` `netlog::tests` (3), `project::tests::network_*` (3) | counters, direction, paging, bounded retention, digest depends on the payload; `[network]` defaults/overrides, every problem with its field, needs a CPU backend | the engine's capture record and the `[network]` section behave as documented |

## Tests that need external engines (P1)

| Test | Needs | Skip behaviour |
|---|---|---|
| `backend-qemu` `e2e` (5, incl. memory read/write) | Xilinx QEMU + co-sim DTB | prints `SKIPPED:` and returns (passes vacuously) |
| `sim-api` `qemu_play` | Xilinx QEMU | same |
| `backend-rtl-xsim` `board` (2: shell/LEDs/switches/buttons, image golden) | Xilinx QEMU + Vivado XSim | same |
| `zynqsim` `board_demo_*` (2), `mcp` `board_demo_through_mcp` | Xilinx QEMU + Vivado XSim | same |
| `zynqsim` `qemu_*` (3, incl. record/replay/tamper), `mcp` `qemu_project_through_mcp` (incl. snapshot/restore/replay/reset), `sim-api` `qemu_journal` (2) | Xilinx QEMU | same |
| `backend-rtl-xsim` `math` (3+1, plus the build cache), `dma_golden`, `zynqsim` `dma_*`, `mcp` `dma_project_through_mcp` | Xilinx QEMU + Vivado XSim (+ builds the DPI library on demand) | same |
| `backend-rtl-xsim` `media` (4: build, codec, HDMI, polling stress) | Vivado XSim | same |
| `zynqsim` `oled_demo_*`, `rgb_demo_*`; `mcp` `pmod_oled_through_mcp` | Xilinx QEMU + Vivado XSim + the DPI library (rebuild it after pulling: the pin DPI functions are new) | same |
| `devices` (9, no tools needed), `sim-api` `tbgen` (12, no tools needed), `zynqsim` `init_ip_*` (4; three need QEMU + XSim), `sim-api` `disasm` (4, no tools needed), `backend-qemu` `e2e` debugger (2), `sim-api` `qemu_debug` (6; core 1 also needs Vivado XSim + DPI), `mcp` `debugger_through_mcp`, `gui` `registers_appear_in_the_model...` | Xilinx QEMU | same |
| `sim-api` `source` unit tests (8, no tools needed: hand-built ELF line table + `rust_debug.elf`), `sim-api` `qemu_source` (3), `mcp` `source` (1 + 1 unit), `zynqsim` `source` (2), `zynqsim-gui` `source_view` (3; one needs QEMU). Source-level debugging of `firmware/debug_demo` (ADR-013) | Xilinx QEMU (the `source` unit tests and the panel-drawing tests need nothing) | prints `SKIPPED:` and returns |
| `zynqsim` `audio_demo_*`, `hdmi_demo_*`; `mcp` `audio_and_hdmi_demos_through_mcp` | Xilinx QEMU + Vivado XSim + the DPI library | same (the MCP test asserts the library exists) |
| `mcp` `waveform` (3: the demo-engine and picture tests need nothing; `waveform_get_shows_the_bus_of_a_qemu_project` needs Xilinx QEMU) | Xilinx QEMU (one test) | same |
| `zynqsim-gui` `model` e2e (2: project load + audio, HDMI frame) | Xilinx QEMU + Vivado XSim + the DPI library | same |
| `backend-qemu` `net` (6: ping + every frame captured, two-run determinism, frame times independent of run slicing, custom subnet/MAC, unplugged NIC, open network) | Xilinx QEMU + co-sim DTB (no PL, no XSim) | prints `SKIPPED:` and returns |
| `zynqsim` `net` (6: ping + pcap pair check, two-process determinism of digest/UART/pcap bytes, record/replay, trace/VCD, `--pcap` refusal, bad `[network]`), `mcp` `net` (2: capability gate; capture, paging, snapshot/restore/replay), `zynqsim-gui` `ethernet_counters_*` | Xilinx QEMU (the bad-section and gate tests need nothing) | same |

| `zynqsim` `xsa::a_user_rtl_only_handoff_runs_the_real_firmware` | Xilinx QEMU + Vivado XSim + the DPI library (the other three `xsa` tests, `sim-api` `xsa_import`, `xsa-import` and `mcp` `xsa` need nothing) | prints `SKIPPED:` and returns |

### §14.2 peripheral conformance: Ethernet

`xemacps + lwIP` is represented by Rust `no_std` firmware (`firmware/net_demo`: GEM driver +
smoltcp) that must report four ICMP echo replies with the payload unchanged, against QEMU's
`cadence_gem` model. The Xilinx BSP's lwIP/xemacps sources are C and are not in this repository,
so they were not run. QEMU's model accepted this driver; which GEM features it models was not
audited, and interrupt-driven operation, checksum offload and jumbo frames are not exercised
(see STATUS.md, Ethernet).
| `zynqsim` `storage` (the cases that boot firmware; the rest are host-only), `sim-api` `storage_session` (2), `mcp` `sd_and_qspi_demo_projects_through_mcp` | Xilinx QEMU | same |

Rust has no stable runtime-skip, so a skipped test is reported as passed. Run with
`cargo test -- --nocapture | findstr SKIPPED` to see which ones did not actually run.

## Not covered (be aware)

* **GUI appearance and native dialogs**: pointer click/hold, orbit/zoom, keyboard shortcuts and
  the project window logic are driven by synthetic input in `app::tests`; how it *looks*
  (layout, fonts, colours) is judged only from `zynqsim-gui --screenshot out.png` (also
  `--view3d`, `--project-window`) on Windows, and the native file dialogs are never opened by a test.
* **Sound output**: a test opens the real output device and checks the queue drains in real time;
  nobody has verified by ear that the sound is right.
* **Linux and macOS**: CI passes there, but without QEMU/Vivado, so the engine tests skip; they are only exercised on the Windows development machine. macOS has no run of the XSim crates at all (Windows-only).
* **Coverage-guided fuzzing**: `cargo fuzz` targets do not exist yet.
* **Third-party viewers**: VCD was not opened in GTKWave and Perfetto JSON was not loaded in the
  Perfetto UI; only their formats are checked.
* **Hardware correctness**: nothing is compared with a real board. Pin assignments are unverified.
* **QEMU session features**: registers at an arbitrary time (not possible, see STATUS.md);
  access (`Z4`) and RAM watchpoints firing; debug sessions' trace digests (not reproducible). Snapshot, restore, reset and replay are covered, by re-execution, on the
  native-PL math project and (replay only) on the RTL board demo by hand.
* **Verilator / Icarus backends** and **Linux / macOS XSim**: not implemented.
