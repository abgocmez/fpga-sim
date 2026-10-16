# Builds fixtures/xsa/zs_vendor_ip.xsa with Vivado 2022.1 in batch mode.
#
#   vivado -mode batch -source make_vendor_xsa.tcl -tclargs WORKDIR OUT.xsa
#
# WORKDIR  scratch project directory (created, may be deleted afterwards). Keep the path
#          SHORT, and run this script from a short path such as C:\zsx (see make_xsa.tcl).
# OUT.xsa  the hardware platform to write
#
# The block design (examples/gpio-timer-uart): Zynq PS7 (Zybo Z7-20 board preset), FCLK_CLK0 =
# 100 MHz, and four Xilinx IP behind M_AXI_GP0:
#   axi_gpio_0     0x41200000  4 outputs               external `leds_4bits`
#   axi_gpio_1     0x41210000  dual, 4 + 4 inputs, interrupt   external `sws_4bits`, `btns_4bits`
#   axi_timer_0    0x42800000  32 bit, interrupt
#   axi_uartlite_0 0x40600000  115200 8N1, interrupt
# Interrupts go through an xlconcat to IRQ_F2P: timer [0], gpio_1 [1], uartlite [2].
# No implementation is run; a small synthesis makes `write_hw_platform` include the HWH.

set work [file normalize [lindex $argv 0]]
set out  [file normalize [lindex $argv 1]]

create_project -force zs_vendor $work/prj -part xc7z020clg400-1
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

set led [create_bd_cell -type ip -vlnv xilinx.com:ip:axi_gpio:2.0 axi_gpio_0]
set_property -dict [list CONFIG.C_GPIO_WIDTH {4} CONFIG.C_ALL_OUTPUTS {1}] $led

set sw [create_bd_cell -type ip -vlnv xilinx.com:ip:axi_gpio:2.0 axi_gpio_1]
set_property -dict [list \
    CONFIG.C_IS_DUAL {1} \
    CONFIG.C_GPIO_WIDTH {4} CONFIG.C_ALL_INPUTS {1} \
    CONFIG.C_GPIO2_WIDTH {4} CONFIG.C_ALL_INPUTS_2 {1} \
    CONFIG.C_INTERRUPT_PRESENT {1}] $sw

set tmr [create_bd_cell -type ip -vlnv xilinx.com:ip:axi_timer:2.0 axi_timer_0]

set urt [create_bd_cell -type ip -vlnv xilinx.com:ip:axi_uartlite:2.0 axi_uartlite_0]
set_property -dict [list CONFIG.C_BAUDRATE {115200}] $urt

# External interfaces with names that say what they are.
make_bd_intf_pins_external [get_bd_intf_pins axi_gpio_0/GPIO]
set_property name leds_4bits [get_bd_intf_ports GPIO_0]
make_bd_intf_pins_external [get_bd_intf_pins axi_gpio_1/GPIO]
set_property name sws_4bits [get_bd_intf_ports GPIO_0]
make_bd_intf_pins_external [get_bd_intf_pins axi_gpio_1/GPIO2]
set_property name btns_4bits [get_bd_intf_ports GPIO2_0]
make_bd_intf_pins_external [get_bd_intf_pins axi_uartlite_0/UART]

set first 1
foreach s {axi_gpio_0 axi_gpio_1 axi_timer_0 axi_uartlite_0} {
    set pin [get_bd_intf_pins $s/S_AXI]
    if {$first} {
        set intc {New AXI Interconnect}
        set first 0
    } else {
        set intc {Auto}
    }
    apply_bd_automation -rule xilinx.com:bd_rule:axi4 \
        -config [list Clk_master {/processing_system7_0/FCLK_CLK0 (100 MHz)} Clk_slave {Auto} Clk_xbar {Auto} \
                      Master {/processing_system7_0/M_AXI_GP0} Slave "/$s/S_AXI" ddr_seg {Auto} \
                      intc_ip $intc master_apm {0}] $pin
}

set cat [create_bd_cell -type ip -vlnv xilinx.com:ip:xlconcat:2.1 xlconcat_0]
set_property -dict [list CONFIG.NUM_PORTS {3}] $cat
connect_bd_net [get_bd_pins axi_timer_0/interrupt] [get_bd_pins xlconcat_0/In0]
connect_bd_net [get_bd_pins axi_gpio_1/ip2intc_irpt] [get_bd_pins xlconcat_0/In1]
connect_bd_net [get_bd_pins axi_uartlite_0/interrupt] [get_bd_pins xlconcat_0/In2]
connect_bd_net [get_bd_pins xlconcat_0/dout] [get_bd_pins processing_system7_0/IRQ_F2P]

# Addresses: Vivado's defaults for these IP, set explicitly.
foreach {pat off} {axi_gpio_0 0x41200000 axi_gpio_1 0x41210000 axi_timer_0 0x42800000 axi_uartlite_0 0x40600000} {
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
add_files -norecurse [file normalize $work/prj/zs_vendor.gen/sources_1/bd/design_1/hdl/design_1_wrapper.v]
generate_target all $bdfile
update_compile_order -fileset sources_1
set_property top design_1_wrapper [current_fileset]
launch_runs synth_1 -jobs 4
wait_on_run synth_1
open_run synth_1
write_hw_platform -fixed -force -file $out
puts "XSA written: $out"
exit
