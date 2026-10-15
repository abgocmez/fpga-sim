# ADR-007: Debugging through guest-caused stops only

Status: **Accepted**. Date: 2026-10-15.

## Context

`cpu_regs`, breakpoints and watchpoints need QEMU to tell us about the CPU. Under the remote-port
lock-step the machine is paused by *not answering* a sync, and then the vCPU thread sits inside
that wait holding QEMU's global lock. The monitor, QMP and the gdb stub all run in the main loop
and need that lock, so none of them answers while the machine is paused. This was measured twice
(monitor in P1, `-gdb` here: the TCP connection is accepted, `qSupported` is never answered).

Asking while the machine runs is possible but samples an arbitrary instruction, which is neither
deterministic nor useful for a debugger.

## Decision

Use only stops that the *guest* causes. QEMU is started with `-gdb tcp:127.0.0.1:<port> -S` when
the project says `[debug] enabled = true`; a small RSP client in `backend-qemu` plants
breakpoints and watchpoints before the first instruction and reads registers when the guest has
stopped on one, because a debug exception exits the vCPU loop and releases the lock.

## Consequences

* Registers exist at breakpoints/watchpoints, not at "t = 3.2 ms". An agent that wants them sets
  a breakpoint, runs, reads. This is how a debugger works, and what MCP `cpu_regs` documents.
* The guest's state at a stop is reproducible (identical registers in separate sessions). The
  *timing* is not: attaching and resuming perturbs virtual time by up to microseconds, so the
  trace digest of a debug session is not stable and snapshots/replays may be refused.
  Therefore the debugger is opt-in per project, never default.
* Breakpoints added while the machine is merely paused are queued in the stub and applied at a
  wall-clock-dependent instant within the next quantum.
* Registers at an arbitrary time would need QEMU's remote-port device to release the lock while
  it waits (a change to third-party C code) or the native PS backend (P5); both are out of scope.
