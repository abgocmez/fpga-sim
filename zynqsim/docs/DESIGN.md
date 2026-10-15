# ZynqSim design

Status: milestone **P1** reached locally (P0 done). This document describes what exists and,
clearly marked, what the next milestones will add. Sections 2-6 describe the P0 control plane;
section 9 describes the P1 engines. The requirements are in `../../docs/RUST_SOC_SIM_PROMPT.md`; the
feature-by-feature truth is in [STATUS.md](STATUS.md).

## 1. What ZynqSim is, and is not (yet)

ZynqSim is meant to simulate a Zynq-7000 PS+PL system on a virtual Digilent Zybo Z7-20 so
developers and AI agents can run, observe and debug designs without hardware.

At P0 the only engine is a small built-in **demo model**. It exists to exercise the control
plane, the board, the trace and replay machinery, the MCP server, the CLI and the GUI end to end.
It runs **no CPU and no RTL**, and every status reports fidelity `demo`. Cortex-A9 behaviour and
PL logic arrive with milestone P1 (QEMU + an RTL simulator behind adapters, see ADR-001/002).

## 2. Architecture

```
              +-----------+   +-----------+   +--------------+
              | zynqsim   |   | zynqsim-  |   | zynqsim-mcp  |   front ends: depend on
              | (CLI)     |   | gui       |   | (MCP, stdio) |   sim-api only
              +-----+-----+   +-----+-----+   +------+-------+
                    |               |                |
                    +---------------+----------------+
                                    |
                            +-------v--------+
                            |    sim-api     |  typed Command / Query / Event,
                            |  SimClient     |  recording + replay, project.toml,
                            |  Simulator     |  backend inventory
                            +--+----+-----+--+
                               |    |     |
                  +------------+  +-+---+ +-----------+
                  |               |     |             |
            +-----v----+   +------v--+  +--v--------+  (P1+)  +---------------------+
            | sim-core |   | trace   |  | board-zybo|  <-----  | PsBackend/PlBackend |
            | time,    |   | log,    |  | def, state|          | adapters: QEMU,     |
            | scheduler|   | hash,   |  | renderer, |          | Verilator/Icarus/   |
            | prng,    |   | VCD,    |  | hit-test  |          | XSim, native        |
            | clocks   |   | Perfetto|  +-----------+          +---------------------+
            +----------+   +---------+
```

**Dependency rule** (prompt §3): `gui`, `cli` and `mcp` depend on `sim-api` only. `sim-api`
re-exports the board and trace types they need (`sim_api::board`, `sim_api::trace`). Nothing
depends on `gui`. Engines will plug in through `PsBackend` / `PlBackend`.

### Crates in this milestone

| Crate | Responsibility |
|---|---|
| `sim-core` | picosecond `Ps`, deterministic `(time, priority, id)` scheduler, SplitMix64 PRNG, clock domains. No host clock, no ambient state. |
| `trace` | `TraceLog` with canonical FNV-1a-64 digest, VCD and Perfetto export. |
| `board-zybo` | board description (TOML), mutable `BoardState`, software renderer, hit-testing. |
| `sim-api` | control plane: commands, queries, event stream, `SimClient` thread, `Simulator` (demo model), `Recording`/replay, `Project`, backend inventory. |
| `mcp` (`zynqsim-mcp`) | MCP server: tools, resources, prompts, schema validation, path allow-list. |
| `cli` (`zynqsim`) | `run`, `test`, `init`, `inspect`, `trace`, `mcp`, `gui`. |
| `gui` (`zynqsim-gui`) | egui front end over a UI-free `Model`. |
| `xtask` | CI, golden regeneration, third-party licence listing. |

Crates for later milestones (`ps-backend`, `backend-qemu`, `pl-api`, `bus`, ...) are created
when their milestone starts, so there is no empty placeholder code.

## 3. Time and determinism

* One timeline in picoseconds (`Ps(u64)`, about 213 days of range).
* Events fire ordered by `(time, priority, creation id)`. The creation id makes ties
  independent of hashing or allocation order.
* The only randomness is the seeded `Prng`; its state is one `u64` and is part of snapshots.
* Nothing in the simulation reads the host clock. The server thread uses wall-clock time **only
  to pace continuous running**: each tick advances a fixed amount of simulated time, with no
  catch-up, so a slow host slows simulated time instead of changing what is simulated.
* **Inputs are the only external stimuli.** Every input (switch, button, UART write, reset) is
  applied at the current simulation time and recorded as `(t_ps, input)`. Replaying the
  recording against a fresh simulator must reproduce the trace digest; this is tested.
* The trace digest covers `(time, event)` for every record. Two runs agree iff their digests
  agree, on the same backend set.

### Lock-step with PS/PL backends (planned, P1)

External engines will be advanced in a bounded quantum (default 1 us): the PL runs to the PS
timestamp before the PS continues, interrupts are applied acknowledged, and PL-to-memory
accesses cost zero simulated time plus a fixed modelled latency. This is the adapter contract
from prompt §4/§19. At P0 the quantum exists and drives `Step`, but there is nothing to couple.

## 4. Control plane

`Simulator` is synchronous and single-threaded. `SimClient` runs one on its own thread and is a
cloneable handle:

* `command(Command)`: `Input(..)`, `Run`, `RunUntil`, `Step`, `Play`, `Pause`, `SetQuantum`,
  `SnapshotSave`, `SnapshotRestore`.
* `query(Query)`: `Status`, `BoardDef`, `BoardState`, `Pin`, `UartRead`, `Trace`, `TraceExport`,
  `Recording`, `Snapshots`, `Screenshot`, and the CPU/memory/PL queries that need a backend.
* `subscribe()`: an event stream of new trace records, play/pause and restore notifications.

Design points that matter to callers:

* **Capabilities, not silent no-ops.** `Status.capabilities` lists what the active backend set
  can do. A request that needs more returns `ApiError { code: unsupported, .. }` naming the
  missing capability and the backend, never an empty or invented result.
* **UART reads are offset-addressed** (`from`, `next`), so any number of readers (GUI, MCP, CLI)
  can read the same stream without stealing bytes from each other.
* **Errors are structured** (`invalid_argument`, `not_found`, `unsupported`, `io`, `cancelled`,
  ...) and carried unchanged through CLI exit codes, GUI status bar and MCP `isError` results.
* The GUI never blocks the simulation and the simulation never waits for the GUI.

## 5. Board model

`boards/zybo-z7-20.toml` holds components, pins and drawing positions. `BoardDef::validate`
reports every inconsistency at once. `BoardState` holds switches, buttons, LED duty (0..255),
RGB duty and Pmod pin levels.

The renderer is a deterministic software rasteriser (`tiny-skia`). The GUI texture, the MCP
`board_screenshot` PNG and the golden-image tests use the same function (ADR-004), so a golden
match means the GUI shows those pixels. It draws no text; the GUI overlays labels.

**Unverified data**: pin assignments were transcribed from memory of Digilent's master XDC and
are marked `VERIFY:` in the board file and `pin_status = "unverified"` in the API. Pmod pins are
deliberately absent. The layout is schematic, not to scale.

## 6. MCP server

JSON-RPC 2.0 over newline-delimited stdio (ADR-003). One tool per row of prompt §13 plus
`trace_get`. Properties worth knowing:

* Every tool has a JSON schema with parameter descriptions and examples, and arguments are
  validated before dispatch (unknown parameters are rejected).
* Tools that need a missing capability are *listed* but fail with `unsupported` and a message
  naming the backend and fidelity; tools planned for a later milestone say which one.
* File access (`project_open`, `waveform_export`, `session_replay`) is confined to `--root`
  directories after canonicalisation. No tool executes commands.
* Outputs are bounded (UART reads, trace pages, inline waveform text) and long runs are chunked,
  report `notifications/progress`, and stop on `notifications/cancelled`.
* Resources: status, board, project, trace, VCD, STATUS.md. Prompts: `debug_a_hang`,
  `explain_axi_violation`.

## 7. Roadmap hooks

| Milestone | What attaches where |
|---|---|
| P1 | `PsBackend` + `backend-qemu` (spawn, remote-port in Rust, ELF load, icount lock-step); one RTL adapter and `pl-api`; the demo model becomes a test double. `Capability::{CpuState, MemoryAccess, PlSignals}` start being advertised and the existing gated MCP tools get handlers. |
| P2 | board devices (SSM2603, HDMI, Pmod modules) behind `BoardDevice`; `pmod_attach`, `video_frame`, `audio_capture`. |
| P3 | `xsa-import`, `vendor-ip`, `boot`; `project_create_from_xsa`; custom boards. |

## 9. P1 engines: QEMU as PS, native or RTL PL

```
                       sim-api  (Engine trait, SimClient thread)
                          |
                    QemuEngine  --- trace (Bus events, UART bytes)
                          |
                   backend-qemu  ---- spawns Xilinx QEMU, UART1 file/tcp,
                          |           holds syncs to pause deterministically
              remote-port TCP (interop)
                          |
                      PlTarget (pl-api)  <----------------+
                   /               \                      |
            pl-native             backend-rtl-xsim ---- spawns xvlog/xelab/xsim
        (register models)                |                 |
                                remote-port TCP      xsim-dpi (no_std .a,
                                  (interop)           inside xsimk): rp_* DPI
                                                      <-> user testbench (SV)
```

* **Time.** QEMU runs with `-icount` and `-sync-quantum`; each sync is answered only up to the
  requested target, so the machine is paused exactly (deterministic stepping). Before answering a
  sync the adapter makes the PL catch up (`PlTarget::advance`): XSim runs to that time.
* **PS to PL.** A QEMU access to `M_AXI_GPn` becomes `PlTarget::read/write`. For XSim it is
  forwarded as a remote-port bus request to the DPI library, which presents it to the testbench.
* **PL to PS.** PL masters (DMA on `S_AXI_HP0`) and interrupts reach the `PsPort` passed into every
  callback, which forwards to QEMU. PL-master accesses and PS accesses are traced as `Bus` events.
* **Determinism.** UART1 goes through a file chardev so output is attributed to the right quantum;
  PL interrupts are acknowledged (non-posted) so QEMU has applied them before PL time moves on;
  PL-to-DDR requests are served in zero simulated time with a latency modelled by the testbench.
* **Why a proxy.** The orchestrator sits between QEMU and XSim (instead of XSim talking to QEMU
  directly as in the prototype) so one control plane can pause, trace and observe both.
* **Board pins.** Switches and buttons are sent to the testbench as remote-port wire updates on
  device 12 and read by the testbench with `tel_ctrl`; LED pins come back as `tel_event` JSON,
  forwarded on device 13 (`pl-api::{DEV_BOARD_IN, DEV_BOARD_OUT}`). The prototype testbench works
  unchanged.
* **Memory.** `QemuMachine::read_memory/write_memory` use the same remote-port path a PL DMA
  master uses, so they work while the machine is paused (the QEMU monitor does not).
* **Fidelity.** F0 PS (functional, instruction count). PL native models are transaction level; XSim
  is RTL-exact at the PL's ports; coupling is quantum-granular.

## 10. Board media, 3D view and project loading

* **Media.** The testbench carries pin-level models of the SSM2603 codec and the HDMI connectors
  (`fixtures/rtl/media/zs_media.sv`). The `xsim-dpi` library turns their DPI calls into
  remote-port bus packets on devices 14-18 (ADR-006); `backend-rtl-xsim` serves them from a
  `VideoSource`/`AudioSource` and collects `MediaEvent`s; `sim-api::QemuEngine` assembles
  frames, keeps 10 s of DAC audio, records codec register writes and frame ends in the trace, and
  answers `Query::{VideoFrame, AudioCapture, AudioFrom}` and `Command::SetMediaSource`.
* **Time pacing.** A request stamped after the run target is left unanswered (the vCPU stays
  blocked, like a held sync). Without this, a firmware that polls a register issues requests
  forever and one `run` call never returned. Firmware should still pace its polls
  (`rt::delay_us`): every PS read of the PL costs ~100 us of wall time and advances simulated
  time by only the instructions executed.
* **Build cache.** Compiled XSim snapshots are keyed by the content of the sources, defines, top
  module, DPI library and Vivado root, and copied into each run's directory. Replacing the PS
  firmware therefore does not recompile the RTL.
* **Project loading in the GUI.** `ProjectDraft` is an editable view of the fields of a
  `project.toml` the GUI changes; everything else is carried over. An engine is built on a worker
  thread (`LoadJob`) and swapped in when ready, so the window stays responsive while XSim
  compiles. *Load PS firmware* and *Load PL design* change one half of the draft and rebuild.
* **3D view.** `board-zybo::scene3d` extrudes the 2D layout into boxes, projects them through an
  orbit camera and rasterises them with the same software renderer, keeping the drawn quads for
  picking. It is an illustration for orientation and interaction, not a mechanical model.

## 11. Snapshots, reset and replay of QEMU sessions

QEMU cannot be snapshotted through its monitor here (the monitor is unreachable while the machine
is paused, ADR-001), and the PL (XSim) has no snapshot at all. `QemuEngine` therefore keeps a
**journal**: every state-changing request in order, with run requests normalised to absolute
`RunUntil` targets, because the trace is identical only for identical run slicing. A snapshot is a
position in the journal plus the trace digest, time and record count at that point. Restore and
reset build a new engine from the same spec (the XSim snapshot comes from the build cache) and
re-apply the journal; a restore is accepted only if it reproduces the stored digest, so
non-determinism is reported instead of hidden. `Recording` carries the journal and the project
path so `zynqsim test`, `zynqsim trace` and MCP `session_replay` can rebuild the engine.

## 12. Debugging a QEMU guest

Why registers are not available on a paused machine, and what is: see ADR-007. In short, the
lock-step holds QEMU's global lock while the vCPU waits for our sync, and every QEMU
introspection channel needs that lock. A stop *caused by the guest* releases it. So debugging is
built on guest-caused stops: with `[debug] enabled = true` QEMU is started with `-gdb tcp:...
-S`, `backend-qemu::gdb` speaks the remote serial protocol, breakpoints are planted before the
first instruction, `QemuMachine::run_until` returns `RunEnd::Stopped` when a stop reply shows up
(polled while the link is idle), and `resume` steps off the breakpoint before continuing, as gdb
does. `QemuEngine` keeps the stop (`Status.debug`), makes `cpu_regs` available only then, and the
server's `Play` ends at a stop (`Engine::halted`).

## 8. Known limitations at P0

See [STATUS.md](STATUS.md). In short: no CPU, no RTL, no AXI; pins unverified; layout
schematic; GUI is a fixed layout without docking; CI for Linux/macOS is configured but unrun.
