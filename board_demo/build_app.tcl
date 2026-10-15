# build_app.tcl - build board_demo/sw/main.c into out/board_test.elf (reuses the Zybo XSA).
# Usage: xsct board_demo/build_app.tcl      (run setup_bd.tcl once first for out/system_wrapper.xsa)
set script_dir [file dirname [file normalize [info script]]]
set root       [file dirname $script_dir]
set xsa        [file join $root out system_wrapper.xsa]
set ws         [file join $root build vitis_ws_board]

if {![file exists $xsa]} { error "missing $xsa - run setup_bd.tcl first" }

file delete -force $ws
file mkdir $ws
setws $ws

platform create -name zybo_plat -hw $xsa -proc ps7_cortexa9_0 -os standalone
platform generate

app create -name board_test -platform zybo_plat -domain standalone_domain \
    -template {Empty Application(C)}
importsources -name board_test -path [file join $script_dir sw]
app build -name board_test

set f [file join $ws board_test Debug board_test.elf]
for {set n 0} {$n < 120 && ![file exists $f]} {incr n} { after 500 }
if {![file exists $f]} { error "build did not produce $f" }
file copy -force $f [file join $root out board_test.elf]
puts "OK: [file join $root out board_test.elf]"
