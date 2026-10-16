# RTL subset per backend (prompt §9.1)

The tool prints **file, line and construct** for anything it cannot take: list-level problems
(unknown options, missing files, unresolved variables, `-sv_lib`, `-y`) come from `xsim-export`
and `project_validate`; language-level problems are the compiler's, quoted from `build.log` in
the error (`xvlog` / `xvhdl` / `xelab` messages carry file and line).

## `xsim` (Vivado XSim)

XSim is the vendor simulator, so the language support is Vivado's, not a subset ZynqSim defines.
What ZynqSim adds or restricts:

| Input | Support | Evidence |
|---|---|---|
| Verilog-2001, SystemVerilog (what `xvlog -sv` takes), behavioural testbenches (`initial`, delays, `$display`, `$finish`, `$readmemh`) | yes | `backend-rtl-xsim::math`, `dma_golden`, `board`, `media` |
| VHDL (`xvhdl`), mixed with Verilog | yes for design files named in an export list (`vhdl LIB "file"`) or `rtl.files` with `.vhd`; **not tried with Vivado-generated VHDL IP** | `backend-rtl-xsim::export::a_vhdl_design_file_is_compiled_with_xvhdl_and_answers_like_the_verilog` |
| UNISIM primitives and `write_verilog -mode funcsim` netlists | yes, `-L unisims_ver` and `glbl` are added | `backend-rtl-xsim::export::the_rtl_export_and_the_funcsim_netlist_give_the_same_answers`, `zynqsim::export_sim::rtl_export_and_funcsim_netlist_pass_...` |
| `export_simulation` folders, `.prj`, `.f`, `elab.opt`, `xsim.ini`, `.sh`/`.bat` | read; see ADR-017 | `xsim-export::tests` |
| `` `include `` directories, `` `define `` values (`-i`, `-d`, `+incdir+`, `+define+`) | yes (`NAME=VALUE` quoted for `cmd`) | `backend-rtl-xsim::export::prj_includes_and_defines_reach_xvlog_and_the_build_is_cached` |
| Timing simulation, SDF (`simprims_ver`) | no | not implemented |
| Encrypted IP / `secureip` models, Vivado IP simulation libraries | `-L` is passed through; **never run** | not tested |
| Design-side DPI/VPI of the export (`-sv_lib`) | no (reported); the testbench's `rp_*` DPI is ZynqSim's own | `xsim-export::tests::dpi_libraries_...` |
| The testbench top | user-supplied, or generated for an AXI4-Lite slave (`init --ip`, `init --export-sim`) | `sim-api::tbgen`, `zynqsim::init_ip_*` |

## `verilator` (Verilator 5, `--binary --timing`; ADR-027)

Verilator translates the sources to C++ and compiles them with the host's C++ compiler; the
testbench's `rp_*` DPI imports are satisfied by the same `xsim-dpi` archive XSim uses. Windows only
for now, and **2-state**: there is no X or Z, an uninitialised variable starts at 0.

| Input | Support | Evidence |
|---|---|---|
| Verilog-2001 and SystemVerilog as Verilator 5.053 reads them (`.v`, `.sv`), `` `include ``, `-D` defines (`NAME=VALUE`) | yes | `backend-rtl-verilator::tests::math` (`my_math_ip` + `cosim_top`), `cli::verilator` |
| Behavioural testbench: `initial`, `#` delays, `$display`, `$finish`, `$readmemh` | yes (`--timing` is passed; `--binary` enables it too) | `cosim_top` uses `#5` clocks and `@(posedge)`; `$readmemh` not run here |
| DPI-C imports with `string`, `longint`, `output int` arguments | yes | `cosim_top.sv` (`rp_open`, `rp_next`, `rp_done`) |
| Classes with `rand` and `randomize()`, `covergroup`, concurrent `assert property`, `bind`, `fork`/`join`, `program` blocks, `real` | accepted by the front end; the build succeeded and only warnings were printed (`IMPLICITSTATIC`, `WIDTHTRUNC`). Not run | probes, `--binary` only (Verilator 5.053) |
| `specify` blocks | ignored with `%Warning-SPECIFYIGN`; the timing they describe is **not** simulated | probe |
| `interface` declared inside a module, then instantiated | **refused**: `%Error-UNSUPPORTED: Unsupported: interface decls within module decls`, reported with file, line and the instance text | probe |
| Syntax errors | refused: `%Error: file:line:col: syntax error ...` with the source line | probe |
| VHDL | **no**: refused by the project check (`backends.pl = "xsim"` for VHDL) | `sim-api::project` tests |
| `export_dir` / `filelist` (Vivado simulation exports, UNISIM, `write_verilog` netlists) | **no**: refused by the project check | `sim-api::project` tests |
| `[rtl] waves = true` (VCD/FST) | **no**: refused by the project check; use XSim's `.wdb` | `sim-api::project` tests |
| Unisims, Vivado IP, encrypted IP | no UNISIM library is given to Verilator here | not tested |
| Design-side DPI/VPI of the design (`-sv_lib`) | no, as for XSim | not tested |

The messages are `%Error` / `%Warning` lines of Verilator, parsed by `backend-rtl-verilator::diag`;
the adapter prints the first errors with file, line and construct when the build fails.

## `icarus`

Not built, and not planned for now. Icarus Verilog has no DPI-C: the testbench's `rp_*` imports
would need a VPI module written in Rust (a second `cdylib` with the VPI entry points), which is
not done (ADR-027). `backends.pl = "icarus"` is refused with a message.

## `native` (Rust PL models)

Not RTL: transaction-level models of the listed IP (`pl-native`).
