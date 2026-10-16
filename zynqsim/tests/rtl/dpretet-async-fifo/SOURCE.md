# dpretet-async-fifo

| | |
|---|---|
| Upstream | https://github.com/dpretet/async_fifo |
| Commit | `38c22208d3948833f275b917c920e02b1cdadf56` |
| Licence | MIT (full text in `LICENSE`, copied verbatim from upstream `LICENSE`) |
| Modifications | none to upstream files |

Files taken (path here, upstream path):

| Here | Upstream |
|---|---|
| `rtl/async_fifo.v` | `rtl/async_fifo.v` |
| `rtl/fifomem.v` | `rtl/fifomem.v` |
| `rtl/rptr_empty.v` | `rtl/rptr_empty.v` |
| `rtl/sync_ptr.v` | `rtl/sync_ptr.v` |
| `rtl/sync_r2w.v` | `rtl/sync_r2w.v` |
| `rtl/sync_w2r.v` | `rtl/sync_w2r.v` |
| `rtl/wptr_full.v` | `rtl/wptr_full.v` |

Upstream's unit test needs SVUT/Icarus; the testbench `tb/corpus_tb.sv` was written for this corpus.
