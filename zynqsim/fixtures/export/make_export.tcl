# Produces the Vivado 2022.1 simulation exports under fixtures/export/ with Vivado in batch mode:
#
#   vivado -mode batch -source make_export.tcl -tclargs WORKDIR OUTDIR RTLDIR
#
# WORKDIR  scratch project directory (created; may be deleted afterwards; keep the path short)
# OUTDIR   where the results go (fixtures/export)
# RTLDIR   directory holding my_math_ip.v and cosim_top.sv (fixtures/rtl/math)
#
# Writes into OUTDIR:
#   rtl/       copies of the two sources, so that the exported lists can name them relatively
#   behav/xsim/            `export_simulation -simulator xsim` of the RTL design
#                          (my_math_ip + the cosim_top testbench): vlog.prj, elab.opt, xsim.ini,
#                          cosim_top.sh, glbl.v, file_info.txt
#   my_math_ip_funcsim.v   `write_verilog -mode funcsim` of the synthesised my_math_ip (UNISIM
#                          cells, no testbench)
#   postsynth/ the scripts `launch_simulation -mode post-synthesis -type functional -scripts_only`
#              wrote: cosim_top_vlog.prj (the funcsim netlist plus the testbench),
#              compile/elaborate/simulate.bat, xsim.ini
#
# Vivado writes the source paths of the lists relative to the export directory, from wherever
# the project was. They are rewritten here to `rtl/NAME` relative to each list, so that the
# committed folders resolve wherever the repository is checked out. Nothing else is changed.

# This machine's Vivado 2022.1 install lacks tps/tcl/tcl8.5/clock.tcl, so `clock format` (used by
# export_simulation for the script header) fails. Answer it ourselves; harmless elsewhere.
rename clock ::zs_clock_orig
proc clock {args} {
    if {[lindex $args 0] eq "format"} { return "Fri Oct 16 00:00:00 2026" }
    return [uplevel 1 [linsert $args 0 ::zs_clock_orig]]
}

set work [file normalize [lindex $argv 0]]
set out  [file normalize [lindex $argv 1]]
set rtl  [file normalize [lindex $argv 2]]

file mkdir $out/rtl
file copy -force $rtl/my_math_ip.v $out/rtl/my_math_ip.v
file copy -force $rtl/cosim_top.sv $out/rtl/cosim_top.sv

create_project -force zs_exp $work/prj -part xc7z020clg400-1
add_files -norecurse $out/rtl/my_math_ip.v
set_property top my_math_ip [current_fileset]
add_files -fileset sim_1 -norecurse $out/rtl/cosim_top.sv
set_property top cosim_top [get_filesets sim_1]
update_compile_order -fileset sources_1
update_compile_order -fileset sim_1

# 1. behavioural export
file mkdir $out/behav
export_simulation -simulator xsim -directory $out/behav -force

# 2. synthesis and the functional netlist
launch_runs synth_1 -jobs 4
wait_on_run synth_1
open_run synth_1 -name synth_1
write_verilog -mode funcsim -force $out/my_math_ip_funcsim.v

# 3. scripts for a post-synthesis functional simulation
launch_simulation -mode post-synthesis -type functional -scripts_only
set simdir [file normalize $work/prj/zs_exp.sim/sim_1/synth/func/xsim]
file mkdir $out/postsynth
foreach f [glob -nocomplain -directory $simdir *] {
    if {[file isfile $f] && [file tail $f] ne "simulate.log"} { file copy -force $f $out/postsynth/ }
}

# 4. make the source paths of the lists independent of where the project was
proc fix_paths {dir prefix} {
    foreach f [concat [glob -nocomplain -directory $dir *.prj] [glob -nocomplain -directory $dir file_info.txt]] {
        set fh [open $f r]; set text [read $fh]; close $fh
        regsub -all {[^\s",]*/rtl/([A-Za-z0-9_.]+)} $text "$prefix/rtl/\\1" text
        set fh [open $f w]; puts -nonewline $fh $text; close $fh
    }
}
fix_paths $out/behav/xsim ../..
fix_paths $out/postsynth ..

puts "EXPORT DONE: $out"
exit
