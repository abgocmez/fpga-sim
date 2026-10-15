# Co-simulation troubleshooting

Flow: `setup_bd.tcl` -> XSA -> `build_app.tcl` -> ELF -> `run_cosim_win.ps1`
(or `run_cosim.sh`). QEMU runs the ELF; its PL remote-port chardev connects to
`rp_dpi.c` (DPI-C inside XSim), which turns each PS access into AXI4-Lite beats
on `my_math_ip`.

First stop for any hang: `build/cosim_win/qemu.err.log`, `uart1.log`,
`xsim.out.log` (Linux: `build/cosim_run/`).

## Symptoms and fixes

| Symptom | Cause | Fix |
|---|---|---|
| `QEMU exited early`, log says `Missing chardesc prop. Forgot -machine-path?` | The co-sim DTB has no `chrdev-id`, so the remote-port device has no chardev | Pass `-chardev socket,id=pl-rp,...` **and** `-global remote-port.chardev=pl-rp` (both scripts do) |
| `unix sockets are not available on windows` | Windows QEMU cannot use `-machine-path`/unix sockets | Use a TCP chardev (`run_cosim_win.ps1` does). `RP_SOCKET=tcp:127.0.0.1:<port>` |
| QEMU asserts `rp_write: assertion failed` right after start | `wait=off`: QEMU sent hello with no peer yet | Use `server=on,wait=on` and start XSim after QEMU |
| `xsim` says `Simulation engine failed to start ... -1073741511` (0xC0000139) | The DPI objects carry static `ws2_32` imports; `xsimk.exe` cannot start with them | Winsock is bound at run time in `sim/winport/wsdyn.c`; do not add `-lws2_32` or link `ws2_32` objects |
| `Invalid path for DPI library ...dpi.a` | xelab on Windows links a static `dpi.a` from `-sv_root`; `xsc -c` or a failed archive step left none | Run plain `xsc <sources>` (not `-c`/`--shared`), then `xelab -sv_root xsim.dir/work/xsc -sv_lib dpi` |
| `xsc` fails with `ar.exe: unknown option -- w` | `--gcc_link_options` is forwarded to `ar` | Do not pass link options to xsc on Windows |
| XSim prints `rp_open failed` | `RP_SOCKET` not set, or QEMU not listening yet | Start QEMU first (`wait=on`), then XSim with `RP_SOCKET` exported |
| UART prints "start", then nothing; XSim idle | QEMU blocked on a PL access that never completes: DUT not out of reset or wrong address | Check `aresetn` in the waveform; check `cosim_top: WR/RD` lines in `xsim.out.log` |
| Reads return 0, writes ignored | PS->PL level shifters off / FPGA resets held (no FSBL in QEMU) | `main.c` `pl_bringup()` writes SLCR `LVL_SHFTR_EN` and `FPGA_RST_CTRL`; keep it in your own app |
| Data abort at `0x43C00000` | `M_AXI_GP0` disabled, or a peer that is gone | BD: `PCW_USE_M_AXI_GP0 = 1`; address editor: `my_math_ip_0` at `0x43C00000`/4K |

## Address map notes

- The co-sim DTB maps `M_AXI_GP0` as remote-port device 7 at `0x40000000`, 1 GB,
  and `M_AXI_GP1` as device 8 at `0x80000000`. QEMU sends the full address; the
  testbench passes only the low 4 bits to `my_math_ip`.
- `my_math_ip` has 4 address bits, but the BD minimum window is 4K. Offsets
  other than `0x00`/`0x04` return 0 and are acknowledged with OKAY.

## What the PL client ignores

Interrupt, cfg and ATS packets are consumed and dropped; wire updates stay posted
because `CAP_WIRE_POSTED_UPDATES` is not advertised. Bus accesses are served
as 32-bit beats, so multi-word bursts are split into single AXI4-Lite beats.
