# Test plan

Maps the requirements of prompt §14 to tests. All test drivers are Rust. Tests that need an
external engine are skipped and listed here; **at P0 no test needs one**, so none are skipped.

Run everything: `cargo xtask ci` (fmt check, clippy `-D warnings`, `cargo test --workspace`,
release build). Regenerate golden images: `cargo xtask golden` (review the diff).

## Gates

| Gate | Command | What runs | Wall time | Use it for |
|---|---|---|---|---|
| **Full** | `cargo xtask ci` | fmt check, clippy `-D warnings` (workspace, and the freestanding DPI crate), `cargo test --workspace` (tests that need a tool run it), release build | 45-60 min; each step's time is printed as it ends, and a summary table at the end (pass or fail) | **Before merging.** Only this gate runs the QEMU, XSim, Verilator and Icarus tests |
| **Fast** | `cargo xtask ci --fast` | fmt check, clippy `-D warnings` (workspace), `cargo test --workspace` with `ZYNQSIM_FAST=1`: every test that needs an external tool prints `SKIPPED: ... (fast mode)` and passes | measured in `docs/STATUS.md` | Feedback while working. It does **not** replace the full gate |
| **Determinism, restricted** | `cargo xtask ci --det-only name,name` | only `crates/cli/tests/determinism.rs`, with `ZYNQSIM_DET_ONLY` set to the listed examples. Unknown names fail the test; every `examples/` directory must still be in `CASES` or `EXCLUDED` | depends on the examples | Checking the 5-run digests of changed examples. `cargo xtask changed-examples` prints the examples changed since `origin/main` |

A skip is a pass. A green fast gate says nothing about QEMU or XSim behaviour: the full gate
(or `--nocapture` output showing no `SKIPPED:` lines) is what proves those.

## Status by §14 item

| §14 item | P0 status | Where |
|---|---|---|
| 1. Prototype regression (DMA accelerator, two-core pipeline) | **done** for the prototype's `dma_xform` design (6 cases, 2 cores, IRQ), vs a Rust golden model. The "64x64 image filter" and "7 vector" cases of the prompt's wording are the prototype's `math` and DMA firmware; there is no separate 64x64 image design | `backend-rtl-xsim::dma_golden`, `zynqsim::dma_regression_*`, `zynqsim-mcp::dma_project_through_mcp` |
| 2. Peripheral conformance (xuartps, xgpiops, ...) | **partly done** (P4): Xilinx's own BSP applications run unmodified on the QEMU PS. 16 of 22 driver examples and the Hello World template print their success line (UART, GIC, SCU timer, TTC, PL330 DMA, QSPI/I2C self tests, SD raw, GEM DMA); GPIO, SPI, CAN, devcfg and the SCU watchdog **fail** because the Xilinx QEMU machine lacks them (recorded, asserted as deviations); XADC returns zeros; lwIP/FatFs not built; nothing compared with hardware | `crates/cli/tests/bsp_apps.rs` (4 tests + 1 `#[ignore]`), `docs/conformance/bsp-apps.md`, ADR-023 |
| 3. Multicore (WFE/SEV, LDREX/STREX, SGI) | not started (P1+) | n/a |
| 4. PL protocol checkers, 4 KB crossing, IRQ completion | **partly done** (P5): protocol checkers (ADR-025) report VALID/READY, reset, 4 KB, WLAST/RLAST, WRAP and response findings in every `init --ip` testbench. Valid designs (math, DMA, IRQ, PMOD OLED) give zero findings; five deliberately illegal fixtures give their rule and time; strict and off are checked. Not covered: checkers inside hand-written testbenches; cache maintenance | `zynqsim::protocol_checks` (9 tests, XSim), `sim-api::tbgen::protocol_checkers_*`, `pl-api::protocol`, `xsim-dpi::core::tests::protocol_*` |
| 5. Universality corpus (>= 10 third-party designs) | **done**: 13 third-party permissively licensed designs (MIT, ISC, BSD-2-Clause, Apache-2.0, SHL-0.51) pass in XSim, plus 18 negative cases and 7 construct probes; table in `docs/CORPUS.md` | `backend-rtl-xsim::corpus::corpus_*` (14), `negative_*` (18), `probe_*` (7), `the_manifest_matches_the_files_on_disk`, `every_negative_case_has_a_test`, `corpus_table_is_current` |
| 6. Determinism, 5 runs, identical hashes | **done**: demo model (5 runs, in-process and 5 processes), QEMU+native PL (3 processes), QEMU+XSim DMA (3 processes in the test, 5 runs manually). Every example: 5 separate processes each (`crates/cli/tests/determinism.rs`), 25 of 26 identical; `board-demo` is known non-deterministic (UART1 over TCP) and listed in the test and STATUS.md. `hdmi-demo` not run there | `sim-api::sim::tests::five_runs_one_digest_and_replay_agrees`, `zynqsim::five_separate_processes_produce_one_trace_hash`, `zynqsim::qemu_project_runs_the_firmware_and_is_deterministic`, `zynqsim::dma_regression_passes_and_is_deterministic_across_processes`, `backend-qemu::two_runs_are_identical_in_time_and_traffic`, `zynqsim::every_example_runs_five_times_with_identical_trace_hashes` |
| 7. GUI headless render vs golden PNGs | **done for the board renderer** | `board-zybo::render::tests::default_board_matches_golden`, `active_board_matches_golden` |
| 8. MCP schema validation, scenario, fuzzed inputs | **done** | `crates/mcp/tests/{scenario,fuzz,fuzz_requests,stdio}.rs`, `tools::tests::*`, `schema::tests::*`. `fuzz_requests`: every tool from `tools/list` gets 200 random argument objects, with and without the demo open, plus raw JSON-RPC lines; 5 s per call (60 s for rendering tools) |
| 9. Performance report | not started | n/a |
| 10. Fuzzing of the project parser, MCP requests and remote-port packets | **done** as seeded tests on stable Rust (`verified`); `cargo-fuzz` targets exist in `fuzz/` (`implemented-untested`, never run under libFuzzer) | `sim-api/tests/fuzz_project.rs`, `mcp/tests/fuzz.rs`, `mcp/tests/fuzz_requests.rs`, `interop/tests/fuzz_packets.rs`; `fuzz/fuzz_targets/*` |
| 8. MCP schema validation, scenario, fuzzed inputs | **done** | `crates/mcp/tests/{scenario,fuzz,stdio}.rs`, `tools::tests::*`, `schema::tests::*` |
| 9. Performance report | **partly done**: `zynqsim bench` measures wall time, simulated time, sim/wall ratio, PL cycles per second (XSim) and an approximate QEMU instruction rate; single runs on one machine, written up with the machine and versions in `docs/PERFORMANCE.md`. No test asserts the numbers; no hard targets | `zynqsim bench` (`crates/cli/src/main.rs`), `docs/PERFORMANCE.md` |
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
| `zynqsim` `bsp_apps` (4, + 1 `#[ignore]`) | each committed Xilinx BSP ELF (`fixtures/bsp`) runs on QEMU: 15 driver examples print their own success line (`PASS` table), 7 recorded deviations print the asserted failure or hang (`DEVIATIONS` table), Hello World twice with one digest, Peripheral Tests' PASSED/FAILED lines, ELFs < 300 KB and listed in `SOURCE.md`; the GEM example (13 min) is `#[ignore]` | the Xilinx drivers work against the QEMU model where the model has the device, and where it does not the failure is recorded, not hidden. Needs the Xilinx QEMU; `SKIPPED:` otherwise. Build recipe: `fixtures/bsp/build_bsp.tcl` (needs Vitis, not run by the tests) |
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

## Native PL (P9, ADR-035)

No external tool. `crates/cli/tests/native_pl.rs`: GPIO/timer/UART Lite firmware on the native
PS (UART text, PL UART Lite bytes, switch to GPIO, five engines give one digest) and the
validation of PS/PL pairs. Unit tests: `sim-api` `pl_attach` and `native_pl` (the RTL
`my_math_ip` answers on the bus, GP windows, PL clock follows the cycles), `rtl-engine` (the
XSim differential is `xsim_diff`, which needs `cargo xtask dpi`). Not run: `examples/native-math`
(see ADR-035).

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
| `backend-rtl-verilator` (diagnostics parser unit tests with captured messages, tool discovery, `math`: 7 writes / 14 reads through the Verilator model, cached second start, `unsupported_constructs_are_reported_with_file_line_and_text`); `zynqsim` `verilator` (`math_firmware_passes_through_verilator_and_matches_xsim`: real QEMU + Verilator, the same project with XSim when Vivado is present); `sim-api` `project::tests::verilator_refuses_what_only_xsim_can_run`; `mcp` `backend_list` | Verilator (`ZYNQSIM_VERILATOR`, `VERILATOR_ROOT`, `PATH` or `<temp>\zs-tools\verilator\oss-cad-suite`) + `make`, a C++ compiler and `sh` + the DPI library; the QEMU test also needs Xilinx QEMU. The unit tests need nothing | `SKIPPED:` and returns (`math` and `verilator`) |
| `backend-rtl-icarus` (diagnostics parser with captured `iverilog` messages, tool discovery), `vpi-icarus` (the 31 `$zs_` names are unique; every `$zs_` call of the math testbench is registered), `cli::icarus` (real QEMU runs `examples/math-icarus`: ALL PASS, 21 accesses, the trace digest is compared with XSim and was equal), determinism case `math-icarus` | Icarus Verilog (`ZYNQSIM_ICARUS`, `PATH` or the unpacked archive) and `cargo xtask vpi`; `SKIPPED:` and returns without them | Windows only; math design only |
| `zynqsim` `oled_demo_*`, `rgb_demo_*`; `mcp` `pmod_oled_through_mcp` | Xilinx QEMU + Vivado XSim + the DPI library (rebuild it after pulling: the pin DPI functions are new) | same |
| `zynqsim` `pmod_simple_demo_lights_the_8ld_and_reads_the_buttons` (8LD pattern, host-set BTN values, determinism; also in the 5x determinism table); `devices` `led8`, `ssd`, `buttons` (unit tests, no tools needed) | Xilinx QEMU + Vivado XSim + the DPI library for the end-to-end test; none for the unit tests | prints `SKIPPED:` and returns (the unit tests always run) |
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

AXI Timer capture and cascade modes (ADR-026): `cargo test -p vendor-ip timer::` (capture on the
rising edge with `TLR` and the count, no capture with `CAPT`, `ENT` or `MDT` clear, a level is
not an edge, timer 1's own trigger, `ARHT` hold until `TINT` is cleared, cascade carry across
the 32-bit boundary, 64-bit rollover interrupts, 64-bit capture, reset) and
`vendor-ip::tests::timer_capture_inputs_are_bound_to_board_bits` (`board.btn0` and `board.sw2`
bound to `CAPTURETRIG0/1`, a whole bus or a Pmod pin refused). `sim-api` `project::tests` refuses a
capture input that is not a single bit or not a capture pin before the run. End to end through the
real QEMU: `examples/timer-modes` (`firmware/timer_modes`) with button 0 pressed at 3 ms
(`crates/cli/tests/timer_modes.rs`, two runs with equal trace digests). Needs the Xilinx QEMU;
the unit tests need nothing.

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

## Shared-object DPI library (P4, ADR-028)

| Test | Needs | What it establishes |
|---|---|---|
| `dpi-cdylib` `symbols` (3) | nothing (builds the shared object on demand) | the imports of every `.sv` fixture and of the testbench generated for `fixtures/rtl/math/my_math_ip.v` are 31 names with the `rp_`, `zs_` and `tel_` prefixes; each one resolves in the loaded library; on Windows the PE export table of the library is exactly that set |
| `dpi-cdylib` `abi` (1) | nothing (builds the shared object on demand) | the shared object, loaded with `libloading` and called through `extern "C"` pointers with the DPI-C types, speaks remote-port with a fake peer (`interop`): hello, a PS read with latency, a PS write, pin out and in, a pin exchange, an acknowledged wire, a codec register, an LED event from a C string, board inputs, lock-step, PL-to-PS write and read through the memory master, close; malformed endpoints are refused. **No commercial simulator loads it here** |
| `xtask dist` (when `dpi-shared` builds) | the Windows build tools | `lib/zynqsim_dpi.dll` is packed and its unpacked copy must export `rp_open`. Not run for this change (`cargo xtask dist` was not run) |

`cargo xtask dpi-shared` builds the shared object into `target/dpi/release/` for use outside the
tests. `ZYNQSIM_DPI_CDYLIB` points the ABI tests at another build.

## Packaging (P4)

| Test | Needs | What it establishes |
|---|---|---|
| `xtask::dist::tests` (2) | nothing | fixture mentions are found in project files and comments (not in `myfixtures/`, `..` refused); archive and directory names per platform |
| `xtask::clean_unpack::the_archive_unpacks_and_runs_from_a_clean_directory` (`#[ignore]`) | an archive in `dist/` (`cargo xtask dist`), or `ZYNQSIM_DIST_ARCHIVE` | the archive unpacks into an empty directory and, with a PATH of system directories only, `zynqsim --version`, the demo run `--json` (`now_ps` 1000000000) and the `zynqsim-mcp` initialize reply all work; `lib/libxsim_dpi.a` exports `rp_open`. Runs as the last step of `cargo xtask dist` too |
| `sim-api` `tools::tests::the_dpi_library_is_found_in_an_unpacked_release_archive` | nothing | `lib/libxsim_dpi.a` next to the executable is found; a file without `rp_open` is not |

`cargo xtask dist` is the packaging gate: it builds, assembles, archives, writes
`SHA256SUMS` and runs the clean-directory check. Run it on each OS that ships an
archive. Windows is the only one run so far.

## Not covered (be aware)

* **GUI appearance and native dialogs**: pointer click/hold, orbit/zoom, keyboard shortcuts and
  the project window logic are driven by synthetic input in `app::tests`; how it *looks*
  (layout, fonts, colours) is judged only from `zynqsim-gui --screenshot out.png` (also
  `--view3d`, `--project-window`) on Windows, and the native file dialogs are never opened by a test.
* **Sound output**: a test opens the real output device and checks the queue drains in real time;
  nobody has verified by ear that the sound is right.
* **Linux and macOS**: CI passes there, but without QEMU/Vivado, so the engine tests skip; they are only exercised on the Windows development machine. macOS has no run of the XSim crates at all (Windows-only).
* **Coverage-guided fuzzing**: the `fuzz/` cargo-fuzz targets are written and type-checked but have
  never run under libFuzzer (needs Linux and nightly). The seeded tests are the verified part.
* **Third-party viewers**: VCD was not opened in GTKWave and Perfetto JSON was not loaded in the
  Perfetto UI; only their formats are checked.
* **Hardware correctness**: nothing is compared with a real board. Pin assignments are unverified.
* **QEMU session features**: registers at an arbitrary time (not possible, see STATUS.md);
  access (`Z4`) and RAM watchpoints firing; debug sessions' trace digests (not reproducible). Snapshot, restore, reset and replay are covered, by re-execution, on the
  native-PL math project and (replay only) on the RTL board demo by hand.
* **Linux / macOS XSim** and the **Icarus DMA, board, media and pin models**: not run. The Verilator
  and Icarus backends are Windows only.

## Native PS memory and peripherals (P5, optional; ADR-031)

Run: `cargo test -p mem-sys -p ps-periph` (no external tools, no QEMU, no XSim). Clippy and fmt
are checked on the two crates only. The firmware checks in `firmware/conf_*` are encoded as host
tests against the bus; they are not run on the native engine.

| Requirement | Test binary / name | Where |
|---|---|---|
| Address map as data, overlaps, MPCore containment, firmware literals | `mem-sys` unit `map::tests::*` | `crates/mem-sys/src/map.rs` |
| Bus: DDR and OCM bounds, straddling, sizes, overlap refusal | `mem-sys` unit `bus::tests::*` | `crates/mem-sys/src/bus.rs` |
| Unmapped policy (fault, RAZ/WI) and the log | `mem-sys` unit `bus::tests::{unmapped_*, raz_wi_*}` | `crates/mem-sys/src/bus.rs` |
| Sparse DDR pages | `mem-sys` unit `ram::tests::*` | `crates/mem-sys/src/ram.rs` |
| Register lanes and merge | `mem-sys` unit `device::tests::*` | `crates/mem-sys/src/device.rs` |
| RAM snapshot round trip, state hash | `mem-sys` unit `bus::tests::{ram_snapshot_*, state_hash_*}` | `crates/mem-sys/src/bus.rs` |
| UART register model, FIFOs, baud pacing, RX injection, IRQ | `ps-periph` unit `uart::tests::*` (10) | `crates/ps-periph/src/uart.rs` |
| SLCR lock and reset table | `ps-periph` unit `slcr::tests::*` (8) | `crates/ps-periph/src/slcr.rs` |
| GIC: enable, pending, priority, targets, SGI, edge, ack, EOI | `ps-periph` unit `gic::tests::*` (8) | `crates/ps-periph/src/gic.rs` |
| Countdown timers, watchdog, global timer | `ps-periph` unit `timers::tests::*` (9) | `crates/ps-periph/src/timers.rs` |
| TTC counters, interval, match, prescaler | `ps-periph` unit `ttc::tests::*` (7) | `crates/ps-periph/src/ttc.rs` |
| Firmware `conf_scutimer` checks 1-17 (PASS) | `cargo test -p ps-periph --test conf_scutimer -- --nocapture` | `crates/ps-periph/tests/conf_scutimer.rs` |
| Firmware `conf_ttc` checks 1-14 (PASS) | `cargo test -p ps-periph --test conf_ttc -- --nocapture` | `crates/ps-periph/tests/conf_ttc.rs` |
| PPI 29 through the distributor, ICCIAR, ICCEOIR, auto-reload period | `ps-periph --test gic_timer` (3) | `crates/ps-periph/tests/gic_timer.rs` |
| Determinism: same session, same hashes; five runs in one process; RAM snapshot; unmapped count | `ps-periph --test determinism` (5) | `crates/ps-periph/tests/determinism.rs` |
| Mapped system builds, stubs, QSPI image, UART handles through the bus | `ps-periph` unit `system::tests::*` (4) | `crates/ps-periph/src/system.rs` |

Not covered: determinism across processes; any comparison with QEMU or a board; the interrupt
path through a real core (the core is the other engineer's crate).
## Native PS ISS (P5, `cpu-a9`)

| Area | Tests | What they establish |
|---|---|---|
| `cpu-a9` `dp_props` (3) | 10000 seeded inputs per data-processing opcode against an independent reference of the NZCV rules; random immediate shifts; register shift by zero keeps C | the flag and carry rules of the ARM ARM hold for the interpreter |
| `cpu-a9` `isa` (31) | assembled programs (`tests/data/isa_words.inc.rs`, from `gen_isa.py`): branches, loads and stores, LDM/STM, multiplies, halfword multiplies, bit operations, extends, saturation, parallel ops, PKH, MSR/MRS, banking, SVC, undefined, data and prefetch aborts, IRQ and FIQ with return, WFI/WFE/SEV, CP15, MMU refusal, exclusive monitor, VFP and Thumb reporting | each class of ADR-030 works as the ARM ARM says, or is reported as unimplemented |
| `cpu-a9` `mmu` (28) | short-descriptor descriptors built from the ARM ARM bit layouts: section, supersection (40-bit PA), small and large pages with offsets; TEX/C/B/S/nG recorded; AP x privilege x read/write/fetch x XN x PXN matrix against a table; XP = 0 subpages; domains 0 to 3 with fault codes; walk external aborts; TTBCR.N split; TLB hits, stale-until-TLBI, TLBIMVA/ASID/MVAA/ALL, faults not cached; core programs: MMU-on identity run, LDRT/STRT, data abort entry and retry after a table fix, domain fault, XN prefetch abort, alignment with WnR, TLBI by MCR, ATS/PAR; property test against an independent reference walk (3000 table generations, 48000 translations, TLB checked too) | the MMU walk, permissions, faults and TLB behave as the ARM ARM short-descriptor pseudocode says, as far as the tests' own reading of it goes |
| `cpu-a9` `vfp_isa` (14) and `vfp_props` (7) | assembled VFP programs (`tests/data/vfp_words.inc.rs`, from `gen_vfp.py`): immediates, arithmetic, multiply-accumulate family, rounding modes, invalid/DZ/overflow/underflow/denormal flags, NaN propagation, VCMP flags, conversions, fixed-point, transfers, memory and writeback, UNDEFINED and Unimplemented cases; property tests against an independent bignum reference, 10000 cases per operation | VFP results and flags follow the ARM ARM rules and the conventions of ADR-030 |
| `cpu-a9` `isa` (31) | assembled programs (`tests/data/isa_words.inc.rs`, from `gen_isa.py`): branches, loads and stores, LDM/STM, multiplies, halfword multiplies, bit operations, extends, saturation, parallel ops, PKH, MSR/MRS, banking, SVC, undefined, data and prefetch aborts, IRQ and FIQ with return, WFI/WFE/SEV, CP15, MMU refusal, exclusive monitor, VFP reporting | each class of ADR-030 works as the ARM ARM says, or is reported as unimplemented |
| `cpu-a9` `thumb` (30) | the Thumb corpus (`thumb_groups.s`, assembled by the Xilinx toolchain at generation time into `thumb_words.inc.rs`) decodes and runs without Unimplemented; hand-computed behaviour of ALU, shifts, IT blocks, interworking, exception entry (SVC, UDF, BKPT, UDIV), LDM/PUSH/POP, TBB/TBH, exclusives, MRC/MCR, MRS/MSR | each class of the ARM ARM Thumb encodings works as stated, or is reported |
| `cpu-a9` `thumb_props` (9) | A32 against T32 over 10000 seeded cases per class: 16-bit ALU, register and immediate shifts, Thumb-2 modified immediates, shifted and register-shifted data processing, multiplies, bitfields, extends and reversals; equal registers and flags | the Thumb executors match the A32 interpreter, which is itself checked against the assembler |
| `zynqsim` `native_thumb_diff` (2) | `firmware/thumb_hello` (Thumb-2, `thumbv7a-none-eabi`) on the native PS: 13 checks pass, and the UART text equals Xilinx QEMU's (skips the QEMU half without QEMU) | Thumb firmware runs on the native engine with the same output as QEMU |
| `cpu-a9` `rust_firmware` (3) | `rust_hello.elf` runs to completion; every distinct instruction word of the 31 Rust firmware images decodes to an implemented class; a per-image report (`RUSTFW` lines) | the firmware the repository builds runs on the interpreter; no image needs Thumb, VFP or the MMU |
| `zynqsim` `cpu_a9_diff` (1, skips without QEMU) | `rust_hello` through `zynqsim run` (Xilinx QEMU) and through the interpreter: UART text identical | a native run prints what QEMU prints for that image |

To rebuild the assembled test words: `python crates/cpu-a9/tests/data/gen_isa.py crates/cpu-a9/tests/data/isa_groups.s crates/cpu-a9/tests/data/isa_words.inc.rs`
(needs the Xilinx GNU assembler; `ARM_GCC_BIN` overrides its directory). The tests do not need the toolchain.

Limits: no instruction-by-instruction differential run (needs the QEMU gdb stub driven per step); the
firmware word sweep proves decoding, not behaviour; `rust_firmware` assertions cover one image only.
* **Icarus backend**: the math design only (`cli::icarus`, `determinism` case `math-icarus`);
  the DMA regression, board, media and pin models are not run under Icarus. Windows only.
  **Linux / macOS XSim**: not implemented. The Verilator backend is Windows only for now.
* **Verilator against XSim**: the math project gives the same firmware output and bus traffic in
  both (`cli::verilator`), but this is one design. The DMA regression, the board, media and pin
  models have **not** been run under Verilator. Trace digests are compared, not asserted equal,
  by the test (equal in the one run recorded in STATUS.md).
## Native PS engine (P5, `backends.ps = "native"`, ADR-033)

Run: `cargo test -p sim-api --lib native_ps` (no external tools; under a second once built) and
`cargo test -p zynqsim --test native_ps_diff -- --nocapture` (about 2.5 minutes, nearly all of it
the QEMU halves; they print `SKIPPED:` without the Xilinx QEMU). `cpu-a9`, `mem-sys` and
`ps-periph` are built at opt-level 3 in the dev profile for this reason.

| Requirement | Test binary / name | Where |
|---|---|---|
| The engine is `Send` (the server thread owns it) | `sim-api` unit `native_ps::tests::the_engine_is_send_*` | `crates/sim-api/src/native_ps.rs` |
| rust-hello prints its text natively and parks | `sim-api` unit `native_ps::tests::rust_hello_prints_its_text_and_parks` | same |
| Core 0 starts in Supervisor mode at the entry, IRQ and FIQ masked; core 1 refused | `sim-api` unit `native_ps::tests::core_zero_starts_*` | same |
| Five engines in one process give one digest | `sim-api` unit `native_ps::tests::five_runs_give_one_trace_digest` | same |
| Breakpoint by symbol stops before the instruction; resume is not stopped again | `sim-api` unit `native_ps::tests::a_breakpoint_*` | same |
| Unimplemented instruction (VFP word): stop, pc and word named, sticky | `sim-api` unit `native_ps::tests::an_unimplemented_*` | same |
| Snapshot restore reproduces the trace digest, cycle and state hash | `sim-api` unit `native_ps::tests::a_snapshot_*` | same |
| Memory: image visible, device space refused | `sim-api` unit `native_ps::tests::memory_*` | same |
| Reset to time zero | `sim-api` unit `native_ps::tests::reset_*` | same |
| UART RX injection; bad port refused | `sim-api` unit `native_ps::tests::uart_input_*` | same |
| Project validation: PL, `[cpu1]`, `[native]`, `[devices]`, `[media]`, `boot_bin` | `sim-api` unit `native_ps::tests::validation_*` | same |
| rust-hello: UART text identical to QEMU | `zynqsim` `native_ps_diff::rust_hello_prints_the_same_text_on_qemu_and_natively` | `crates/cli/tests/native_ps_diff.rs` |
| conf-scutimer: 17 PASS natively; the 3 QEMU N/A checks are the only differences | `zynqsim` `native_ps_diff::conf_scutimer_*` | same |
| conf-ttc: check 12 and check 9 are the only differences; result FAIL pinned | `zynqsim` `native_ps_diff::conf_ttc_*` | same |
| Five separate processes give one trace digest | `zynqsim` `native_ps_diff::five_processes_give_one_trace_digest` | same |
| `inspect` lists the native PS as available | `zynqsim` `native_ps_diff::inspect_lists_the_native_ps_as_available` | same |
| A native project with `[cpu1]` is refused with the reason | `zynqsim` `native_ps_diff::a_native_project_with_a_second_core_*` | same |
| `mem-sys` RAM reads after the single-lookup change | `mem-sys` unit `ram::tests::*` | `crates/mem-sys/src/ram.rs` |

Not covered: the GUI project window (no GUI test drives the PS choice); the native `file:line`
breakpoint form and recording replay of a native session; interrupt latency measured against the
bound; any comparison with a board.

## Native RTL front end (P6, optional)

Run: `cargo test -p rtl-frontend` (93 tests, about 4 s) and `cargo test -p zynqsim --test rtl_check`
(5 tests). Nothing here needs an external tool, so none of these tests skip.

| Test | Needs | What it establishes |
|---|---|---|
| `rtl-frontend` `lexer_pp` (23) | nothing | literals (sized, based, unsized, fill, reals), operators, comments, continuations, positions; macros (object, function, defaults, pasting); conditionals; `include` (file-relative, cycles, missing); `timescale`, `default_nettype`, `-D` |
| `rtl-frontend` `parser` (40) | nothing | every construct in `docs/RTL_SUBSET.md` parses to the expected AST shape; operator precedence through an S-expression printer; unsupported constructs are named with their position and parsing continues; a broken statement does not lose the next module |
| `rtl-frontend` `elab` (26) | nothing | constant evaluation (widths, signs, shifts, reductions, `$clog2`, constant functions, step limit); parameter overrides; generate unrolling and paths; widths of ports and nets; undefined module, port, identifier and function checks; recursion limit |
| `rtl-frontend` `corpus` (1) | the repository's sources (no tool) | every Verilog and SV file under `zynqsim/{fixtures,examples,tests}` (148): the pinned list of 39 files with their constructs; no other file produces diagnostics; `RTL_CORPUS_PRINT=1` prints the current list |
| `rtl-frontend` `fuzz` (3) | nothing | 5000 mutated sources: no panic, no budget exhaustion; the lexer loop found by the fuzzer; the budget stops a large input |
| `zynqsim` `rtl_check` (5) | the built binary | exit codes 0, 1 (`file:line:col: construct: message`), 2 (unreadable file); JSON hierarchy and port widths with `--top`; `-I` and `-D` |

Not covered: that a parse-clean design elaborates or simulates like XSim (no differential run yet);
the DPI, class, interface and package constructs (reported, not parsed); performance on large designs
(the corpus is the largest input; a 2000-module input is only checked against the budget).

## Native RTL engine (P6, optional; ADR-034)

Run: `cargo test -p rtl-engine` (60 tests, about 20 s). The XSim test needs Vivado and `cargo xtask dpi`
(it prints `SKIPPED:` and passes without Vivado). The corpus test rewrites `docs/CORPUS_NATIVE.md`.

| Test | Needs | What it establishes |
|---|---|---|
| `rtl-engine` `scheduling` (11) | nothing | NBA swap (and blocking swap), race-free counter, delta cycles through assigns, `negedge`, delays in the `timescale` unit, `$finish`, combinational-loop report, budget report for a loop without timing control, `always_comb` at time 0 |
| `rtl-engine` `expr` (18) | nothing | literals, concatenation and replication, selects (also of memory words and ascending ranges), reductions and logical operators, ternary, context width, mixed signedness, `>>>`, division, memories, constant functions, x and z as 0, the 128-bit refusal, memory bit targets, functions that write memories declared after them |
| `rtl-engine` `property` (3) | nothing | seeded random operands against an independent Rust reference: add, subtract, multiply, shifts, comparisons, divide and modulo; every width 1 to 64 both signednesses; widths 65 to 128 |
| `rtl-engine` `constructs` (18) | nothing | `case`/`casez`/`casex`; loops with `break`/`continue`; functions; tasks with delays and outputs; `$readmemh`; `$display` formats; `$finish`; generate and parameters with hierarchical names; refusals (recursion, unknown tasks, missing modules, `$random`, diagnostics stop a simulation); `set`/`get` rules; snapshot and restore (the same future, refused for another design or a truncated snapshot); identical state hashes over five runs; VCD structure; `tick_clock` |
| `rtl-engine` `math_ip` (7) | `fixtures/rtl/math` | `my_math_ip` through `RtlModule` against `pl_native::MathIp` (the firmware golden model): writes, reads, strobes, reset, unmapped offsets, aliasing past the address width (a known difference, pinned) |
| `rtl-engine` `xsim_diff` (1) | Vivado XSim, `cargo xtask dpi` | the same register stimulus on `my_math_ip` in XSim (through `XsimPl`) and in the engine: every read agrees |
| `rtl-engine` `corpus` (1) | `tests/rtl` (no tool) | six of nine corpus designs pass their `CORPUS_PASS` testbenches; uriscv, pulp-common-cells and vivado-funcsim are refused for the stated reasons; the table is pinned and written to `docs/CORPUS_NATIVE.md`; `irq_ip`, `oled_spi`, `rgb_pwm` elaborate |
| `rtl-engine` unit (1) | nothing | `timescale` units to picoseconds |

Not covered: 4-state behaviour (none exists); the `backends.pl = "rtl-native"` switch (not wired); a
differential run against XSim for any design but `my_math_ip`; `$readmemb`; performance; Windows-only
checks of the XSim path on other hosts.

## ps-periph batch 2 (GPIO, SPI, I2C, XADC, devcfg, CAN, watchdog)

* `cargo test -p ps-periph -p mem-sys`: unit tests of each new model, `conf_gpiops`, `conf_spips`,
  `conf_iicps`, `conf_xadcps` (firmware checks encoded), `bsp_periph2` (CAN and devcfg BSP
  sequences, watchdog reset and timer modes, private timer rate), `determinism_periph2`.
* `cargo clippy -p ps-periph -p mem-sys --all-targets -- -D warnings`.
* Not in the gate: the QEMU BSP examples are unchanged; no native-engine run of BSP ELFs.

## Dual-core native PS (ADR-033, "Two cores")

| Requirement | Test |
|---|---|
| Core 1 parks in the boot-ROM loop; runs only after the mailbox release | `sim-api` `native_ps::tests::core1_stays_in_the_boot_rom_until_core0_writes_the_mailbox` (hand-written A32) |
| Exclusive monitor across cores: a store to the word or its 8-byte granule clears the other reservation; the next granule does not | `native_ps::tests::a_store_by_another_core_clears_the_exclusive_reservation_of_its_granule` |
| SEV wakes WFE on another core | `native_ps::tests::sev_on_core0_wakes_a_wfe_on_core1` |
| WFI wakes on a pending SGI with IRQs masked | `native_ps::tests::an_sgi_from_core0_wakes_a_wfi_on_core1_with_irqs_masked` |
| SGI filters; SGI versus PPI priority | `ps-periph` `gic::tests::sgi_filters_pick_the_cores_and_priorities_order_sgi_against_a_ppi` |
| MPIDR per core; ACTLR SMP bit | `cpu-a9` `cp15::tests::mpidr_carries_the_core_number_and_actlr_keeps_the_smp_bit` |
| SCU registers | `ps-periph` `mpcore::tests::{scu_control_config_and_invalidate_all, scu_filter_and_access_registers_store_their_fields}` |
| Round-robin interleaving, equal cycles per core | `native_ps::tests::the_cores_interleave_round_robin_and_run_the_same_cycles` |
| Snapshot of two cores | `native_ps::tests::a_two_core_snapshot_restores_both_cores` |
| Dual-core demo: UART1 identical to QEMU with the RESULT line; UART0 native-only | `cli/tests/native_ps_diff::dual_core_demo_prints_the_same_uart1_text_on_qemu_and_natively` |
| Determinism: 5 in-process runs; 5 processes | `native_ps::tests::a_two_core_session_gives_one_trace_digest_five_times`, `native_ps_diff::dual_core_five_processes_give_one_trace_digest` |
