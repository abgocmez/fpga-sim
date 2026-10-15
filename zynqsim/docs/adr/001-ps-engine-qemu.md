# ADR-001: Use Xilinx QEMU as the PS engine behind an adapter

Status: **Accepted** (implementation: milestone P1). Date: 2026-10-15.

## Context

The processing system needs a Cortex-A9 model, an MMU, a GIC and the Zynq PS peripherals that
unmodified Xilinx standalone BSP software expects. Writing that natively in Rust is estimated
well over the 4 engineer-week threshold of prompt §2.3 and needs closed documentation to get
cycle behaviour right.

## Options

1. **Xilinx QEMU** (Zynq-7000 machine, co-sim device tree) driven as a child process.
2. Upstream QEMU `xilinx-zynq-a9` machine.
3. A native Rust ARMv7-A ISS plus peripherals (the optional P5 backend).

## Decision

Use option 1 behind a `PsBackend` trait, spawned and controlled by a Rust adapter
(`backend-qemu`) over remote-port, the monitor and TCP chardevs. Keep option 3 as an optional
drop-in replacement behind the same trait.

Why Xilinx's build: the prototype in this repository already ran a bare-metal Cortex-A9 program
against PL logic using Xilinx's QEMU and its `zynq-arm-cosim.dtb` (remote-port devices for the
GP/HP/ACP ports and interrupt wires). Upstream QEMU has no remote-port PL interface.

## Consequences

* Fidelity is **F0 plus instruction-count timing** only. Caches, DDR timing and non-coherent
  DMA strictness (F1/F2) need the native backend. The UI fidelity badge must say so.
* PS snapshots are limited to what QEMU offers.
* Determinism needs `-icount shift=N,align=off,sleep=off`, time-locked `sync`, acknowledged
  interrupt updates and zero-time blocking PL-to-DDR responses (prompt appendix §19). These are
  part of the adapter's contract and get tests.
* QEMU is never vendored; it is located at run time (PATH, `XILINX_VITIS`, install roots). On
  this machine it was found under `C:\Xilinx\Vitis\2022.1\data\emulation\qemu_win\qemu`.
* Windows QEMU has no unix sockets; the adapter uses TCP.
* QEMU's licence (GPL) is not a distribution concern because it is a separate, user-installed
  program, not linked or bundled.

## Outcome (recorded after implementation)

* **Pausing is done by not answering a sync.** With `-icount` the sync timer runs in the vCPU
  thread while holding QEMU's global lock, so a machine held at a sync is fully stopped, **and
  its monitor cannot answer** (measured: the monitor's banner never arrives while a sync is
  outstanding). The adapter therefore does not use the monitor at all. Memory is read and
  written through the remote-port PL-to-PS path (`S_AXI_HP0`), which does work while held.
  CPU registers are not observable deterministically; `cpu_regs` reports that honestly.
* QEMU creates and waits on `-chardev` sockets before `-serial` ones, so the adapter connects to
  all of its sockets in whatever order they become ready.
* `-serial file:` makes UART1 output deterministic (written by the vCPU thread, so complete at
  every sync boundary); the TCP mode is the interactive alternative.

## Exit plan

The `PsBackend` trait is engine-neutral. The native backend (P5) is validated against QEMU with
differential tests and replaces it by changing `[backends] ps` in `project.toml`.
