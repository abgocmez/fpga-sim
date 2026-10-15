Act as a Senior AMD/Xilinx SoC Verification Engineer. I need to set up a complete Hardware/Software Co-Simulation environment using **QEMU + Vivado XSim** for a **Digilent Zybo Z7-20 (`xc7z020clg400-1`)** target.

### My Environment:

- **OS:** [e.g., Ubuntu 22.04 LTS]
- **Toolchain:** AMD Vivado & Vitis [e.g., 2023.2]
- **Software Target:** Bare-metal C application (`.elf`) running on ARM Cortex-A9 (PS).
- **Hardware Target:** Custom AXI4-Lite slave Verilog module in PL connected to `M_AXI_GP0`.

### Deliverables Requested:

Please generate a complete, reproducible workflow with copy-pasteable scripts (no placeholders):

1. **Minimal Custom AXI4-Lite Verilog Module (`my_math_ip.v`):**
   - Writes a 32-bit value to `slv_reg0` (offset `0x00`), computes `slv_reg1 = slv_reg0 * 2` (offset `0x04`), and asserts a valid AXI read/write response.
2. **Vivado Block Design & Co-Sim Setup (`setup_bd.tcl`):**
   - TCL script to instantiate `processing_system7_0`, connect the custom Verilog module to `M_AXI_GP0` at base address `0x43C00000`, export hardware (`.xsa`), and configure XSim for QEMU RemotePort / TLM co-simulation.
3. **Bare-Metal C Test Application (`main.c`):**
   - Uses `Xil_Out32(0x43C00000, test_val)` and `Xil_In32(0x43C00004)` to verify the PL hardware computation and prints the result via UART.
4. **Co-Simulation Execution Script (`run_cosim.sh`):**
   - Exact command-line instructions (using Vitis `launch_emulator` or direct `qemu-system-aarch64` + `xsim` with `--remote-port` / unix socket path) to launch both simulators concurrently, synchronize them, and open the XSim waveform viewer.
5. **Troubleshooting Guide:**
   - How to resolve common deadlocks (e.g., PS waiting for PL clock/reset, socket path mismatches, or `M_AXI_GP0` enablement issues).
