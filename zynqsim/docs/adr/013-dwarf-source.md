# ADR-013: Source lines from the ELF's DWARF line table

Status: **Accepted**. Date: 2026-10-16.

## Context

The debugger (ADR-007) stops the guest at breakpoints and knows ELF symbols, so a stop reads
`pc in collatz_steps+0x1c`. A developer wants `main.rs:19` and the lines around it, and wants to say
`main.rs:19` when setting a breakpoint. The information is in the ELF's DWARF, if the firmware is
built with it.

## Decision

* `sim-api::source::SourceMap` reads `.debug_line` (all units, DWARF 4 and 5) and the subprogram
  DIEs of `.debug_info` (function names and ranges) with the pure-Rust `gimli` and `object` crates
  (Apache-2.0/MIT, no C). It is built once when the engine starts, from core 0's ELF; an ELF
  without debug info gives an empty map, not an error. Rows are sorted by address, the end of a
  sequence is a row with line 0, and sequences that start at address 0 (code the linker discarded)
  are dropped.
* `file.rs:LINE` resolves to the first address of each contiguous block of that line's `is_stmt`
  rows (a loop or a generic gives several; a breakpoint is set at each, at most 16). A line with
  no code resolves to the next line of that file that has some, as gdb does. The file part matches
  by trailing path components, ASCII case-insensitive; an ambiguous name is refused with the
  candidates listed.
* Queries, not commands, carry the data: `Query::SourceLine { addr }`, `SourceFile { path }`,
  `SourceAddrs { file, line }`. `cpu_regs` gains `pc_source`, `BreakpointInfo` gains `source`, and
  the stop reason names `file:line`. Breakpoint *setting* stays `Command::SetBreakpoint { addr }`;
  front ends resolve `file:line` with `SourceAddrs`, the engine does it itself for
  `[debug] breakpoints`.
* **Finding the text.** The ELF names files relative to the compiler's working directory
  (`DW_AT_comp_dir`), which is a build-machine path. `SourceFile` tries, in order: an absolute
  name; the absolute comp dir; each search directory (the project directory, then
  `[debug] source_dirs`), also with leading path components dropped down to two. Only files the
  line table names can be read, so the query is not a file server; text is capped at 2 MiB.
* **Reproducible fixtures.** `cargo xtask firmware` passes `--remap-path-prefix=<firmware dir>=.`
  so the committed `rust_debug.elf` carries comp dir `.` instead of the builder's path; the
  example project sets `source_dirs = ["../../firmware"]`. (Setting `CARGO_ENCODED_RUSTFLAGS`
  replaces the `.cargo/config.toml` rustflags, so xtask repeats them; the other firmware ELFs came
  out byte-identical.)
* **Firmware.** `firmware/debug_demo` is built with `debug = 1` through a per-package release
  profile override: line tables and function names, no variables or types, about 9 KiB over a
  plain demo. `line-tables-only` was tried first and leaves out the function DIEs, so functions
  would only have their mangled (v0) ELF symbol names. The other demos stay without debug info.
* **GUI.** A "Source" panel (`gui::source_view`) beside the CPU registers shows the file with the
  current line highlighted and `+`/`-` for the context. State is a `SourceTracker` in the model.
* **MCP.** `source_line` (addr, symbol or `location`), `cpu_regs` adds `pc_source` and
  `source_context`, `breakpoint_set` accepts `location`.
* **CLI.** `zynqsim run --break LOC` adds a breakpoint (turning the debugger on) and the result
  names where the run stopped.

## Consequences

* Only lines and function names. No variables, types, inlined-call chains (the innermost line is
  reported, as `addr2line` without `-i`), step over/into, or lines for core 1's ELF.
* Optimised code jumps around: a stop may map to a different line than the one a breakpoint was
  asked for if the compiler moved it; the reported line is what the table says for the pc.
* DWARF in other crates (`core`, `/rustc/<hash>/library/...`) is included when the firmware's
  profile asks for it; those files are not on the host, so the panel says the text is unavailable
  and still shows `file:line`.
* Debug-info sections are not loaded (`.debug_*` are not allocated), so simulation behaviour and
  addresses are unchanged. Timing under the debugger remains non-reproducible (ADR-007).
