# Corpus on the native RTL engine (generated)

Written by `cargo test -p rtl-engine --test corpus`. Each row is one design from
`tests/rtl` run with its `tb/corpus_tb.sv` (top `corpus_tb`) for at most 10 ms of
simulated time. *accepted* means the front end and the engine's elaboration took it
with no diagnostic. *runs* means no run-time error. *passes* means the testbench printed
`CORPUS_PASS`. The engine is 2-state (x and z read as 0); a testbench that depends on
x or z comparisons is not decided the same way as in XSim.

| design | accepted | runs | passes | note |
|---|---|---|---|---|
| forencich-lfsr | yes | yes | yes | CORPUS_PASS |
| forencich-uart | yes | yes | yes | CORPUS_PASS |
| forencich-axis | yes | yes | yes | CORPUS_PASS |
| dpretet-async-fifo | yes | yes | yes | CORPUS_PASS |
| picorv32 | yes | yes | yes | CORPUS_PASS |
| serv | yes | yes | yes | CORPUS_PASS |
| uriscv | no | no | no | refused: task at corpus_tb.sv:59 (63 in all): task `u_mem.write` is not defined |
| pulp-common-cells | no | no | no | refused: parameter type at cc_fifo.sv:20 (240 in all): parameter of package type `type` is not supported by the native front end |
| vivado-funcsim | no | no | no | refused: undefined module at corpus_tb.sv:19 (1 in all): module `my_math_ip` is not defined |

**Corpus: 6 of 9 pass.**

## Fixtures without a testbench

| design | accepted | note |
|---|---|---|
| irq_ip (fixtures/rtl/irq) | yes | elaborates; no testbench (not run) |
| oled_spi (fixtures/rtl/pmod) | yes | elaborates; no testbench (not run) |
| rgb_pwm (fixtures/rtl/pmod) | yes | elaborates; no testbench (not run) |
