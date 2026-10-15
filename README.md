# Zybo Z7-20 PS/PL co-simulation: QEMU + Vivado XSim

A bare-metal Cortex-A9 program runs in Xilinx QEMU and talks to a custom
AXI4-Lite IP (`my_math_ip`, `slv_reg1 = slv_reg0 * 2`) simulated in Vivado XSim.
The PS7 is replaced by QEMU; the PL side is a small DPI-C client of Xilinx's
remote-port protocol (`sim/winport/rp_dpi.c`), so it needs no SystemC.

Target: Digilent Zybo Z7-20 (`xc7z020clg400-1`), `M_AXI_GP0`, IP at `0x43C00000`.

## Status

| Part | Verified |
|---|---|
| `setup_bd.tcl` -> XSA (Vivado 2022.1, Windows 11) | yes |
| `build_app.tcl` -> ELF (Vitis/xsct 2022.1) | yes |
| `run_cosim_win.ps1` end to end (Windows, QEMU from Vitis 2022.1) | **yes: 7/7 vectors PASS, 21 AXI beats** |
| `run_cosim.sh` (Linux / WSL2) | no, same design but never run |

## Requirements

- Vivado and Vitis **2022.1** (QEMU and the Zynq co-sim device tree come from Vitis:
  `data/emulation/qemu_win/qemu/qemu-system-aarch64.exe`,
  `data/emulation/dtbs/zynq/zynq-arm-cosim.dtb`)
- Git (the scripts clone `Xilinx/libsystemctlm-soc` into `third_party/` for its
  protocol encoder/decoder, `libremote-port/remote-port-proto.c`)

## Run (Windows)

```powershell
vivado -mode batch -source setup_bd.tcl        # out\system_wrapper.xsa
C:\Xilinx\Vitis\2022.1\bin\xsct.bat build_app.tcl   # out\math_test.elf
powershell -ExecutionPolicy Bypass -File run_cosim_win.ps1          # batch
powershell -ExecutionPolicy Bypass -File run_cosim_win.ps1 -Gui     # XSim waveform viewer
```

Expected UART output ends with `RESULT: ALL PASS`; waveforms are in
`build\cosim_win\cosim.wdb`. Linux/WSL2: `./run_cosim.sh` (see its header).

## Layout

| Path | Purpose |
|---|---|
| `rtl/my_math_ip.v` | AXI4-Lite slave (reg0 RW, reg1 = reg0 * 2) |
| `setup_bd.tcl` | Block design with `processing_system7_0`, IP at `0x43C00000`, exports XSA |
| `sw/main.c`, `build_app.tcl` | Bare-metal test and its xsct build |
| `sim/cosim_top.sv` | Clock/reset, DUT, AXI-Lite master driven through DPI-C |
| `sim/winport/rp_dpi.c` | Remote-port client (hello, sync, bus read/write) |
| `sim/winport/wsdyn.[ch]`, `remote-port-sk.c`, `safeio.c` | Windows (winsock) replacements |
| `run_cosim_win.ps1`, `run_cosim.sh` | Build + launch QEMU and XSim |
| `TROUBLESHOOTING.md` | Failures seen while getting this to work, and fixes |

## How it works

1. QEMU starts the `arm-generic-fdt-7series` machine with the Xilinx co-sim DTB. The
   DTB declares the PL as `remote-port` devices; `M_AXI_GP0` is device 7.
2. QEMU listens on a chardev (`-chardev socket,...,wait=on` +
   `-global remote-port.chardev=pl-rp`; TCP on Windows) and blocks until XSim connects.
3. XSim runs `cosim_top`; `rp_next()` hands it one 32-bit beat at a time, it drives
   AXI4-Lite into `my_math_ip`, and `rp_done()` returns the response to QEMU.
4. The firmware prints `COSIM_DONE`; the script stops QEMU, XSim sees the socket
   close, calls `$finish` and flushes the waveform database.

## Limits

- Single-master AXI4-Lite only; no interrupts back to the PS, no AFI/ACP ports.
- Windows build links nothing from `ws2_32` statically (see `TROUBLESHOOTING.md`).
- Simulated time is not locked to QEMU time (the sync response echoes QEMU's
  timestamp); this checks function, not cycle-accurate PS/PL timing.
