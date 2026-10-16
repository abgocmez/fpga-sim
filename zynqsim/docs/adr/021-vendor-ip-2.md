# ADR-021: More Xilinx IP models (`vendor-ip`), scatter-gather DMA, passive IP

Status: **Accepted** (milestone P3). Date: 2026-10-16.

## Context

ADR-016 modelled the three peripherals that most designs carry (AXI GPIO, AXI Timer, AXI UART
Lite). Real XSA handoffs also carry an AXI DMA, an AXI IIC, an AXI BRAM controller and an AXI
interrupt controller, and infrastructure blocks (interconnects, reset generators, concatenations,
clock wizards, block memory) that have no registers of their own. Until now the DMA stopped a run
through `[xsa.unmodelled]`, and every infrastructure block had the same effect.

## Decision

* **Four more register-level models in `crates/vendor-ip`**, each a `pl_native::RegisterBlock`
  like ADR-016's:
  * `AxiDma` (axi_dma 7.1, PG021): direct register mode and scatter-gather mode with a linear
    descriptor chain, loopback of the AXI4-Stream (`STREAM_LOOPBACK`), IOC/Dly/Err interrupts
    with threshold and delay, error bits, soft reset. Multi-channel mode, micro DMA, cyclic
    descriptors, keyhole and 64-bit addresses are rejected or not modelled.
  * `AxiIic` (axi_iic 2.1, PG090): dynamic controller mode, master only, bit-banged on a Pmod
    port's two pins (`VendorPl` binding), so the device models of `crates/devices` answer as they
    do for RTL designs. Standard controller mode, slave mode, arbitration and clock stretching are
    not modelled.
  * `AxiBramCtrl` (axi_bram_ctrl 4.1, PG078) with its block memory as one window: 32-bit accesses
    with byte strobes, zero power-on contents. ECC registers and the second port are not modelled.
  * `AxiIntc` (axi_intc 4.1, PG099): software and hardware interrupts, edge and level inputs,
    `IPR`/`IER`/`IAR`/`SIE`/`CIE`/`IVR`/`MER`. Fast interrupt mode and cascade are stored or
    stubbed. Its inputs are the interrupt outputs of other models, routed by `[connect]`.
* **Register facts are from memory.** As for ADR-016, every fact not checked against a Vivado
  handoff or a Xilinx simulation carries a `VERIFY:` marker in the module documentation and in
  `docs/STATUS.md`. Some offsets, field names and reset values were checked against the register
  metadata of Vivado 2022.1 handoffs written for the fixtures (`fixtures/xsa/make_*_xsa.tcl`);
  the behaviour was not.
* **Scatter-gather DMA is the same model with `C_INCLUDE_SG = 1`.** A descriptor is 64 bytes in
  memory, read and written through the HP port the design connects; the tail-descriptor write
  starts the chain. The limit is that only linear chains are modelled, and that there is no Vivado
  handoff for the SG configuration (`examples/dma-sg` is hand-written). Its firmware checks the
  status words, which is the part a driver actually depends on.
* **Passive IP is recognised, not modelled.** Interconnects, processor-system resets, concatenations,
  clock wizards and block memories that carry no register the firmware can reach are classed as
  infrastructure by `xsa-import` (`catalog.rs`) and reported as "Passive IP" in the import report.
  They map to nothing and do not stop a run. A block with an address window of its own that is not
  in the passive class stays unmodelled.
* **Interrupt routing.** A block's interrupt output reaches the controller through `[connect]`
  (`"axi_timer_0.interrupt" = "axi_intc_0.intr[1]"`), and the controller's output takes one
  `IRQ_F2P` line. Interrupt timing follows ADR-016: a change caused by an access travels with the
  access, one caused by time or input at the next lock-step sync.
* **The examples** are `examples/dma-loopback` (direct mode, the Vivado handoff
  `zs_dma_loopback.xsa`), `examples/dma-sg` (scatter-gather, hand-written), `examples/iic-bram-demo`
  (IIC reads the TMP2 model, BRAM holds data) and `examples/intc-demo` (INTC collects the GPIO and
  timer interrupts). Their firmware is Rust `no_std` in `firmware/dma_loopback`, `firmware/dma_sg`,
  `firmware/iic_bram_demo` and `firmware/intc_demo`.
* **The unmodelled example moves.** The hand-written handoff with an AXI DMA that stops a run is
  now `fixtures/xsa/zs_math_vdma.hwh` (an AXI VDMA, `axi_vdma`, which is not modelled), because the
  DMA is modelled.

## Consequences

* A design with the DMA, IIC, BRAM controller or interrupt controller runs with the real firmware
  on the native PL, with or without user RTL in XSim (`VendorPl` routes as in ADR-016).
* The models are a deterministic function of simulated time, like the ADR-016 models. The DMA
  moves its data in one go at the access that starts it and signals completion after the AXI beat
  count; a DMA-driven race that depends on bus timing is not reproduced.
* The register maps remain the largest risk. Firmware that works here and fails on silicon is
  most likely to rely on a `VERIFY:` item in `docs/STATUS.md`.
* Not built: VDMA, Quad SPI, Clocking Wizard as a model (it is passive), timer capture and cascade,
  UART 16550, multi-channel DMA.
