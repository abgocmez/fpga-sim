/* Zynq-7000 DDR at 0x0; the simulator loads the ELF and starts core 0 here. */
ENTRY(_start)
MEMORY { RAM : ORIGIN = 0x00100000, LENGTH = 0x00400000 }
SECTIONS
{
    .text : { KEEP(*(.text._start)) *(.text .text.*) } > RAM
    .rodata : { *(.rodata .rodata.*) } > RAM
    .data : { *(.data .data.*) } > RAM
    .bss (NOLOAD) : { __bss_start = .; *(.bss .bss.*) *(COMMON) __bss_end = .; } > RAM
    /DISCARD/ : { *(.ARM.exidx*) *(.ARM.extab*) *(.comment) }
}
_stack_top = ORIGIN(RAM) + LENGTH(RAM);
