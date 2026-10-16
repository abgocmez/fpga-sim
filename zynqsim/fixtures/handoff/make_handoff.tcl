# Builds the P3 exit-demonstration design with Vivado 2022.1 in batch mode and writes BOTH
# hand-offs of that one block design:
#
#   vivado -mode batch -source make_handoff.tcl -tclargs WORKDIR OUTDIR RTLDIR
#
# WORKDIR  scratch project directory (created, deleted by the caller afterwards). Keep the path
#          SHORT and run this script from a short path such as C:\zsh\make_handoff.tcl: from a
#          deep path (a `.claude\worktrees\...` directory) Vivado's block-design init fails
#          with "couldn't read file utils_dbg.tcl / clkrst.tcl" (see fixtures/xsa/make_xsa.tcl).
# OUTDIR   fixtures/handoff: receives design.xsa, export/ and rtl/ (copies of the two sources, so
#          that the exported lists can name them relatively)
# RTLDIR   a scratch directory holding my_math_ip.v (copy of fixtures/rtl/math/my_math_ip.v) and
#          cosim_top.sv, the testbench `zynqsim init --ip my_math_ip.v --out T` generates
#          (T/tb/cosim_top.sv; the hand-written fixtures/rtl/math/cosim_top.sv would also run,
#          but lacks the "pl finished" trace event, so the trace digest would differ: ADR-019).
#          It must not be OUTDIR/rtl itself (the script copies from it into OUTDIR/rtl).
#
# The block design `design_1` (Zybo Z7-20, xc7z020clg400-1):
#   processing_system7_0   FCLK_CLK0 = 100 MHz, M_AXI_GP0, fabric interrupts
#   u_math                 the user's RTL `my_math_ip` as a MODULE REFERENCE, 0x43C00000 (4 KB)
#   axi_gpio_0             4 outputs (`leds_4bits`), 0x41200000 (64 KB)
#   axi_timer_0            32 bit, interrupt, 0x42800000 (64 KB)
#   axi_uartlite_0         115200 8N1, interrupt, 0x40600000 (64 KB)
#   xlconcat_0             timer -> IRQ_F2P[0], uartlite -> IRQ_F2P[1]
#
# Outputs:
#   design.xsa   `write_hw_platform -fixed` (the hardware hand-off: HWH, ps7_init, ...)
#   export/xsim  `export_simulation -simulator xsim` of the same project (the simulation hand-off:
#                vlog.prj, vhdl.prj, elab.opt, xsim.ini, <top>.sh, glbl.v, ...). Its sim_1 set holds
#                the user RTL and the ZynqSim testbench; Vivado lists only what the testbench top
#                reaches (the block design's Xilinx IP is NOT in it: its models come from the XSA).
#
# The only edits to Vivado's export (step 4): the sources' paths are made relative to the export
# (`../../rtl/NAME`), the Vivado install root becomes $XILINX_VIVADO, and the --include entries
# that point into the scratch project are dropped. Nothing else is changed.
#
set work [file normalize [lindex $argv 0]]
set out  [file normalize [lindex $argv 1]]
set rtl  [file normalize [lindex $argv 2]]

create_project -force zs_handoff $work/prj -part xc7z020clg400-1
if {[catch {set_property board_part digilentinc.com:zybo-z7-20:part0:1.2 [current_project]} msg]} {
    puts "NOTE: board part not available ($msg); using the bare part"
}
file mkdir $out/rtl
file copy -force $rtl/my_math_ip.v $out/rtl/my_math_ip.v
file copy -force $rtl/cosim_top.sv $out/rtl/cosim_top.sv
add_files -norecurse $out/rtl/my_math_ip.v
update_compile_order -fileset sources_1

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

set math [create_bd_cell -type module -reference my_math_ip u_math]

set led [create_bd_cell -type ip -vlnv xilinx.com:ip:axi_gpio:2.0 axi_gpio_0]
set_property -dict [list CONFIG.C_GPIO_WIDTH {4} CONFIG.C_ALL_OUTPUTS {1}] $led
set tmr [create_bd_cell -type ip -vlnv xilinx.com:ip:axi_timer:2.0 axi_timer_0]
set urt [create_bd_cell -type ip -vlnv xilinx.com:ip:axi_uartlite:2.0 axi_uartlite_0]
set_property -dict [list CONFIG.C_BAUDRATE {115200}] $urt

make_bd_intf_pins_external [get_bd_intf_pins axi_gpio_0/GPIO]
set_property name leds_4bits [get_bd_intf_ports GPIO_0]
make_bd_intf_pins_external [get_bd_intf_pins axi_uartlite_0/UART]

set first 1
foreach s {u_math axi_gpio_0 axi_timer_0 axi_uartlite_0} {
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
set_property -dict [list CONFIG.NUM_PORTS {2}] $cat
connect_bd_net [get_bd_pins axi_timer_0/interrupt] [get_bd_pins xlconcat_0/In0]
connect_bd_net [get_bd_pins axi_uartlite_0/interrupt] [get_bd_pins xlconcat_0/In1]
connect_bd_net [get_bd_pins xlconcat_0/dout] [get_bd_pins processing_system7_0/IRQ_F2P]

# Addresses: where the firmware (and the Zybo examples) expect them.
foreach {pat off rng} {u_math 0x43C00000 4K axi_gpio_0 0x41200000 64K axi_timer_0 0x42800000 64K axi_uartlite_0 0x40600000 64K} {
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
add_files -norecurse [file normalize $work/prj/zs_handoff.gen/sources_1/bd/design_1/hdl/design_1_wrapper.v]
generate_target all $bdfile
update_compile_order -fileset sources_1
set_property top design_1_wrapper [current_fileset]

# The ZynqSim testbench (stands in for the PS7 towards the user RTL) goes into the simulation set.
add_files -fileset sim_1 -norecurse $out/rtl/cosim_top.sv
update_compile_order -fileset sim_1

# A pre-synthesis write_hw_platform leaves the handoff out, so synthesise once.
launch_runs synth_1 -jobs 4
wait_on_run synth_1
open_run synth_1
file mkdir $out
write_hw_platform -fixed -force -file $out/design.xsa
puts "XSA written: $out/design.xsa"

# This machine's Vivado 2022.1 install lacks tps/tcl/tcl8.5/clock.tcl, so `clock format` (used by
# export_simulation for the script header) fails. Answer it ourselves; harmless elsewhere. It is
# defined only now: renaming `clock` earlier makes the block-design feature fail to load.
rename clock ::zs_clock_orig
proc clock {args} {
    if {[lindex $args 0] eq "format"} { return "Fri Oct 16 00:00:00 2026" }
    return [uplevel 1 [linsert $args 0 ::zs_clock_orig]]
}

file mkdir $out/export
export_simulation -simulator xsim -directory $out/export -force

# 4. make the lists independent of where the project and Vivado were
if {[info exists ::env(XILINX_VIVADO)]} {
    set root [file normalize $::env(XILINX_VIVADO)]
} else {
    set root [file normalize [file join [file dirname [info nameofexecutable]] .. .. .. ..]]
}
puts "Vivado root: $root"
foreach f [concat [glob -nocomplain -directory $out/export/xsim *.prj] [glob -nocomplain -directory $out/export/xsim file_info.txt]] {
    set fh [open $f r]; set text [read $fh]; close $fh
    regsub -all {[^\s",]*/rtl/([A-Za-z0-9_.]+)} $text {../../rtl/\1} text
    regsub -all { --include "[^"]*/w/prj/[^"]*"} $text {} text
    regsub -all {,?incdir="[^"]*/w/prj/[^"]*"} $text {} text
    set text [string map [list $root {$XILINX_VIVADO}] $text]
    set fh [open $f w]; puts -nonewline $fh $text; close $fh
}
puts "EXPORT DONE: $out/export"
exit
