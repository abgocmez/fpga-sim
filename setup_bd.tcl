# setup_bd.tcl - Zybo Z7-20 (xc7z020clg400-1) block design with my_math_ip on M_AXI_GP0.
# Usage: vivado -mode batch -source setup_bd.tcl
set script_dir [file dirname [file normalize [info script]]]
set proj_name  zybo_cosim
set proj_dir   [file join $script_dir build $proj_name]
set out_dir    [file join $script_dir out]
file mkdir $out_dir

create_project -force $proj_name $proj_dir -part xc7z020clg400-1
set_property target_language Verilog [current_project]

add_files -norecurse [file join $script_dir rtl my_math_ip.v]
add_files -fileset sim_1 -norecurse [file join $script_dir sim cosim_top.sv]
update_compile_order -fileset sources_1

# ---- Block design -----------------------------------------------------------
create_bd_design system

set ps [create_bd_cell -type ip -vlnv xilinx.com:ip:processing_system7 processing_system7_0]
apply_bd_automation -rule xilinx.com:bd_rule:processing_system7 \
    -config {make_external "FIXED_IO, DDR" apply_board_preset "0" Master "Disable" Slave "Disable"} $ps

# Zybo Z7 wiring: UART1 on MIO48/49, 100 MHz FCLK0, GP0 master + FCLK0/reset on.
set_property -dict [list \
    CONFIG.PCW_USE_M_AXI_GP0              {1} \
    CONFIG.PCW_FPGA0_PERIPHERAL_FREQMHZ   {100} \
    CONFIG.PCW_EN_CLK0_PORT               {1} \
    CONFIG.PCW_EN_RST0_PORT               {1} \
    CONFIG.PCW_UART1_PERIPHERAL_ENABLE    {1} \
    CONFIG.PCW_UART1_UART1_IO             {MIO 48 .. 49} \
    CONFIG.PCW_EN_UART1                   {1} \
    CONFIG.PCW_QSPI_PERIPHERAL_ENABLE     {1} \
    CONFIG.PCW_QSPI_GRP_SINGLE_SS_ENABLE  {1} \
    CONFIG.PCW_SD0_PERIPHERAL_ENABLE      {1} \
    CONFIG.PCW_SD0_SD0_IO                 {MIO 40 .. 45} \
] $ps

set ip [create_bd_cell -type module -reference my_math_ip my_math_ip_0]

# Interconnect + proc_sys_reset are created by the automation.
apply_bd_automation -rule xilinx.com:bd_rule:axi4 \
    -config {Master "/processing_system7_0/M_AXI_GP0" Clk_master "Auto" Clk_slave "Auto" Clk_xbar "Auto" Master_Rst "Auto" Slave_Rst "Auto"} \
    [get_bd_intf_pins my_math_ip_0/S_AXI]

# Pin the slave at 0x43C00000 (4K).
assign_bd_address
set segs [get_bd_addr_segs -of_objects [get_bd_addr_spaces processing_system7_0/Data] -filter {NAME =~ "*my_math_ip_0*"}]
if {[llength $segs] != 1} {
    error "expected exactly one address segment for my_math_ip_0, got: $segs"
}
set_property offset 0x43C00000 [lindex $segs 0]
set_property range  4K         [lindex $segs 0]

regenerate_bd_layout
validate_bd_design
save_bd_design

# ---- Wrapper, outputs, XSA --------------------------------------------------
make_wrapper -files [get_files system.bd] -top
add_files -norecurse [file join $proj_dir $proj_name.gen sources_1 bd system hdl system_wrapper.v]
set_property top system_wrapper [current_fileset]
generate_target all [get_files system.bd]

# Hardware-only XSA is enough for Vitis/QEMU (no bitstream needed for co-sim).
write_hw_platform -fixed -force -file [file join $out_dir system_wrapper.xsa]

# ---- XSim set-up for QEMU RemotePort co-simulation ---------------------------
# The PS is replaced by QEMU, so the simulation top is cosim_top (clock/reset +
# my_math_ip + a DPI-C remote-port client). run_cosim_win.ps1 / run_cosim.sh build
# and launch it with xsc/xelab; this file is only registered for the Vivado GUI.
set_property file_type SystemVerilog [get_files -of_objects [get_filesets sim_1] cosim_top.sv]
set_property top cosim_top [get_filesets sim_1]
set_property -name {xsim.simulate.runtime}         -value {0ns}  -objects [get_filesets sim_1]
set_property -name {xsim.simulate.log_all_signals} -value {true} -objects [get_filesets sim_1]

puts "OK: XSA written to [file join $out_dir system_wrapper.xsa]"
close_project
