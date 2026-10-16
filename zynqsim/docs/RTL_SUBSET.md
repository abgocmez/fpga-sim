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

## Native front end (`crates/rtl-frontend`, milestone P6)

The native front end reads source without any external tool. It preprocesses, parses and elaborates
the design. It does not simulate; the native engine is a later phase (see `docs/STATUS.md`, *Native RTL*).
Every construct it cannot take is a diagnostic `file:line:col: construct: message`. The `construct`
column names the item in the tables below. Parsing continues after the construct where feasible.

Command: `zynqsim rtl-check FILE... [--top M] [-I DIR] [-D NAME=VAL] [--json]`. Exit 0 when there
are no diagnostics, 1 when there are some, 2 when a file cannot be read.

### Supported

| Area | Accepted |
|---|---|
| Lexing | identifiers and escaped identifiers; sized, based (`8'hFF`, `4'b10x1`, `4'sd5`, `32'h_DE_AD`), unsized (`42`) and fill (`'0`, `'1`, `'x`, `'z`) literals; strings; every Verilog and SystemVerilog operator (`**`, `<<<`, `>>>`, `===`, `!==`, `+:`, `-:`, `++`, `--` in statements); `//` and `/* */` comments; `\`-continued lines |
| Preprocessor | `` `define `` (object-like, function-like, default arguments, `` `"arg`" `` stringify), `` `undef ``, `` `ifdef `` / `` `ifndef `` / `` `elsif `` / `` `else `` / `` `endif `` (nested), `` `include `` (relative to the including file, then `-I`; cycles rejected), `` `timescale `` and `` `default_nettype `` (recorded), `` `__FILE__ `` and `` `__LINE__ ``; ignored: `resetall`, `celldefine`, `begin_keywords` and similar |
| Modules | ANSI and non-ANSI headers; `parameter` and `localparam` (header and body); `#(...)` overrides in instances; `input`/`output`/`inout` with `wire`, `reg`, `logic`, `bit`, `signed`, packed ranges |
| Declarations | `wire`, `tri*`, `wand`, `wor`, `supply0/1` (drive strengths skipped), `reg`, `logic`, `bit`, `integer`, `int`, `byte`, `shortint`, `longint`, `time`, `genvar`; unpacked dimensions (memories); declaration initialisers; `typedef` of vectors and of enums (members become constants) |
| Behaviour | `assign` (with `#` delay, flagged); `always`, `always_ff`, `always_comb`, `always_latch`, `always @(...)`, `always @*`; `initial` (kept, flagged not synthesizable); blocking and non-blocking assignments; `+=`-style compound assignments; `if`/`else`; `case`/`casez`/`casex` with `default`; `for` (also with a declaration in the initialiser); `while`; `repeat`; `forever`; `begin`/`end` with names and local declarations; `#` delays and `@`/`wait` controls (flagged); labels; `return`, `break`, `continue`; `disable` (flagged) |
| Routines | `task` and `function`, `automatic`/`static`, ANSI and non-ANSI arguments, local declarations, return types |
| Instances | named and positional connections, `.name` implicit connections, `.*`, `#(...)` overrides, gate primitives (`and`, `or`, `nand`, `nor`, `xor`, `xnor`, `not`, `buf`) |
| Generate | `generate ... endgenerate` regions, `for` (with `genvar`), `if`/`else if`/`else`, `case`; labels become `label[i]` and `label.` path segments |
| Expressions | full Verilog precedence (see the test list in `docs/STATUS.md`); concatenation, replication, bit and part selects, `+:` and `-:`; ternary; reductions; `$clog2`, `$signed`, `$unsigned`, `$bits`; `signed'(x)` and `unsigned'(x)`; size casts `N'(x)`; hierarchical names `a.b.c` (parsed, not resolved) |
| Other | attributes `(* ... *)` skipped; `$error`, `$warning`, `$info`, `$fatal` at module level accepted (elaboration-time, no effect) |

Elaboration (`--top M`): parameters with overrides (named and positional) are evaluated in
order; constant expressions use 128-bit integers with the Verilog width and sign rules, `$clog2`
and constant functions (loops, returns, a 10^6-step limit); generate constructs are unrolled
(10 000 iterations per loop at most); the instance tree and the widths of ports and nets are
reported (`--json`). Checks: undefined module, unknown named port, too many positional
connections, undeclared identifier (a bare identifier in a port connection is an implicit net and
is accepted), undefined function or task, duplicate module, instantiation depth over 64.

### Unsupported (each one is a diagnostic; the construct column is shown in backticks)

| Construct (diagnostic) | Behaviour |
|---|---|
| `package`, `import pkg::...` at file level | skipped to `endpackage` / `;` |
| `import pkg::item` or `import pkg::*` in a module | accepted; names are not resolved, and a wildcard import disables the undeclared-name check |
| `DPI import` | `import "DPI-C" ...` is reported; the function is not declared |
| `interface`, `class`, `program`, `config` | skipped to the matching `end...` |
| `covergroup`, `bind`, `property`, `sequence`, `assertion` (`assert`, `assume`, `cover`, `expect` in code and at module level) | skipped |
| `typedef struct`, `typedef union` | rejected; the name is still declared as a type |
| `parameter type`, `parameter` of a package type | reported; the parameter is not declared as a type |
| `package reference` (`pkg::name` in an expression) | reported |
| `real literal` (`1.5`, `2e3`), `real`, `realtime` declarations | reported |
| `system task` (`$itor`, `$pow`, `$rtoi`, and any unknown `$name`) | reported |
| `fork/join`, `do ... while`, `final`, `specify`, `specparam`, `defparam`, `procedural assign` (`assign`, `force`, `release` in code) | reported |
| `assignment pattern` (`'{...}`) | reported; the value is 0 |
| `packed array` (`logic [A][B] x`) | reported |
| `event` declarations | reported |
| `lexical` (for example `$` as in `[$]` queues) | reported |
| `port expression`, `interface port` (`.name(expr)` port lists, interface and modport ports) | reported |
| `indexed range` in a declaration (`[a +: b]`) | reported |
| `instance array` (`mod u [N] (...)`) | parsed; not elaborated (reported at `--top`) |
| `macro`, `include`, `conditional` | undefined macro, missing or recursive include, unmatched or unclosed conditional |
| `syntax`, `budget` | the parse error, and the step budget being exhausted (default 20 million steps) |

### Corpus status

`fixtures/`, `examples/` and `tests/` contain 148 Verilog and SystemVerilog files (including the
third-party designs in `tests/rtl/`: picorv32, serv, the forencich and secworks cores, the
dpretet FIFO, uriscv). **109 parse clean** and **39 still give diagnostics**. The 39 are pinned
per file in `crates/rtl-frontend/tests/corpus.rs`: the cosim tops and peripheral models that use
DPI-C imports for the `rp_*` protocol (DPI is outside the subset by design), the negative and probe cases (which are meant to fail), the pulp-common-cells
files (parameter types from packages, assertions, multi-dimensional packed arrays, `$` queues)
and the real-valued media models. The third-party cores in `tests/rtl/` that are not negative
or probe cases parse clean, including picorv32, serv, the forencich AXI-stream and UART, the
secworks crypto cores and the dpretet FIFO.

Parsing clean is not elaboration or simulation: it means the front end found no construct it cannot take.
