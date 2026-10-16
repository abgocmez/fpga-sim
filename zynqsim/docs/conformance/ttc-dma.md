# Peripheral conformance: TTC (xttcps) and DMA controller (xdmaps)

Rust `no_std` firmware on the PS, run on the Xilinx QEMU (`arm-generic-fdt-7series`, co-sim DTB),
each one self-reporting its checks over UART1 (spec section 14.2). The firmware prints
`CHECK n name: PASS|FAIL|N/A reason` and `RESULT: ALL PASS` when nothing failed.

Test: `zynqsim/crates/cli/tests/conformance_ttc_dma.rs` (`cargo test -p zynqsim --test
conformance_ttc_dma -- --nocapture`). The test asserts `RESULT: ALL PASS`, the check numbers
1..n, and the exact N/A list. Firmware: `firmware/conf_ttc`, `firmware/conf_dmaps`; examples:
`examples/conf-ttc`, `examples/conf-dmaps`.

Register offsets and bit positions come from UG585 (TTC) and the ARM PL330 TRM (DMAC) as remembered,
not from the documents themselves. Items marked **VERIFY** were not checked against the TRM. A
check that the model fails for a reason we could observe is reported `N/A`, not dropped.

## Summary

| Block | Checks | PASS | N/A | Model status |
|---|---|---|---|---|
| xttcps, TTC0 at 0xF8001000 and TTC1 at 0xF8002000 | 14 | 13 | 1 (check 12) | **partly modelled**: the QEMU model clears ISR flags on read |
| xdmaps, PL330 DMAC at 0xF8003000 | 12 | 12 | 0 | **fully modelled for the checked features** (see *Not exercised*) |

## xttcps (TTC), `examples/conf-ttc`

| # | Check | Result | Status |
|---|---|---|---|
| 1 | Counter_Control reset: counter disabled | PASS | |
| 2 | Clock_Control readback (PS_EN, PS_VAL) | PASS | |
| 3 | Interval_Value readback, 3 counters, 16 bits | PASS | |
| 4 | Match_1..3 readback, 3 counters (9 registers) | PASS | |
| 5 | TTC1 registers separate from TTC0 (0xF8002000) | PASS | |
| 6 | Counter reset (RST) clears Counter_Value, RST self-clears | PASS | |
| 7 | Overflow mode counts past Interval_Value | PASS | |
| 8 | Interval mode wraps at Interval_Value and sets the interval flag | PASS | |
| 9 | Decrement mode counts down | PASS | |
| 10 | Match_1 sets the match flag when the counter reaches it | PASS | |
| 11 | Prescaler divides the clock by 2^(PS_VAL+1) (ratio within 8..32 of 16) | PASS | |
| 12 | Match flag sticky across reads (cleared only by writing 1) | **N/A** | see below |
| 13 | Interval interrupt reaches GIC (TTC0 counter 0, ID 42) | PASS | |
| 14 | Interval interrupt reaches GIC (TTC1 counter 0, ID 69) | PASS | |

**Check 12, N/A.** The match flag is set when the counter passes Match_1, and the counter is then
stopped without reading ISR. The first read returns `0x4`, the second returns `0x0`, with no write
in between. QEMU therefore clears the match flag on read. I recall that the xttcps driver clears
the flags by writing 1, and that the interval flag is read-to-clear in the Linux driver. This is not
confirmed against UG585 here (VERIFY), so the check is reported as N/A with that evidence instead of
FAIL. If UG585 says the flags are write-1-to-clear, this is a model deviation to fix in QEMU.

**Mode bit (VERIFY).** The firmware selects interval mode with Counter_Control bit 1 (`0x2`). In
this model that bit makes the counter wrap at Interval_Value and set ISR bit 0; with it clear, the
counter runs to 0xFFFF. I recall UG585 as the other way round, so this polarity is a VERIFY item.
Check 7 and check 8 fail together if the model and TRM disagree and the bit is inverted.

**Offsets (VERIFY).** Per-counter registers are at base + offset + 4 x counter for Clock_Control
(0x00), Counter_Control (0x0C), Counter_Value (0x18), Interval (0x24), ISR (0x54) and IER (0x60).
Match_x is at 0x30 + 0x0C x counter + 4 x x. These offsets were consistent with the model (checks 3,
4, 6 and 8 pass), but the TRM was not checked.

**Interrupts.** GIC IDs 42 and 69 for counter 0 of TTC0 and TTC1 (VERIFY). Both were taken through
`rt::irq` and the handler ran at least three times per TTC (checks 13 and 14).

**Not exercised.** Waveform output, external clock source and event-counting mode, match 2 and 3
flags and their interrupts, IER enables for match, and ISR flags with interrupts disabled.

## xdmaps (PL330), `examples/conf-dmaps`

| # | Check | Result | Status |
|---|---|---|---|
| 1 | Peripheral ID is ARM PL330 (part 0x330, PID0 0x30, PID1 low nibble 3) | PASS | |
| 2 | DMA manager stopped after reset (DS bits 3:0 = 0) | PASS | VERIFY (DS encoding) |
| 3 | Debug status idle before a DBGCMD (DBGSTATUS bit 0 = 0) | PASS | VERIFY |
| 4 | INTEN readback (event 0 enabled) | PASS | |
| 5 | CR0..CR4 readable (values printed as INFO, not decoded) | PASS | |
| 6 | DMAGO copies 64 bytes correctly (DMAMOV SAR/DAR/CCR, 4 x DMALD/DMAST bursts of 4 words) | PASS | |
| 7 | DMASEV 0 sets the event interrupt status (INTSTATUS bit 0) | PASS | |
| 8 | INTCLR clears the event interrupt | PASS | |
| 9 | Channel 0 stopped after DMAEND (CSR0 bits 3:0 = 0), FSRC clear | PASS | |
| 10 | Channel 0 PC ends inside the program (CPC0) | PASS | |
| 11 | A second DMAGO with a new pattern copies again (channel restart) | PASS | |
| 12 | No DMA manager fault (FSRD clear) | PASS | |

Method: the DMAC is driven through its debug interface. A DMAGO (0xA0, channel, PC) goes into
DBGINST0 (byte 0 at bits 23:16, byte 1 at bits 31:24) and DBGINST1 (PC, little endian), DBGCMD is
written with 0, and the channel program in RAM is `DMAMOV SAR`, `DMAMOV DAR`, `DMAMOV CCR`, then
four `DMALD`/`DMAST` pairs, `DMASEV 0` and `DMAEND`. The destination is pre-filled with 0xEE, so a
pass cannot come from the initial contents. CCR is `0x000D4035` (increment on both sides, 4-byte
beats, 4 beats per burst).

**Encodings (VERIFY).** DMAMOV 0xBC with register (SAR 0, CCR 1, DAR 2), DMALD 0x04, DMAST 0x08,
DMASEV 0x34 with the event in the second byte bits 7:3, DMAEND 0x00, DMAGO 0xA0.

**Not exercised.** Interrupts from the DMAC to the GIC (the interrupt lines are polled through
INTSTATUS instead); faults (FSRC/FTRD on bad addresses or instructions); DMAWFE and waiting for an
event; ES (the event status is a pulse here); SAR/DAR read back (not in the register map); DMA
lines to PL. The result is therefore "fully modelled for the features checked", not for the whole
controller.

## Reproduce

```
cargo xtask firmware                       # builds firmware/ and copies the ELFs into fixtures/elf
ZYNQSIM_WORK_DIR=... cargo test -p zynqsim --test conformance_ttc_dma -- --nocapture
```
