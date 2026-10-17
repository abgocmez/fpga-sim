# ADR-031: Native PS memory system and peripheral models (`mem-sys`, `ps-periph`)

Status: **Accepted** (milestone P5, optional; part of the native PS engine). Date: 2026-10-17.

## Context

The native PS engine (spec §6 phase 3, §8, §15 P5) needs the physical address space the cores
see, and register-level models of the PS blocks that the firmware touches first. The CPU core
(`cpu-a9`) is built in parallel against the `Bus` trait of `crates/ps-bus`, which must not change.
The QEMU backend keeps providing the PS for every other test, so these models are checked
against the conformance firmware (`firmware/conf_*`) and against UG585 as remembered, not against
QEMU's own behaviour.

## Decision

### Crates

* **`mem-sys`** (depends on `ps-bus`): the address map as data (`map`), the device trait and
  register-access helpers (`device`), sparse RAM for DDR (`ram`), a named stub and a read-only
  image (`simple`), and the system bus (`bus`). `#![forbid(unsafe_code)]`.
* **`ps-periph`** (depends on `ps-bus`, `mem-sys`): UART (`uart`), SLCR (`slcr`), the MPCore
  block with the GIC (`gic`, `mpcore`), the timers (`timers`), TTC (`ttc`), the clock rates
  (`clock`), and `zynq_system`, which builds the bus with the devices at the map's addresses
  (`system`). `#![forbid(unsafe_code)]`.

### The bus

* `SystemBus` implements `ps_bus::Bus` as core 0; `SystemBus::core(n)` gives a `CoreBus` that
  implements `Bus` for core `n`. Banked registers (the CPU interface, the private timer and the
  watchdog) answer each core with its own copy: the bus calls `Device::select_cpu` before every
  access.
* DDR is `SparseRam` (64 KiB pages allocated on the first write, default 512 MiB, at most 1 GiB).
  The 256 KiB on-chip RAM is at `0xFFFC_0000`. Only the reset (high) mapping is modelled.
* Any access that is not in DDR, OCM or a device is **unmapped**: the policy (`Fault` by default,
  or `RazWi`) applies, and the access is logged. The first 1024 are kept; all are counted.
  An access that straddles the end of a region is unmapped. An unsupported size or a misaligned
  register access is `BadAccess`. Devices see sizes 1, 2 and 4; an 8-byte access is split into
  two 4-byte accesses by the bus.
* Interrupts: after every tick and every device write, the bus gathers the level of every line
  each device drives (`Device::irq_lines`, both levels) and hands the set to every device
  (`Device::accept_irqs`). The GIC consumes it. `SystemBus::core_irq(cpu)` is the IRQ output of
  the core (`Device::core_irq`), which the core model polls.
* Time: `SystemBus::tick(cycles)` takes CPU cycles at 666.67 MHz. The PERIPHCLK (private timer,
  watchdog, global timer) is CPU/2, the TTC input is CPU/6, the UART reference clock is a
  parameter (100 MHz assumed). Each device keeps its own remainder, so no cycle is lost.
* Snapshot: `ram_snapshot()` and `ram_restore()` cover DDR and OCM as bytes. `state_hash()` is
  FNV-1a over the RAM snapshot, the cycle count, the unmapped count and every device's
  `save_state`. Device state is not restored by a snapshot: a restore re-creates the bus.

### Device list (attached by `zynq_system`)

| Device | Region | Interrupt | Model |
|---|---|---|---|
| UART0, UART1 | `0xE000_0000`, `0xE000_1000` | SPI 59, 82 | `Uart`: xuartps registers, FIFOs at 0x30, TX paced at the baud rate into a host log (`take_tx`), RX injected (`inject_rx`) |
| SLCR | `0xF800_0000` | none | `Slcr`: lock/unlock keys, reset table `REGS`, protected writes, PSS soft reset recorded |
| TTC0, TTC1 | `0xF800_1000`, `0xF800_2000` | SPI 42-44, 69-71 | `Ttc`: three counters each, interval, overflow, decrement, match, prescaler |
| DDRC, DEVCFG | `0xF800_6000`, `0xF800_7000` | none | `Stub` (reads zero) |
| MPCORE | `0xF8F0_0000`, 0x3000 bytes | PPI 27, 29, 30 per core | `MpCore`: SCU (control, config), `Gic` CPU interface at 0x100 and distributor at 0x1000, `GlobalTimer` at 0x200, `Countdown` private timer at 0x600 and `Watchdog` at 0x620 (both per core), L2C stub at 0x2000 |
| QSPI_LINEAR | `0xFC00_0000` | none | `ReadOnlyImage` when an image is given; erased bytes read 0xFF, writes ignored |
| DDR, OCM | `0x0`, `0xFFFC_0000` | none | bus-owned RAM |

The GIC: 96 IDs (SGI 0-15, PPI 16-31, SPI 32-95), two cores, priorities and targets, level and
edge inputs, pending and active state, SGIs with the source core in the acknowledge, the priority
mask and the running priority, acknowledge (`ICCIAR`, spurious 1023) and end (`ICCEOIR`).

### Address map (from `mem_sys::ZYNQ_MAP` and `MPCORE_BLOCKS`; `VERIFY:` against UG585 Table 2-1)

| Name | Base | End (incl.) | Size | Kind | Model |
|---|---|---|---|---|---|
| DDR | 0x0000_0000 | 0x3FFF_FFFF | window 1 GiB (512 MiB default) | RAM | `SparseRam` |
| M_AXI_GP0 | 0x4000_0000 | 0x7FFF_FFFF | 1 GiB | PL port | none (PL) |
| M_AXI_GP1 | 0x8000_0000 | 0xBFFF_FFFF | 1 GiB | PL port | none (PL) |
| IOP | 0xE000_0000 | 0xE0FF_FFFF | 16 MiB | peripheral | UART0, UART1; the rest unmapped |
| SMC_MEM | 0xE100_0000 | 0xE1FF_FFFF | 16 MiB | not modelled | none (`VERIFY:`) |
| SLCR | 0xF800_0000 | 0xF800_0FFF | 4 KiB | peripheral | `Slcr` |
| TTC0 | 0xF800_1000 | 0xF800_1FFF | 4 KiB | peripheral | `Ttc` |
| TTC1 | 0xF800_2000 | 0xF800_2FFF | 4 KiB | peripheral | `Ttc` |
| DMAC | 0xF800_3000 | 0xF800_3FFF | 4 KiB | not modelled | none |
| SWDT | 0xF800_5000 | 0xF800_5FFF | 4 KiB | not modelled | none (`VERIFY:`) |
| DDRC | 0xF800_6000 | 0xF800_6FFF | 4 KiB | stub | reads zero |
| DEVCFG | 0xF800_7000 | 0xF800_7FFF | 4 KiB | stub | reads zero (XADC at 0xF800_7100 not modelled) |
| OCM | 0xFFFC_0000 | 0xFFFF_FFFF | 256 KiB | RAM | `SystemBus` (high mapping) |
| MPCORE | 0xF8F0_0000 | 0xF8F0_2FFF | 12 KiB | peripheral | `MpCore`, blocks below |
| QSPI_LINEAR | 0xFC00_0000 | 0xFDFF_FFFF | 32 MiB | ROM image | `ReadOnlyImage` |

MPCore blocks: SCU 0xF8F0_0000 (0x100), GIC CPU interface 0xF8F0_0100 (0x100), global timer
0xF8F0_0200 (0x100), private timer 0xF8F0_0600 (0x20), watchdog 0xF8F0_0620 (0x20), GIC
distributor 0xF8F0_1000 (0x1000), L2C-310 0xF8F0_2000 (0x1000, stub).

The IOP blocks not listed above (USB0/1, I2C0/1, SPI0/1, CAN0/1, GPIO, GEM0/1, QSPI controller,
SMC controller, SDIO0/1) are in `mem_sys::IOP_BLOCKS` and are unmapped here.

### Interface for the CPU core (`cpu-a9`)

* Use `SystemBus::core(cpu)` for the core's loads and stores (`CoreBus` implements `Bus`).
* Call `SystemBus::tick(cycles)` with the CPU cycles the core ran, then poll
  `SystemBus::core_irq(cpu)`. Interrupt entry is the core's business: acknowledge through
  `ICCIAR` (`0xF8F0_010C`) and end through `ICCEOIR` (`0xF8F0_0110`).
* `SystemBus` and the UART handles are single-threaded (`Rc<RefCell<_>>` for the handles, boxed
  devices): the bus is not `Send`. A multi-threaded core would need a different device wrapper.

## Deviations from UG585 and from QEMU (recorded, not hidden)

* **QEMU differences that this model does not reproduce.** The global timer keeps its equality
  behaviour (the flag does not re-assert after write-1-to-clear; the counter holds when disabled;
  COUNTER writes take effect). The TTC match flags are write-1-to-clear; QEMU clears on read.
  The conformance checks that QEMU fails are PASS here (`docs/conformance/gpio-timer.md`,
  `docs/conformance/ttc-dma.md`).
* **TTC interval period.** The counter wraps when it reaches INTERVAL, so the period is INTERVAL
  ticks and the counter rests in 0..INTERVAL-1. The firmware check 8 (`v < 1000`) needs this.
  `VERIFY:` against UG585.
* **Private timer LOAD write** does not change COUNTER. Writing LOAD while running is not
  specified here.
* **Watchdog** does not reset the machine. A watchdog-mode event sets RESET_STATUS.
* **UART** TX is paced at the baud rate; a driver that writes faster sees TXFULL and TX overflow,
  as on hardware. RX timeout, modem, break and parity are not modelled. The UART reference clock
  (100 MHz) and the reset values of BAUDGEN, RXWM and TXWM are `VERIFY:`.
* **SLCR** reset values are from memory and marked `VERIFY:`; the MIO pin registers are stored
  and not decoded.
* **OCM** low remap (SLCR `OCM_CFG`) is not modelled.
* **GIC** security extensions, the binary-point priority split and interrupt groups are not
  modelled. The identification registers and ICDICTR are `VERIFY:`.
* **Not modelled, unmapped and logged**: the DMAC, SWDT, SMC, USB, I2C, SPI, CAN, GPIO, GEM,
  QSPI controller, SDIO and XADC. A native access to them is an unmapped access.

## Consequences

* The register models and the bus are verified only against the firmware checks encoded as tests
  and against the model's own specification. Nothing here has been compared with a board, and
  the QEMU-differential runs need the native core, which is the other engineer's work.
* Snapshots cover RAM only; a determinism test re-runs a scripted session and compares hashes.
* Adding a block means one `Device` implementation, one entry in `mem_sys::ZYNQ_MAP` or a block
  table, and an attachment in `ps_periph::zynq_system`.

## Appendix (batch 2): GPIO, SPI, I2C, XADC, devcfg, CAN, watchdog reset

Added the models listed in `STATUS.md` (*Native PS peripherals, batch 2*) as new files of
`ps-periph`: `gpio.rs`, `spi.rs`, `i2c.rs`, `devcfg.rs` (devcfg and XADC share one window), and
`can.rs`, plus a watchdog reset request in `timers.rs` and `mpcore.rs`. Each is attached in
`system.rs::zynq_system`; `PsHandles` now carries the shared handles the host uses (GPIO pins,
SPI/I2C peers, CAN frames, XADC temperature, MPCore reset requests). The address map entries for
GPIO, SPI0/1, I2C0/1 and CAN0/1 change from "not modelled" to "peripheral".

Decisions:

* **GPIO pin range.** The request listed four banks covering pins 0..117. The register layout
  (`conf_gpiops`, `xgpiops`) has four banks of at most 32 bits, widths 16/16/22/32, so pins 0..85
  are modelled and 86..117 are refused. `VERIFY:` against UG585.
* **SPI and I2C host hooks** are traits (`SpiPeer`, `I2cPeer`) rather than a dependency on
  `crates/devices`, so `ps-periph` stays free of that crate. Adapting the TMP2/ADT7420 model is
  left to the integrator.
* **Watchdog reset** is a queued event (`take_wdt_resets`). The model does not reset anything
  itself: the engine must turn a nonzero result into a system reset.
* **BSP sequences.** The devcfg SelfTest and the CAN self-test register order were disassembled
  from `fixtures/bsp/bsp_devcfg_selftest.elf` and `bsp_canps_polled.elf` with the Vitis
  `arm-none-eabi-objdump`. The CAN mode codes (1, 2, 4, 8, 16) are the ones the driver decodes.
* **XADC data format.** UG480 puts the 12-bit sensor code in bits 15:4. The firmware's temperature
  check applies its formula to the unshifted value and fails; `conf_xadcps` test documents this.

Not done: the GIC IDs of CAN (19, 46) and devcfg (40) are `VERIFY:`; the BSP GPIO/SPI/I2C driver
self tests were not disassembled; the native engine cannot run any BSP ELF (ADR-033).
