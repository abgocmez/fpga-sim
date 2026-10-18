# Native PS against QEMU: the conformance and hello firmwares

The native PS engine (`backends.ps = "native"`, ADR-033) runs the same firmware as the Xilinx
QEMU engine. `crates/cli/tests/native_ps_diff.rs` runs each project on both and compares the UART
text, the CHECK lines and the final status. Every difference is pinned in that test with its
reason. A difference that is not pinned fails the test.

The QEMU halves skip with `SKIPPED:` when `qemu-system-aarch64` is not installed.

## Results (2026-10-17)

| Project | Firmware | QEMU | Native | Result |
|---|---|---|---|---|
| `examples/rust-hello` / `examples/native-hello` | `rust_hello.elf` | 3 lines, `RESULT: ALL PASS` | the same 3 lines | **identical** UART text |
| `examples/conf-scutimer` | `rust_conf_scutimer.elf` | checks 13, 14, 15: N/A; `RESULT: ALL PASS` | checks 1 to 17 all PASS; `RESULT: ALL PASS` | **documented difference**: 3 checks |
| `examples/conf-ttc` | `rust_conf_ttc.elf` | check 12: N/A; check 9: PASS; `RESULT: ALL PASS` | check 12: N/A; check 9: **FAIL**; `RESULT: FAIL (1 checks)` | **documented difference**: 1 check (check 9), see below |

Final status: both report board `zybo-z7-20`, no PL (`pl_backend` `none`) and fidelity F0. Virtual
times differ: QEMU's icount clock and the native cycle clock are different clocks, and QEMU does
not pace its UART. The UART text is what is compared.

## Difference 1: global timer checks 13 to 15 (conf-scutimer)

QEMU's global timer deviates from the hardware as the firmware describes it, so the firmware
reports these N/A (the deviations are documented in `docs/conformance/gpio-timer.md`):

* 13: the global counter ignores `COUNTER` writes.
* 14: the global counter keeps running while the timer is disabled.
* 15: after a write-1-to-clear the comparator flag reads 1 again while the counter is past the
  comparator.

The native model follows UG585 as remembered (`ps-periph::timers`), so the same three checks
PASS. The native results are not evidence of the hardware either: they are the model's.

## Difference 2: TTC check 12, match flag on read (resolved)

Per Cadence TTC hardware specification and Xilinx driver `XTtcPs_InterruptHandler`, reading the
Interrupt Status Register (`0x54..=0x5C`) clears the status bits. The native TTC model implements
clear-on-read, matching QEMU and real driver behaviour. Consequently, both QEMU and native report
check 12 as N/A (`first read 0x4, second read 0x0, no write in between`). This eliminates Difference 2.

## Difference 3: TTC check 9, decrement mode (conf-ttc): FAIL on native

Native output: `CHECK 9 Decrement mode counts down: FAIL first 209 then 64145`. QEMU passes.

What the check does: it writes `INTERVAL = 0xFFFF`, starts the counter in decrement mode, waits
about 10 us, reads, waits about 20 us, reads again, and requires the second value to be smaller.
The firmware **does not reset the counter** before starting it, so the counter starts from
whatever check 8 left in it.

What the numbers show. The first read (209) and the second (64145) are 1600 ticks apart. That
is the 14 us between the reads at the 111 MHz TTC clock, so the counter wrapped from 0 to 65535
between them. Working back from the first read, the counter held about 1009 ticks when check 9
started. The margin of this check is therefore the leftover count, and the leftover depends on the
instruction timing of check 8's last poll.

Ruled out: the UART. The result was the same with the UART at 100 Mbaud, so back-pressure from
the 115200 baud transmit FIFO is not the cause. The native timing is one instruction per 1.5 ns;
QEMU's `icount shift=1` runs two nanoseconds per instruction, so its leftover count differs and
it passes. That explanation is inferred from the numbers, not measured on QEMU.

The native model is not changed to make this pass: its timing is the one ADR-033 specifies, and
a firmware that depends on a leftover counter value is racy on hardware too. The test pins
check 9 as FAIL and the result line as `RESULT: FAIL (1 checks)`, so a change in the model or in
the firmware shows up.

## What is not compared

* Virtual time and the trace digest: the two engines use different clocks, so their digests are
  not comparable. Each engine's digest is deterministic across processes
  (`native_ps_diff::five_processes_give_one_trace_digest`).
* Anything the native engine does not drive yet: LEDs, Pmod devices, HDMI, Ethernet. The examples
  compared here use only the UART, SLCR, the private and global timers, TTC and the GIC.
* Firmware that needs a PL, Thumb or VFP: refused by the native engine (ADR-033). The MMU is
  implemented (ADR-030, `cpu-a9::mmu`); see the BSP section below.

## Xilinx BSP ELFs on the native engine (2026-10-18)

17 unmodified Xilinx standalone-BSP applications run to completion and print their success lines on
the native engine (`backends.ps = "native"`; integration test suite `crates/cli/tests/bsp_native.rs`):

* `bsp_hello_world` (prints `Hello World`, `Successfully ran Hello World application`)
* `bsp_uartps_polled`, `bsp_uartps_intr`, `bsp_uartps_selftest` (UART local loopback and interrupts)
* `bsp_scugic`, `bsp_scugic_lowlevel` (GIC distributor and CPU interface, PrimeCell CIDR IDs)
* `bsp_scutimer_polled`, `bsp_scutimer_intr` (SCU private timer load and countdown)
* `bsp_ttcps_tapp`, `bsp_ttcps_lowlevel` (Cadence TTC interval/match interrupts and clear-on-read)
* `bsp_canps_polled` (CAN controller in loopback)
* `bsp_iicps_selftest` (I2C controller self-test)
* `bsp_spips_selftest` (SPI controller self-test)
* `bsp_devcfg_selftest` (Device configuration controller)
* `bsp_peripheral_tests` (`testperiph.c` generated by Vitis: SCUGIC, CAN)
* `bsp_xadcps_polled` (XADC DRP pipeline reads temperature: 24.979 °C natively)

The prerequisites implemented to run these unmodified ELFs were:
1. EFUSE stub window at `0xF800D000..=0xF800DFFF` (`CheckEFUSE` read by BSP startup).
2. PrimeCell CIDR0..3 identification registers (`0xB105F00D`) on Distributor offsets `0xFF0..=0xFFC`.
3. Private timer Load Register reload on write.
4. Cadence TTC clear-on-read on `ISR` (`0x54..=0x5C`).
5. XADC DRP pipeline returning internal sensor read on NOP/read command.
6. UART local loopback forwarding (`MR.CHMODE = 2`) and receiver timeout counter.
7. Thumb-2 parallel arithmetic/media instructions (`SADD8`/`SSUB8`/`SADD16`/`SSUB16`/`SEL`/`SXTB16`/`UXTB16`) and Thumb-2 VFP coprocessor 10/11 dispatch.

## Batch 2 blocks on the native engine (ps-periph)

The new register models (GPIO, SPI0/1, I2C0/1, XADC, devcfg, CAN0/1, the watchdog reset request)
are checked by the firmware-derived tests in `crates/ps-periph/tests/conf_*.rs` and the BSP
sequences in `bsp_periph2.rs`. They do not run the firmware on the native engine: the conformance
firmware is run by the QEMU tests only. The table in this file is unchanged; see *Native PS
peripherals, batch 2* in `STATUS.md` for the per-block results (GPIO 12/12, SPI 28/28, I2C 22/22,
XADC 15/15 checks PASS; the XADC temperature finding is documented there).
Running the BSP driver ELFs on the native engine was not attempted (ADR-033 refuses VFP and MMU).

## Two cores (`[cpu1]`)

Checked with `examples/dual-core-native` and `examples/dual-core-qemu`: the same ELF
(`firmware/dual_core_demo`) on both cores.

| Item | QEMU | Native | Difference |
|---|---|---|---|
| UART1 (core 0): the lines and `RESULT: ALL PASS` | printed | printed | none (compared byte for byte) |
| UART0 (core 1): its lines | discarded (`-serial null`) | printed | pinned: native-only; the test asserts QEMU reads nothing on port 0 |
| Core 1 start | the loader sets its PC; `_start` parks it on the mailbox | boot-ROM loop at 0xFFFFFE00 polls the mailbox | both reach the same entry through the mailbox |
| Timing | instruction count (icount) | one cycle per instruction, round-robin batches of 8 | not compared (F0 on both) |

The native half always runs; the QEMU half skips without the Xilinx QEMU. Five processes give one
trace digest (`cli/tests/native_ps_diff::dual_core_five_processes_give_one_trace_digest`).
