# ZynqSim: handover and plan for what is left

This file is for the next engineer or agent. It says where the project stands, how to work in it
without repeating the mistakes already made, and gives a plan, with acceptance tests, for every
open item that is **not** impossible. Read it together with `docs/STATUS.md` (the honest per-subsystem
status; it wins over any prose here), `docs/DEMO.md` (what to run to see everything) and the ADRs in
`docs/adr/` (decisions and their limits).

Written on 2026-10-17 at commit `38b8452` (branch `p9-merge`).

## 1. State in one page

* Spec: `../docs/RUST_SOC_SIM_PROMPT.md`. Milestones P0-P4 are implemented to the extent STATUS.md
  states; P5/P6 (native PS and native RTL) exist as a usable first slice, not as full replacements.
* Last complete local gate (`cargo xtask ci`): 1285 tests passed on `p9-merge` plus the pieces the gate
  did not reach, run separately (`stdio`, `waveform`, doc-tests, release build); after the last small
  changes only the affected sets were re-run (GUI 91+1, xtask 20, sim-api lib 119). The demo tour
  (`cargo xtask demo`) is 47 PASS / 1 SKIPPED (the skip was fixed afterwards).
* GitHub CI could not verify the newest work: the free Actions minutes ran out. PRs run Linux only now
  (`.github/workflows/ci.yml`); the three-OS matrix runs on pushes to `main` and on demand. The last
  three-OS results are those of PRs #5-#14 (green).
* **Branch state.** `origin/main` was still `f6a857f` when this was written. The newest verified work is
  `origin/p9-merge` and it is a fast-forward of `main`; the owner merges it:
  `git fetch origin && git push origin origin/p9-merge:main`. PRs #5-#16 are stacked and obsolete once
  that is done (close them with a note). Do not push to `main` yourself unless the owner says so.
* Everything that works needs, for the QEMU and XSim paths, a Windows machine with the Xilinx tools
  (Vitis 2022.1 QEMU, Vivado XSim). The native engines, Verilator and Icarus run without them
  (Verilator/Icarus come from a portable oss-cad-suite in `%TEMP%\zs-tools\verilator`).

## 2. Map of the code

| Area | Where |
|---|---|
| Control plane, projects, engines | `crates/sim-api` (`project.rs`, `qemu_engine.rs`, `native_ps.rs`, `sim.rs`, `types.rs`) |
| XSA / Vivado ingestion | `crates/xsa-import`, `sim-api/src/xsa.rs`, `xsim-export`, `sim-api/src/exportsim.rs`, `exportinit.rs`, `boot` + `bootbin.rs` |
| Testbench generation | `sim-api/src/tbgen.rs` (AXI4-Lite slave bridge, AXI4 master bridge, irq, pin groups, protocol checker `fixtures/rtl/common/zs_axi_checker.sv`) |
| PS: QEMU | `crates/backend-qemu` (`dtb.rs` device-tree patches, `gdb.rs`, `net.rs`) |
| PS: native | `cpu-a9` (A32, Thumb-2, VFPv3, MMU), `mem-sys`, `ps-periph`, `ps-bus` |
| PL: external RTL | `backend-rtl-xsim`, `backend-rtl-verilator`, `backend-rtl-icarus` (`vpi-icarus`), DPI library `xsim-dpi` (+ `dpi-cdylib`) |
| PL: native | `pl-native`, `vendor-ip` (GPIO, Timer, UART Lite, DMA, IIC, BRAM, INTC), `rtl-frontend`, `rtl-engine` (4-state) |
| Board, devices | `board-zybo` (also ZedBoard, PYNQ-Z2, Z7-10 definitions), `devices` (OLED, sensors, 8LD, SSD, BTN, Pcam) |
| Front ends | `cli`, `gui`, `mcp` (depend on `sim-api` only) |
| Firmware | `firmware/` (own Cargo workspace, target `armv7a-none-eabi`; prebuilt ELFs are committed in `fixtures/elf`) |
| Automation | `crates/xtask`: `ci [--fast|--det-only a,b]`, `dpi`, `dpi-shared`, `firmware`, `dist`, `demo`, `changed-examples` |

ADR numbers 009 and 010 are referenced from code (bidirectional Pmod pins, Pcam) but the files were never
written: see task H1.

## 3. Working rules that were learned the hard way

1. **Honesty contract.** STATUS.md rows are `verified` (a named test proves it), `implemented-untested`,
   `stub`, `not-planned`. Register facts from memory carry `VERIFY:`. Never write more than a test shows.
2. **No C/C++ authored** (Xilinx's own sources are compiled only as test inputs; Verilator output and VPI
   declarations in Rust are fine). Firmware is Rust `no_std`.
3. **Rebuild the DPI library** (`cargo xtask dpi`) after any change in `xsim-dpi`, and the firmware ELFs
   (`cargo xtask firmware`) after any change in `firmware/rt`: a changed `rt` changes every ELF, and then
   `fixtures/boot/BOOT.BIN` and `fixtures/handoff/BOOT.BIN` must be regenerated with Vitis `bootgen`
   from the same recipes (`boot.bif`), or the boot tests fail.
4. **Never `export ZYNQSIM_NO_BUILD_CACHE=1` for a gate** (one test checks the cache).
5. **Known intermittent failures** (all pass when re-run alone; the cause is wall-clock timing under load):
   `backend-rtl-xsim board::shell_leds_switches_and_buttons` and `mcp board_demo_through_mcp` (host-written UART
   input reaches the guest at a wall-clock dependent virtual time, windows were widened), `zynqsim cli
   board_demo_image_filter_...` (once), a QEMU that hangs (tests now kill a run after 10 minutes). If a test fails:
   kill stray `qemu-system-aarch64.exe`/`xsim.exe`, re-run it alone, only then look at your change.
6. **Do not run two gates at once** and do not start builds while one runs; XSim/QEMU runs are
   wall-clock sensitive. The full gate takes 45-60 minutes; `cargo xtask ci --fast` (about 18 minutes) skips
   every test that needs an external tool and does not replace the full gate before a merge.
7. **Fixtures that look stale are usually test expectations that encode an old fact.** When a feature makes a
   "not implemented" path unreachable (a backend, an instruction), update the test that pinned the old
   message (this happened four times: backends list, `ps = "native"`, Icarus, an NEON/VFP word).
8. **New `examples/<dir>`** must be added to `crates/cli/tests/determinism.rs` (`CASES` or `EXCLUDED`):
   the test fails on an uncovered directory.
9. Windows path quirks: use forward slashes in generated TOML, `relative_path` canonicalizes (macOS
   `/var` vs `/private/var`, Windows verbatim prefix).

## 4. Working with sub-agents (what worked)

* Haiku for well-specified, checkable work (a model, a test table, a checklist, running and reading a gate);
  Sonnet for design and for anything where a wrong first decision costs a re-run. The token budget
  matters: ask for reports under 150 words and keep logs in files.
* An agent started with `isolation: "worktree"` begins from the repository's FIRST commit, not your branch:
  its first step must be `git checkout -b <name> && git merge --ff-only <your-branch>`.
* Give each parallel agent its own files; shared files (`types.rs`, `project.rs`, `qemu_engine.rs`,
  `native_ps.rs`, `STATUS.md`, `TESTPLAN.md`, `README.md`, `xtask`) get small additive edits. Merge branches yourself
  one after another into an integration branch; conflicts in lists (workspace members, FIRMWARE list,
  README rows) are "keep both"; conflicts in code need a human decision (an automatic "keep both" broke
  `cpu-a9/state.rs` once).
* Resume a stopped agent with `SendMessage` rather than starting a new one. Agents stop when the session
  ends or a usage limit hits: their worktrees keep the uncommitted work, so tell agents to commit early.
* Long commands run in the background with output to a file; do not poll in short loops.

## 5. The plan

Priority is value over effort. Effort is S (hours), M (a day), L (several days); these are estimates, not
measurements. Every task ends with: tests named in the commit, STATUS.md rows updated honestly, an ADR when a
decision was made, the affected `examples/` added to the determinism table, and `cargo xtask ci` green.

### A. `ps7_init` replay and the PS clock tree (MOSTLY DONE, ADR-038)
* **Status.** Parser, clock-tree derivation, `[boot] ps7_init` switch, poll-timeout diagnostic and the native-PS replay are **done and verified** (`cli::tests::ps7_init_replay`, ADR-038). **Open:** on QEMU only SLCR writes get through the HP0 path (DDRC/DEVCFG are skipped), and the QEMU private timer still ticks at 100 MHz, so the 333.33 MHz acceptance test on QEMU is **not met**.
* *Original plan, kept for reference:*
* **Why.** The XSA carries `ps7_init.tcl/.c/.h`; today the data is only listed. Real systems get their PLLs, clock
  dividers, MIO muxing, DDR and peripheral enables from it. QEMU's reset-state clock tree is why the A9 private
  timer ticks at 100 MHz instead of 333 MHz (the global timer was fixed by a DTB property; the private one was not).
* **How.** (1) Parse the `mask_write`/`mask_poll`/`mask_delay`/`write` sequences of `ps7_init.tcl` in
  `xsa-import` into an ordered list of `(op, addr, mask, value)`. (2) Apply it before the guest's first
  instruction: on QEMU through the PS memory-write path the engine already has (`mem_write`, remote-port
  HP0 route: check that it reaches SLCR/DDRC/MIO addresses, otherwise add a tiny A32 stub that performs the
  writes and runs first), on the native PS by writing the same registers into `ps-periph` (SLCR models the
  PLL/clock registers; derive the CPU/peripheral clocks from them). (3) Project switch `[boot] ps7_init = true|false`
  (default on when an XSA/BOOT.BIN provides one, otherwise unchanged behaviour, so existing examples and hashes stay
  put). (4) `mask_poll` must terminate: model PLL lock bits.
* **Acceptance.** `examples/vivado-handoff` runs with the replay on; a new test reads the private timer rate on
  QEMU after the replay (should be 333.33 MHz: measure with `firmware/timer_rate`); BSP `sleep()` examples take
  the right simulated time; native PS gives the same clock numbers. A negative test: a poll that never completes
  fails with a message naming the register. Update the rows that say `ps7_init` is not applied.
* **Risk.** QEMU may reject some writes (DDR controller, MIO of absent blocks): log and skip only registers on an
  explicit allow-list file, never silently.
* Agent: Sonnet to design, Haiku for the parser and tests.

### B. More than one AXI4-Lite slave, AXI4 slaves, HP/ACP masters from an XSA (COMPLETED, ADR-039)
* **Status.** **Done and verified.** Multi-slave user RTL (`user_slaves >= 2`) generates an address decoder testbench with per-slave read/write tasks and sequential protocol checkers (`zs_chk_lite_{i}`). Pure-Rust `rtl-native` maps multiple `RtlModule` instances to the native bus. Master ports (`S_AXI_GP0..1`, `S_AXI_HP0..3`, `S_AXI_ACP`) map dynamically to remote-port devices 0..6.
* **Evidence.**
  - `fixtures/xsa/zs_two_math.xsa` (Vivado 2022.1 handoff with `u_math0` at `0x43C00000` and `u_math1` at `0x43C10000`).
  - `crates/cli/tests/multi_slave.rs`:
    - `init_xsa_multi_slave_produces_valid_project`: verified.
    - `multi_slave_runs_on_rtl_native`: verified (`RESULT: ALL PASS`, `math0: 21 * 2 = 42`, `math1: 50 * 2 = 100`, `bus_accesses: 5`, `protocol_checks: 0`).
    - `multi_slave_runs_on_xsim`: verified (`RESULT: ALL PASS`, `bus_accesses: 5`, `protocol_checks: 0`).
    - `multi_slave_runs_on_verilator`: verified (`RESULT: ALL PASS`, `bus_accesses: 5`, `protocol_checks: 0`).
    - `multi_slave_runs_on_icarus`: verified (`RESULT: ALL PASS`, `bus_accesses: 5`, `protocol_checks: 0`).
  - `sim_api::tbgen::tests::{multi_slave_testbench_generates_instances_decoders_and_checkers, multi_slave_testbench_with_master_and_irqs, master_interface_device_maps_standard_ports}`.
  - `sim_api::native_pl::tests::multiple_rtl_modules_on_the_rtl_engine_answer_on_the_ps_bus`.
  - Master bridge verified by `cli::tests::cli::init_ip_generates_the_dma_testbench_with_a_memory_bridge`.
  - Protocol checkers verified by `cli::tests::protocol_checks::*` (9 tests passing: legal produces 0, illegal fixtures caught).
  - Single-slave backward compatibility preserved (all 6 tests in `cli::tests::xsa` pass).


### C. Run the Xilinx BSP applications natively and finish native-math (COMPLETED)
* **Status.** **Done and verified.** Implemented in commit `08c40d3`: devcfg mapping, CP15 VFP/NEON coprocessor enable, full VFP instruction dispatch and single-precision arithmetic, Thumb-2 parallel arithmetic/saturating instructions (`qadd`, `qsub`, `sadd16`, etc.), and unmodified Xilinx BSP execution.
* **Evidence.**
  - `examples/native-math` passes with `ps = "native"` and `pl = "rtl-native"`, printing `RESULT: ALL PASS` on UART1 with 7 vectors and 21 bus accesses.
  - Determinism: verified in `crates/cli/tests/determinism.rs` (`fnv1a64:0a8f224ff0ba710c` across 5 runs).
  - BSP applications pass in `crates/cli/tests/bsp_native.rs` and `crates/cli/tests/native_ps_diff.rs`.


### D. IP models, and real IP through the Vivado simulation export (effort M per IP; L overall)
* **Status.**
  - **AXI Quad SPI v3.2 (COMPLETED, ADR-040):** Implemented in `crates/vendor-ip/src/spi.rs` as a `pl_native::RegisterBlock` (`DGIER`, `IPISR`, `IPIER`, `SRR`, `SPICR`, `SPISR`, `SPIDTR`, `SPIDRR`, `SPISSR`, `TX_FIFO_OCY`, `RX_FIFO_OCY`), with loopback mode, FIFO reset bits, slave select active-low masking, and interrupt generation to `IRQ_F2P`. Verified with guest firmware `firmware/spi_demo`, `fixtures/elf/rust_spi.elf`, `examples/spi-demo`, `cli::native_pl::axi_quad_spi_firmware_runs_on_the_native_ps_with_the_vendor_model`, `sim-api::vendor_ip2::the_axi_quad_spi_model_runs_loopback_transfers_and_interrupts`, and determinism 5/5 identical runs (`fnv1a64:ad75b66de3ec904e`).
* **Remaining Models.** Candidates in order: AXI VDMA, AXI Ethernet Lite, AXI4-Stream FIFO, Clocking
  Wizard (registers), Video Timing Controller, AXI4-Stream to Video Out. Pattern: `crates/vendor-ip`,
  XSA mapping in `sim-api/src/xsa.rs`, an example with Rust firmware, unit tests per register and end-to-end tests.
* **Real IP.** For designs whose IP has a Vivado simulation model, `export_simulation` gives the compiled-library flow:
  verify `-L secureip`/`unisims_ver` runs for a design with an AXI GPIO or a clocking wizard through XSim
  (`docs/adr/017-export-simulation.md` says this was not verified), then document which IP can run this way
  instead of a model. This avoids guessing register behaviour.
* **Acceptance.** Per IP: a design containing it runs its Vitis driver example (`fixtures/bsp` has `xaxidma`/`xiic`
  style examples; add the missing ones) and ALL PASS.

### E. FSBL execution (effort L, optional)
* Run the user's FSBL ELF from OCM (BootROM stub: load FSBL at 0x0, set the registers the BootROM leaves, jump), let it
  read the partitions from the BOOT.BIN (SD/QSPI model already exists) and hand off. Depends on A (FSBL uses
  `ps7_init`), on SD/QSPI behaviours of ADR-012 and on C for the native route. Only worth it if users bring their own
  FSBL. Acceptance: a Vitis-built FSBL+app BOOT.BIN boots to the app on QEMU.

### F. Platforms (effort M)
* Linux and macOS: run `cargo xtask dist` and the clean-unpack test in CI (one Linux job is cheap), document Xilinx tool
  locations on Linux (`/tools/Xilinx`, `/opt/Xilinx`), make Verilator/Icarus discovery work there (`sim_api::locate*` has
  the search order), keep macOS to native + Verilator + Icarus (no Vivado/Vitis there).
* Licence and metadata: the workspace says Apache-2.0 only and the repository URL is a placeholder; ask the owner
  before adding any LICENSE file.
* When CI minutes return, run the three-OS matrix once on `main` and record the result in STATUS.md.

### G. Hardware comparison kit (effort S-M; the comparison itself needs a board)
* A document and a script: capture the UART of a real Zybo (serial terminal, a log file), run the same ELF
  here, diff the two logs with a tolerance for timing lines; for registers, a firmware that dumps a register table
  on both. Gives users the evidence the simulator cannot give itself. Add the first real comparison when someone has a board.

### H. Quality and housekeeping
* **H1.** Write the missing ADR-009 (bidirectional Pmod pins and the pin-exchange timing guarantee) and ADR-010 (Pcam): code
  comments refer to them (`grep -rn "ADR-009\|ADR-010" crates`). Source of truth: the code and STATUS.md rows.
* **H2.** Flaky UART input: make host-written UART bytes arrive at a deterministic virtual time (an idea: deliver them through
  a device on the remote-port bus at the requested virtual time instead of a TCP chardev; or use the native PS where the UART
  is a model). Then drop the widened windows and `board-demo` can join the determinism table.
* **H3.** The QEMU hang seen twice under load (a QEMU stuck while XSim waits): reproduce with a loop of 3 parallel `zynqsim run`
  of `examples/iic-bram-demo`, capture `qemu.err.log` and the remote-port trace, find the lost sync.
* **H4.** Gate speed: profile with the per-step timings `cargo xtask ci` prints now; likely wins are `cargo nextest`,
  fewer test binaries, shorter simulated durations, a smaller determinism table for unchanged examples (`ci --det-only`).
* **H5.** GUI items from the reviews: 3D view, memory viewer and running-status text were not re-photographed after the last
  merges; no GUI import of an XSA/BOOT.BIN (File > New from XSA is absent); Pmod panel for the Pcam; storage panel "writes
  discarded" wording.
* **H6.** Private timer rate in the QEMU machine (task A may fix it), devcfg/SCU watchdog models in the QEMU machine are
  not available in this QEMU build: the native PS has them; document the route instead of patching QEMU.

## 6. Not planned (and why)

* Bitstream-level simulation, placement/routing/timing behaviour: the PL runs from RTL.
* Distributing Xilinx's QEMU or XSim: licensing.
* Proving equivalence with hardware without hardware: see G for what users can do.
* VHDL in the native engine, SystemVerilog classes/UVM/interfaces, SDF timing: stated as `stub`/`not-planned` in STATUS.md.

## 7. Suggested order and parallelism

1. Owner merges `p9-merge` into `main`; close PRs #5-#16.
2. Start A, B and C in parallel (disjoint files: `xsa-import`+`qemu_engine`/`ps-periph`, `tbgen`+`xsa.rs`, `ps-periph`+`native_ps`);
   merge one after the other; one full gate after the last merge.
3. D and F next; H1 is a quick win for any idle agent; H2/H3 when a flaky test hurts again.
4. E and G only on demand.

## 8. Commands cheat sheet (Windows, Git Bash)

```
export PATH="$HOME/.cargo/bin:/c/Qt/Tools/mingw1310_64/bin:$PATH"   # cargo is not on PATH by default
cd zynqsim
cargo xtask dpi            # after xsim-dpi changes
cargo xtask firmware       # after firmware/rt changes (then regenerate the BOOT.BIN fixtures)
cargo xtask ci --fast      # ~18 min, no external tools needed
cargo xtask ci             # 45-60 min, the real gate (QEMU + XSim + Verilator + Icarus)
cargo xtask demo --quick   # 5 min tour, artifacts in demo-out/
cargo xtask changed-examples
```
