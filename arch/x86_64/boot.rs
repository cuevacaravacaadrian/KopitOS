//! # Entradas del núcleo en x86_64
//!
//! Tres formas de llegar a `kernel_main`, un único ELF:
//!
//! ```text
//!  KopitBoot (UEFI) ──────────────────────────────► _start            (64 bits, ya paginado)
//!  GRUB (Multiboot2) ─► _mb2_entry32 ─┐
//!                                     ├─► 32 bits → modo largo → mitad alta → kopit_boot32_entry64
//!  QEMU -kernel (PVH) ─► _pvh_entry32 ┘
//! ```
//!
//! ## El trampolín de 32 a 64 bits
//!
//! GRUB y PVH nos dejan en **modo protegido de 32 bits sin paginación**. Para
//! llegar al modo largo hay que, en este orden:
//!
//! 1. comprobar que la CPU tiene CPUID y modo largo (si no, mensaje y parada);
//! 2. construir tablas de páginas de 4 niveles (aquí, en `.bss`): identidad
//!    0–4 GiB (para seguir ejecutando tras activar la paginación), el mismo
//!    rango en el HHDM y los 2 GiB altos para el núcleo — todo con páginas
//!    de 2 MiB, que caben en 7 páginas de tablas;
//! 3. CR4.PAE → CR3 → EFER.LME (+ NXE) → CR0.PG: ya estamos en modo
//!    "compatibilidad";
//! 4. cargar una GDT con un segmento de código de 64 bits y saltar a él con
//!    un salto lejano: ya estamos en **modo largo**;
//! 5. saltar a la dirección virtual alta y llamar a Rust.
//!
//! El núcleo está enlazado en `0xFFFFFFFF80200000` pero GRUB/QEMU lo cargan
//! en la física `0x200000` (la LMA del guion del enlazador). Por eso el
//! código de 32 bits usa siempre `símbolo - KERNEL_VMA`: su dirección física.

use core::arch::global_asm;

global_asm!(
    r#"
    .set KERNEL_VMA, 0xFFFFFFFF80000000
    .set MB2_HEADER_MAGIC, 0xE85250D6
    .set MB2_BOOT_MAGIC, 0x36D76289

/* ===================== Cabecera Multiboot2 ===================== */
    .section .multiboot2, "a"
    .balign 8
mb2_header:
    .long MB2_HEADER_MAGIC
    .long 0                                         /* i386: modo protegido de 32 bits */
    .long mb2_header_end - mb2_header
    .long 0x100000000 - (MB2_HEADER_MAGIC + (mb2_header_end - mb2_header))
    .balign 8                                       /* etiqueta 3: dirección de entrada */
    .short 3, 0
    .long 12
    .long _mb2_entry32 - KERNEL_VMA
    .balign 8                                       /* etiqueta 5: framebuffer preferido */
    .short 5, 1
    .long 20
    .long 1280, 800, 32
    .balign 8                                       /* fin */
    .short 0, 0
    .long 8
mb2_header_end:

/* ===================== Nota PVH (Xen/QEMU/Firecracker) ===================== */
    .section .note.Xen, "a", @note
    .balign 4
    .long 4                                         /* namesz: "Xen\0" */
    .long 8                                         /* descsz */
    .long 18                                        /* XEN_ELFNOTE_PHYS32_ENTRY */
    .asciz "Xen"
    .balign 4
    .quad _pvh_entry32 - KERNEL_VMA

/* ===================== Código de 32 bits ===================== */
    .section .boot.text, "ax"
    .code32
    .global _mb2_entry32
_mb2_entry32:
    cli
    cld
    cmpl $MB2_BOOT_MAGIC, %eax
    jne .Lbad_magic
    movl $1, %esi                                   /* protocolo 1 = Multiboot2 */
    movl %ebx, %edi                                 /* estructura de información */
    jmp .Lboot32

    .global _pvh_entry32
_pvh_entry32:
    cli
    cld
    movl $2, %esi                                   /* protocolo 2 = PVH */
    movl %ebx, %edi                                 /* hvm_start_info */
    jmp .Lboot32

.Lbad_magic:
    movl $(msg_bad_magic - KERNEL_VMA), %esi
    jmp .Lfatal32

.Lboot32:
    movl $(boot_stack_top - KERNEL_VMA), %esp
    movl %esi, (boot32_protocol - KERNEL_VMA)
    movl %edi, (boot32_info - KERNEL_VMA)

    /* ¿Existe CPUID? (el bit ID de EFLAGS se puede cambiar) */
    pushfl
    popl %eax
    movl %eax, %ecx
    xorl $0x00200000, %eax
    pushl %eax
    popfl
    pushfl
    popl %eax
    pushl %ecx
    popfl
    xorl %ecx, %eax
    jz .Lno_long_mode

    /* ¿Modo largo? (CPUID 0x80000001, EDX bit 29) */
    movl $0x80000000, %eax
    cpuid
    cmpl $0x80000001, %eax
    jb .Lno_long_mode
    movl $0x80000001, %eax
    cpuid
    testl $0x20000000, %edx
    jz .Lno_long_mode
    movl %edx, %ebp                                 /* bit 20 = NX, lo usamos abajo */

    /* Tablas de páginas de arranque: 7 páginas a cero */
    movl $(boot_pml4 - KERNEL_VMA), %edi
    xorl %eax, %eax
    movl $(7 * 1024), %ecx
    rep stosl

    /* PML4[0] (identidad) y PML4[256] (HHDM) → pdpt_low; PML4[511] → pdpt_high */
    movl $(boot_pdpt_low - KERNEL_VMA + 3), %eax
    movl %eax, (boot_pml4 - KERNEL_VMA)
    movl %eax, (boot_pml4 - KERNEL_VMA + 256 * 8)
    movl $(boot_pdpt_high - KERNEL_VMA + 3), %eax
    movl %eax, (boot_pml4 - KERNEL_VMA + 511 * 8)

    /* pdpt_low[0..4] → boot_pd[0..4] (4 GiB) */
    movl $(boot_pd - KERNEL_VMA + 3), %eax
    movl $(boot_pdpt_low - KERNEL_VMA), %edi
    movl $4, %ecx
.Lfill_pdpt:
    movl %eax, (%edi)
    addl $4096, %eax
    addl $8, %edi
    decl %ecx
    jnz .Lfill_pdpt

    /* pdpt_high[510..512] → boot_pd[0..2]: los 2 GiB altos ven la física 0..2 GiB */
    movl $(boot_pd - KERNEL_VMA + 3), %eax
    movl %eax, (boot_pdpt_high - KERNEL_VMA + 510 * 8)
    addl $4096, %eax
    movl %eax, (boot_pdpt_high - KERNEL_VMA + 511 * 8)

    /* boot_pd: 2048 páginas de 2 MiB, presentes + escribibles + PS */
    movl $(boot_pd - KERNEL_VMA), %edi
    movl $0x83, %eax
    movl $2048, %ecx
.Lfill_pd:
    movl %eax, (%edi)
    movl $0, 4(%edi)
    addl $0x200000, %eax
    addl $8, %edi
    decl %ecx
    jnz .Lfill_pd

    /* CR4.PAE → CR3 → EFER.LME (+NXE) → CR0.PG|WP */
    movl %cr4, %eax
    orl $0x20, %eax
    movl %eax, %cr4
    movl $(boot_pml4 - KERNEL_VMA), %eax
    movl %eax, %cr3
    movl $0xC0000080, %ecx
    rdmsr
    orl $0x100, %eax
    testl $0x00100000, %ebp
    jz .Lno_nx
    orl $0x800, %eax
.Lno_nx:
    wrmsr
    movl %cr0, %eax
    orl $0x80010001, %eax
    movl %eax, %cr0

    /* GDT de 64 bits y salto lejano: ¡modo largo! */
    lgdt (boot_gdtr - KERNEL_VMA)
    ljmp $0x08, $(.Lboot64_low - KERNEL_VMA)

.Lno_long_mode:
    movl $(msg_no_long_mode - KERNEL_VMA), %esi
.Lfatal32:
    /* Mensaje por COM1 y en la memoria de texto VGA (blanco sobre rojo). */
    movl $0xB8000, %edi
.Lfatal_loop:
    movb (%esi), %bl
    testb %bl, %bl
    jz .Lfatal_halt
    movw $0x3FD, %dx
.Lfatal_wait:
    inb %dx, %al
    testb $0x20, %al
    jz .Lfatal_wait
    movw $0x3F8, %dx
    movb %bl, %al
    outb %al, %dx
    movb %bl, (%edi)
    movb $0x4F, 1(%edi)
    addl $2, %edi
    incl %esi
    jmp .Lfatal_loop
.Lfatal_halt:
    cli
    hlt
    jmp .Lfatal_halt

/* ===================== Código de 64 bits (aún en la identidad) ===================== */
    .code64
.Lboot64_low:
    movw $0x10, %ax
    movw %ax, %ds
    movw %ax, %es
    movw %ax, %ss
    xorl %eax, %eax
    movw %ax, %fs
    movw %ax, %gs
    movabsq $.Lboot64_high, %rax
    jmpq *%rax
.Lboot64_high:
    leaq boot_stack_top(%rip), %rsp
    xorl %ebp, %ebp
    movl boot32_protocol(%rip), %edi
    movl boot32_info(%rip), %esi
    call kopit_boot32_entry64
    ud2

/* ===================== Entrada desde KopitBoot (UEFI) ===================== */
    .section .text._start, "ax"
    .global _start
_start:
    cli
    cld
    leaq boot_stack_top(%rip), %rsp                 /* pila propia: la del cargador se recupera */
    xorl %ebp, %ebp
    call kopit_entry_kbp                            /* rdi = BootInfo* (System V) */
    ud2

/* ===================== Datos de arranque ===================== */
    .section .rodata.boot, "a"
    .balign 16
boot_gdt:
    .quad 0
    .quad 0x00AF9A000000FFFF                        /* 0x08: código de 64 bits */
    .quad 0x00CF92000000FFFF                        /* 0x10: datos */
boot_gdt_end:
boot_gdtr:
    .short boot_gdt_end - boot_gdt - 1
    .long boot_gdt - KERNEL_VMA
msg_no_long_mode:
    .asciz "KopitOS necesita una CPU de 64 bits (x86_64). Arranque detenido."
msg_bad_magic:
    .asciz "KopitOS: firma Multiboot2 incorrecta. Arranque detenido."

    .section .bss.boot, "aw", @nobits
    .balign 4096
boot_pml4:
    .skip 4096
boot_pdpt_low:
    .skip 4096
boot_pdpt_high:
    .skip 4096
boot_pd:
    .skip 4096 * 4
    .global __boot_stack_guard
    .global __boot_stack_bottom
    .global __boot_stack_top
__boot_stack_guard:                                 /* sin mapear: un desbordamiento → doble fallo */
    .skip 4096
__boot_stack_bottom:
    .skip 65536
__boot_stack_top:
boot_stack_top:
    .balign 8
boot32_protocol:
    .skip 4
boot32_info:
    .skip 4

    .section .text
"#,
    options(att_syntax)
);
