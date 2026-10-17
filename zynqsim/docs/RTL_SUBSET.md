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

## `icarus` (Icarus Verilog 14, `iverilog -g2012` and `vvp`; ADR-037)

Icarus has no DPI-C. The testbench's `rp_*` and `zs_*` functions are the VPI system functions
`$zs_<name>` of `zynqsim.vpi` (crate `vpi-icarus`, `cargo xtask vpi`), compiled with `-DZS_ICARUS`.
Windows only for now. 4-state, event-driven.

| Input | Support | Evidence |
|---|---|---|
| Verilog-2001 and SystemVerilog as Icarus 14 reads them with `-g2012`, `` `include ``, `-D` defines | yes | `cli::icarus` (`cosim_top` + `my_math_ip`) |
| Behavioural testbench: `initial`, `#` delays, `$display`, `$finish` | yes | `cosim_top` |
| DPI imports (`import "DPI-C"`) | **not accepted**; the testbench's `ZS_ICARUS` branch replaces them with wrappers of `$zs_` system calls, and `rp_next`'s call sites call `$zs_rp_next` (`sim-api::tbgen`, `fixtures/rtl/math/cosim_top.sv`) | `tbgen::tests::the_icarus_form_wraps_every_dpi_import_and_the_dpi_form_is_kept` |
| A `$time` passed directly to a `$zs_` system call | reads **0** in vvp; the testbench passes time through a `longint` variable | ADR-037 |
| `output` arguments of a function | **refused** by Icarus ("Function arguments must be input ports"); such calls use the system function | probe |
| Syntax errors and unsupported constructs | refused with `file:line: error: text` (`backend-rtl-icarus::diag`) | `diag` unit tests |
| Board, media, Pmod pins, protocol checkers, DMA | the wrappers exist in the generator; **not run** under Icarus | not tested |
| VHDL, `[rtl] waves`, `export_dir`, `filelist` | refused by the project check (as for Verilator) | `sim-api::project` |
| Design-side VPI/DPI (`-sv_lib`) | no | not tested |

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

## Native engine (`crates/rtl-engine`, ADR-034)

The native engine simulates the subset below with no external tool. It is **2-state**: every value is
0 or 1, and x and z in source read as 0. It is not selectable as a backend (`backends.pl = "rtl-native"`
is refused). Anything not listed under *Supported* is refused at elaboration with a diagnostic that
names the construct (or the signal, for width) and its position.

### Supported

| Area | Exact list |
|---|---|
| Sources | everything the front end accepts (*Native front end* above); `` `timescale `` sets the unit of `#` delays (default 1 ns; units finer than 1 ps round to 1 ps) |
| Hierarchy | module instances with named or positional connections, `.*`, parameter overrides (named and positional, numbers and strings); port directions `input` and `output`; gate primitives `and`, `or`, `nand`, `nor`, `xor`, `xnor`, `not`, `buf` (one output) |
| Parameters | `parameter` and `localparam` in the header and the body; constant expressions with the front end's constant evaluator (`$clog2`, `$bits`, constant functions, string comparisons) |
| Generate | `for` (with `genvar`), `if`/`else`, `case`, blocks; names `label[i].name` and `label.name`; at most 10 000 iterations per loop |
| Declarations | `wire`, `reg`, `logic`, `bit`, `integer`, `int`, `time`, `signed`, packed ranges up to 128 bits, unpacked one-dimensional memories up to 2^20 words, declaration initialisers (`reg` at time 0, `wire` as a continuous assign), `typedef` of vectors and enums (enum members are 32-bit constants 0, 1, ...; explicit enum values are read as the position) |
| Continuous assignment | `assign` (with `#` delay), with concatenation targets, part and bit selects, memory words |
| Processes | `always` (with any event control, `@*`, or none), `always_ff`, `always_comb`, `always_latch` (run at time 0 and wait on their inputs), `initial` |
| Statements | blocking and non-blocking assignments (with `#` intra-assignment delay), `if`/`else`, `case`/`casez`/`casex` (wildcard `?`, `z` and `x` digits in labels), `for`, `while`, `repeat`, `forever`, `break`, `continue`, `begin`/`end` with names and local declarations, `#` delays, `@(...)` event controls (`posedge`, `negedge`, plain; a bit of a vector or a constant bit index), `wait (cond)`, `return` |
| Routines | `function` (inputs; the function name is the return variable; locals including memories), `task` (inputs, outputs, `inout` copied both ways; delays allowed; inlined at each call). Not recursive: a routine that calls itself is refused |
| Expressions | literals (sized, based, unsized, fill `'0 '1`, strings as bytes); every unary, binary, reduction, logical, ternary, concatenation, replication (a zero replication is allowed inside a wider concatenation), bit, part and indexed (`+:`, `-:`) select, on signals, memory words and ascending ranges; `$signed`, `$unsigned`, `$time`, `$clog2`, `$bits`; size casts `N'(x)` |
| Arithmetic | width and signedness as Verilog defines them (context-determined sizing, operands extended by the expression's signedness); `>>>` is arithmetic only for a signed left operand; `/` and `%` by zero give 0 (2-state) |
| System tasks | `$display`, `$write`, `$strobe`, `$monitor` (one display at its call), `$error`, `$warning`, `$info`, `$fatal` (display then stop), `$finish`, `$stop`, `$readmemh`, `$readmemb` (relative to the design's base directory, or the current directory); formats `%d %D %h %H %x %X %b %B %o %O %c %s %t %m %%` with width and zero-fill flags |
| Time | a single time base per module (from its file's last `timescale`); `$time` in that unit; `#0` runs after the current events |
| Results | `Simulator::{new, set, set_word, get, get_word, run_until, tick_clock, snapshot, restore, state_hash, signal_names, width_of, log, finished, set_recording, changes, vcd}`; `RtlModule` (AXI4-Lite slave, see below) |

### Limits (each is refused or stated; none is silent)

| Limit | Behaviour |
|---|---|
| 2-state values; x and z read as 0 | a testbench that compares with `===` against `x`, or depends on x propagation, is decided differently from XSim |
| Width over 128 bits | refused, naming the signal (declarations) or the concatenation |
| Recursive functions and tasks | refused |
| Hierarchical references (`a.b` in expressions or as a task call, `u_mem.write(...)`) | refused (uriscv's testbench is refused for this) |
| Package types, `type` parameters, interfaces, classes, assertions, covergroups, `fork`/`join`, `disable`, DPI imports, real numbers | refused (the front end names them; `pulp-common-cells` is refused for its package-typed parameters) |
| `$random`, `$urandom`, `$fopen` and other system tasks not listed above | refused at elaboration |
| Multi-dimensional memories; instance arrays | refused |
| `inout` ports | refused |
| Enum members with explicit values | read as their position; the enum base type is ignored (variables are 32-bit) |
| Partial out-of-range writes (a part select partly outside the vector) | the whole write is skipped; Verilog writes the in-range bits |
| Out-of-range reads | 0 (a 2-state read of a non-existent word) |
| Time | `timescale` is taken per file (its last directive); there is no `precision` modelling, and sub-picosecond units round to 1 ps |
| Delays in `assign` (and `#` between a process's waits) | the process sleeps for the delay and then re-evaluates its right-hand side, so the output is the input's value at that later time; input changes during the delay are not seen, and no pulse filtering is modelled beyond that |
| `$readmemb` | implemented-untested; `$readmemh` is tested |
| Waveforms | `Simulator::vcd()` writes the recorded changes of every visible scalar and vector (2-state, all starting at 0); the engine does not write FST |
| Budgets | 20 million operations per process activation (a loop without timing control stops with a message); 2 million process activations or 200 000 non-blocking rounds in one time step are reported as a combinational loop |
| Speed | not measured for this document; the engine interprets bytecode |

### AXI4-Lite through `RtlModule` (ADR-034)

A design with an `S_AXI_*` slave (the prototype's `my_math_ip` is the example) runs behind
`pl_native::RegisterBlock`. Each access is one AXI4-Lite transaction of as many clock cycles as the design
takes: VALID and data are driven, READY and VALID are sampled just before each rising edge, and VALID is
dropped after the edge that accepted it. The clock is driven by the adapter, one 10 ns period per cycle.
Only 32-bit data is accepted. Offsets past the slave's address width alias as the RTL decodes them.
