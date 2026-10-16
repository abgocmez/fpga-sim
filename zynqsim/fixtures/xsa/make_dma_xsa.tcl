# Builds fixtures/xsa/zs_dma_loopback.xsa with Vivado 2022.1 in batch mode.
#
#   vivado -mode batch -source make_dma_xsa.tcl -tclargs WORKDIR OUT.xsa [fifo]
#
# WORKDIR  scratch project directory (created, may be deleted afterwards). Keep the path SHORT
#          and run the script from a short path such as C:\zsd2\make_dma_xsa.tcl (see make_xsa.tcl).
# OUT.xsa  the hardware platform to write
# fifo     put an AXI4-Stream Data FIFO (TLAST enabled) between M_AXIS_MM2S and S_AXIS_S2MM;
#          without it the two stream interfaces are connected directly, as in Xilinx's
#          axi_dma_loopback example design.
#
# The block design (examples/dma-loopback): Zynq PS7 (Zybo Z7-20 board preset), FCLK_CLK0 =
# 100 MHz, M_AXI_GP0 for the control registers and S_AXI_HP0 for the DMA's memory masters:
#   axi_dma_0   0x40400000  simple (direct register) mode, no scatter-gather, 23-bit LENGTH,
#                           MM2S -> S2MM loopback, mm2s_introut -> IRQ_F2P[0], s2mm_introut -> [1]
# No implementation is run; a small synthesis makes `write_hw_platform` include the HWH.

set work [file normalize [lindex $argv 0]]
set out  [file normalize [lindex $argv 1]]
set with_fifo [expr {[lindex $argv 2] eq "fifo"}]

create_project -force zs_dma $work/prj -part xc7z020clg400-1
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
    CONFIG.PCW_USE_S_AXI_HP0 {1} \
    CONFIG.PCW_USE_FABRIC_INTERRUPT {1} \
    CONFIG.PCW_IRQ_F2P_INTR {1} \
    CONFIG.PCW_EN_CLK0_PORT {1} \
    CONFIG.PCW_FPGA0_PERIPHERAL_FREQMHZ {100}] $ps

set dma [create_bd_cell -type ip -vlnv xilinx.com:ip:axi_dma:7.1 axi_dma_0]
set_property -dict [list \
    CONFIG.c_include_sg {0} \
    CONFIG.c_sg_include_stscntrl_strm {0} \
    CONFIG.c_sg_length_width {23}] $dma

# Control registers: M_AXI_GP0 -> S_AXI_LITE.
apply_bd_automation -rule xilinx.com:bd_rule:axi4 \
    -config {Clk_master {/processing_system7_0/FCLK_CLK0 (100 MHz)} Clk_slave {Auto} Clk_xbar {Auto} Master {/processing_system7_0/M_AXI_GP0} Slave {/axi_dma_0/S_AXI_LITE} ddr_seg {Auto} intc_ip {New AXI Interconnect} master_apm {0}} \
    [get_bd_intf_pins axi_dma_0/S_AXI_LITE]
# Data: M_AXI_MM2S and M_AXI_S2MM -> S_AXI_HP0.
apply_bd_automation -rule xilinx.com:bd_rule:axi4 \
    -config {Clk_master {/processing_system7_0/FCLK_CLK0 (100 MHz)} Clk_slave {Auto} Clk_xbar {Auto} Master {/axi_dma_0/M_AXI_MM2S} Slave {/processing_system7_0/S_AXI_HP0} ddr_seg {Auto} intc_ip {New AXI Interconnect} master_apm {0}} \
    [get_bd_intf_pins processing_system7_0/S_AXI_HP0]
apply_bd_automation -rule xilinx.com:bd_rule:axi4 \
    -config {Clk_master {/processing_system7_0/FCLK_CLK0 (100 MHz)} Clk_slave {Auto} Clk_xbar {Auto} Master {/axi_dma_0/M_AXI_S2MM} Slave {/processing_system7_0/S_AXI_HP0} ddr_seg {Auto} intc_ip {Auto} master_apm {0}} \
    [get_bd_intf_pins axi_dma_0/M_AXI_S2MM]

# The stream loopback.
if {$with_fifo} {
    set fifo [create_bd_cell -type ip -vlnv xilinx.com:ip:axis_data_fifo:2.0 axis_data_fifo_0]
    set_property -dict [list CONFIG.HAS_TLAST {1} CONFIG.FIFO_DEPTH {2048}] $fifo
    connect_bd_intf_net [get_bd_intf_pins axi_dma_0/M_AXIS_MM2S] [get_bd_intf_pins axis_data_fifo_0/S_AXIS]
    connect_bd_intf_net [get_bd_intf_pins axis_data_fifo_0/M_AXIS] [get_bd_intf_pins axi_dma_0/S_AXIS_S2MM]
    connect_bd_net [get_bd_pins processing_system7_0/FCLK_CLK0] [get_bd_pins axis_data_fifo_0/s_axis_aclk]
    connect_bd_net [get_bd_pins rst_ps7_0_100M/peripheral_aresetn] [get_bd_pins axis_data_fifo_0/s_axis_aresetn]
} else {
    connect_bd_intf_net [get_bd_intf_pins axi_dma_0/M_AXIS_MM2S] [get_bd_intf_pins axi_dma_0/S_AXIS_S2MM]
}

set cat [create_bd_cell -type ip -vlnv xilinx.com:ip:xlconcat:2.1 xlconcat_0]
set_property -dict [list CONFIG.NUM_PORTS {2}] $cat
connect_bd_net [get_bd_pins axi_dma_0/mm2s_introut] [get_bd_pins xlconcat_0/In0]
connect_bd_net [get_bd_pins axi_dma_0/s2mm_introut] [get_bd_pins xlconcat_0/In1]
connect_bd_net [get_bd_pins xlconcat_0/dout] [get_bd_pins processing_system7_0/IRQ_F2P]

puts "SEGMENTS BEFORE:"
foreach s [get_bd_addr_segs] { puts "  $s offset=[get_property OFFSET $s] range=[get_property RANGE $s]" }
foreach s [get_bd_addr_segs -of_objects [get_bd_addr_spaces processing_system7_0/Data]] {
    if {[string match *axi_dma_0* $s]} {
        set_property offset 0x40400000 $s
        set_property range 64K $s
    }
}
puts "SEGMENTS AFTER:"
foreach s [get_bd_addr_segs] { puts "  $s offset=[get_property OFFSET $s] range=[get_property RANGE $s]" }

validate_bd_design
save_bd_design
set bdfile [get_files design_1.bd]
make_wrapper -files $bdfile -top
add_files -norecurse [file normalize $work/prj/zs_dma.gen/sources_1/bd/design_1/hdl/design_1_wrapper.v]
generate_target all $bdfile
update_compile_order -fileset sources_1
set_property top design_1_wrapper [current_fileset]
launch_runs synth_1 -jobs 4
wait_on_run synth_1
open_run synth_1
write_hw_platform -fixed -force -file $out
puts "XSA written: $out"
exit
