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
| 5. Universality corpus (>= 10 third-party designs) | **done**: 13 third-party permissively licensed designs (MIT, ISC, BSD-2-Clause, Apache-2.0, SHL-0.51) pass in XSim, plus 18 negative cases and 7 construct probes; table in `docs/CORPUS.md` | `backend-rtl-xsim::corpus::corpus_*` (14), `negative_*` (18), `probe_*` (7), `the_manifest_matches_the_files_on_disk`, `every_negative_case_has_a_test`, `corpus_table_is_current` |
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
| `board-zybo` (3D) | deterministic render, camera clamps, picking at three cameras, state shows in pixels; every built-in board and the custom example extrudes without panicking and its last switch/button is pickable top-down | the 3D view draws and picks what the state says, on any board |
| Boards (ADR-022): `board-zybo` `def`/`svg`/`render` tests, `sim-api` `boards` (7), `zynqsim` `boards` (4), `mcp` `boards` (5), `zynqsim-gui` `app::tests::picking_a_board...` and `loader::tests::the_board_picker...`, `xsim-dpi` and `vendor-ip` 8-bit tests | each built-in board loads with its counts and renders to a golden PNG (`zybo_z7_10_*`, `zedboard_*`, `pynq_z2_*`, `lab_board_active` with SVG art); every control hit-tests to itself; every class of board-file mistake gets its own message; `[board] file` resolves next to the project, is refused with the field, and travels in recordings; switch/button indices are validated against the board in the demo engine, CLI and MCP; the GUI picker swaps the board and the new controls work; 8-bit `tel_ctrl`/`tel_event`; ZedBoard QEMU + XSim run with 8 LEDs | custom boards (§10/§11) are loaded, validated, drawn and wired through every front end; the facts of the new boards are **not** tested (all `VERIFY:`) |
| `backend-qemu` `net::tests` (13) | RFC 1071 checksum vectors, ARP/ICMP/UDP summaries and direction, corrupted checksums reported, malformed/truncated/random frames never panic, Ethernet padding ignored, pcap reader across partial writes and garbage, virtual-time recovery, pcap writer, config validation, QEMU arguments, MAC/CIDR parsers | the Ethernet frame decoder and capture reader are right without QEMU |
| `xsa-import` (9), `sim-api` `xsa_import` (5), `zynqsim` `xsa` (4), `zynqsim-mcp` `xsa` (2) | real Vivado 2022.1 XSA parsed (VLNV, address map, IRQ, clocks, PS7 and MIO); hand-written HWH for `xlconcat`/HP/ACP/EMIO and the class catalogue; zip safety and message quality; user RTL mapped to a validating project; unmodelled IP stops `run` and `project_open` with a diagnostic; report content; MCP arguments and no-overwrite | XSA/HWH ingestion (§9.1, §12, start of P3) is right on the one design Vivado produced here and honest about everything else |
| `boot` (15), `sim-api` `bootbin` (3), `zynqsim` `bootbin` (5), `zynqsim-mcp` `bootbin` (2) | a real `bootgen` 2022.1 image read field by field (values equal `bootgen -read`); MD5 against RFC 1321 vectors; every kind of corruption named with field and file offset (boot header, partition header, payload MD5, truncation, magic words); encrypted/authenticated images refused; 3000 seeded mutations never panic; payloads equal the ELF segments given to bootgen; project validation per field; `boot-info`, `init --boot`, MCP `boot_inspect`; booted from BOOT.BIN the UART text equals the ELF boot | BOOT.BIN support (§12, P3) is right on the one image bootgen produced here and honest about what it refuses and does not run (FSBL, `ps7_init`, bitstream) |
| `xsim-export` (14), `backend-rtl-xsim` `export::tests` (5, no tools), `sim-api` `exportsim::tests` (4), `zynqsim` `export_sim` (2 of 3), `zynqsim-mcp` `export` (2) | `.prj` shapes (comments, quoted and spaced paths, relative `..`, continuations, per-line `-i`/`-d`/`-L`, libraries), `.f` lists (nested, `-work`, `+incdir+`/`+define+`), `elab.opt`, `xsim.ini`, scripts with variables, one netlist file, environment variables, include loops; the two real Vivado 2022.1 exports; diagnostics with file:line:construct; xvlog/xvhdl/xelab command lines (order, libraries, `glbl`, quoting, library mapping); glob matching and expansion; project validation with an export; `init --export-sim` and MCP `project_create_from_export` | Vivado simulation exports (§9.1 items 3-4, P3) are read faithfully and everything unsupported is named |
| `sim-api` `netlog::tests` (3), `project::tests::network_*` (3) | counters, direction, paging, bounded retention, digest depends on the payload; `[network]` defaults/overrides, every problem with its field, needs a CPU backend | the engine's capture record and the `[network]` section behave as documented |

## Universality corpus (P3, Vivado XSim)

Driver: `crates/backend-rtl-xsim/tests/corpus.rs` (+ `src/standalone.rs`); designs in
`tests/rtl/`; generated table `docs/CORPUS.md` (regenerate with
`ZYNQSIM_CORPUS_BLESS=1 cargo test -p backend-rtl-xsim --test corpus corpus_table_is_current`).
Needs Vivado/XSim; skipped with a message when it is not found.

| Area | Tests | What they establish |
|---|---|---|
| Third-party designs compile, elaborate and run their own or a stated self-check | `corpus_secworks_*` (5), `corpus_forencich_*` (3), `corpus_picorv32`, `corpus_uriscv`, `corpus_serv`, `corpus_pulp_common_cells`, `corpus_dpretet_async_fifo` | the simulator handles real RTL outside this project |
| Design in this repository, from a Vivado export | `corpus_vivado_funcsim_netlist` | a UNISIM netlist runs under the driver |
| Negative cases: each unsupported or erroneous construct fails at the stated stage with its file:line diagnostic | `negative_*` (18) | the failure is reported, not silent |
| Construct probes (bind, config, covergroup, constrained random, assertions, UVM with library, virtual modport) | `probe_*` (7) | which SystemVerilog and UVM constructs run |
| Corpus manifest: licence, commit and source files exist for every design | `the_manifest_matches_the_files_on_disk` | no design without provenance |
| Table is current | `corpus_table_is_current` | `docs/CORPUS.md` matches the test results |

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

| `zynqsim` `bootbin::a_bootbin_project_prints_what_the_elf_prints`, `zynqsim-mcp` `bootbin::a_bootbin_project_boots_through_mcp` | Xilinx QEMU (the parser, validation, `boot-info`, `init --boot` and `boot_inspect` tests need nothing) | prints `SKIPPED:` and returns |
| `zynqsim` `xsa::a_user_rtl_only_handoff_runs_the_real_firmware`, `xsa::user_rtl_in_xsim_and_a_vendor_model_share_one_pl` | Xilinx QEMU + Vivado XSim + the DPI library (the other `xsa` tests, `sim-api` `xsa_import`, `xsa-import` and `mcp` `xsa` need nothing) | prints `SKIPPED:` and returns |
| `zynqsim` `xsa::a_vendor_ip_handoff_runs_the_real_firmware_on_the_models`, `sim-api` `vendor_ip` (2), `mcp` `vendor_ip` | Xilinx QEMU only (no Vivado: the vendor-ip models are native) | same |
| `sim-api` `boards::zedboard_drives_eight_leds_from_firmware_and_takes_eight_switches_and_seven_buttons` (the other six `boards` tests need nothing) | Xilinx QEMU + Vivado XSim + the DPI library | prints `SKIPPED:` and returns |
| `backend-rtl-xsim` `export` (3: RTL export vs funcsim netlist on the bus, VHDL with xvhdl, include/define/cache), `zynqsim` `export_sim::rtl_export_and_funcsim_netlist_pass_with_the_real_firmware_and_agree` (needs QEMU too: three projects, one trace digest) | Vivado XSim + the DPI library (the second also Xilinx QEMU); the other `export_sim` tests, `xsim-export`, `sim-api` `exportsim` and `mcp` `export` need nothing | prints `SKIPPED:` and returns |
| `zynqsim` `handoff::the_exit_design_runs_end_to_end_from_the_bootbin_in_both_variants`, `zynqsim-mcp` `handoff::the_exit_design_runs_end_to_end_through_mcp` (P3 exit demonstration, ADR-019) | Xilinx QEMU + Vivado XSim + the DPI library (the other `handoff` tests and `sim-api` `xsa_import::a_boot_bin_next_to_an_xsa_gives_one_project_with_both` need nothing) | prints `SKIPPED:` and returns |

### P3 exit demonstration (`fixtures/handoff`, `examples/vivado-handoff`)

`crates/cli/tests/handoff.rs`: one `init --xsa ... --boot-bin ...` (and the `--export-sim`
variant) is checked without any tool (mapping, `boot_bin` mode, no `cpu0`, validation, the old
`init --boot` and `init --xsa --elf` paths unchanged); the end-to-end test runs, on real QEMU +
XSim, the firmware booted from the BOOT.BIN: seven math vectors on the user RTL, the GPIO on the
LEDs, three timer interrupts, a UART Lite line, `RESULT: ALL PASS`; three runs give one trace
digest; the committed example project and the export variant give the same digest and UART text;
the same firmware booted as an ELF differs only in the `qemu start elf=...` trace note. MCP:
`project_create_from_xsa` (with `boot_bin`) + `project_open` + `sim_run`, both variants.

What these cannot show: that a real FSBL, `ps7_init` or bitstream would boot the design on a
board (none is run), that the Xilinx IP models match the real IP (`VERIFY:` items), or anything
about a design other than this one.

### Xilinx IP models (`vendor-ip`)

Unit tests (`cargo test -p vendor-ip`, no tools): per model register behaviour, reset values,
GPIO interrupt set/clear (toggle on write) and dual channel, timer up/down/load/auto reload/
rollover period/narrow width/`ENALL`/PWM level, UART Lite baud pacing, 16-deep FIFOs, overrun,
interrupt, and `VendorPl` routing (model windows vs. RTL, board binding, IRQ lines, overlap and
parameter errors). End to end through the real QEMU: `firmware/gpio_timer_uart` (LED walk read
back through the register, three timer interrupts, UART Lite greeting, switches mirrored onto
LEDs, button press taking the GPIO interrupt, echo of host bytes sent to UART port 2), once as
the example project and once from the real Vivado XSA through `init --xsa`; a mixed design with
user RTL in XSim next to the GPIO model (`firmware/vendor_mix`).

ADR-021 adds the AXI DMA (`cargo test -p vendor-ip dma::`: 13 unit tests, direct and
scatter-gather, loopback, interrupts, errors), AXI IIC (7: dynamic mode, repeated start, NACK,
FIFO, interrupts), AXI BRAM Controller (3) and AXI Interrupt Controller (8). End to end through
QEMU, `sim-api` `vendor_ip2` (6): the DMA copy and its interrupts, the scatter-gather packet
(`examples/dma-sg`, `firmware/dma_sg`), DMA determinism, the INTC serving the timer and GPIO on
one line (`examples/intc-demo`, `firmware/intc_demo`), the IIC reading the TMP2 model and BRAM
data (`examples/iic-bram-demo`, `firmware/iic_bram_demo`), and a host-set sensor value read back
by the IIC master. Import: `xsa-import` `ip2` (5) against the Vivado fixtures, including passive
IP. The XSA-driven runs are in `zynqsim` `xsa` and `zynqsim-mcp` `xsa`/`vendor_ip`. Needs the
Xilinx QEMU for the `sim-api` and `xsa` runs, nothing for the unit and import tests.

What the tests cannot show: that the models match the real IP. No Xilinx simulation model or
board was available to compare against; the `VERIFY:` items in STATUS.md are the open questions.

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
