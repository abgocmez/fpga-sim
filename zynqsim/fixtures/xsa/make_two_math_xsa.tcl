# Builds fixtures/xsa/zs_two_math.xsa with Vivado 2022.1 in batch mode.
#
#   vivado -mode batch -source make_two_math_xsa.tcl -tclargs WORKDIR OUT.xsa RTLDIR
#
# WORKDIR  scratch project directory (created, may be deleted afterwards). Keep the path
#          SHORT, and run this script from a short path such as C:\zsx\make_two_math_xsa.tcl.
# OUT.xsa  the hardware platform to write
# RTLDIR   directory holding my_math_ip.v (fixtures/rtl/math)
#
# The block design: Zynq PS7 (Zybo Z7-20 board preset), two user AXI4-Lite slaves:
# `u_math0` at 0x43C00000 (range 4K) and `u_math1` at 0x43C10000 (range 4K)
# behind M_AXI_GP0, FCLK_CLK0 = 100 MHz.

set work [file normalize [lindex $argv 0]]
set out  [file normalize [lindex $argv 1]]
set rtl  [file normalize [lindex $argv 2]]

create_project -force zs_two_math $work/prj -part xc7z020clg400-1
if {[catch {set_property board_part digilentinc.com:zybo-z7-20:part0:1.2 [current_project]} msg]} {
    puts "NOTE: board part not available ($msg); using the bare part"
}
add_files -norecurse $rtl/my_math_ip.v
update_compile_order -fileset sources_1

create_bd_design design_1
set ps [create_bd_cell -type ip -vlnv xilinx.com:ip:processing_system7:5.5 processing_system7_0]
catch {
    apply_bd_automation -rule xilinx.com:bd_rule:processing_system7 \
        -config {make_external "FIXED_IO, DDR" apply_board_preset "1" Master "Disable" Slave "Disable"} $ps
}
set_property -dict [list \
    CONFIG.PCW_USE_M_AXI_GP0 {1} \
    CONFIG.PCW_EN_CLK0_PORT {1} \
    CONFIG.PCW_FPGA0_PERIPHERAL_FREQMHZ {100}] $ps

set math0 [create_bd_cell -type module -reference my_math_ip u_math0]
set math1 [create_bd_cell -type module -reference my_math_ip u_math1]

apply_bd_automation -rule xilinx.com:bd_rule:axi4 \
    -config {Clk_master {/processing_system7_0/FCLK_CLK0 (100 MHz)} Clk_slave {Auto} Clk_xbar {Auto} Master {/processing_system7_0/M_AXI_GP0} Slave {/u_math0/S_AXI} ddr_seg {Auto} intc_ip {New AXI Interconnect} master_apm {0}} \
    [get_bd_intf_pins u_math0/S_AXI]

apply_bd_automation -rule xilinx.com:bd_rule:axi4 \
    -config {Clk_master {/processing_system7_0/FCLK_CLK0 (100 MHz)} Clk_slave {Auto} Clk_xbar {Auto} Master {/processing_system7_0/M_AXI_GP0} Slave {/u_math1/S_AXI} ddr_seg {Auto} intc_ip {Auto} master_apm {0}} \
    [get_bd_intf_pins u_math1/S_AXI]

puts "SEGMENTS BEFORE:"
foreach s [get_bd_addr_segs] { puts "  $s offset=[get_property OFFSET $s] range=[get_property RANGE $s]" }

foreach s [get_bd_addr_segs -of_objects [get_bd_addr_spaces processing_system7_0/Data]] {
    if {[string match *u_math0* $s]} {
        set_property offset 0x43C00000 $s
        set_property range 4K $s
    }
    if {[string match *u_math1* $s]} {
        set_property offset 0x43C10000 $s
        set_property range 4K $s
    }
}

puts "SEGMENTS AFTER:"
foreach s [get_bd_addr_segs] { puts "  $s offset=[get_property OFFSET $s] range=[get_property RANGE $s]" }

validate_bd_design
save_bd_design
set bdfile [get_files design_1.bd]
make_wrapper -files $bdfile -top
add_files -norecurse [file normalize $work/prj/zs_two_math.gen/sources_1/bd/design_1/hdl/design_1_wrapper.v]
generate_target all $bdfile
update_compile_order -fileset sources_1
set_property top design_1_wrapper [current_fileset]
launch_runs synth_1 -jobs 4
wait_on_run synth_1
open_run synth_1
write_hw_platform -fixed -force -file $out
puts "XSA written: $out"
exit

