/*
 * Guion del enlazador del núcleo de KopitOS (x86_64)
 *
 *  - VMA (dirección virtual):  0xFFFFFFFF80200000…  (los 2 GiB altos: -mcmodel=kernel)
 *  - LMA (dirección física):   0x200000…            (donde lo dejan GRUB y QEMU/PVH)
 *    KopitBoot ignora la LMA: reubica la imagen en cualquier física libre.
 *
 *  Tres segmentos PT_LOAD alineados a 4 KiB, uno por permiso (W^X):
 *    text   R-X   cabecera Multiboot2 (primeros 32 KiB del archivo) + código
 *    rodata R--   constantes + nota PVH
 *    data   RW-   datos + .bss (tablas de arranque, pila con página de guarda)
 */

OUTPUT_FORMAT("elf64-x86-64")
OUTPUT_ARCH(i386:x86-64)
ENTRY(_start)

KERNEL_VMA  = 0xFFFFFFFF80000000;
KERNEL_PHYS = 0x200000;

PHDRS
{
    text   PT_LOAD FLAGS(5);   /* R-X */
    rodata PT_LOAD FLAGS(4);   /* R-- */
    data   PT_LOAD FLAGS(6);   /* RW- */
    note   PT_NOTE FLAGS(4);
}

SECTIONS
{
    . = KERNEL_VMA + KERNEL_PHYS;
    __kernel_start = .;

    .text : AT(ADDR(.text) - KERNEL_VMA) {
        __text_start = .;
        KEEP(*(.multiboot2))
        *(.boot.text)
        *(.text._start)
        *(.text .text.*)
        . = ALIGN(4096);
        __text_end = .;
    } :text

    .rodata : AT(ADDR(.rodata) - KERNEL_VMA) {
        __rodata_start = .;
        *(.rodata .rodata.*)
    } :rodata

    .note.Xen : AT(ADDR(.note.Xen) - KERNEL_VMA) {
        KEEP(*(.note.Xen))
    } :rodata :note

    .eh_frame_hdr : AT(ADDR(.eh_frame_hdr) - KERNEL_VMA) { *(.eh_frame_hdr) } :rodata
    .eh_frame : AT(ADDR(.eh_frame) - KERNEL_VMA) { KEEP(*(.eh_frame)) } :rodata

    . = ALIGN(4096);
    __rodata_end = .;

    .data : AT(ADDR(.data) - KERNEL_VMA) {
        __data_start = .;
        *(.data .data.*)
        *(.got .got.*)
    } :data

    .bss (NOLOAD) : AT(ADDR(.bss) - KERNEL_VMA) {
        . = ALIGN(4096);
        __bss_start = .;
        *(.bss.boot)
        *(.bss .bss.*)
        *(COMMON)
        . = ALIGN(4096);
        __bss_end = .;
    } :data

    __kernel_end = .;

    /DISCARD/ : {
        *(.comment)
        *(.note.GNU-stack)
        *(.note.gnu.*)
    }
}
