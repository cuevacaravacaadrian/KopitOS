//! # GDT y TSS
//!
//! En modo largo la segmentación casi no existe, pero la GDT sigue siendo
//! obligatoria para tres cosas:
//!
//! 1. los selectores de código/datos del núcleo y del usuario (el orden
//!    `núcleo código, núcleo datos, usuario datos, usuario código` es el que
//!    exige `syscall/sysret`, que usará la fase 3);
//! 2. el **TSS**, que da a la CPU pilas conocidas para atender interrupciones:
//!    `RSP0` (al entrar desde el usuario) y la **IST**, pilas de emergencia que
//!    se usan *siempre* para ciertas excepciones. Así un desbordamiento de la
//!    pila del núcleo produce un doble fallo que se puede diagnosticar, en vez
//!    de un triple fallo que reinicia la máquina sin decir nada.

use crate::sync::RacyCell;
use core::arch::asm;

pub const KERNEL_CODE: u16 = 0x08;
pub const KERNEL_DATA: u16 = 0x10;
pub const USER_DATA: u16 = 0x18 | 3;
pub const USER_CODE: u16 = 0x20 | 3;
pub const TSS_SELECTOR: u16 = 0x28;

/// Índices IST (1..=7) para las excepciones críticas.
pub const IST_DOUBLE_FAULT: u8 = 1;
pub const IST_NMI: u8 = 2;
pub const IST_MACHINE_CHECK: u8 = 3;

const IST_STACK_SIZE: usize = 16 * 1024;

#[repr(C, align(16))]
struct Stack([u8; IST_STACK_SIZE]);

static IST_STACKS: RacyCell<[Stack; 3]> = RacyCell::new([const { Stack([0; IST_STACK_SIZE]) }; 3]);

#[repr(C, packed(4))]
struct Tss {
    _reserved0: u32,
    rsp: [u64; 3],
    _reserved1: u64,
    ist: [u64; 7],
    _reserved2: u64,
    _reserved3: u16,
    iomap_base: u16,
}

static TSS: RacyCell<Tss> = RacyCell::new(Tss {
    _reserved0: 0,
    rsp: [0; 3],
    _reserved1: 0,
    ist: [0; 7],
    _reserved2: 0,
    _reserved3: 0,
    iomap_base: core::mem::size_of::<Tss>() as u16,
});

static GDT: RacyCell<[u64; 7]> = RacyCell::new([0; 7]);

#[repr(C, packed)]
struct DescriptorPointer {
    limit: u16,
    base: u64,
}

pub fn init() {
    // SAFETY: arranque, un solo hilo; estructuras estáticas que viven siempre.
    unsafe {
        let stacks = &mut *IST_STACKS.get();
        let top = |s: &mut Stack| s.0.as_mut_ptr().add(IST_STACK_SIZE) as u64;
        let ist = [top(&mut stacks[0]), top(&mut stacks[1]), top(&mut stacks[2]), 0, 0, 0, 0];
        let tss = TSS.get();
        (*tss).ist = ist;
        (*tss).rsp = [crate::mm::layout::stack_top(), 0, 0];

        let tss_base = tss as u64;
        let tss_limit = (core::mem::size_of::<Tss>() - 1) as u64;
        let gdt = &mut *GDT.get();
        gdt[0] = 0;
        gdt[1] = 0x00AF_9A00_0000_FFFF; // código del núcleo (64 bits)
        gdt[2] = 0x00CF_9200_0000_FFFF; // datos del núcleo
        gdt[3] = 0x00CF_F200_0000_FFFF; // datos de usuario (DPL 3)
        gdt[4] = 0x00AF_FA00_0000_FFFF; // código de usuario (DPL 3)
        gdt[5] = (tss_limit & 0xFFFF)
            | ((tss_base & 0xFF_FFFF) << 16)
            | (0x89 << 40) // presente, TSS de 64 bits disponible
            | (((tss_limit >> 16) & 0xF) << 48)
            | (((tss_base >> 24) & 0xFF) << 56);
        gdt[6] = tss_base >> 32;

        let ptr = DescriptorPointer { limit: (core::mem::size_of::<[u64; 7]>() - 1) as u16, base: gdt.as_ptr() as u64 };
        asm!(
            "lgdt [{ptr}]",
            // Recargar CS con un retorno lejano (no hay `mov cs`).
            "push {cs}",
            "lea {tmp}, [rip + 2f]",
            "push {tmp}",
            "retfq",
            "2:",
            "mov ds, {data:x}",
            "mov es, {data:x}",
            "mov ss, {data:x}",
            "xor {tmp:e}, {tmp:e}",
            "mov fs, {tmp:x}",
            "mov gs, {tmp:x}",
            "ltr {tss:x}",
            ptr = in(reg) &ptr,
            cs = in(reg) KERNEL_CODE as u64,
            data = in(reg) KERNEL_DATA as u64,
            tss = in(reg) TSS_SELECTOR,
            tmp = out(reg) _,
        );
    }
}

/// Rango de una pila IST (para reconocer en qué pila estamos).
pub fn ist_stack_range(index: usize) -> (u64, u64) {
    // SAFETY: sólo se toma la dirección.
    let base = unsafe { (*IST_STACKS.get())[index].0.as_ptr() as u64 };
    (base, base + IST_STACK_SIZE as u64)
}
