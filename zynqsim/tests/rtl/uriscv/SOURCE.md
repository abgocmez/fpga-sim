# uriscv

| | |
|---|---|
| Upstream | https://github.com/ultraembedded/core_uriscv |
| Commit | `a6dc9bee3a99f693ba9f2ec526f41c54b4e191a2` |
| Licence | Apache-2.0 (full text in `LICENSE`, copied verbatim from upstream `LICENSE`) |
| Modifications | none to upstream files |

Files taken (path here, upstream path):

| Here | Upstream |
|---|---|
| `rtl/riscv_core.v` | `src_v/riscv_core.v` |
| `rtl/uriscv_alu.v` | `src_v/uriscv_alu.v` |
| `rtl/uriscv_branch.v` | `src_v/uriscv_branch.v` |
| `rtl/uriscv_csr.v` | `src_v/uriscv_csr.v` |
| `rtl/uriscv_defs.v` | `src_v/uriscv_defs.v` |
| `rtl/uriscv_lsu.v` | `src_v/uriscv_lsu.v` |
| `rtl/uriscv_muldiv.v` | `src_v/uriscv_muldiv.v` |
| `tb/tcm_mem.v` | `tb/tb_core_icarus/tcm_mem.v` |
| `tb/tcm_mem_ram.v` | `tb/tb_core_icarus/tcm_mem_ram.v` |

`tb/tcm_mem.v` and `tb/tcm_mem_ram.v` are upstream's testbench memory; `tb/corpus_tb.sv` is written for this corpus.
