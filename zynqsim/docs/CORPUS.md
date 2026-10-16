# Universality corpus

<!-- BEGIN:designs -->
| Design | Licence | Commit | Lines (rtl + tb) | Compiled | Elaborated | Ran | Self-check | Warnings | Result / notes |
|---|---|---|---|---|---|---|---|---|---|
| [secworks-sha256](secworks-sha256/SOURCE.md) | BSD-2-Clause | `837c5cc396f0` | 1194 + 718 | yes | yes | yes | pass | 1 | SHA-224/256 core, upstream testbench with NIST vectors |
| [secworks-sha1](secworks-sha1/SOURCE.md) | BSD-2-Clause | `50f82f4ca2f8` | 942 + 585 | yes | yes | yes | pass | 0 | SHA-1 core, upstream testbench |
| [secworks-md5](secworks-md5/SOURCE.md) | BSD-2-Clause | `54a6308d4d43` | 784 + 608 | yes | yes | yes | pass | 0 | MD5 core, upstream testbench |
| [secworks-aes](secworks-aes/SOURCE.md) | BSD-2-Clause | `80dc4718e1dc` | 2714 + 575 | yes | yes | yes | pass | 0 | AES-128/256 encrypt and decrypt, upstream testbench with NIST vectors |
| [secworks-chacha](secworks-chacha/SOURCE.md) | BSD-2-Clause | `7eaba360df9f` | 1108 + 787 | yes | yes | yes | pass | 0 | ChaCha stream cipher, upstream testbench with RFC vectors |
| [forencich-lfsr](forencich-lfsr/SOURCE.md) | MIT | `c1f86d036745` | 664 + 54 | yes | yes | yes | pass | 0 | parallel CRC generator; CRC-32 and CRC-16/XMODEM check values of "123456789" |
| [forencich-uart](forencich-uart/SOURCE.md) | MIT | `1b867e53af73` | 257 + 87 | yes | yes | yes | pass | 0 | AXI-Stream UART, transmitter looped into receiver, 8 bytes |
| [forencich-axis](forencich-axis/SOURCE.md) | MIT | `48ff7a7e2ef7` | 846 + 127 | yes | yes | yes | pass | 0 | AXI-Stream FIFO + skid register with random stalls, 200 words in frames of 16 |
| [picorv32](picorv32/SOURCE.md) | ISC | `ef203c2b0a3f` | 3049 + 80 | yes | yes | yes | pass | 1 | RV32IM CPU running a hand-assembled sum/multiply program |
| [uriscv](uriscv/SOURCE.md) | Apache-2.0 | `a6dc9bee3a99` | 2294 + 290 | yes | yes | yes | pass | 1 | RV32IM CPU with upstream's TCM testbench memory, same program as picorv32 |
| [pulp-common-cells](pulp-common-cells/SOURCE.md) | SHL-0.51 (Apache-2.0 at the licensee's option) | `e73baaec2ca6` | 785 + 182 | yes | yes | yes | pass | 0 | SystemVerilog: package, `parameter type`, `always_comb`, macro headers with                 assertions; popcount, lzc, gray, FIFO vs queue model, round-robin arbiter |
| [serv](serv/SOURCE.md) | ISC | `f200eb2ed7b6` | 3073 + 81 | yes | yes | yes | pass | 0 | bit-serial RV32I CPU with SRAM register file, Wishbone memory, hand-assembled program |
| [dpretet-async-fifo](dpretet-async-fifo/SOURCE.md) | MIT | `38c22208d394` | 372 + 99 | yes | yes | yes | pass | 0 | dual-clock FIFO (100 MHz write, 71 MHz read), 500 words with random gaps |
| [vivado-funcsim](vivado-funcsim/SOURCE.md) | Apache-2.0 (this repository) | - | 1500 + 97 | yes | yes | yes | pass | 0 | UNISIM LUT/FDRE netlist, `-L unisims_ver`, its own `glbl`; AXI4-Lite read/write |
<!-- END:designs -->

<!-- BEGIN:probes -->
| Probe | Construct | Compiled | Ran | Result |
|---|---|---|---|---|
| [probe-bind](probe-bind/top.sv) | `bind` of a checker module into a design | yes | yes | pass |
| [probe-virtual-modport](probe-virtual-modport/top.sv) | SystemVerilog interface, modports, class with a `virtual` interface handle | yes | yes | pass |
| [probe-config-block](probe-config-block/top.sv) | Verilog-2001 `config` block | yes | yes | pass |
| [probe-covergroup](probe-covergroup/top.sv) | functional coverage `covergroup` | yes | yes | pass |
| [probe-constrained-random](probe-constrained-random/top.sv) | class `rand` fields and `constraint` solved by `randomize()` | yes | yes | pass |
| [probe-concurrent-assertion](probe-concurrent-assertion/top.sv) | `property` and `assert property` with a cycle delay range | yes | yes | pass |
| [probe-uvm-with-library](probe-uvm-with-library/top.sv) | UVM 1.2 test with `xvlog -L uvm` / `xelab -L uvm` | yes | yes | pass |
<!-- END:probes -->

<!-- BEGIN:negatives -->
| Case | Unsupported construct | Stops at | Diagnostic (code, file:line, construct) | Passes with |
|---|---|---|---|---|
| [missing_include](negative/missing_include/top.sv) | `include` of a header that is not on the include path | xvlog | VRFC 10-3195 top.sv:2 `no_such_header.vh` | `+incdir+inc` |
| [missing_define](negative/missing_define/top.sv) | macro used but its `-d` define is missing | xvlog | VRFC 10-3805 top.sv:3 `DATA_WIDTH` | `-d DATA_WIDTH=8` |
| [missing_unisim_library](negative/missing_unisim_library/top.sv) | UNISIM primitive (`FDRE`) without the `unisims_ver` library | xelab | VRFC 10-2063 top.sv:5 `FDRE` | `-L unisims_ver` and Vivado's `glbl.v` |
| [missing_module](negative/missing_module/top.sv) | instance of a module no source provides (vendor black box) | xelab | VRFC 10-2063 top.sv:4 `vendor_black_box` | - |
| [uvm_without_library](negative/uvm_without_library/top.sv) | `import uvm_pkg::*` without `-L uvm` | xvlog | VRFC 10-2989 top.sv:2 `uvm_pkg` | `-L uvm` |
| [dpi_import_without_library](negative/dpi_import_without_library/top.sv) | DPI-C import with no C library linked (`-sv_lib` is not supported for designs) | xelab | error top.sv:3 `c_add` | - |
| [encrypted_envelope](negative/encrypted_envelope/top.sv) | IEEE-1735 `pragma protect` envelope without a vendor key | runs, design silently empty | critical warning top.sv:3 ``pragma protect begin_protected` | - |
| [vhdl_2008_construct](negative/vhdl_2008_construct/top.vhd) | VHDL-2008 `process (all)` compiled as VHDL-93 | xvhdl | VRFC 10-1449 top.vhd:8 `process (all) begin` | `xvhdl --2008` (`vhdl2008` in a project file) |
| [nettype](negative/nettype/top.sv) | SystemVerilog user-defined `nettype` | xelab | XSIM 43-3980 top.sv:4 `nettype` | - |
| [unknown_system_task](negative/unknown_system_task/top.sv) | system task from a VPI library that is not loaded | xelab | XSIM 43-4287 top.sv:4 `$my_vpi_hello` | - |
| [generic_interconnect](negative/generic_interconnect/top.sv) | SystemVerilog `interconnect` net type | xelab | VRFC 10-98 top.sv:4 `interconnect w;` | - |
| [syntax_error](negative/syntax_error/top.sv) | plain syntax error | xvlog | VRFC 10-4982 top.sv:5 `;` | - |
| [undeclared_identifier](negative/undeclared_identifier/top.sv) | assignment to an undeclared identifier | xvlog | VRFC 10-2989 top.sv:4 `b` | - |
| [vhdl_syntax_error](negative/vhdl_syntax_error/top.vhd) | VHDL syntax error (missing `;`) | xvhdl | VRFC 10-4982 top.vhd:7 `wait` | - |
| [use_before_declaration](negative/use_before_declaration/top.v) | net used before its declaration (tolerated by Icarus Verilog) | xvlog | VRFC 10-3380 top.v:5 `flag` | `xvlog --relax` |
| [empty_statement_after_label](negative/empty_statement_after_label/top.v) | stray `;` after a block label, `begin : name;` | xvlog | VRFC 10-2096 top.v:4 `begin : blk;` | `xvlog --relax` |
| [runtime_fatal](negative/runtime_fatal/top.sv) | `$fatal` during the run | run (fatal) | fatal top.sv:4 `#5 $fatal(1, "deliberate failure");` | - |
| [readmem_missing_file](negative/readmem_missing_file/top.sv) | `$readmemh` of a file that is not there (warning only) | run (warning) | warning top.sv:4 `no_such_file.hex` | - |
<!-- END:negatives -->

<!-- BEGIN:timings -->
| Case | Wall time (s) |
|---|---|
| secworks-sha256 | 9.5 |
| secworks-sha1 | 8.7 |
| secworks-md5 | 9.0 |
| secworks-aes | 10.4 |
| secworks-chacha | 8.6 |
| forencich-lfsr | 8.5 |
| forencich-uart | 8.0 |
| forencich-axis | 7.1 |
| picorv32 | 7.7 |
| uriscv | 8.2 |
| pulp-common-cells | 7.9 |
| serv | 8.4 |
| dpretet-async-fifo | 6.9 |
| vivado-funcsim | 8.1 |
| probe-bind | 7.0 |
| probe-virtual-modport | 6.8 |
| probe-config-block | 6.6 |
| probe-covergroup | 6.7 |
| probe-constrained-random | 6.7 |
| probe-concurrent-assertion | 7.1 |
| probe-uvm-with-library | 18.3 |
| negative/missing_include | 8.5 |
| negative/missing_define | 8.3 |
| negative/missing_unisim_library | 10.0 |
| negative/missing_module | 3.2 |
| negative/uvm_without_library | 19.7 |
| negative/dpi_import_without_library | 3.3 |
| negative/encrypted_envelope | 6.8 |
| negative/vhdl_2008_construct | 8.5 |
| negative/nettype | 3.1 |
| negative/unknown_system_task | 3.0 |
| negative/generic_interconnect | 3.2 |
| negative/syntax_error | 1.6 |
| negative/undeclared_identifier | 1.7 |
| negative/vhdl_syntax_error | 1.6 |
| negative/use_before_declaration | 8.4 |
| negative/empty_statement_after_label | 9.0 |
| negative/runtime_fatal | 6.6 |
| negative/readmem_missing_file | 6.6 |
<!-- END:timings -->
