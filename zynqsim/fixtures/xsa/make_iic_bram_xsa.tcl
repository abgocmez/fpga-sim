# Builds fixtures/xsa/zs_iic_bram.xsa with Vivado 2022.1 in batch mode.
#
#   vivado -mode batch -source make_iic_bram_xsa.tcl -tclargs WORKDIR OUT.xsa
#
# WORKDIR  scratch project directory (created, may be deleted afterwards). Keep the path SHORT
#          and run the script from a short path such as C:\zsd2\make_iic_bram_xsa.tcl (see
#          make_xsa.tcl).
# OUT.xsa  the hardware platform to write
#
# The block design (examples/iic-bram-demo): Zynq PS7 (Zybo Z7-20 board preset), FCLK_CLK0 =
# 100 MHz, M_AXI_GP0 to
#   axi_iic_0       0x41600000  AXI IIC, external interface `iic_pmod_ja` (Pmod JA), interrupt -> IRQ_F2P[0]
#   axi_bram_ctrl_0 0x40000000  AXI BRAM Controller, 32 bit, no ECC, 8 KiB, with a Block Memory
#                               Generator behind it
# No implementation is run; a small synthesis makes `write_hw_platform` include the HWH.

set work [file normalize [lindex $argv 0]]
set out  [file normalize [lindex $argv 1]]

create_project -force zs_iicbram $work/prj -part xc7z020clg400-1
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

set iic [create_bd_cell -type ip -vlnv xilinx.com:ip:axi_iic:2.1 axi_iic_0]
make_bd_intf_pins_external [get_bd_intf_pins axi_iic_0/IIC]
set_property name iic_pmod_ja [get_bd_intf_ports IIC_0]

set bc [create_bd_cell -type ip -vlnv xilinx.com:ip:axi_bram_ctrl:4.1 axi_bram_ctrl_0]
set_property -dict [list CONFIG.SINGLE_PORT_BRAM {1} CONFIG.DATA_WIDTH {32} CONFIG.ECC_TYPE {0}] $bc

set first 1
foreach s {axi_iic_0 axi_bram_ctrl_0} {
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
# The block memory behind the controller.
apply_bd_automation -rule xilinx.com:bd_rule:bram_cntlr -config {BRAM "Auto"} [get_bd_intf_pins axi_bram_ctrl_0/BRAM_PORTA]

connect_bd_net [get_bd_pins axi_iic_0/iic2intc_irpt] [get_bd_pins processing_system7_0/IRQ_F2P]

foreach {pat off rng} {axi_iic_0 0x41600000 64K axi_bram_ctrl_0 0x40000000 8K} {
    foreach s [get_bd_addr_segs -of_objects [get_bd_addr_spaces processing_system7_0/Data]] {
        if {[string match *${pat}* $s]} {
            set_property offset $off $s
            set_property range $rng $s
        }
    }
}
puts "SEGMENTS:"
foreach s [get_bd_addr_segs] { puts "  $s offset=[get_property OFFSET $s] range=[get_property RANGE $s]" }

validate_bd_design
save_bd_design
set bdfile [get_files design_1.bd]
make_wrapper -files $bdfile -top
add_files -norecurse [file normalize $work/prj/zs_iicbram.gen/sources_1/bd/design_1/hdl/design_1_wrapper.v]
generate_target all $bdfile
update_compile_order -fileset sources_1
set_property top design_1_wrapper [current_fileset]
launch_runs synth_1 -jobs 4
wait_on_run synth_1
open_run synth_1
write_hw_platform -fixed -force -file $out
puts "XSA written: $out"
exit
