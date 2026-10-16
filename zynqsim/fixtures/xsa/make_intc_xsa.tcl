# Builds fixtures/xsa/zs_intc.xsa with Vivado 2022.1 in batch mode.
#
#   vivado -mode batch -source make_intc_xsa.tcl -tclargs WORKDIR OUT.xsa
#
# WORKDIR  scratch project directory (created, may be deleted afterwards). Keep the path SHORT
#          and run the script from a short path such as C:\zsd2\make_intc_xsa.tcl (see make_xsa.tcl).
# OUT.xsa  the hardware platform to write
#
# The block design (examples/intc-demo): Zynq PS7 (Zybo Z7-20 board preset), FCLK_CLK0 = 100 MHz,
# M_AXI_GP0 to
#   axi_intc_0     0x41800000  AXI Interrupt Controller, two inputs, irq -> IRQ_F2P[0]
#   axi_gpio_0     0x41200000  4 inputs (switches), interrupt -> intr[0] of the controller
#   axi_timer_0    0x42800000  32 bit timer, interrupt -> intr[1] of the controller
# The two interrupt lines meet in an xlconcat in front of the controller's `intr` input.
# No implementation is run; a small synthesis makes `write_hw_platform` include the HWH.

set work [file normalize [lindex $argv 0]]
set out  [file normalize [lindex $argv 1]]

create_project -force zs_intc $work/prj -part xc7z020clg400-1
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
    CONFIG.PCW_USE_M_AXI_GP0 {1} \
    CONFIG.PCW_USE_FABRIC_INTERRUPT {1} \
    CONFIG.PCW_IRQ_F2P_INTR {1} \
    CONFIG.PCW_EN_CLK0_PORT {1} \
    CONFIG.PCW_FPGA0_PERIPHERAL_FREQMHZ {100}] $ps

set intc [create_bd_cell -type ip -vlnv xilinx.com:ip:axi_intc:4.1 axi_intc_0]
set_property -dict [list CONFIG.C_IRQ_CONNECTION {1}] $intc

set sw [create_bd_cell -type ip -vlnv xilinx.com:ip:axi_gpio:2.0 axi_gpio_0]
set_property -dict [list \
    CONFIG.C_GPIO_WIDTH {4} CONFIG.C_ALL_INPUTS {1} CONFIG.C_INTERRUPT_PRESENT {1}] $sw
make_bd_intf_pins_external [get_bd_intf_pins axi_gpio_0/GPIO]
set_property name sws_4bits [get_bd_intf_ports GPIO_0]

set tmr [create_bd_cell -type ip -vlnv xilinx.com:ip:axi_timer:2.0 axi_timer_0]

set first 1
foreach s {axi_intc_0 axi_gpio_0 axi_timer_0} {
    set pin [get_bd_intf_pins $s/S_AXI]
    if {$first} {
        set ic {New AXI Interconnect}
        set first 0
    } else {
        set ic {Auto}
    }
    apply_bd_automation -rule xilinx.com:bd_rule:axi4 \
        -config [list Clk_master {/processing_system7_0/FCLK_CLK0 (100 MHz)} Clk_slave {Auto} Clk_xbar {Auto} \
                      Master {/processing_system7_0/M_AXI_GP0} Slave "/$s/S_AXI" ddr_seg {Auto} \
                      intc_ip $ic master_apm {0}] $pin
}

set cat [create_bd_cell -type ip -vlnv xilinx.com:ip:xlconcat:2.1 xlconcat_0]
set_property -dict [list CONFIG.NUM_PORTS {2}] $cat
connect_bd_net [get_bd_pins axi_gpio_0/ip2intc_irpt] [get_bd_pins xlconcat_0/In0]
connect_bd_net [get_bd_pins axi_timer_0/interrupt] [get_bd_pins xlconcat_0/In1]
connect_bd_net [get_bd_pins xlconcat_0/dout] [get_bd_pins axi_intc_0/intr]
connect_bd_net [get_bd_pins axi_intc_0/irq] [get_bd_pins processing_system7_0/IRQ_F2P]

foreach {pat off} {axi_intc_0 0x41800000 axi_gpio_0 0x41200000 axi_timer_0 0x42800000} {
    foreach s [get_bd_addr_segs -of_objects [get_bd_addr_spaces processing_system7_0/Data]] {
        if {[string match *${pat}* $s]} {
            set_property offset $off $s
            set_property range 64K $s
        }
    }
}
puts "SEGMENTS:"
foreach s [get_bd_addr_segs] { puts "  $s offset=[get_property OFFSET $s] range=[get_property RANGE $s]" }

validate_bd_design
save_bd_design
set bdfile [get_files design_1.bd]
make_wrapper -files $bdfile -top
add_files -norecurse [file normalize $work/prj/zs_intc.gen/sources_1/bd/design_1/hdl/design_1_wrapper.v]
generate_target all $bdfile
update_compile_order -fileset sources_1
set_property top design_1_wrapper [current_fileset]
launch_runs synth_1 -jobs 4
wait_on_run synth_1
open_run synth_1
write_hw_platform -fixed -force -file $out
puts "XSA written: $out"
exit
