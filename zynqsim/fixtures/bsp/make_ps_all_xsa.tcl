# Builds fixtures/bsp/zs_ps_all.xsa with Vivado 2022.1 in batch mode.
#
#   vivado -mode batch -source make_ps_all_xsa.tcl -tclargs WORKDIR OUT.xsa
#
# Keep WORKDIR short and run this script from a short path such as C:\zsx\make_ps_all_xsa.tcl
# (see fixtures/xsa/make_xsa.tcl: from a deep path Vivado's block-design init fails).
#
# A PS-only design (no PL logic) whose only purpose is to make Vitis generate a standalone BSP
# with the driver for every PS peripheral a standalone BSP application might use: the Zybo Z7-20
# board preset (UART1, QSPI, SD0, ENET0, USB0, GPIO on MIO) plus TTC0, I2C0, SPI0 and CAN0 on EMIO.
# No synthesis is needed beyond what write_hw_platform requires for the handoff.

set work [file normalize [lindex $argv 0]]
set out  [file normalize [lindex $argv 1]]

create_project -force zs_psall $work/prj -part xc7z020clg400-1
if {[catch {set_property board_part digilentinc.com:zybo-z7-20:part0:1.2 [current_project]} msg]} {
    puts "NOTE: board part not available ($msg); using the bare part"
}
create_bd_design design_1
set ps [create_bd_cell -type ip -vlnv xilinx.com:ip:processing_system7:5.5 processing_system7_0]
catch {
    apply_bd_automation -rule xilinx.com:bd_rule:processing_system7 \
        -config {make_external "FIXED_IO, DDR" apply_board_preset "1" Master "Disable" Slave "Disable"} $ps
}
set_property -dict [list \
    CONFIG.PCW_USE_M_AXI_GP0 {0} \
    CONFIG.PCW_EN_CLK0_PORT {1} \
    CONFIG.PCW_TTC0_PERIPHERAL_ENABLE {1} \
    CONFIG.PCW_TTC0_TTC0_IO {EMIO} \
    CONFIG.PCW_I2C0_PERIPHERAL_ENABLE {1} \
    CONFIG.PCW_I2C0_I2C0_IO {EMIO} \
    CONFIG.PCW_SPI0_PERIPHERAL_ENABLE {1} \
    CONFIG.PCW_SPI0_SPI0_IO {EMIO} \
    CONFIG.PCW_CAN0_PERIPHERAL_ENABLE {1} \
    CONFIG.PCW_CAN0_CAN0_IO {EMIO}] $ps
validate_bd_design
save_bd_design
set bdfile [get_files design_1.bd]
make_wrapper -files $bdfile -top
add_files -norecurse [file normalize $work/prj/zs_psall.gen/sources_1/bd/design_1/hdl/design_1_wrapper.v]
generate_target all $bdfile
update_compile_order -fileset sources_1
set_property top design_1_wrapper [current_fileset]
launch_runs synth_1 -jobs 4
wait_on_run synth_1
open_run synth_1
write_hw_platform -fixed -force -file $out
puts "XSA written: $out"
exit
