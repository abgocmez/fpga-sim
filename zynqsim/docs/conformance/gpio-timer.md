# Peripheral conformance: PS GPIO and PS timers (spec §14.2)

Two Rust `no_std` self-tests run on the Xilinx QEMU PS model (`ps = "qemu"`, `pl = "demo"`). Each
firmware checks register-level expectations and prints one line per check:
`CHECK n name: PASS|FAIL|N/A reason`, then `RESULT: ALL PASS` when no check failed.

| Block | Firmware | Example | Test |
|---|---|---|---|
| xgpiops, PS GPIO at 0xE000A000 | `firmware/conf_gpiops` | `examples/conf-gpiops` | `conf_gpiops_checks_match_the_expected_na_list` |
| xscutimer private timer at 0xF8F00600 and global timer at 0xF8F00200 | `firmware/conf_scutimer` | `examples/conf-scutimer` | `conf_scutimer_checks_match_the_expected_na_list` |

Both tests are in `crates/cli/tests/conformance_gpio_timer.rs`. They run `zynqsim --json run`, assert
`RESULT: ALL PASS`, check the numbering, and assert the exact N/A list by check name. A new model, or
a regression, changes that list and fails the test.

Run: `cargo test -p zynqsim --test conformance_gpio_timer -- --nocapture` (after `cargo xtask firmware`).
Both examples are also in the §14.6 determinism test (`crates/cli/tests/determinism.rs`).

## Model verdicts

| Block | Verdict | Why |
|---|---|---|
| xgpiops (PS GPIO) | **not modelled** | QEMU maps nothing at 0xE000A000. The firmware's probe (DIRM_0 must read back) fails, so all 12 checks are N/A. QEMU's `-d unimp` log shows `Invalid access at addr 0xE000A204 ... region '(null)', reason: rejected` for the offsets this firmware uses. Reads return 0 and writes are dropped. A zero readback is not a pass, so the checks are gated. |
| xscutimer private timer | **fully** for the checked features | All 11 private-timer checks pass, including the interrupt through the GIC (PPI 29). Watchdog (the second timer block at +0x20) is not checked. |
| Global timer | **partly** | The counter advances and the comparator sets its flag. The GIC interrupt (PPI 27) is delivered. COUNTER writes are ignored, the counter keeps running while disabled, and the flag re-asserts after write-1-to-clear. |

## Per-check results

### xgpiops (`conf-gpiops`, 12 checks, 0 PASS, 12 N/A)

| # | Check | Expectation (UG585 ch. 14) | Result | Model |
|---|---|---|---|---|
| 1 | reset_values | DIRM/OEN 0, INT_MASK all ones, INT_TYPE 0 after reset | N/A: not mapped | not |
| 2 | dirm_readback | DIRM_n keeps written bits, per bank width | N/A: not mapped | not |
| 3 | oen_readback | OEN_n keeps written bits | N/A: not mapped | not |
| 4 | data_readback | DATA_n keeps output data | N/A: not mapped | not |
| 5 | masked_write_lsw | MASK_DATA_0_LSW: mask bits 31:16 (1 = inhibit) protect pins | N/A: not mapped | not |
| 6 | masked_write_msw | MASK_DATA_3_MSW: same for bank 3 pins 31:16 | N/A: not mapped | not |
| 7 | bank2_width | bank 2 has 22 pins, bits 31:22 read 0 | N/A: not mapped | not |
| 8 | int_mask_en_dis | INT_DIS sets, INT_EN clears the bank 1 mask bits | N/A: not mapped | not |
| 9 | int_type_polarity_any | INT_TYPE, INT_POLARITY, INT_ANY keep written values | N/A: not mapped | not |
| 10 | int_status_no_events | INT_STAT reads 0 with no pin event; W1C leaves it 0 | N/A: not mapped | not |
| 11 | data_ro_mirrors_output | DATA_RO reads back a driven output pin | N/A: not mapped | not |
| 12 | pin_event_interrupt | a pin edge sets INT_STAT and raises the GIC line | N/A: no pin source | not |

Check 12 is N/A for a separate reason: no pin is driven from outside the PS model (`pl = "demo"`), so
INT_STAT cannot be set by software. Nothing in the simulator can produce a pin event for this check.

### xscutimer and global timer (`conf-scutimer`, 17 checks, 14 PASS, 3 N/A)

| # | Check | Expectation | Result | Model |
|---|---|---|---|---|
| 1 | reset_state | private timer stopped, no event; global timer stopped | PASS | fully |
| 2 | pt_load_readback | LOAD is a 32-bit read/write register | PASS | fully |
| 3 | pt_counter_writable_when_stopped | COUNTER loads while the timer is stopped (VERIFY: TRM wording) | PASS | fully |
| 4 | pt_control_readback | CONTROL keeps the IRQ-enable and prescaler fields | PASS | fully |
| 5 | pt_counts_down_and_stops | counter counts down while enabled, holds when disabled | PASS | fully |
| 6 | pt_event_flag_w1c | one-shot sets the event flag at zero; write 1 clears it | PASS | fully |
| 7 | pt_auto_reload | three events back to back; counter reloads (never above LOAD) | PASS | fully |
| 8 | pt_one_shot_stops | no second event and the counter holds after the first (VERIFY) | PASS | fully |
| 9 | pt_prescaler_slows | prescaler 0xFF runs the counter at about 1/256 (VERIFY: divisor is prescaler+1) | PASS | fully |
| 10 | pt_flag_without_irq | with IRQ enable off the flag sets and no interrupt is taken | PASS | fully |
| 11 | pt_irq_gic_ppi29 | private timer interrupt through the GIC (PPI 29): three handler runs | PASS | fully |
| 12 | gt_counter_advances | global counter advances while enabled | PASS | partly |
| 13 | gt_counter_writable_when_stopped | COUNTER load while stopped | N/A: QEMU ignores COUNTER writes | partly |
| 14 | gt_counter_holds_when_disabled | counter holds while the timer is disabled | N/A: QEMU keeps counting | partly |
| 15 | gt_comparator_flag | comparator match sets the flag; write 1 clears it | N/A: flag re-asserts | partly |
| 16 | gt_auto_increment | after a match the comparator moves by AUTO_INCREMENT | PASS | partly |
| 17 | gt_irq_gic_ppi27 | global timer interrupt through the GIC (PPI 27) | PASS | partly |

## Observed deviations (evidence for the N/A reasons)

Run on the Xilinx QEMU 2022.1 (`qemu-system-aarch64`, `xilinx-zynq-a9`). Values are from one
diagnostic run of the same firmware, with the counter read at the times stated.

* **Global counter ignores COUNTER writes (check 13).** The check reads back `0x0000260c` after writing
  `0x0000bee0` with the timer disabled. A diagnostic read `0x18` straight after the same write.
* **Global counter keeps running while disabled (check 14).** With GT_CONTROL = 0 the counter read
  `0x4c0e`, then `0x97e5` 100 us later, so it does not hold. Check 12 still passes: it only needs the
  counter to advance while enabled.
* **Comparator flag re-asserts (check 15).** The match flag set after 5 polls (counter `0x130a9`,
  comparator `0x12adb`). After write 1 it read 1 at once, and still 1 100 us later with the counter at
  `0x17d70`, past the comparator. On hardware a match is an equality event, so a cleared flag stays
  cleared until the counter next equals the comparator. This was observed on one run and is not a
  statement about every QEMU version.
* **Auto-increment (check 16) passes but is measured once.** It reads the comparator once, right after
  the first match and its write-1 clear, and sees one increment of AUTO_INCREMENT. It does not prove the
  comparator stays stable after that.
* **Interrupt storm guard (firmware, not a deviation).** Because the flag re-asserts, the global-timer
  handler drops the timer's IRQ-enable bit after taking the interrupt. Without that the level interrupt
  re-enters the handler continuously and the main loop never gets to report. The check still proves
  delivery (check 17 PASS).

## Offsets and bit meanings from memory (VERIFY)

Written from the UG585 and Cortex-A9 MPCore TRM from memory. The `VERIFY:` comments in the firmware
mark each item. The simulator's behaviour shows that the GPIO offsets are not exercised by QEMU at all,
so the GPIO offsets are unverified against any model.

* GPIO: bank widths 16/16/22/32 (bank 2 = 22 pins); DIRM 0x204 with a 0x40 bank stride;
  INT_MASK reset all ones; MASK_DATA_n_LSW/MSW bit 16+i protects pin i; DATA_RO mirrors a driven pin.
* Private timer: LOAD 0x00, COUNTER 0x04, CONTROL 0x08 (bit 0 enable, bit 1 auto-reload, bit 2 IRQ,
  bits 15:8 prescaler, divisor prescaler+1), ISR 0x0C (write 1 clears), PPI 29.
* Global timer: COUNTER 0x00/0x04, CONTROL 0x08 (bit 0 enable, bit 1 comparator, bit 2 IRQ, bit 3
  auto-increment, bits 15:8 prescaler), ISR 0x0C, COMPARATOR 0x10/0x14, AUTO_INCREMENT 0x18, PPI 27.
  COUNTER writable while stopped (check 13) and the stop-when-disabled behaviour (check 14) are stated
  from memory.

## Firmware and example changes in this commit

* `firmware/rt/src/irq.rs`: new `enable_ppi(id)` for private interrupts (16..=31). Existing firmware is
  unaffected.
* `firmware/conf_gpiops`, `firmware/conf_scutimer`: the two checks crates, added to
  `firmware/Cargo.toml` and to the FIRMWARE list in `crates/xtask/src/main.rs`.
* `fixtures/elf/rust_conf_gpiops.elf`, `fixtures/elf/rust_conf_scutimer.elf`: prebuilt from `cargo xtask
  firmware`.
* `examples/conf-gpiops`, `examples/conf-scutimer`: example projects (run durations 20 ms and 200 ms).
* `crates/cli/tests/determinism.rs`: both examples added to `CASES` (the §14.6 test requires every
  example to be listed).
