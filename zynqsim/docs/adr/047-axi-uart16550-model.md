# ADR-047: AXI UART 16550 model (`vendor-ip`)

Status: **Accepted**. Date: 2026-10-19.

## Context

The Xilinx AXI UART 16550 core (`axi_uart16550` 2.0, PG143) provides an asynchronous serial data communications interface compatible with the industry-standard National Semiconductor 16550A device, wrapped for AXI4-Lite slave attachment. It is widely used in bare-metal and Linux systems for serial console and modem communications, supported by Xilinx's standalone `xuartns550` driver.

Previously, `axi_uart16550` was recognised in the catalog (`xsa-import::catalog`) as a vendor IP, but lacked a behavioural model in `crates/vendor-ip`. Designs containing an AXI UART 16550 stopped with an unmodelled IP diagnostic (and `fixtures/xsa/zs_math_uart16550.hwh` was originally created to test that rejection path).

## Decision

* **Register-level behavioural model in `crates/vendor-ip/src/uart16550.rs`** as a `pl_native::RegisterBlock`:
  * 64 KiB address window (`0x10000` bytes) with standard 16550 registers mapped at base offset `0x1000` (`XUN_REG_OFFSET` in Xilinx Vitis `xuartns550_l.h`).
  * Divisor Latch Access Bit (DLAB) banking at `0x1000` (RBR/THR when DLAB=0 vs DLL when DLAB=1) and `0x1004` (IER when DLAB=0 vs DLM when DLAB=1).
  * Implemented registers:
    * `RBR` (`0x1000`, RO): Receiver Buffer Register, pops received bytes from FIFO.
    * `THR` (`0x1000`, WO): Transmitter Holding Register.
    * `DLL` / `DLM` (`0x1000` / `0x1004`, RW): Baud rate divisor latches accessible when DLAB=1.
    * `IER` (`0x1004`, RW): Interrupt Enable Register (`ERBFI`, `ETBEI`, `ELSI`, `EDSSI`).
    * `IIR` (`0x1008`, RO): Interrupt Identification Register with priority encoding (Receiver Line Status, Received Data Available, Character Timeout, THRE, Modem Status) and FIFO status bits [7:6]. Reading IIR clears THRE interrupt.
    * `FCR` (`0x1008`, WO): FIFO Control Register with FIFO enable, RX/TX FIFO resets, DMA mode, and RX trigger levels (1, 4, 8, 14 bytes).
    * `LCR` (`0x100C`, RW): Line Control Register with word length (5..8), stop bits, parity enables, and DLAB bit 7.
    * `MCR` (`0x1010`, RW): Modem Control Register (`DTR`, `RTS`, `OUT1`, `OUT2`, `LOOP`). In hardware and model, `OUT2` (bit 3) serves as the master gate for the external interrupt output line `ip2intc_irpt`.
    * `LSR` (`0x1014`, RO): Line Status Register (`DR`, `OE`, `PE`, `FE`, `BI`, `THRE`, `TEMT`, `FIFOERR`). Reading LSR clears error flags.
    * `MSR` (`0x1018`, RO): Modem Status Register with delta bits (`DCTS`, `DDSR`, `TERI`, `DDCD`) cleared on read, and status bits (`CTS`, `DSR`, `RI`, `DCD`).
    * `SCR` (`0x101C`, RW): Scratchpad Register.
  * **Loopback diagnostic mode (`MCR & 0x10`)**:
    * Redirects bytes written to THR directly into the receiver FIFO.
    * Loops modem outputs to inputs (RTS->CTS, DTR->DSR, OUT1->RI, OUT2->DCD) and tracks deltas in MSR.
  * **Interrupt output (`ip2intc_irpt`)**:
    * Driven high when `MCR_OUT2` is asserted and any enabled interrupt source is active.
* **Unmodelled IP rejection test preservation**:
  * Introduced `fixtures/xsa/zs_math_hwicap.hwh` (`xilinx.com:ip:axi_hwicap:3.0`) to replace `zs_math_uart16550.hwh` in unmodelled rejection tests across `cli::tests::xsa`, `mcp::tests::xsa`, and `sim-api::tests::xsa_import`.
  * Added new mapping tests in `crates/sim-api/tests/xsa_import.rs` and `crates/cli/tests/xsa.rs` verifying that `zs_math_uart16550.hwh` maps to `axi_uart16550`.
* **Firmware and example**:
  * Guest firmware `firmware/uart16550_demo/src/main.rs`, built to `fixtures/elf/rust_uart16550.elf` via `cargo xtask firmware`.
  * Example project `examples/uart16550-demo/project.toml`.
  * Determinism case in `crates/cli/tests/determinism.rs` verified 5/5 identical runs.
  * Integration test `the_axi_uart16550_model_runs_loopback_and_interrupts` in `crates/sim-api/tests/vendor_ip2.rs`.

## Not verified (`VERIFY:`)

* Hardware serial line timing and start/stop bit jitter under external baud clocks.
* Character timeout delay counter in multi-byte FIFO trigger configurations.

## Consequences

* Bare-metal applications using Xilinx's `xuartns550` driver or standard 16550 UART registers can execute in co-simulation without Vivado or external RTL simulators.
* `#![forbid(unsafe_code)]` remains strictly enforced across all workspace crates.

