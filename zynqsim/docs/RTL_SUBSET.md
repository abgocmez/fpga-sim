# RTL subset per backend (prompt §9.1)

The tool prints **file, line and construct** for anything it cannot take: list-level problems
(unknown options, missing files, unresolved variables, `-sv_lib`, `-y`) come from `xsim-export`
and `project_validate`; language-level problems are the compiler's, quoted from `build.log` in
the error (`xvlog` / `xvhdl` / `xelab` messages carry file and line).

## `xsim` (Vivado XSim; the only RTL backend in this build)

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

## `verilator`, `icarus`

Not built (`backends.pl` rejects them with a message). Verilator is 2-state and has no UNISIM
library here; Icarus would need `unisims_ver` compiled for it. Their subsets will be listed when
the adapters exist.

## `native` (Rust PL models)

Not RTL: transaction-level models of the listed IP (`pl-native`).
