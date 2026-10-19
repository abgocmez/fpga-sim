# ADR-041: AXI4-Stream FIFO (`axi_fifo_mm_s`) model (`vendor-ip`)

Status: **Accepted**. Date: 2026-10-19.

## Context

ADR-040 added the AXI Quad SPI model and set the pattern for the remaining Xilinx IP candidates
(`docs/HANDOVER.md`, section D): a register-level `pl_native::RegisterBlock` in `crates/vendor-ip`, the
XSA mapping through the IP's VLNV, a Rust `no_std` firmware with a fixture ELF, an example, unit tests per
register and end-to-end tests. The AXI4-Stream FIFO (`xilinx.com:ip:axi_fifo_mm_s`, Xilinx PG080, memory
mapped interface) is the next candidate. Designs use it to buffer stream data between a processor and PL
logic. Until now such a design stopped the import as unmodelled.

## Decision

* **Register-level model in `crates/vendor-ip/src/axis_fifo.rs`** as a `pl_native::RegisterBlock`:
  * `ISR` (0x00, write 1 to clear), `IER` (0x04), `TDFR` (0x08, key `0xA5`), `TDFV` (0x0C, vacancy in
    words), `TDFD` (0x10, one word per write), `TLR` (0x14, writing it commits a packet of that many bytes),
    `RDFR` (0x18, key `0xA5`), `RDFO` (0x1C, occupancy in words), `RDFD` (0x20, one word per read), `RLR`
    (0x24, byte length of the packet at the head), `SRR` (0x28, key `0xA5`), `TDR` (0x2C, destination of the
    next packet), `RDR` (0x30, destination of the head packet).
  * **Loopback.** The stream interface is modelled as TX feeding RX inside the FIFO. Writing `TLR` moves
    `ceil(TLR / 4)` words from the transmit FIFO into the receive FIFO as one packet and raises the transmit
    complete (`TC`) and receive complete (`RC`) status bits. `RDFD` pops the words and `RLR` follows the packets.
  * **Errors.** Empty `RDFD` reads (`RPURE`), `TDFD` writes to a full FIFO (`TPOE`), `TLR` larger than the
    transmit data (`TSE`, nothing committed), and a packet that does not fit the receive FIFO (`RPORE`, dropped).
  * **Interrupts.** `IRQ = (ISR & IER) != 0` on the `IRQ_F2P` line of the instance; there is no global enable.
* **Parameters.** `C_TX_FIFO_DEPTH` and `C_RX_FIFO_DEPTH` (words, default 512) are read from the block
  design; the window size comes from the address map.
* **Integration.**
  * Added to `vendor_ip::MODELS`, `param_names`, `VendorPl::add` and `VendorPl::fifo(name)`, and to
    `sim-api::pl_attach::NATIVE_MODELS`. The catalog already recognised `axi_fifo_mm_s` as a vendor IP, so the
    XSA importer maps it to `[native]` without further configuration.
  * `sim-api/src/xsa.rs` refuses a FIFO whose AXI4-Stream port is wired to another block (an IP or
    user RTL). The model cannot deliver those streams, so the import reports the FIFO as unmodelled instead
    of simulating a loopback that the design does not have.
* **Verification and guest firmware.**
  * Guest firmware `firmware/axis_fifo_demo`, built with `cargo xtask firmware` to `fixtures/elf/rust_axis_fifo.elf`
    (listed in `crates/xtask` `FIRMWARE`).
  * Example `examples/axis-fifo-demo/project.toml` (base `0x43C00000`, `IRQ_F2P[0]`).
  * Unit tests in `vendor-ip::axis_fifo::tests` (one per register group plus reset values and the parameters).
  * End-to-end: `cli::native_pl::axi_fifo_firmware_runs_on_the_native_ps_with_the_vendor_model`,
    `sim-api::vendor_ip2::the_axi_fifo_model_loops_packets_back_and_raises_interrupts`.
  * XSA mapping: `sim-api::xsa_import::{an_axi_fifo_mm_s_instance_is_mapped_as_a_native_model,
    an_axi_fifo_whose_stream_is_wired_to_another_block_is_unmodelled}`.
  * Added to `crates/cli/tests/determinism.rs` (5/5 identical trace hashes).

## Consequences

* Vivado handoffs with a memory-mapped AXI4-Stream FIFO whose stream ports are unconnected or looped back
  inside the FIFO import and run on the native PS.
* A FIFO whose stream goes to other logic is reported as unmodelled; it still stops the run as before.
* The register map, bit positions and reset values are from memory of PG080 and are not checked against
  hardware or a Xilinx simulation. Every such fact is marked `VERIFY:` in `axis_fifo.rs` and in STATUS.md.
  The interrupt output port name used in the example's `[connect]` (`interrupt`) is also `VERIFY:`.
* `#![forbid(unsafe_code)]` is preserved across all workspace crates.
