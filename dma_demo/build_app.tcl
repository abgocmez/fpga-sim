# build_app.tcl - build both Cortex-A9 images from the Zybo XSA:
#   out/dma_test.elf  (CPU0, dma_demo/sw)       out/dma_cpu1.elf (CPU1, dma_demo/sw_cpu1)
# Usage: xsct dma_demo/build_app.tcl      (run setup_bd.tcl once first for out/system_wrapper.xsa)
set script_dir [file dirname [file normalize [info script]]]
set root       [file dirname $script_dir]
set xsa        [file join $root out system_wrapper.xsa]
set ws         [file join $root build vitis_ws_dma]

if {![file exists $xsa]} { error "missing $xsa - run setup_bd.tcl first" }

file delete -force $ws
file mkdir $ws
setws $ws

platform create -name zybo_plat -hw $xsa -proc ps7_cortexa9_0 -os standalone
# second domain for the other Cortex-A9
domain create -name standalone_a9_1 -os standalone -proc ps7_cortexa9_1
platform generate

# ---- CPU0 ----
app create -name dma_test -platform zybo_plat -domain standalone_domain \
    -template {Empty Application(C)}
importsources -name dma_test -path [file join $script_dir sw]
app build -name dma_test

# ---- CPU1: own image and own DDR window (0x04000000..) ----
app create -name dma_cpu1 -platform zybo_plat -domain standalone_a9_1 \
    -template {Empty Application(C)}
set src1 [file join $ws dma_cpu1 src]
set lds  [file join $src1 lscript.ld]
set fh [open $lds r]; set txt [read $fh]; close $fh
regsub {ps7_ddr_0 : ORIGIN = 0x[0-9A-Fa-f]+, LENGTH = 0x[0-9A-Fa-f]+} $txt \
    {ps7_ddr_0 : ORIGIN = 0x04000000, LENGTH = 0x01000000} txt
set fh [open $lds w]; puts -nonewline $fh $txt; close $fh
file copy -force [file join $script_dir sw mailbox.h] $src1
file copy -force [file join $script_dir sw_cpu1 main.c] $src1
app build -name dma_cpu1

foreach {app elf} {dma_test dma_test.elf dma_cpu1 dma_cpu1.elf} {
    set f [file join $ws $app Debug $elf]
    # the link step can finish just after `app build` returns
    for {set n 0} {$n < 120 && ![file exists $f]} {incr n} { after 500 }
    if {![file exists $f]} { error "build did not produce $f" }
    file copy -force $f [file join $root out $elf]
}
puts "OK: [file join $root out dma_test.elf] [file join $root out dma_cpu1.elf]"
