# build_app.tcl - build sw/main.c into an ELF for ps7_cortexa9_0 from the XSA.
# Usage: xsct build_app.tcl
set script_dir [file dirname [file normalize [info script]]]
set xsa        [file join $script_dir out system_wrapper.xsa]
set ws         [file join $script_dir build vitis_ws]

if {![file exists $xsa]} { error "missing $xsa - run setup_bd.tcl first" }

file delete -force $ws
file mkdir $ws
setws $ws

platform create -name zybo_plat -hw $xsa -proc ps7_cortexa9_0 -os standalone
platform generate

app create -name math_test -platform zybo_plat -domain standalone_domain \
    -template {Empty Application(C)}
importsources -name math_test -path [file join $script_dir sw]
app build -name math_test

set elf [file join $ws math_test Debug math_test.elf]
if {![file exists $elf]} { error "build did not produce $elf" }
file copy -force $elf [file join $script_dir out math_test.elf]
puts "OK: [file join $script_dir out math_test.elf]"
