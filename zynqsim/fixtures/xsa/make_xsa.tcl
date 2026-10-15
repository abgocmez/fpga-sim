# Builds fixtures/xsa/zs_math_gpio.xsa with Vivado 2022.1 in batch mode.
#
#   vivado -mode batch -source make_xsa.tcl -tclargs WORKDIR OUT.xsa RTLDIR [math_only]
#
# `math_only` leaves out the AXI GPIO and the fabric interrupt (fixtures/xsa/zs_math_only.xsa);
# without it the design is fixtures/xsa/zs_math_gpio.xsa.
#
# WORKDIR  scratch project directory (created, may be deleted afterwards). Keep the path
#          SHORT, and run this script from a short path such as C:\zsx\make_xsa.tcl: when it is
#          sourced from a deep path (here a `.claude\worktrees\...` directory) Vivado's
#          block-design init fails with "couldn't read file utils_dbg.tcl / clkrst.tcl".
# OUT.xsa  the hardware platform to write
# RTLDIR   directory holding my_math_ip.v (fixtures/rtl/math)
#
# The block design: Zynq PS7 (Zybo Z7-20 board preset), the user's AXI4-Lite slave
# `my_math_ip` as a module reference at 0x43C00000, an AXI GPIO (4 outputs, interrupt) at
# 0x41200000 whose interrupt is wired to IRQ_F2P[0], FCLK_CLK0 = 100 MHz.
# No synthesis/implementation is run: `write_hw_platform -fixed` (pre-synthesis XSA).

set work [file normalize [lindex $argv 0]]
set out  [file normalize [lindex $argv 1]]
set rtl  [file normalize [lindex $argv 2]]
set with_gpio [expr {[lindex $argv 3] ne "math_only"}]

create_project -force zs_xsa $work/prj -part xc7z020clg400-1
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
    CONFIG.PCW_USE_FABRIC_INTERRUPT [expr {$with_gpio ? 1 : 0}] \
    CONFIG.PCW_IRQ_F2P_INTR [expr {$with_gpio ? 1 : 0}] \
    CONFIG.PCW_EN_CLK0_PORT {1} \
    CONFIG.PCW_FPGA0_PERIPHERAL_FREQMHZ {100}] $ps

set math [create_bd_cell -type module -reference my_math_ip u_math]
if {$with_gpio} {
    set gpio [create_bd_cell -type ip -vlnv xilinx.com:ip:axi_gpio:2.0 axi_gpio_0]
    set_property -dict [list \
        CONFIG.C_GPIO_WIDTH {4} \
        CONFIG.C_ALL_OUTPUTS {1} \
        CONFIG.C_INTERRUPT_PRESENT {1}] $gpio
    make_bd_intf_pins_external [get_bd_intf_pins axi_gpio_0/GPIO]
    apply_bd_automation -rule xilinx.com:bd_rule:axi4 \
        -config {Clk_master {/processing_system7_0/FCLK_CLK0 (100 MHz)} Clk_slave {Auto} Clk_xbar {Auto} Master {/processing_system7_0/M_AXI_GP0} Slave {/axi_gpio_0/S_AXI} ddr_seg {Auto} intc_ip {New AXI Interconnect} master_apm {0}} \
        [get_bd_intf_pins axi_gpio_0/S_AXI]
}
apply_bd_automation -rule xilinx.com:bd_rule:axi4 \
    -config {Clk_master {/processing_system7_0/FCLK_CLK0 (100 MHz)} Clk_slave {Auto} Clk_xbar {Auto} Master {/processing_system7_0/M_AXI_GP0} Slave {/u_math/S_AXI} ddr_seg {Auto} intc_ip {New AXI Interconnect} master_apm {0}} \
    [get_bd_intf_pins u_math/S_AXI]

if {$with_gpio} {
    connect_bd_net [get_bd_pins axi_gpio_0/ip2intc_irpt] [get_bd_pins processing_system7_0/IRQ_F2P]
}

puts "SEGMENTS BEFORE:"
foreach s [get_bd_addr_segs] { puts "  $s offset=[get_property OFFSET $s] range=[get_property RANGE $s]" }
# Put the user IP where the Zybo firmware expects the math IP.
foreach s [get_bd_addr_segs -of_objects [get_bd_addr_spaces processing_system7_0/Data]] {
    if {[string match *u_math* $s]} {
        set_property offset 0x43C00000 $s
        set_property range 4K $s
    }
    if {[string match *axi_gpio_0* $s]} {
        set_property offset 0x41200000 $s
        set_property range 64K $s
    }
}
puts "SEGMENTS AFTER:"
foreach s [get_bd_addr_segs] { puts "  $s offset=[get_property OFFSET $s] range=[get_property RANGE $s]" }

validate_bd_design
save_bd_design
# The HWH is an output product of the block design: without this the XSA holds no handoff.
set bdfile [get_files design_1.bd]
make_wrapper -files $bdfile -top
add_files -norecurse [file normalize $work/prj/zs_xsa.gen/sources_1/bd/design_1/hdl/design_1_wrapper.v]
generate_target all $bdfile
update_compile_order -fileset sources_1
set_property top design_1_wrapper [current_fileset]
# A pre-synthesis write_hw_platform leaves the handoff out ("Failed to write hardware handoff
# data"), so synthesise once (the design is tiny).
launch_runs synth_1 -jobs 4
wait_on_run synth_1
open_run synth_1
write_hw_platform -fixed -force -file $out
puts "XSA written: $out"
exit
