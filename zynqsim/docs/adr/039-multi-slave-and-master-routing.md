# ADR-039: Multi-Slave AXI4-Lite and Master Bridge Routing

Status: **Accepted** (Milestone Task B; Windows, Linux, macOS). Date: 2026-10-19.

## Context

Vivado hardware handoffs (`.xsa`) regularly incorporate multiple user AXI4-Lite IP blocks attached to the PS via an AXI interconnect on `M_AXI_GP0` or `M_AXI_GP1`, as well as IP blocks functioning as AXI masters writing/reading PS memory through high-performance (`S_AXI_HP0..3`), general-purpose (`S_AXI_GP0..1`), or accelerator coherency (`S_AXI_ACP`) ports.

Previously, `zynqsim init --xsa` supported only a single user RTL slave; any additional user slaves were flagged as unmodelled, and master ports lacked dynamic device ID mapping to QEMU's Remote-Port interface. Furthermore, the native RTL engine (`rtl-native`) only mapped a single `RtlModule` instance.

## Facts found on this machine (checked, not assumed)

| Fact | Evidence |
|---|---|
| In Vivado 2022.1 block designs with multiple module references (`fixtures/xsa/zs_two_math.xsa`), each slave is assigned an independent base address and window (`0x43C00000` and `0x43C10000`, 4 KiB each) | Inspecting `design_1.hwh` in `zs_two_math.xsa` |
| Verilog DPI-C `rp_next` takes `integer a_i`, which is signed 32-bit; comparing raw `a_i` against addresses >= `0x80000000` requires unsigned comparison `wire [31:0] u_addr = a_i;` to avoid signed-extension bugs | Verilog LRM and DPI-C ABI |
| Xilinx QEMU remote-port device IDs 0..6 correspond to PS slave ports: dev 0 = `S_AXI_GP0`, dev 1 = `S_AXI_GP1`, dev 2..5 = `S_AXI_HP0..3`, dev 6 = `S_AXI_ACP` | `backend-qemu` and Remote-Port spec |
| Protocol checkers (`zs_axi_checker`) require unique integer IDs (`.ID(i)`) to report violations distinguishable per bus interface | `crates/sim-api/src/tbgen.rs` |
| Pure-Rust `rtl-native` simulates synthesizable Verilog modules without DPI imports; testbench wrappers (`cosim_top.sv` and `zs_axi_checker.sv`) must be excluded from `RtlNativeSpec.sources` | Elaboration in `crates/rtl-front` and `crates/sim-api/src/pl_attach.rs` |

## Decision

1. **Multi-Slave Testbench Generation (`crates/sim-api/src/tbgen.rs`)**:
   - `generate_multi_slave_testbench(slaves, clock_hz)` generates prefixed port wires and instances (`{instance}_{port}`) for each slave.
   - Individual read/write tasks `lite_write_{i}` and `lite_read_{i}` handle bus handshakes with local offset `u_addr - base`.
   - Sequential protocol checkers `zs_chk_lite_{i}` monitor each slave interface.
   - An unsigned 32-bit address decoder routes `u_addr` to the matching slave window, returning DECERR (`ok = 0`, `rd_val = 32'hDEC0_DE00`) on unmapped accesses.
   - Single-slave generation `generate_testbench` is preserved byte-identically when only one slave is present to guarantee backwards compatibility and existing trace digests.

2. **Master Bridge Routing**:
   - `master_interface_device(iface)` maps `S_AXI_GP0..1`, `S_AXI_HP0..3`, and `S_AXI_ACP` to Remote-Port devices 0..6.
   - `master_bridge_with_dev(mst, dev, pfx)` generates the memory bridge with parameterized remote-port device and signal prefixes.

3. **Pure-Rust RTL-Native Engine (`crates/sim-api/src/pl_attach.rs`, `crates/sim-api/src/project.rs`)**:
   - `RtlNativeSpec` supports multiple slave placements via `slaves: Vec<RtlNativeSlaveSpec>`.
   - `rtl_native_target` elaborates each module independently and attaches it to `NativePl` via `pl.map_irq`.
   - Testbench-only files (`cosim_top.sv`, `zs_axi_checker.sv`) are automatically excluded from `RtlNativeSpec.sources`.

4. **Testing**:
   - Unit tests in `tbgen`: `multi_slave_testbench_generates_instances_decoders_and_checkers`, `multi_slave_testbench_with_master_and_irqs`, and `master_interface_device_maps_standard_ports`.
   - Unit test in `native_pl`: `multiple_rtl_modules_on_the_rtl_engine_answer_on_the_ps_bus`.
   - Integration tests in `crates/cli/tests/multi_slave.rs`:
     - `init_xsa_multi_slave_produces_valid_project`
     - `multi_slave_runs_on_rtl_native`
     - `multi_slave_runs_on_xsim`
     - `multi_slave_runs_on_verilator`
     - `multi_slave_runs_on_icarus`

