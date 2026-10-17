# ADR-033: The native PS engine (`backends.ps = "native"`)

Status: **Accepted** (milestone P5, optional, PS only: core 0, no PL). Date: 2026-10-17.

## Context

ADR-030 built the Cortex-A9 interpreter (`cpu-a9`), ADR-031 the address map and peripheral
models (`mem-sys`, `ps-periph`). Until now the PS always ran in the Xilinx QEMU. The native
engine puts the three together behind the `Engine` trait so that a PS-only project runs with
no QEMU. It does not attach a PL: the remote-port link to a PL design is out of scope here.

## Decision

### The engine

* `crates/sim-api/src/native_ps.rs`: `NativePsEngine` implements `Engine`. `project.rs` builds it
  for `backends.ps = "native"`. The front ends (CLI, MCP, GUI) use it through `sim-api` only.
* It builds `ps_periph::zynq_system()`, loads the PT_LOAD segments of `[cpu0] elf` into DDR or
  OCM (through `object`, as the symbol and line readers do), resets core 0 to the entry point
  (Supervisor mode, IRQ and FIQ masked) and runs it.
* **Core 1 is parked.** It is never stepped, which is what the boot ROM leaves it doing (the
  firmware runtime parks it too). `[cpu1]` is a validation error: two native cores are not
  implemented.
* **No PL.** `backends.pl` must be `demo`. `[native]` models, `[devices]`, `[media]`, network and
  `boot_bin` are validation errors with the reason. Board switches, buttons and LEDs are not
  driven.

### Timing (F0, not cycle-accurate)

* One instruction is one CPU cycle, and the core runs at 666.67 MHz, so a cycle is **1500 ps**.
  `run_for(d)` advances to the last whole cycle at or before the requested time; `now_ps` is
  the cycle count times 1500, so it can lag a request by less than 1.5 ns.
* The peripherals tick every `BATCH_CYCLES` = 128 cycles (192 ns). The core samples its
  interrupt line before the next instruction after any access to a device (an access outside
  DDR and OCM, which can change device state; RAM accesses cannot). The latency of a device
  interrupt is therefore at most 192 ns, well under the 1 us the spec asks of timers.
* No caches, bus contention, DDR timing or pipeline. The status reports `fidelity = F0`.

### UART bring-up

Both UARTs are set to 115200 8N1 (BAUDGEN 124, BDIV 6 from the 100 MHz reference) with TX and
RX enabled at power-up. This is what the FSBL does on hardware. The Xilinx QEMU model transmits
from reset, and `firmware/rt` prints without configuring UART1, so the native PS has to start
the UART for the same firmware to print. The reset baud rate (9600, UG585 reset state with TX
disabled) would make each character cost about a millisecond of virtual time. UART TX is paced
at the baud rate by `ps-periph`, as on hardware; QEMU does not pace.

### Faults and breakpoints

* An instruction the core does not implement (VFP/NEON, MMU, the rest listed in ADR-030)
  stops the engine with a message naming the pc, the symbol and source line when known, the
  word, and the cycle. The instruction did not run; no state changed. The fault is sticky: later
  runs report it again. Nothing is computed wrongly.
* Execution breakpoints (`Command::SetBreakpoint`, `[debug] breakpoints`, `main`, an address or
  `file.rs:LINE`) are checked before each instruction. A stop is reported as the QEMU engine
  reports it (`halted()`, the debug status, a trace marker). A resume does not stop again on the
  instruction it stopped at. **Watchpoints are not implemented** and are refused.
* `cpu_regs` is answered for core 0 at any time (the interpreter is in-process, so no debugger is
  needed). Core 1 is refused.

### Snapshots and replay

Snapshot and restore are **re-execution from the journal**, as on the QEMU engine: a snapshot
records the journal length, the cycle, the requested time, the trace digest and a state hash, and
a restore builds a fresh engine, replays the journal prefix and must reproduce all of them. The
state hash covers the bus (`SystemBus::state_hash`: RAM, cycle count, unmapped count and every
device's state), core 0's registers, CPSR, the banked SPSRs and the requested time.

The task asked for a direct copy of core and bus state. That is not done, for a reason: the
device models (`ps-periph`) have state that is hashed but has no restore, so a RAM-and-register
copy would leave timers and the GIC in the wrong state. Re-execution with a verified digest is
exact, and `Query::Recording` plus `zynqsim replay` work for native sessions too.

### Changes to other crates

To be `Engine: Send` (the server thread owns the engine), the PS crates no longer use `Rc` and
`RefCell`:

* `mem-sys`: `Shared<D>` wraps `Arc<Mutex<D>>` (the handles `shared()` returns are that
  `Arc`); the bus stores `Box<dyn Device + Send>`.
* `ps-periph`: `PsHandles::uart0/uart1` are `Arc<Mutex<Uart>>`; `zynq_system` attaches
  `Send` devices.
* `cpu-a9`: the translation hook is `Box<dyn Translate + Send>`.

Behaviour is unchanged; the lock is uncontended. `mem-sys::SparseRam::read` does one page lookup
per access instead of one per byte (the same result; instruction fetches are the hot path). The
workspace `Cargo.toml` builds these three crates at opt-level 3 in the dev profile (see Consequences).

### Not implemented (stub, reported as such)

VFP and NEON, the MMU and TLB walk, two cores, a PL (remote-port attachment), board pins,
LEDs and switches, Pmod devices, HDMI and audio, Ethernet, screenshots, watchpoints,
`set_quantum`, BOOT.BIN, OCM low remap, caches and cycle-accurate timing. `docs/STATUS.md` lists
each with its status.

## Two cores (`[cpu1]`)

* **Cores.** `[cpu1] elf` adds a second `cpu-a9` core (MPIDR affinity 1). Without it core 1 is not
  instantiated. Both cores share one `SystemBus`, so memory is coherent by construction; the SCU
  snoop filter is not modelled.
* **Boot ROM parking.** Core 1 resets to `0xFFFFFE00`, a four-instruction-plus-literal loop written
  into the top of the on-chip RAM (`VERIFY:` the real ROM mapping): `wfe`, load the word at
  `0xFFFFFFF0`, return to `wfe` while it is zero, otherwise `bx` to it. The `[cpu1]` ELF is loaded
  into memory but not started: core 0 starts it by writing the entry address to `0xFFFFFFF0` and
  sending an event. The engine does not write the mailbox itself (no FSBL is modelled).
* **Scheduler.** Device time advances in windows of `BATCH_CYCLES` (128) cycles, as before. Within a
  window the core with the fewest elapsed cycles runs next, for at most `SCHED_BATCH` (8) cycles,
  ties to core 0: round robin, deterministic for the same inputs. A waiting core spends its cycles
  waiting, so time moves on for both. The batch size is written in the trace start marker. With one
  core the schedule and the trace are unchanged (the digest of `native-hello` at 5 ms was checked
  before and after).
* **Cross-core effects, at the step that causes them.** A store clears the other cores' exclusive
  reservations on the 8-byte granule it hits (`EXCL_GRANULE`, `VERIFY:` the Cortex-A9 granule). SEV
  sets the event register of every core. A store to the mailbox is traced. Interrupt lines are
  re-sampled for every core after a device access or a tick.
* **Breakpoints and registers.** Breakpoints are global addresses; a stop names the core that hit
  one (`core N stopped`). `cpu_regs` answers for core 0 and core 1 when it exists.
* **Snapshots and state.** Re-execution as before; the state hash covers both cores' registers,
  event and exclusive state, their cycle counts and the tick count.
* **Not modelled.** Cache maintenance beyond the no-op CP15 operations, SCU snoop and filtering,
  the SLCR CPU reset bits (core 1 is never reset by software here), an FSBL, and UART0 on the QEMU
  engine (its UART0 is `-serial null`, so core 1's console is native-only).

## Consequences

* **Differential results** (`crates/cli/tests/native_ps_diff.rs`, pinned in
  `docs/conformance/native-ps.md`): `rust-hello` prints identical text. `conf-scutimer` matches
  QEMU except three global-timer checks that QEMU reports N/A and the native model passes.
  `conf-ttc` matches except check 12 (QEMU N/A, native PASS) and check 9, which fails natively
  for a timing reason explained in that document.
* Speed (release build, measured on `rust-hello` after the rest of its run, a busy loop):
  about 2 s of wall time per 100 ms of virtual time. The dev profile (test runs) builds
  `cpu-a9`, `mem-sys` and `ps-periph` at opt-level 3 (workspace `Cargo.toml`), so the
  conformance runs take seconds; without that the same runs took minutes.
* Determinism: the engine reads no clock and no host input, so five processes give one digest
  (`native_ps_diff::five_processes_give_one_trace_digest`).

## Alternatives considered

* **A worker thread with channels** instead of making the PS `Send`: rejected. `Engine::trace()`
  and `board()` return references, so a handle would need mirrored state.
* **Checking the interrupt line before every instruction**: rejected for cost. The line changes
  only through a device access or a tick, so sampling after those gives the same behaviour.
* **Memory-copy snapshots**: rejected, see above.
* **Simulating core 1 parked in WFI**: rejected for now. Parking it means not stepping it, and a
  GIC interrupt targeted at it stays pending, which is what a parked core does.
