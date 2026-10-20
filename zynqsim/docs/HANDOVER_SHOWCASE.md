# Handover: finish the showcase (findings and next steps)

For the next engineer or agent. It continues `docs/HANDOVER.md` (round 2) and `docs/SHOWCASE.md` (what the
showcase is). Written on 2026-10-20 at commit `847b41e` (branch `p12-merge`, pushed). Read `docs/STATUS.md` first:
it wins over this file.

## 0. Where things stand

The showcase (a Vivado 2022.1 block design with the user's 8-tap FIR in Verilog plus two AXI GPIO, Timer, UART
Lite and Quad SPI; Rust firmware; `BOOT.BIN`) **runs and passes** on both routes, with five identical trace
digests per route:

* `examples/showcase`: Xilinx QEMU boots the `BOOT.BIN`, the FIR runs in Vivado XSim with the AXI protocol checkers.
* `examples/showcase-native`: native Cortex-A9 + the pure-Rust RTL engine, no Xilinx tool.

Tests: `cargo test -p zynqsim --test showcase` (3), `cargo test -p rtl-engine --test showcase_fir` (8), the
determinism cases `showcase` and `showcase-native`, and the demo steps (`cargo xtask demo --only showcase,showcase-native`).
Not done: the **full gate** (`cargo xtask ci`) on this commit (the `--fast` gate was green before the showcase was added).

Commands: see the cheat sheet in `docs/HANDOVER.md` section 9 (Git Bash; `export PATH="$HOME/.cargo/bin:/c/Qt/Tools/mingw1310_64/bin:$PATH"`).
Vivado needs a SHORT path (copy `fixtures/showcase/make_showcase.tcl` to `C:\zsh\`, run it from there with the
call synchronous and `-notrace`; a hidden `Start-Process` window breaks block-design init). One Vivado run takes about 15 minutes.

## 1. Findings from the first real trial, and what to do about each

### S1. The native PS cannot boot a `BOOT.BIN` (value high, effort S)
* **Symptom.** `zynqsim init --xsa ... --boot BOOT.BIN --ps native --pl rtl-native` reports
  `boot.mode: boot_bin is not supported by the native PS yet; use backends.ps = "qemu"`. `showcase-native` therefore
  edits `[cpu0] elf` by hand (see `crates/cli/tests/showcase.rs::the_showcase_runs_natively_without_any_tool`).
* **Where.** The guard is in `crates/sim-api/src/project.rs` (~line 1403, the native-PS validation). The QEMU path
  already extracts the application: `crate::bootbin::stage_application(&bin, &work_dir)` returns an ELF path
  (`project.rs` ~line 1047, `bootbin.rs:80`).
* **How.** Remove the guard; in the native-PS start-up use the same `boot_bin_file(base)` + `stage_application`
  to get core 0's ELF (check `plan_project` in `bootbin.rs` for how a second application partition, core 1, is handled). Keep `[boot] ps7_init` replay as is.
* **Acceptance.** `init --xsa ... --boot BOOT.BIN --ps native --pl rtl-native` yields a runnable project with no
  manual edit and the showcase passes with the same UART text; the trace digest equals the ELF-booted native run
  (or the difference is explained). Replace the manual TOML edit in `showcase.rs`; add a native BOOT.BIN case to
  `crates/cli/tests/bootbin.rs`; update the STATUS row "Native PS booting a BOOT.BIN" (from *not supported*) and
  `examples/showcase-native` (regenerate it with `--boot`).
* **Risk.** A BOOT.BIN's FSBL partition must still not run; its register-init table stays listed, not applied.

### S2. `xsa-report.md` says `ps7_init` is not executed (value medium, effort S)
* **Symptom.** The generated report's *Not applied* section still says the archive's `ps7_init` files "are in the
  archive but are not executed" and that the PS clock tree does not change what runs. Both are out of date since
  ADR-038 (replay on the native PS, partial on QEMU). The CLI line `boot : ... ps7_init is not applied` and the two
  MCP tool descriptions say the same (`crates/cli/src/main.rs` ~1610, `crates/mcp/src/tools.rs` lines ~130 and ~197).
* **Where.** `crates/sim-api/src/xsa.rs` ~lines 1643-1665 (the report), plus the CLI and MCP strings above.
* **How.** Make the text depend on the project: with `[boot] ps7_init = true` say "replayed before the CPU starts
  (native PS: all writes; QEMU: the registers on the allow-list, the rest skipped; see ADR-038 and
  `sim_api::ps7_init::qemu_skips_addr`)"; with `false` or no `ps7_init` files keep the old sentence. The BOOT.BIN
  FSBL is still never executed: keep that sentence.
* **Acceptance.** `xsa-report.md` of `examples/showcase*` and `examples/vivado-handoff` matches what the run does;
  the tests that pin the old strings (`grep -rn "not executed\|ps7_init is not applied" crates`) are updated on
  purpose (rule 7 of `docs/HANDOVER.md`: a stale test encodes an old fact, say so in the commit). Regenerate the
  committed reports (`examples/showcase*/xsa-report.md`, `examples/vivado-handoff/xsa-report.md`).
* Also fix the same stale sentence in `docs/TEST_YOUR_DESIGN.md` and `examples/vivado-handoff/README.md` if still present.

### S3. The Quad SPI's `spi_0` port has no device (value medium, effort M)
* **Symptom.** The design's external port `spi_0` gets no note and no binding in `xsa-report.md`; the model loops
  back, so the showcase passes, but a real SPI device cannot be attached.
* **Where.** GPIO binding by port name is in `crates/sim-api/src/xsa.rs` (~lines 558-630); the Pmod IIC binding
  (~lines 727-750) is the closest template (`"{inst}.IIC" = "board.pmod_ja"`); the model is `crates/vendor-ip/src/spi.rs`.
* **How.** (1) Report the `spi_0` port in the *Not applied/Notes* part of `xsa-report.md` (one line saying that it is
  not connected and loopback is the default). (2) Optionally let `[connect] "axi_quad_spi_0.SPI" = "board.pmod_jb"`
  bind it to a Pmod connector whose device is an SPI Pmod (the OLED, `examples/oled-demo`, is the existing SPI device;
  the PS SPI path shows how the host decodes the pins).
* **Acceptance.** (1) is a unit test on the report text. For (2): a new `examples/showcase-oled`-style project (or an
  addition to `showcase`) that draws on the Pmod OLED through the PL's Quad SPI and a test on the decoded picture; if
  (2) is too big, record it in STATUS as *not planned* with the reason.

### S4. Interactive switches and buttons (value high for the demo, effort S-M)
* **Why.** Switch and button values are read once, at their reset value, so the GUI shows nothing happen when a
  user flips a switch. A demo you can play with is the point of the GUI.
* **How.** (1) Firmware (`firmware/showcase_demo/src/main.rs`): after `RESULT: ALL PASS` keep the program alive in an
  interactive loop instead of ending: enable the GPIO interrupts (`axi_gpio_0` is `IRQ_F2P[1]`, GIC 62; `axi_gpio_1`
  `IRQ_F2P[2]`, GIC 63: write `GIER`/`IP_IER`, ack `IP_ISR`), then on a switch change pick a coefficient set
  (switch 0 = low-pass, 1 = high-pass, 2 = all-pass `[1,0,0,0,0,0,0,0]`, 3 = a moving average), refilter the
  64-sample signal in the FIR and mirror the result's sign/peak on the LEDs; button 0 restarts, button 1 prints the
  last result. Keep the first part of the program (the `LINES` the tests check) unchanged and gate the interactive
  loop so the batch runs (tests, determinism, demo) still finish: end the run on a scripted marker, for example a
  switch pattern `0b1111` or a flag read from the timer, and document it. (2) Host side: scripted input already exists
  (`zynqsim run --at 1ms=sw1:on`; see `board-demo` in the determinism table for the syntax of `--at`).
* **Acceptance.** A new test drives the switches by `--at` and checks that the firmware printed
  `coef set: lowpass|highpass|...` and the matching LED pattern; `examples/showcase`/`-native` documented in
  `docs/SHOWCASE.md` with the GUI steps; a determinism case with the scripted inputs (UART host input is wall-clock
  dependent only for the TCP UART path, switches and buttons are not: still check with 5 runs).
* **Risk.** `board-demo` is known flaky for host UART input, not for switches; verify before relying on it.

### S5. Done already: the "hw fir: 0 ticks" line (nothing to do)
* It was a hard-coded `0` (a decoration, not a measurement). The firmware now measures the polled high-pass pass
  with the A9 global timer and prints `hw fir: N global-timer ticks per sample (polled)`. The line is informational
  and not asserted. `N` differs between QEMU and the native PS because the two count different simulated time per
  polled MMIO access; if a stable number is wanted, measure in the PL (a free-running counter in `zs_fir.v` read before
  and after, then a new Vivado run to regenerate `design.xsa` and `export/`).

### S6. The full gate and a clean record (value high, effort S)
* Run `cargo xtask ci` (45-60 min, QEMU + XSim + Verilator + Icarus) on the commit that contains S1-S4; fix only what it
  shows; record the result in `docs/STATUS.md`. Then the owner merges `p12-merge` into `main`
  (`git fetch origin && git push origin origin/p12-merge:main`). Do not push to `main` yourself.

## 2. Suggested order

S1 and S2 first (small, independent, and S2 removes a statement that misleads users), then S4 (the visible one), then
S3, S6 last. S1 touches `sim-api/src/project.rs` and `bootbin.rs`; S2 touches `sim-api/src/xsa.rs`, `cli/src/main.rs`,
`mcp/src/tools.rs`; S4 touches `firmware/showcase_demo` and its ELF fixture and BOOT.BIN (`cargo xtask firmware`, then
`bootgen` in `fixtures/showcase`, then regenerate the two examples): all disjoint, so they can run in parallel.

## 3. Reminders that apply here

* A changed firmware changes `fixtures/elf/rust_showcase.elf`: rebuild `fixtures/showcase/BOOT.BIN` with `bootgen` and
  regenerate `examples/showcase` (`zynqsim init --xsa ... --boot ... --out examples/showcase`); the native example needs the
  `[cpu0] elf` line pointing at `../../fixtures/elf/rust_showcase.elf` (until S1 is done).
* Do not weaken a test to get green; update STATUS.md and the docs in the same commit as the code.
* Every worktree has its own `target/` (5-10 GB): remove it when its branch is merged. A path longer than 260 characters
  inside a worktree can make `git worktree remove` fail ("Filename too long"): delete the directory with
  `cmd /c rmdir /s /q "\\?\<path>"`, then `git worktree prune`.
