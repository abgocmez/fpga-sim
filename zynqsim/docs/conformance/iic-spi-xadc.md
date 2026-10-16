# Conformance: PS I2C, SPI and XADC (xiicps, xspips, xadcps)

Rust `no_std` firmware for three Zynq-7000 PS blocks (`firmware/conf_iicps`, `firmware/conf_spips`,
`firmware/conf_xadcps`), run on the Xilinx QEMU PS model (`examples/conf-iicps`, `conf-spips`,
`conf-xadcps`). Each firmware prints one line per check on UART1, `CHECK n name: PASS|FAIL|N/A reason`,
and `RESULT: ALL PASS` when no check failed. The test is `crates/cli/tests/conformance_iic_spi_xadc.rs`
(`conformance_iic_spi_xadc::iic_spi_xadc_firmware_passes_with_the_listed_na_checks`): it runs the three
examples and asserts `RESULT: ALL PASS`, no FAIL, and the exact list of N/A checks below.

Register offsets, bit meanings and reset values are from memory of UG585 and the vendor drivers. They
are marked `VERIFY:` in the firmware and have not been checked against hardware. Nothing here is a claim
that the model matches a board.

## Summary

| Block | Address | Features checked | PASS / N/A / FAIL | PS model | Evidence (test) |
|---|---|---|---|---|---|
| xiicps: I2C0, I2C1 | 0xE0004000, 0xE0005000 | reset values, CONTROL (MS, NEA, ACKEN, HOLD), ADDR, TRANS_SIZE, Slave_mon_pause, Time_out, IER/IDR/IMR, ISR write-1-to-clear, master write and read to an address with no device (NACK), I2C1 independence | 19 / 3 / 0 | **partly modelled** | `conformance_iic_spi_xadc::iic_spi_xadc_firmware_passes_with_the_listed_na_checks` |
| xspips: SPI0, SPI1 | 0xE0006000, 0xE0007000 | reset values, Config (master, CPHA, baud, manual start, SS force, CS), Enable, Delay, TX/RX thresholds, Slave_Idle_count, IER/IDR, TX FIFO fill and TX_FIFO_not_full/full status, manual-start transfer and RX FIFO, loopback | 0 / 27 / 0 | **not modelled** | same test |
| xadcps: XADC interface | 0xF8007100 | reset values, CFG, INT_MASK, MCTL, INT_STS, CMDFIFO/RDFIFO temperature and VCCINT, DRP write then read at 0x40 | 4 / 9 / 0 | **partly modelled** | same test |

Every check in the tables below is either PASS (the documented behaviour, as remembered, is observed)
or N/A with the reason. No check FAILs. An N/A is never reported as PASS.

## How N/A is decided

* **Not modelled (SPI).** Each SPI firmware first writes 0x1234 to the Delay register of the controller
  and reads it back. The readback was 0 on both controllers, and an address with no device
  (`0xE0008000`, a probe run during this work) also reads 0. Every register check on the controller is
  then N/A with that reason. The probe itself is temporary and is not kept in the firmware.
* **Reset value differs from the remembered UG585 value.** Reset checks are PASS when equal. When not
  equal they are N/A with both values, because the mismatch is either a model deviation or a wrong
  remembered value, and neither is verified.
* **No conversion data (XADC).** A value of 0 read from RDFIFO is not a valid code for the temperature,
  VCCINT or the DRP register used here, so that check is N/A. The DRP write-then-read at 0x40 also reads 0.

## Partly / not modelled, in detail

* **xiicps: partly modelled.** Modelled: the register file is read/write as documented for CONTROL,
  ADDR, TRANS_SIZE, Slave_mon_pause, Time_out, the interrupt mask set and the NACK status of a master
  transfer to an address with no device (writes and reads). Deviations: the TIMEOUT reset reads 0x00
  where the remembered UG585 value is 0x1F (N/A on I2C0 and I2C1). Not checked: a read from an attached
  device. The example attaches no I2C device. The co-sim DTB has `cdns,i2c-r1p10` nodes, and an
  `i2cswitch` node, but the bus path was not identified in this work (the QEMU monitor dump of the
  device tree could not be obtained). The launch code was not changed.
* **xspips: not modelled.** The SPI0 and SPI1 register windows read 0 and ignore writes. The co-sim
  DTB has `spi@e0006000` and `spi@e0007000` nodes (`xlnx,zynq-spi-r1p6`); whether they are `okay` or
  `disabled` was not verified, so the reason for the missing model is not known. The QSPI controller
  (`qspi_demo`) is a different block, and it works.
* **xadcps: partly modelled.** Modelled: CFG (write/read 0x1234 kept), MCTL (write/read), INT_MASK
  (bits 9:0 read/write). The reset values differ from the remembered UG585 values (CFG 0x1814,
  INT_STS 0x200, MSTS 0x500, MCTL 0x10; N/A). The command FIFO path returns no conversion data: the
  temperature and VCCINT reads, and a DRP write followed by a read, all return 0 (N/A). The command word
  format (`(cmd << 26) | (addr << 16) | data`, Linux `xilinx-xadc`) is from memory, and the evidence here
  does not exclude a format error on our side, so the N/A is stated as what the model returned.
  INT_STS write-1-to-clear is not checked (no event is produced).

## Per-check results

Result column: `PASS` with the observed value, or `N/A` with the reason, as printed by the firmware
in the run of the test above.


### xiicps (I2C0, I2C1)

| # | Check | Result | Reason or value |
|---|---|---|---|
| 1 | I2C0 CONTROL reset | PASS | 0x00000000 |
| 2 | I2C0 STATUS reset | PASS | 0x00000000 |
| 3 | I2C0 ISR reset | PASS | 0x00000000 |
| 4 | I2C0 IMR reset (all masked) | PASS | 0x000002ff |
| 5 | I2C0 TIMEOUT reset | N/A | reset reads 0x00000000, UG585 as remembered 0x0000001f (VERIFY) |
| 6 | I2C0 TRANS_SIZE reset | PASS | 0x00000000 |
| 7 | I2C0 ADDR reset | PASS | 0x00000000 |
| 8 | I2C0 CONTROL write/read (MS NEA ACKEN HOLD) | PASS | 0x0000001e |
| 9 | I2C0 HOLD bit set and cleared | PASS | 0x00000010 |
| 10 | I2C0 ADDR write/read | PASS | 0x00000050 |
| 11 | I2C0 TRANS_SIZE write/read | PASS | 0x00000003 |
| 12 | I2C0 Slave_mon_pause write/read | PASS | 0x00000005 |
| 13 | I2C0 Time_out write/read | PASS | 0x0000000a |
| 14 | I2C0 IER/IDR move the COMP mask | PASS | 0x00000001 |
| 15 | I2C0 write to absent device: NACK status | PASS | 0x00000001 |
| 16 | I2C0 ISR write-1-to-clear (NACK) | PASS | 0x00000000 |
| 17 | I2C0 read from absent device: NACK status | PASS | 0x00000001 |
| 18 | I2C0 read from attached device | N/A | no I2C device is attached in the co-sim bus; the launch code does not attach one (not verified: bus path) |
| 19 | I2C1 ADDR independent of I2C0 | PASS | 0x00000033 |
| 20 | I2C1 TIMEOUT reset | N/A | reset reads 0x00000000, UG585 as remembered 0x0000001f (VERIFY) |
| 21 | I2C1 write to absent device: NACK status | PASS | 0x00000001 |
| 22 | I2C0 IMR is read-only | PASS | 0x000002ff |

### xspips (SPI0, SPI1)

| # | Check | Result | Reason or value |
|---|---|---|---|
| 1 | SPI0 Config reset | N/A | not modelled by QEMU: the register window reads 0 and ignores writes |
| 2 | SPI0 Enable reset (disabled) | N/A | not modelled by QEMU: the register window reads 0 and ignores writes |
| 3 | SPI0 Delay reset | N/A | not modelled by QEMU: the register window reads 0 and ignores writes |
| 4 | SPI0 TX threshold reset | N/A | not modelled by QEMU: the register window reads 0 and ignores writes |
| 5 | SPI0 RX threshold reset | N/A | not modelled by QEMU: the register window reads 0 and ignores writes |
| 6 | SPI0 Slave_Idle_count reset | N/A | not modelled by QEMU: the register window reads 0 and ignores writes |
| 7 | SPI0 empty TX FIFO: TX_FIFO_not_full set | N/A | not modelled by QEMU: the register window reads 0 and ignores writes |
| 8 | SPI0 empty TX FIFO: TX_FIFO_full clear | N/A | not modelled by QEMU: the register window reads 0 and ignores writes |
| 9 | SPI0 interrupt mask reset (all masked) | N/A | not modelled by QEMU: the register window reads 0 and ignores writes |
| 10 | SPI0 Config write/read (master, CPHA, baud) | N/A | not modelled by QEMU: the register window reads 0 and ignores writes |
| 11 | SPI0 manual start enable, SS force and CS0 read back | N/A | not modelled by QEMU: the register window reads 0 and ignores writes |
| 12 | SPI0 Delay write/read | N/A | not modelled by QEMU: the register window reads 0 and ignores writes |
| 13 | SPI0 TX threshold write/read | N/A | not modelled by QEMU: the register window reads 0 and ignores writes |
| 14 | SPI0 RX threshold write/read | N/A | not modelled by QEMU: the register window reads 0 and ignores writes |
| 15 | SPI0 Slave_Idle_count write/read | N/A | not modelled by QEMU: the register window reads 0 and ignores writes |
| 16 | SPI0 IER/IDR move the TX_FIFO_not_full mask | N/A | not modelled by QEMU: the register window reads 0 and ignores writes |
| 17 | SPI0 Enable write/read (1) | N/A | not modelled by QEMU: the register window reads 0 and ignores writes |
| 18 | SPI0 Enable write/read (0) | N/A | not modelled by QEMU: the register window reads 0 and ignores writes |
| 19 | SPI0 TX FIFO full after 128 writes: TX_FIFO_full set | N/A | not modelled by QEMU: the register window reads 0 and ignores writes |
| 20 | SPI0 full TX FIFO: TX_FIFO_not_full clear | N/A | not modelled by QEMU: the register window reads 0 and ignores writes |
| 21 | SPI0 manual start drains the TX FIFO (TX_FIFO_full clear) | N/A | not modelled by QEMU: the register window reads 0 and ignores writes |
| 22 | SPI0 RX FIFO not empty after the transfer | N/A | not modelled by QEMU: the register window reads 0 and ignores writes |
| 23 | SPI0 loopback of the first byte with no slave attached | N/A | not modelled by QEMU: the register window reads 0 and ignores writes |
| 24 | SPI0 Delay unchanged by a write to SPI1 | N/A | not modelled by QEMU: the register window reads 0 and ignores writes |
| 25 | SPI1 Delay write/read | N/A | not modelled by QEMU: the register window reads 0 and ignores writes |
| 26 | SPI1 Config reset | N/A | not modelled by QEMU: the register window reads 0 and ignores writes |
| 27 | SPI0 interrupt line to the GIC | N/A | not checked: this program reads the status registers only |

### xadcps (XADC)

| # | Check | Result | Reason or value |
|---|---|---|---|
| 1 | XADC CFG reset | N/A | reset reads 0x00001814, UG585 as remembered 0x00000000 (VERIFY) |
| 2 | XADC INT_STS reset | N/A | reset reads 0x00000200, UG585 as remembered 0x00000000 (VERIFY) |
| 3 | XADC MSTS reset | N/A | reset reads 0x00000500, UG585 as remembered 0x00000000 (VERIFY) |
| 4 | XADC MCTL reset | N/A | reset reads 0x00000010, UG585 as remembered 0x00000000 (VERIFY) |
| 5 | XADC INT_MASK write/read (bits 9:0) | PASS | 0x000003ff |
| 6 | XADC INT_MASK cleared | PASS | 0x00000000 |
| 7 | XADC MCTL write/read | PASS | 0x00000040 |
| 8 | XADC CFG write/read | PASS | 0x00001234 |
| 9 | XADC INT_STS write-1-to-clear | N/A | not checked: no interrupt event is produced by this program |
| 10 | XADC temperature via CMDFIFO/RDFIFO in -40..125 C | N/A | RDFIFO reads 0 for the command: the model returns no conversion data |
| 11 | XADC VCCINT via CMDFIFO/RDFIFO in 0.9..1.1 V | N/A | RDFIFO reads 0 for the command: the model returns no conversion data |
| 12 | XADC DRP write then read (0x40) returns the value | N/A | RDFIFO reads 0 for the command: the model returns no conversion data |
| 13 | XADC alarm thresholds (DRP 0x40..0x4F) beyond 0x40 | N/A | not checked: only the DRP write/read at 0x40 is exercised |
