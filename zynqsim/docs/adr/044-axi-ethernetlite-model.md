# ADR-044: AXI Ethernet Lite model (`vendor-ip`)

Status: **Accepted**. Date: 2026-10-19.

## Context

The Xilinx AXI Ethernet Lite MAC 3.0 (PG135) core provides a lightweight 10/100 Mb/s Ethernet MAC interfacing directly to an external MII/RMII PHY. Unlike bus-mastering network or DMA peripherals (such as GEM or AXI DMA/VDMA), the CPU directly reads and writes network packets to/from internal 2 KiB transmit (TX) and receive (RX) buffer memories through memory-mapped I/O on `M_AXI_GP`.

Previously, `axi_ethernetlite` was recognized in `xsa-import::catalog` but unmodelled in `vendor-ip`, serving as the representative unmodelled IP in test fixture `fixtures/xsa/zs_math_ethlite.hwh`. Implementing a behavioural register-level model enables software and driver simulation of Ethernet Lite MAC communication, MDIO PHY management, packet transmission, loopback reception, and interrupt handling.

## Decision

* **Register-level behavioural model in `crates/vendor-ip/src/ethernetlite.rs`** as a `pl_native::RegisterBlock`:
  * 8 KiB address window (`0x2000` bytes):
    * TX Ping data buffer memory at `0x0000`..`0x07E3` (up to 2020 bytes).
    * `TX_PING_LEN` at `0x07E4`: transmit frame byte length (bits 10:0).
    * MDIO PHY management registers: `MDIOADDR` (`0x07E8`), `MDIOWR` (`0x07EC`), `MDIORD` (`0x07F0`), `MDIOCTRL` (`0x07F4`). Supports PHY address, register address, OP (read/write), and ready bit. Default PHY model simulates standard IEEE 802.3 100BASE-TX full-duplex transceiver registers (ID `0x0141:0x0DD1`, basic status `0x786D` link up).
    * Global Interrupt Enable `GIE` at `0x07F8`: bit 31 `GIE_ENABLE`.
    * `TX_PING_CTRL` at `0x07FC`: bit 0 `TX_CTRL_BUSY` (initiates transmission, self-clearing upon completion), bit 1 `TX_CTRL_PROGRAM` (MAC address programming), bit 3 `TX_CTRL_IE` (transmit interrupt enable), bit 4 `TX_CTRL_LOOPBACK` (internal loopback enable). Writing clears pending TX interrupt.
    * TX Pong data buffer memory at `0x0800`..`0x0FE3`, `TX_PONG_LEN` at `0x0FE4`, and `TX_PONG_CTRL` at `0x0FFC` (active when `C_INCLUDE_GLOBAL_BUFFERS = 1`).
    * RX Ping data buffer memory at `0x1000`..`0x17E3`, `RX_PING_LEN` at `0x17E4` / `0x17F4`, and `RX_PING_CTRL` at `0x17FC`: bit 0 `RX_CTRL_STATUS` (set by hardware when frame is ready, cleared to 0 by software to release buffer), bit 3 `RX_CTRL_IE` (receive interrupt enable).
    * RX Pong data buffer memory at `0x1800`..`0x1FE3`, `RX_PONG_LEN` at `0x1FE4` / `0x1FF4`, and `RX_PONG_CTRL` at `0x1FFC`.
  * Frame routing and loopback:
    * Internal loopback routes transmitted TX frames directly into RX buffers.
    * Host-level frame injection (`push_rx`) and frame extraction (`take_tx`) for external network simulation.
  * Interrupt line:
    * Single active-high level interrupt line `ip2intc_irpt` asserting when `GIE` is set and an enabled TX complete or RX frame event is active.
* **Integration across crates:**
  * Registered in `vendor_ip::MODELS`, `param_names`, `irq_output_index`, `VendorPl::add`, and `VendorPl::ethernetlite`.
  * Registered in `sim-api::pl_attach::NATIVE_MODELS`.
  * Preserved unmodelled IP verification: introduced `fixtures/xsa/zs_math_uart16550.hwh` (`xilinx.com:ip:axi_uart16550:2.0`) to replace `zs_math_ethlite.hwh` in tests verifying unmodelled IP rejection (`cli::tests::xsa`, `mcp::tests::xsa`, `sim-api::tests::xsa_import`).
  * Added handoff import tests verifying `zs_math_ethlite.hwh` is now mapped to native model `axi_ethernetlite`.
* **Firmware and example:**
  * Guest firmware `firmware/ethlite_demo/src/main.rs`, built to `fixtures/elf/rust_ethlite.elf` via `cargo xtask firmware`.
  * Example project `examples/ethlite-demo/project.toml`.
  * Determinism test case in `crates/cli/tests/determinism.rs` passing 5 runs identically.
  * Unit tests in `crates/vendor-ip/src/ethernetlite.rs` and integration test `sim-api::vendor_ip2::the_axi_ethernetlite_model_loops_packets_and_interrupts`.

## Not verified (`VERIFY:`)

* Hardware MII/RMII interface clock synchronization and PHY negotiation timings.
* MAC address destination filtering when not in promiscuous mode.

## Consequences

* Networking software and drivers designed for the Xilinx AXI Ethernet Lite 3.0 IP core can now run in ZynqSim with full packet transmission, loopback verification, MDIO management, and interrupt signaling.
* `#![forbid(unsafe_code)]` remains strictly enforced across all workspace crates.

