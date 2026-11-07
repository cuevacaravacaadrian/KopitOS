/*
 * Guion del enlazador del núcleo de KopitOS (RISC-V 64, fase 1)
 *
 * OpenSBI ocupa 0x80000000-0x801FFFFF; el núcleo empieza en 0x80200000, que
 * es donde QEMU `virt` (y la mayoría de placas con OpenSBI) saltan al modo S.
 * MMU en identidad: dirección virtual = física.
 */

OUTPUT_ARCH(riscv)
ENTRY(_start)

PHDRS
{
    text   PT_LOAD FLAGS(5);
    rodata PT_LOAD FLAGS(4);
    data   PT_LOAD FLAGS(6);
}

SECTIONS
{
    . = 0x80200000;
    __kernel_start = .;

    .text : {
        __text_start = .;
        *(.text._start)
        *(.text .text.*)
        . = ALIGN(4096);
        __text_end = .;
    } :text

    .rodata : {
        __rodata_start = .;
        *(.rodata .rodata.*)
        *(.srodata .srodata.*)
        *(.eh_frame*)
        . = ALIGN(4096);
        __rodata_end = .;
    } :rodata

    .data : {
        __data_start = .;
        *(.data .data.*)
        *(.sdata .sdata.*)
        *(.got .got.*)
    } :data

    .bss (NOLOAD) : {
        . = ALIGN(4096);
        __bss_start = .;
        *(.bss.boot)
        *(.sbss .sbss.*)
        *(.bss .bss.*)
        *(COMMON)
        . = ALIGN(4096);
        __bss_end = .;
    } :data

    __kernel_end = .;

    /DISCARD/ : { *(.comment) *(.note*) *(.riscv.attributes) }
}
