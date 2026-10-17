# ADR-034: Native RTL engine for the subset (`rtl-engine`), before any backend switch

Status: **Accepted** (milestone P6, engine API and tests; not wired to `backends.pl`). Date: 2026-10-17.

## Context

ADR-032 built the front end (`rtl-frontend`): it parses the Verilog-2001 and SystemVerilog subset
and elaborates parameters, generates and widths, but it does not simulate. Its elaborator keeps no
statement bodies, so it cannot serve as the engine's model. The P6 goal (spec §9.2, §15) is a
simulator that needs no external tool for the subset it accepts. Two results from P1 bound the
work: a backend must be honest about what it does not model (§1), and an RTL result is only
trusted once it agrees with the vendor simulator on something real.

## Decision

1. **A new crate, `crates/rtl-engine`** (`#![forbid(unsafe_code)]`, depends on `rtl-frontend` and
   `pl-native`). It reuses the parser, the preprocessor and the constant evaluator
   (`rtl_frontend::consteval::Ctx`). It has its own flattening elaboration, because the front end's
   `Design` has no bodies. The engine's `Design` is a flat netlist of signals, processes and
   functions (`ir.rs`).
2. **Values.** Two-state. Every signal is a `u128` masked to its width; declarations wider than 128
   bits are refused with a diagnostic naming the signal. x and z in a literal read as 0. This is a
   deliberate limit, not a 4-state model (`docs/RTL_SUBSET.md`).
3. **Sizing.** Each expression is lowered at a context width and signedness (`lower.rs`). Leaves are
   extended with the context signedness; comparisons share one operand width; shifts take the left
   operand's width; reductions and logical operators are one bit. The rules are tested against an
   independent reference (`tests/property.rs`) rather than against the code that implements them.
4. **Scheduling** (`sim.rs`), in the Verilog order: the active region runs every ready process; the
   non-blocking queue is applied in order when the active region is empty; the cycle repeats until
   both are empty. Timed events (`#`) advance time in the module's `timescale` unit. A time step with
   more than 2 million process activations, or more than 200 000 non-blocking rounds, is reported as a
   combinational loop. A process that runs 20 million operations without suspending is reported as
   having no timing control.
   - `assign` and `always_comb` start at time 0 with their sensitivity already registered, so a change
     made by their own first evaluation is seen (`assign a = ~a;` oscillates and is reported). A
     plain `always @*` waits for its first change, as the standard says.
   - All processes run to their first wait when the simulator is created, so an input set afterwards
     is seen by the processes that wait on it.
5. **Bytecode.** Each process is a flat list of operations (`Assign`, `Jump`, `JumpIf`, `Delay`,
   `Wait`, `Sys`, `Halt`) that the scheduler can suspend and resume. Loops, `case` and tasks are
   compiled to jumps; tasks are inlined (they are non-recursive); functions are separate bodies run to
   completion, which may not suspend.
6. **Snapshots** are a versioned binary encoding of the complete state (values, process positions,
   waits, queues, timed events, log), checked against the design's sizes. The state hash is FNV-1a of
   that encoding. Restore clears the change record.
7. **`RtlModule`** implements `pl_native::RegisterBlock`. A register access is one AXI4-Lite
   transaction: the adapter drives VALID and the data, samples READY and VALID just before each rising
   edge, and drops VALID after the edge. It drives the clock itself (100 MHz period in engine time).
   The port prefix, clock and reset are detected from the names `S_AXI_*`.

## Verification

* `scheduling` (11): NBA swap, blocking swap, race-free counter, delta chain, edges, delays,
  loop and budget guards, `$finish`, `always_comb`.
* `expr` (18) and `constructs` (18): hand-computed values for each construct and the diagnostics for
  each refused one.
* `property` (3): seeded random operands for add, subtract, multiply, shifts, comparisons, divide and
  modulo, at every width from 1 to 64 for both signednesses, and a few widths up to 128.
* `math_ip` (7): `fixtures/rtl/math/my_math_ip.v` through `RtlModule`, compared with `pl_native::MathIp`,
  the golden model the firmware tests use. One known difference is pinned: the RTL decodes four address
  bits, so offset 0x10 aliases to 0x0 where the golden model answers 0.
* `xsim_diff` (1): the same register stimulus on `my_math_ip` in Vivado XSim, through the existing
  `XsimPl` adapter, and in the engine; every read agrees (Windows, XSim 2023.1).
* `corpus` (1): six of nine third-party designs in `tests/rtl` pass their self-checking testbenches
  (`CORPUS_PASS`); three are refused (the reasons are in `docs/CORPUS_NATIVE.md`). The table is pinned
  by the test and written to `docs/CORPUS_NATIVE.md`.

## Consequences

* The engine runs real, unmodified third-party RTL (picorv32, serv, the Forencich LFSR, UART and AXI
  stream FIFO, the dpretet async FIFO) to a verdict, without a tool. That is evidence about the
  subset, not about the RTL's behaviour on hardware.
* A differential check against XSim exists for one design. The corpus results are not compared with
  XSim; the engine's 2-state model would differ from XSim on any testbench that depends on x or z.
* `backends.pl = "rtl-native"` is **not** wired. Wiring needs a `PlTarget` implementation with
  lock-step advance, board pins, media and IRQ forwarding, as `backend-rtl-verilator::VerilatorPl`
  provides. That is a separate step with its own design review; `RtlModule` is a register-level
  adapter and does not replace it.
* Limits are listed in `docs/RTL_SUBSET.md` (native engine section). The largest one is 2-state.

## Alternatives considered

* **Icarus Verilog as a second backend** (ADR-027): it has no DPI-C, so the RTL-level PS link would
  need a VPI module written in Rust. Not chosen for the P6 goal, which requires no external tool.
* **Reusing `sv-parser` or a Verilog simulator crate.** Dependencies that bring C code are excluded by
  the project rules, and none supports the scheduling and snapshot guarantees the spec asks for.
* **Elaborating inside `rtl-frontend`.** Would make the front end a simulator and blur what the
  "verified" rows in `docs/STATUS.md` mean. The engine has its own flattening instead.
