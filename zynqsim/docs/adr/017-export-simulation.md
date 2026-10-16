# ADR-017: Vivado simulation exports and post-synthesis netlists

Status: **Accepted** (milestone P3). Date: 2026-10-16.

## Context

Prompt §9.1 items 3 and 4 ask for post-synthesis functional netlists (`write_verilog -mode
funcsim`, UNISIM primitives) and Vivado simulation exports (`export_simulation` file lists,
`.prj`, `.f`, `glbl.v`) to run through the RTL backend. Until now `[rtl].files` was a literal
list of Verilog/SystemVerilog files compiled with `xvlog -sv` into `work`, no libraries, no VHDL,
no `-L`, no `glbl`, and no globs. ADR-015 parsed `.prj`/`.f` lists only far enough to collect
file names.

## What Vivado writes (observed, 2022.1)

`fixtures/export/make_export.tcl` runs Vivado in batch on `my_math_ip` and its testbench:

| Flow | Files | Notes |
|---|---|---|
| `export_simulation -simulator xsim` | `vlog.prj`, `elab.opt`, `xsim.ini`, `<top>.sh`, `cmd.tcl`, `glbl.v`, `file_info.txt`, `README.txt` | `verilog LIB "path"` / `sv LIB "path"` lines with `\` continuations, `nosort`; paths relative to the export folder (`../../rtl/x.v`); `elab.opt` is one `xelab` line with `-L`, `--snapshot`, `LIB.top LIB.glbl`; `xvlog_opts="--incr --relax -L uvm"` lives in the `.sh` |
| `launch_simulation -mode post-synthesis -type functional -scripts_only` | `<top>_vlog.prj`, `compile.bat`, `elaborate.bat`, `simulate.bat`, `xsim.ini`, `<top>_func_synth.v` | the netlist is `write_verilog -mode funcsim -nolib`: UNISIM `LUT2`/`FDRE`/... instances, `module glbl` included; elaboration `-L unisims_ver ... xil_defaultlib.<tb> xil_defaultlib.glbl` |

Two practical findings: this machine's Vivado install is missing `tps/tcl/tcl8.5/clock.tcl`
(`clock format` fails inside `export_simulation`; the script answers it itself), and the exported
lists name sources relative to wherever the Vivado project was, so the script copies the sources
under the output folder and rewrites those paths to `rtl/NAME` (the only edit to Vivado's output).

## Decision

* **A new dependency-free crate `xsim-export`** reads a folder, a `.prj`, a `.f`, an `elab.opt`,
  an `xsim.ini`, a script (`.sh`/`.bat`: `xvlog`/`xvhdl`/`xelab` command lines with the script's
  own `name=value` variables expanded) or a single `.v/.sv/.vhd` file into an `ExportDesign`:
  ordered `SourceFile`s (path, language, library, own include dirs and defines, `file:line` of the
  list), global includes/defines, `-L` libraries, library mappings, elaboration tops, generics,
  timescale. `$VAR`, `${VAR}`, `%VAR%` and `$::env(VAR)` are expanded (`XILINX_VIVADO` falls back to
  the detected install). It runs no tool.
* **Everything not understood is a `Diagnostic` with file, line and construct**, never skipped:
  unknown options and directives (warning), `-y`/`-sourcelibdir` search directories and `-sv_lib`
  DPI libraries of the export (warning: ZynqSim links its own), missing sources, unresolved
  variables (error). A folder holding several flows (an export root with `behav/xsim` and
  `postsynth`) is refused with the candidates listed: mixing them would compile a module twice.
* **`[rtl]` gets two additive keys**, `export_dir` (a folder, or a list file) and `filelist`
  (alias for a `.prj`/`.f`). Their sources compile first, `files` after (the testbench), into the
  export's default library. `top` defaults to the export's single non-`glbl` top. `files` accepts
  glob patterns (`*`, `?`, `[a-z]`, `**`), expanded relative to the project file, sorted per
  pattern; a pattern that matches nothing is an error.
* **The XSim adapter compiles prj-style**, as Vivado's scripts do: it writes `zs_vlog.prj` /
  `zs_vhdl.prj` with one line per file (`verilog|sv|vhdl LIB -i "dir" -d "N=V" "path"`), runs
  `xvlog -prj` and `xvhdl -prj`, then `xelab -L <libs> -L <link libs> LIB.top LIB.glbl` plus the
  DPI archive. `glbl` is elaborated as a second top whenever the design wants it (`unisims_ver` in
  `-L`, `glbl` among the tops, `glbl.v` named but absent) and is taken from the netlist or the
  sources when they define it, else from `<Vivado>/data/verilog/src/glbl.v`. Libraries with a
  directory in the export's `xsim.ini` that are not Vivado's own are passed as `-L name=dir`. The
  existing non-export build line is unchanged; the snapshot cache key additionally covers the
  project files, the elaboration arguments and every input and header file.
* **`zynqsim init --export-sim PATH --out DIR`** (MCP `project_create_from_export`) references the
  export from `project.toml` (it is not copied) and either uses the export's own testbench (any
  source that calls `rp_open`/`rp_next`) or generates one, with the `init --ip` generator, for the
  first AXI4-Lite slave found (a named `--module`, a top of the export, or in compile order). It
  also writes `export-report.md`.

## Consequences

* A Vivado RTL export and its post-synthesis functional netlist run the same firmware with the
  same bus accesses and the same trace digest (test names in STATUS.md).
* VHDL design files run (mixed with the Verilog testbench) through `xvhdl`; this was a hand-written
  design, not Vivado output.
* Not done: timing/SDF simulation (`simprims_ver`, `write_verilog -mode timesim`), compiled IP
  libraries from `-lib_map_path`/`compile_simlib` (the mapping code exists and is unit tested but
  no real IP library was run), encrypted IP (`secureip` is only named in `-L`), `-sv_lib` DPI of
  the export, `-y`/`-v` library search, per-file `-generic`, Verilator or Icarus (they cannot read
  UNISIM primitives without a library the project does not have). The export's own simulator
  scripts are never executed.
* Include directories and defines on a `.prj` line apply to that line's files; `.f` options and
  script-level `-i`/`-d` apply to the whole design.
