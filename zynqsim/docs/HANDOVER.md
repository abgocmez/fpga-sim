# ZynqSim: handover (round 2) and plan for what is left

For the next engineer or agent (written for continuing in Antigravity). Read it with `docs/STATUS.md` (the honest
per-subsystem status; it wins over any prose here), `docs/DEMO.md` (what to run to see everything),
`docs/TEST_YOUR_DESIGN.md` (the user path) and the ADRs in `docs/adr/`.

Written on 2026-10-19 at commit `050f010` (branch `p12-merge`, pushed to `origin/p12-merge`).
Replaces the round-1 handover; its finished items are summarised in section 2.

## 1. State in one page

* Spec: `../docs/RUST_SOC_SIM_PROMPT.md`. P0-P4 are implemented to the extent STATUS.md states; P5/P6 (native PS,
  native RTL) are a usable first slice.
* **Branches.** `origin/main` is still old (`f6a857f` when last checked). `origin/p12-merge` contains everything
  (it includes `p9-merge`); it is a fast-forward of `main`. The owner merges: `git fetch origin && git push origin
  origin/p12-merge:main`. Do not push to `main` yourself. PRs #5-#16 are stacked and obsolete after that.
* **Gates on `050f010`:** `cargo xtask ci --fast` green (9 min 12 s). The **full** gate (`cargo xtask ci`, 45-60 min,
  needs QEMU + XSim + Verilator + Icarus) was last run on `p9-merge` (1285 tests) and **not** on `p12-merge`.
  Independently re-run on `p12-merge` and green: `ps7_init_replay`, `bsp_native` (16), `native_ps_diff` (8),
  `native_pl` (9), `vendor_ip2`, `xsa_import`, `multi_slave` (5), `handoff` (4), lib tests, fmt, clippy; determinism
  5/5 for `spi-demo`, `axis-fifo-demo`, `clk-wiz-demo`. **First job: run the full gate once.**
* GitHub Actions minutes ran out: PRs run Linux only (`.github/workflows/ci.yml`); the 3-OS matrix runs on `main`
  and on demand.
* QEMU and XSim paths need a Windows machine with Vitis 2022.1 QEMU and Vivado XSim. Native engines, Verilator and
  Icarus run without them.

## 2. What round 2 delivered (all in `p12-merge`)

| Item | Result | Evidence |
|---|---|---|
| ADR-009 (bidirectional Pmod pins), ADR-010 (Pcam) | written | `docs/adr/009-*.md`, `010-*.md` |
| Native BSP apps, VFP, Thumb-2 parallel math, devcfg, native-math | done, verified | `bsp_native`, `native_ps_diff`, determinism `native-math`; TTC ISR now clears on read (matches QEMU, test expectation updated on purpose) |
| `ps7_init` replay + PS clock tree (ADR-038) | native: done; QEMU: **partial** | only SLCR gets through the HP0 path, DDRC/DEVCFG skipped (`qemu_skips_addr`); QEMU private timer still 100 MHz |
| Multi AXI4-Lite slave, HP/ACP masters from an XSA (ADR-039) | done, verified on rtl-native, XSim, Verilator, Icarus | `multi_slave.rs`, `fixtures/xsa/zs_two_math.xsa` |
| AXI Quad SPI (ADR-040) | done, tested | `vendor-ip/src/spi.rs`, `examples/spi-demo` |
| AXI4-Stream FIFO (ADR-041) | implemented + tested, `VERIFY:` items open | `vendor-ip/src/axis_fifo.rs`, `examples/axis-fifo-demo` |
| Clocking Wizard (ADR-042) | implemented + tested, `VERIFY:` items open; model changes no clock | `vendor-ip/src/clk_wiz.rs`, `examples/clk-wiz-demo`; `xsa-import/src/hwh.rs` reclassifies a `clk_wiz` with an AXI4-Lite window from passive to native |
| AXI VDMA (ADR-043) | implemented + tested, `VERIFY:` items open | `vendor-ip/src/vdma.rs`, `examples/vdma-demo`; 2D line stride, park & circular buffer store addressing up to 32 frame stores, stream loopback |
| AXI Ethernet Lite (ADR-044) | done, tested | `vendor-ip/src/ethernetlite.rs`, `examples/ethlite-demo` |
| Video Timing Controller (ADR-045) | done, tested | `vendor-ip/src/v_tc.rs`, `examples/vtc-demo` |
| AXI CDMA (ADR-046) | done, tested | `vendor-ip/src/cdma.rs`, `examples/cdma-demo` |
| P1: User path, `zynqsim doctor`, `try-xsa` | done, verified | `zynqsim doctor`, `cargo xtask try-xsa`, `demo::xsa-native-*`, `docs/TEST_YOUR_DESIGN.md` |
| Fast gate, demo tour, packaging | done | `cargo xtask ci --fast`, `cargo xtask demo [--quick]` |

## 3. Can a user try simple Vivado/Vitis code today?

**Yes, for a defined class; no for "anything".** Honest summary (STATUS.md has the rows):

Works (needs QEMU+XSim for the full path; the native path needs neither):
* A **Vitis bare-metal ELF** for Cortex-A9 (hello world, GPIO/timer/UART/GIC/TTC, Xilinx BSP drivers, many BSP
  example apps: `fixtures/bsp`) run on QEMU, or natively with `ps = "native"`.
* A **Vivado XSA** (`write_hw_platform`) with the PS7 and any of: AXI GPIO, Timer, UART Lite, DMA, VDMA, IIC, BRAM, INTC,
  Quad SPI, AXI4-Stream FIFO (MM), Clocking Wizard (as models), plus **one or more user AXI4-Lite slaves** in
  Verilog/SystemVerilog (module reference). `zynqsim init --xsa design.xsa --rtl my_ip.v --boot-bin BOOT.BIN`
  (or `--elf`). A **BOOT.BIN** from `bootgen` boots (only the application partition runs).
* A **Vivado simulation export** (`export_simulation`) or post-synthesis netlist as the PL, run in XSim.
* The user RTL can also run in Verilator, Icarus or the native 4-state engine without Vivado.

Does not work / not proven:
* An XSA containing any IP without a model stops the run and is listed under `[xsa.unmodelled]` (Ethernet Lite,
  video IP, CDMA, most others).
* The FSBL is never executed; the bitstream is not simulated; no timing; QEMU gets `ps7_init` only partially.
* All Xilinx IP are register-level models from product guides, **never compared with hardware** (`VERIFY:`).
* AXI4 slaves and AXI-Stream in user RTL, non-32-bit AXI masters, audio/video from an XSA: hand-written testbenches only.
* Windows is the tested platform; Linux/macOS dist is built but unverified. Using it needs `cargo` or the release
  archive, and installed Xilinx tools for the QEMU/XSim paths.

Known stale text in `docs/TEST_YOUR_DESIGN.md` (fix in task P1): it still says `ps7_init` is not applied and "one
user AXI4-Lite slave per project" (both changed in round 2).

## 4. Map of the code

| Area | Where |
|---|---|
| Control plane, projects, engines | `crates/sim-api` (`project.rs`, `qemu_engine.rs`, `native_ps.rs`, `sim.rs`, `types.rs`) |
| XSA / Vivado ingestion | `crates/xsa-import` (incl. `ps7_init.rs`, `hwh.rs`), `sim-api/src/xsa.rs`, `ps7_init.rs`, `xsim-export`, `sim-api/src/exportsim.rs`, `exportinit.rs`, `boot` + `bootbin.rs` |
| Testbench generation | `sim-api/src/tbgen.rs` (AXI4-Lite slaves (multi), AXI4 master bridge, irq, pin groups, `fixtures/rtl/common/zs_axi_checker.sv`) |
| PS: QEMU | `crates/backend-qemu` (`dtb.rs`, `gdb.rs`, `net.rs`) |
| PS: native | `cpu-a9` (A32, Thumb-2, VFPv3, MMU), `mem-sys`, `ps-periph` (incl. `ddrc.rs`, `slcr.rs`, `ttc.rs`), `ps-bus` |
| PL: external RTL | `backend-rtl-xsim`, `backend-rtl-verilator`, `backend-rtl-icarus` (`vpi-icarus`), `xsim-dpi` (+ `dpi-cdylib`) |
| PL: native | `pl-native`, `vendor-ip` (GPIO, Timer, UART Lite, DMA, IIC, BRAM, INTC, SPI, AXIS FIFO, Clk Wiz), `rtl-frontend`, `rtl-engine` |
| Board, devices | `board-zybo` (+ ZedBoard, PYNQ-Z2, Z7-10), `devices` |
| Front ends | `cli`, `gui`, `mcp` (depend on `sim-api` only) |
| Firmware | `firmware/` (own workspace, `armv7a-none-eabi`; prebuilt ELFs in `fixtures/elf`) |
| Automation | `crates/xtask`: `ci [--fast|--det-only a,b]`, `dpi`, `dpi-shared`, `firmware`, `dist`, `demo`, `changed-examples` |

**Adding a vendor IP model** (template = the Quad SPI commit `ac21e3f`): model in `vendor-ip/src/<ip>.rs` (a
`pl_native::RegisterBlock`) -> register in `vendor-ip/src/lib.rs` (`MODELS`, `param_names`, build arm) and
`sim-api/src/pl_attach.rs` (`NATIVE_MODELS`) -> mapping by HWH VLNV in `xsa-import` / `sim-api/src/xsa.rs` ->
Rust firmware `firmware/<ip>_demo` (+ `firmware/Cargo.toml` member, `xtask` FIRMWARE list, `cargo xtask firmware`)
-> `examples/<ip>-demo` -> `determinism.rs` case -> unit tests per register + `vendor_ip2`/`xsa_import`/`native_pl`
tests -> ADR -> STATUS rows.

## 5. Working rules learned the hard way

1. **Honesty contract.** STATUS.md rows are `verified` (a named test proves it), `implemented-untested`, `stub`,
   `not-planned`. Facts from memory carry `VERIFY:`. Never write more than a test shows. Update STATUS.md and the
   plan sections in the same commit as the code (round 2 left stale `ps7_init` rows behind; a review caught it).
2. **No C/C++ authored.** Firmware is Rust `no_std`. Xilinx sources are compiled only as test inputs.
3. **Rebuild** the DPI library (`cargo xtask dpi`) after `xsim-dpi` changes, and the firmware ELFs
   (`cargo xtask firmware`) after `firmware/rt` changes; a changed `rt` changes every ELF, then regenerate
   `fixtures/boot/BOOT.BIN` and `fixtures/handoff/BOOT.BIN` with Vitis `bootgen` from the same `boot.bif`.
4. **Never `export ZYNQSIM_NO_BUILD_CACHE=1` for a gate.**
5. **Known intermittent failures** (pass when re-run alone; wall-clock dependent): `backend-rtl-xsim
   board::shell_leds_switches_and_buttons`, `mcp board_demo_through_mcp` (host UART input arrives at a
   wall-clock dependent virtual time), `zynqsim cli board_demo_image_filter_...`, a hung QEMU (tests kill a run
   after 10 minutes). On failure: kill stray `qemu-system-aarch64.exe`/`xsim.exe`, re-run alone, then look at your change.
6. **One gate at a time**; no builds while a gate runs. Full gate 45-60 min; `ci --fast` ~10-18 min skips every
   test that needs an external tool and does not replace the full gate before a merge.
7. **A "stale" test usually encodes an old fact.** When a feature makes a "not implemented" path unreachable,
   update the test that pinned the old message and say so in the commit. Never weaken a test to get green.
8. **New `examples/<dir>`** must be in `crates/cli/tests/determinism.rs` (`CASES` or `EXCLUDED`).
9. **Windows quirks.** Forward slashes in generated TOML; `relative_path` canonicalizes (macOS `/var`, Windows `\\?\`).
   PowerShell 5.1 turns native stderr into red `NativeCommandError` noise; use `cmd /c "... 2>&1"` or Git Bash.
   The MinGW `corrupt .drectve` linker warning is harmless.
10. **Merging parallel branches.** Conflicts in lists (workspace members, FIRMWARE list, `MODELS`, determinism table,
    docs) are "keep both", but check each hunk: a blind union lost a closing `}` in `vendor-ip/src/lib.rs`, glued two
    `Case { .. }` entries and interleaved two tests. Conflicts in code need a human decision. Re-run clippy and the
    touched tests after every merge.
11. **Disk.** Every git worktree has its own `target/` (5-10 GB each; 208 GB accumulated in `.claude/worktrees`).
    Remove worktrees when their branch is merged: `git worktree remove --force <path>; git worktree prune`.
12. If you delegate to sub-agents: give each its own files, tell it to `git merge --ff-only origin/p12-merge` first
    (worktrees start from the repository's first commit), to commit early, to report in under 150 words, and not to
    push or touch `main`.

## 6. The plan

Priority is value over effort (S hours, M a day, L several days; estimates). Every task ends with: tests named in
the commit, STATUS.md rows updated honestly, an ADR when a decision was made, affected `examples/` in the
determinism table, and `cargo xtask ci --fast` green; the full gate before the owner merges.

### P0. Gate the current branch (S)
Run `cargo xtask ci` on `p12-merge`, fix only what it shows, record the result in STATUS.md. Then the owner merges.

### P1. Make the user path real (M, highest value) — DONE
Goal: a stranger can take a trivial Vitis project + Vivado XSA and see it run, with documentation that is true.
* **`zynqsim doctor` implemented and verified:** One-screen diagnostic report and `--json` format inspecting host
  toolchains (cargo, rustc, C/C++ compiler, make) and EDA tools (QEMU, Vivado, XSim, DPI lib, Verilator, Icarus, VPI module),
  route readiness, and recommendations. Acceptance met: outputs clear native route readiness on machines without Xilinx tools.
* **`--ps` and `--pl` flags in `zynqsim init`:** Allows direct selection of `backends.ps = "native"` and `backends.pl = "rtl-native"`
  during XSA initialization, enabling full PS+PL co-simulation on systems without QEMU.
* **First-run walkthrough in `xtask` (`cargo xtask try-xsa`):** Automated walkthrough script taking an XSA, running `init --ps native`,
  inspecting the generated project, and running it with SPI demo firmware (all in pure Rust, zero external EDA dependencies).
* **Demo tour integration:** Added `doctor`, `xsa-native-init`, and `xsa-native-run` steps into the demo tour and `--quick` tour.
* **Documentation updated:** Stale text in `docs/TEST_YOUR_DESIGN.md` and `README.md` corrected (`ps7_init` replay, multi-slave,
  A32/Thumb/VFP native PS capabilities, `.\demo.cmd --quick`).

### A. QEMU side of `ps7_init` (M) — DONE
* **Explicit Skip Allow-List & Auditing (`QemuReplayReport`):** Replaced silent `qemu_skips_addr` checks with an
  explicit allow-list (`QEMU_SKIPPED_REGISTER_ALLOW_LIST`, `SkippedRegisterRule`) documenting exact block names and
  architectural reasons (DDRC, DEVCFG, UART, USB, I2C, SPI, CAN, GPIO, GEM, QSPI, SMC, SDIO, TTC, SWDT, AFI, CoreSight,
  MPCore/timers, and DDRIOB DCI calibration status). Operations targeting addresses outside HP0-decoded memory (SLCR, RAM, OCM)
  and not in the allow-list immediately fail with an informative diagnostic instead of silently proceeding.
* **QEMU `arm_mptimer` Limitation Documented (H6):** Upstream Xilinx QEMU 6.1.0 (`arm_mptimer.c`) hardcodes the timer scale
  to `10` ns (100 MHz) in C (`(((control >> 8) & 0xff) + 1) * 10`) without DTB or QOM properties to reclock it (confirmed by
  code inspection and `arm_mptimer,?` model dump; ADR-029). The global timer is clocked at 333.33 MHz via DTB (`clock-frequency`),
  while the private timer deviation remains pinned by `a9_timer_rate::the_private_timer_still_ticks_at_100_mhz_a_known_deviation`.
  Native PS models both timers at 333.33 MHz (`PERIPH_DIV = 2`).
* **Verification & Status:** All 5 tests in `ps7_init_replay.rs`, `handoff.rs` (running BOOT.BIN end-to-end), and `a9_timer_rate.rs`
  pass. STATUS.md rows 66, 414, and 546 updated.


### D. More IP models and real IP (M each)
* **AXI VDMA (`axi_vdma` 6.3, ADR-043) — DONE:** Behavioural model in `crates/vendor-ip/src/vdma.rs`,
  2D stride/frame transfers, park & circular buffer store modes (1..32 frame stores with `REG_INDEX` banking),
  interrupts on output 0 (MM2S) and output 1 (S2MM), `STREAM_LOOPBACK`, firmware demo `firmware/vdma_demo`,
  example `examples/vdma-demo/project.toml`, determinism 5/5, unit tests and integration tests passing.
* **AXI Ethernet Lite (`axi_ethernetlite` 3.0, ADR-044) — DONE:** Behavioural model in `crates/vendor-ip/src/ethernetlite.rs`,
  8 KiB address window (`0x2000` bytes), TX ping/pong buffers, RX ping/pong buffers, MDIO PHY management
  (`MDIOADDR`, `MDIOWR`, `MDIORD`, `MDIOCTRL` with simulated Marvell-like PHY registers), `GIE` (`0x07F8`),
  `ip2intc_irpt` interrupt output, internal loopback, external frame injection/extraction (`push_rx`, `take_tx`),
  firmware demo `firmware/ethlite_demo`, example `examples/ethlite-demo/project.toml`, determinism 5/5,
  unit tests and integration tests passing.
* **Video Timing Controller (`v_tc` 6.2, ADR-045) — DONE:** Behavioural model in `crates/vendor-ip/src/v_tc.rs`,
  64 KiB address window (`0x10000` bytes), generator active size (`GASIZE`), horizontal/vertical timing (`GHSIZE`,
  `GVSIZE`, `GHSYNC`, `GVSYNC`), frame syncs (`FS00`..`FS15`), generator delay (`GGD`), pixel-clock accumulator,
  generator timing progression emitting FSYNC0, VBLANK, and AVIDEO interrupts, W1C ISR, firmware demo `firmware/vtc_demo`,
  example `examples/vtc-demo/project.toml`, determinism 5/5, unit tests and integration tests passing.
* **AXI CDMA (`axi_cdma` 4.1, ADR-046) — DONE:** Behavioural model in `crates/vendor-ip/src/cdma.rs`,
  64 KiB address window (`0x10000` bytes), control register `CDMACR` (soft reset, SGMode, Keyhole Read/Write,
  interrupt enables, thresholds), status register `CDMASR` (Idle, SGIncld, W1C completion and error interrupts),
  descriptor pointers (`CURDESC_PNTR`, `TAILDESC_PNTR`), source/destination addresses (`SA`, `DA`),
  bytes to transfer (`BTT`), memory-to-memory copy via `PsPort` in Simple and Scatter-Gather modes,
  `cdma_introut` interrupt, firmware demo `firmware/cdma_demo`, example `examples/cdma-demo/project.toml`,
  determinism 5/5, unit tests and integration tests passing.
* **AXI UART 16550 (`axi_uart16550` 2.0, ADR-047) — DONE:** Behavioural model in `crates/vendor-ip/src/uart16550.rs`,
  64 KiB address window (`0x10000` bytes), standard 16550 register bank mapped at base offset `0x1000` (`XUN_REG_OFFSET`),
  DLAB banking for `DLL`/`DLM` divisor latches, registers `RBR`, `THR`, `IER`, `IIR`, `FCR`, `LCR`, `MCR`, `LSR`, `MSR`, `SCR`,
  16-deep FIFOs with configurable trigger levels (1, 4, 8, 14 bytes), diagnostic loopback mode (`MCR & 0x10`),
  `OUT2` (`MCR & 0x08`) interrupt output gating, `ip2intc_irpt` interrupt output, firmware demo `firmware/uart16550_demo`,
  example `examples/uart16550-demo/project.toml`, determinism 5/5, unit tests and integration tests passing.
  Preserved unmodelled IP rejection tests by introducing `fixtures/xsa/zs_math_hwicap.hwh` (`xilinx.com:ip:axi_hwicap:3.0`).
* **AXI4-Stream to Video Out (`v_axi4s_vid_out` 4.0, ADR-048) — DONE:** Behavioural model in `crates/vendor-ip/src/v_axi4s_vid_out.rs`,
  64 KiB address window (`0x10000` bytes), registers `CR` (enable, reset, master mode, test pattern generator, loopback),
  `SR` (lock, underflow, overflow, SOF/EOL, AVIDEO/VBLANK/HBLANK status codes), `ISR`/`IER` (W1C interrupts for lock, unlock,
  underflow, overflow, SOF, EOL, frame done), active video dimensions `ASIZE`, total frame dimensions `TSIZE`, FIFO fill level
  and watermarks, manual stream injection data/ctrl port, frame/line/pixel counters, version `0x0400_0000`, simulated video beam
  progression, autonomous test pattern generation, frame completion interrupts, `MediaEvent` emission, firmware demo
  `firmware/vidout_demo`, example `examples/vidout-demo/project.toml`, determinism 5/5, unit tests and integration tests passing.
* **Video In to AXI4-Stream (`v_vid_in_axi4s` 4.0, ADR-049) — DONE:** Behavioural model in `crates/vendor-ip/src/v_vid_in_axi4s.rs`,
  64 KiB address window (`0x10000` bytes), registers `CR` (enable, reset, continuous/single-frame capture mode, test pattern input, loopback),
  `SR` (lock, overflow, underflow, SOF/EOL, AVIDEO/VBLANK/HBLANK status codes), `ISR`/`IER` (W1C interrupts for lock, unlock,
  overflow, underflow, SOF, EOL, frame done), active video dimensions `ASIZE`, total frame dimensions `TSIZE`, FIFO fill level
  and watermarks, stream readout and pop port (`STREAM_DATA`/`STREAM_CTRL`), frame/line/pixel counters, version `0x0400_0000`,
  simulated video beam progression, autonomous test pattern generation, frame completion interrupts, `MediaEvent` emission, firmware demo
  `firmware/vidin_demo`, example `examples/vidin-demo/project.toml`, determinism 5/5, unit tests and integration tests passing.
* **Integrated Logic Analyzer (`ila` 6.2, ADR-050) — DONE:** Behavioural model in `crates/vendor-ip/src/ila.rs`,
  64 KiB address window (`0x10000` bytes), registers `CR` (arm, disarm, trig_now, auto_rearm, reset, stim_en),
  `SR` (armed, triggered, full, idle, state codes), `ISR`/`IER` (W1C interrupts for trigger match and buffer full),
  trigger probe value/mask/mode comparison (`PROBE0_VAL`, `PROBE0_MASK`, `PROBE0_MODE`), sample buffer depth/count/trig_pos,
  version `0x0602_0000`, sequential trace readout port (`TRACE_RADDR`/`TRACE_RDATA` with auto-increment), direct memory-mapped
  trace buffer window (`0x1000..=0x1000 + 4*depth`), live probe readout, autonomous probe stimulus generator, firmware demo
  `firmware/ila_demo`, example `examples/ila-demo/project.toml`, determinism 5/5, unit tests and integration tests passing.
* **Virtual Input/Output (`vio` 3.0, ADR-051) — DONE:** Behavioural model in `crates/vendor-ip/src/vio.rs`,
  64 KiB address window (`0x10000` bytes), registers `CR` (reset, update_out, latch_in, auto_update, loopback_en, stim_en),
  `SR` (out_updated, in_latched, activity_det, probe counts), `ISR`/`IER` (W1C interrupts for input state change, output commit ack,
  and input activity), staged output writing and atomic commit (`PROBE_OUT_VAL`/`PROBE_OUT_STAGING`), live and latched
  input sampling (`PROBE_IN_VAL`/`PROBE_IN_LIVE`), activity/toggle detection flags (`PROBE_IN_ACT`, W1C), initial reset values
  (`PROBE_OUT_INIT`), version `0x0300_0000`, autonomous loopback and stimulus generation, firmware demo `firmware/vio_demo`,
  example `examples/vio-demo/project.toml`, determinism 5/5, unit tests and integration tests passing.
* **XADC Wizard (`xadc_wiz` 3.3, ADR-052) — DONE:** Behavioural model in `crates/vendor-ip/src/xadc_wiz.rs`,
  64 KiB address window (`0x10000` bytes), registers `SRR` (reset key `0x0000_000A`), `SR` (done, eoc, eos, busy, channel, alarms, OT),
  `AIFR`, `CONVST`, `IPISR`/`IPIER` (W1C interrupts for alarms, OT, EOS, EOC), `GIER`, direct memory-mapped DRP registers
  (`0x200..=0x3FC`: `TEMP`, `VCCINT`, `VCCAUX`, `VCCBRAM`, `VAUX[0..15]`, Max/Min tracking, alarm thresholds), simulated conversion
  sequencer with EOC/EOS/alarm interrupts, physical transfer functions, firmware demo `firmware/xadc_demo`, example `examples/xadc-demo/project.toml`,
  determinism 5/5, unit tests and integration tests passing.
* **AXI Performance Monitor (`axi_perf_mon` 5.0, ADR-053) — DONE:** Behavioural model in `crates/vendor-ip/src/axi_perf_mon.rs`,
  64 KiB address window (`0x10000` bytes), registers `SRR` (reset key `0x0000_000A`), `SR` (metric/sample count enable, sample expired),
  `CR` (metric/sample count enables, resets, sample load), `SICR` (manual snapshot trigger), `IPISR`/`IPIER` (W1C interrupts for sample expired
  and metric overflow), `SI_REG`/`SIC_REG` (sample interval timer with auto-reload), `MSR0`/`MSR1` (metric selection), active metric counters
  (`0x100..=0x17C`, MC0..MC7) and sampled snapshot counters (`0x200..=0x27C`, SMC0..SMC7), simulated bus traffic progression, external
  transaction recording via handle, firmware demo `firmware/perf_mon_demo`, example `examples/perf-mon-demo/project.toml`, determinism 5/5,
  unit tests and integration tests passing. Preserved unmodelled IP rejection test on `fixtures/xsa/zs_math_hwicap.hwh`.
  Each: Vitis driver example passes (`fixtures/bsp` has `xaxidma`/`xiic` style apps; add the missing ones).
* **Real IP through `export_simulation`:** verify `-L secureip`/`unisims_ver` runs for a design with an AXI GPIO or
  clocking wizard in XSim (ADR-017 says this was never verified); then document which IP can run this way instead of
  a model. This avoids guessing register behaviour and is the cheapest way to verify the `VERIFY:` items of ADR-040/041/042/043.
* Compare the SPI/FIFO/ClkWiz/VDMA models with XSim runs of the real IP where possible and promote rows to `verified` only
  with a test that shows it.

### H. Quality
* **H2.** Deterministic host UART input (deliver bytes through a device on the remote-port bus at a requested virtual
  time, or use the native PS where the UART is a model); then drop the widened test windows and put `board-demo` into
  the determinism table.
* **H3.** The QEMU hang seen twice under load: reproduce with 3 parallel `zynqsim run` of `examples/iic-bram-demo`,
  capture `qemu.err.log` and the remote-port trace, find the lost sync.
* **H4.** Gate speed: use the per-step timings; candidates `cargo nextest`, fewer test binaries, shorter simulated
  durations, smaller determinism table for unchanged examples (`ci --det-only`).
* **H5.** GUI leftovers: 3D view, memory viewer, running-status text not re-photographed; Pmod panel for the Pcam;
  storage panel "writes discarded" wording; New from XSA (see P1).
* **H6.** Document the QEMU-machine limits (private timer rate, no devcfg/SCU watchdog in this QEMU build) and the
  native-PS route instead of patching QEMU.

### E. FSBL execution (L, optional)
Run the user's FSBL ELF from OCM (BootROM stub), let it read the partitions from BOOT.BIN (SD/QSPI model exists), hand
off. Depends on A. Only worth it if users bring their own FSBL. Acceptance: a Vitis-built FSBL+app BOOT.BIN boots to the app.

### F. Platforms (M)
Run `cargo xtask dist` and the clean-unpack test on Linux in CI; document Xilinx locations on Linux; Verilator/Icarus
discovery on Linux; macOS = native + Verilator + Icarus. Licence/metadata: the workspace says Apache-2.0 only and the
repository URL is a placeholder; ask the owner before adding a LICENSE file. When CI minutes return, run the 3-OS
matrix once on `main` and record it.

### G. Hardware comparison kit (S-M; needs a board for the comparison itself)
Document + script: capture a real Zybo's UART to a log, run the same ELF here, diff with a tolerance for timing lines;
a register-dump firmware for both. Add the first real comparison when someone has a board.

## 7. Not planned (and why)

* Bitstream-level simulation, placement/routing/timing behaviour: the PL runs from RTL.
* Distributing Xilinx's QEMU or XSim: licensing.
* Proving equivalence with hardware without hardware (see G).
* VHDL in the native engine, SystemVerilog classes/UVM/interfaces, SDF timing: `stub`/`not-planned` in STATUS.md.

## 8. Suggested order

1. P0 (full gate) -> owner merges `p12-merge` into `main`; close PRs #5-#16.
2. P1 (user path) first: it decides what users hit. In parallel, disjoint files: A (`qemu_engine`, `sim-api/ps7_init.rs`),
   D (`vendor-ip` + `xsa.rs`), H2 (`backend-qemu`/devices).
3. H3/H4/H6 when a flaky test or slow gate hurts; E and G only on demand.

## 9. Commands cheat sheet (Windows, Git Bash)

```
export PATH="$HOME/.cargo/bin:/c/Qt/Tools/mingw1310_64/bin:$PATH"   # cargo is not on PATH by default
cd zynqsim
cargo xtask dpi            # after xsim-dpi changes
cargo xtask firmware       # after firmware/rt changes (then regenerate the BOOT.BIN fixtures)
cargo xtask ci --fast      # ~10-18 min, no external tools needed
cargo xtask ci             # 45-60 min, the real gate (QEMU + XSim + Verilator + Icarus)
cargo xtask demo --quick   # ~5 min tour, artifacts in demo-out/
cargo xtask changed-examples
zynqsim init --xsa design.xsa --rtl rtl/my_ip.v --boot-bin BOOT.BIN --out myproj
zynqsim run --project myproj/project.toml --for 8ms --json
```
