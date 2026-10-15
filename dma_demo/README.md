# dma_demo: 2 x Cortex-A9 + DMA engine in PL, time-locked co-simulation with live UI

A "hard" test for the QEMU + XSim setup: both Cortex-A9 cores run bare-metal code, the
PL contains an AXI4 DMA engine (`rtl/dma_xform.v`) that reads/writes DDR through
`S_AXI_HP0` and interrupts the PS through `IRQ_F2P[0]`.

| Piece | What it does |
|---|---|
| CPU0 (`sw/main.c`) | programs the engine, sleeps in WFI until the PL interrupt, verifies data, prints on UART1 |
| CPU1 (`sw_cpu1/main.c`) | produces input buffers (ping-pong) while the PL transforms the previous one; talks to CPU0 through a DDR mailbox (`sw/mailbox.h`) with WFE/SEV |
| PL (`rtl/dma_xform.v`) | AXI4-Lite registers + AXI4 INCR-burst master (<=16 beats, no 4 KB crossing), `dst = src*SCALE + OFFSET`, checksum, IRQ |
| Testbench (`sim/cosim_top.sv`) | bridges QEMU <-> AXI (GP0 slave side, HP0 master side, IRQ), adds DDR latency, publishes telemetry |
| `sim/tb_dma_xform.sv` | stand-alone self-checking RTL testbench (no QEMU) |

## Build and run (Windows)

```powershell
vivado -mode batch -source setup_bd.tcl                  # once: XSA for the BSP
xsct dma_demo\build_app.tcl                              # out\dma_test.elf, out\dma_cpu1.elf
powershell -ExecutionPolicy Bypass -File run_cosim_win.ps1 `
  -Elf out\dma_test.elf -Cpu1Elf out\dma_cpu1.elf `
  -Rtl dma_demo\rtl\dma_xform.v -TopSv dma_demo\sim\cosim_top.sv `
  -RunName dma_ui -IcountShift 1 -QuantumNs 1000 -Ui -SlowMs 25
```

`-Ui` starts `ui\server.py` and opens http://127.0.0.1:8765/. `-SlowMs` sleeps per
telemetry sample so a ~1 ms simulation can be watched live (omit it for full speed).

## What "time-locked" means here

* QEMU runs with `-icount shift=1`: virtual time is derived from instructions executed
  (2 ns per instruction ~ 500 MIPS, IPC = 1), not from the host clock.
* `-sync-quantum 1000`: every 1000 ns QEMU sends a sync; the PL answers only after XSim
  has simulated up to that time (`rp_sync_target` / `rp_sync_ack`).
* PS accesses carry QEMU's timestamp; the testbench waits for it and reports the
  PL-side latency back (`rp_done_t`).
* PL -> DDR requests are served in zero simulated time and a fixed `DDR_LAT` is added in
  the testbench, so host speed never changes simulated time.
* PL -> PS interrupts are acknowledged non-posted (`CAP_WIRE_POSTED_UPDATES`), so QEMU has
  applied the wire before PL time moves on.

Result: repeated runs produce **bit-identical event traces** (704 events, end of test at
1,026,836 ns, 4/4 runs identical).

## Honest limits

* CPU timing is an instruction-count model: no pipeline, caches, branch prediction, DDR
  controller or interconnect contention on the PS side. PL timing at its AXI ports is RTL-exact.
* QEMU has no caches, so missing `Xil_DCacheFlush/Invalidate` calls are not caught.
* Time coupling is quantum-granular (1 us here): PS/PL interactions inside one quantum are
  ordered, not cycle-interleaved.
