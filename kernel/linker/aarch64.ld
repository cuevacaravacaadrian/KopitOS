/*
 * Guion del enlazador del núcleo de KopitOS (ARM64, fase 1)
 *
 * QEMU `virt` deja el árbol de dispositivos al principio de la RAM
 * (0x40000000); el núcleo empieza 2 MiB después. MMU en identidad.
 */

OUTPUT_ARCH(aarch64)
ENTRY(_start)

PHDRS
{
    text   PT_LOAD FLAGS(5);
    rodata PT_LOAD FLAGS(4);
    data   PT_LOAD FLAGS(6);
}

SECTIONS
{
    . = 0x40200000;
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
        *(.eh_frame*)
        . = ALIGN(4096);
        __rodata_end = .;
    } :rodata

    .data : {
        __data_start = .;
        *(.data .data.*)
        *(.got .got.*)
    } :data

    .bss (NOLOAD) : {
        . = ALIGN(4096);
        __bss_start = .;
        *(.bss.boot)
        *(.bss .bss.*)
        *(COMMON)
        . = ALIGN(4096);
        __bss_end = .;
    } :data

    __kernel_end = .;

    /DISCARD/ : { *(.comment) *(.note*) }
}
