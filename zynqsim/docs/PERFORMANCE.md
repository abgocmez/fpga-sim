# Performance: `zynqsim bench` (prompt §14.9)

What was measured, how, and on what. These are **single runs** on one machine, not a benchmark
suite: no repetition statistics, no warm-up protocol, no target. Use them to see the order of
magnitude and to compare runs of the same command on the same machine, not as a claim about any
other engine.

## Machine and versions

| Item | Value |
|---|---|
| Date | 2026-10-16 (about 16:20 to 16:30 local, +03:00) |
| CPU | Intel Core i7-13700H (`Win32_Processor.Name`), 14 cores / 20 threads, WMI `MaxClockSpeed` 2400 MHz (WMI's nominal value, not the turbo clock) |
| RAM | 31.7 GB |
| OS | Windows 11 Pro 10.0.26300 |
| Load | Not isolated. One WMI `LoadPercentage` sample read 20 % at the time of the runs. Other agent sessions on the same machine were not checked. |
| zynqsim | 0.1.0, `cargo build --release`, rustc 1.99.0 (GNU toolchain, MinGW gcc 13.1 from Qt) |
| QEMU | Xilinx QEMU 6.1.0 (`qemu-system-aarch64.exe` from Vitis 2022.1) |
| PL RTL | Vivado 2022.1 XSim |

## Command

```
zynqsim bench --project examples/rust-hello/project.toml --for 100ms
zynqsim bench --project examples/math-qemu/project.toml --for 100ms
zynqsim bench --project examples/dma-regression/project.toml --for 3ms
zynqsim bench --project examples/dma-regression/project.toml --for 10ms
zynqsim bench --for 10s                       # built-in demo engine
```

The timed part is the run itself: `startup` (spawning QEMU, building or loading the RTL, reading
the project) is reported separately and is not in the ratios. Simulated time is the engine's
clock after the run. The PL cycle count is the simulated time times the **nominal 100 MHz**
FCLK0 (10 ns per cycle); the clock is not read back from the design. The guest rate is
`simulated time / 2^icount_shift` (QEMU `-icount shift=1` makes one instruction 2 ns of virtual
time). That is approximate: it counts time the guest spends idle in WFI as instructions, so it is
an over-estimate, and for the two-core DMA regression it counts one virtual clock, so it is a
per-core figure.

## Results

| Project (backends) | `--for` | startup | wall | simulated | sim / wall | PL cycles (100 MHz nominal) | PL cycles per wall s | guest instr. per wall s (approx.) |
|---|---|---|---|---|---|---|---|---|
| `demo` (built-in, demo PS and PL) | 10 s | < 1 ms | 0.000024 s | 10 s | 413,223 | n/a | n/a | n/a |
| `rust-hello` (QEMU, no PL) | 100 ms | 0.523 s | 4.365 s | 100.001 ms | 0.0229 | n/a | n/a | about 11.5 M/s |
| `math-qemu` (QEMU, native PL models) | 100 ms | 0.512 s | 4.478 s | 100.001 ms | 0.0223 | n/a | n/a | about 11.2 M/s |
| `dma-regression` (QEMU, RTL in XSim, 2 cores) | 3 ms | 3.278 s | 0.491 s | 3.000836 ms | 0.0061 | 300,084 | 611,587 | about 3.1 M/s |
| `dma-regression` (QEMU, RTL in XSim, 2 cores) | 10 ms | 3.225 s | 0.754 s | 10.000836 ms | 0.0133 | 1,000,084 | 1,326,854 | about 6.6 M/s |

Notes on the rows:

* `rust-hello` was also run twice more with the same arguments, giving wall times of 4.355 s and
  4.419 s, so one configuration varied by about 1.5 % across three runs. That is a rough
  repeatability figure, not a statistic.
* `math-qemu` uses the native PL models, not XSim; `project.toml` says `pl = "native"`. The XSim
  PL figures come from `dma-regression`, the QEMU + XSim project in the examples.
* The demo row is not a CPU speed. The demo engine is an event schedule with no CPU or RTL, so its
  wall time is at the timer's resolution and the ratio says only that it is very fast.
* The QEMU rows are slow relative to real time: the lock-step quantum (1 us) and the QEMU
  instruction-count mode cost a factor of about 44 (`rust-hello`, `math-qemu`) to about 165
  (`dma-regression` at 3 ms, with XSim) against simulated time. Where the time goes was not
  profiled.

## What this does not show

* No comparison with another simulator, board or hardware. No target is claimed.
* No repeated runs with a spread: one run per row, apart from the repeat of `rust-hello` above.
* The machine was not isolated, and the PL clock is the nominal 100 MHz, not read from the design.
* The determinism test (`crates/cli/tests/determinism.rs`) does not check any of these numbers.
