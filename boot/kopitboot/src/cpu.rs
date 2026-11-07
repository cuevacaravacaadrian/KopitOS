//! Instrucciones x86_64 que necesita el cargador.

use core::arch::asm;

pub fn rdtsc() -> u64 {
    let (lo, hi): (u32, u32);
    // SAFETY: rdtsc no tiene efectos secundarios.
    unsafe { asm!("rdtsc", out("eax") lo, out("edx") hi, options(nomem, nostack, preserves_flags)) };
    ((hi as u64) << 32) | lo as u64
}

/// CPUID. `rbx` lo reserva LLVM, así que se salva a mano.
pub fn cpuid(leaf: u32, sub: u32) -> (u32, u32, u32, u32) {
    let (eax, ebx, ecx, edx): (u32, u64, u32, u32);
    // SAFETY: cpuid sólo lee información del procesador.
    unsafe {
        asm!(
            "mov {tmp}, rbx",
            "cpuid",
            "xchg {tmp}, rbx",
            tmp = out(reg) ebx,
            inout("eax") leaf => eax,
            inout("ecx") sub => ecx,
            out("edx") edx,
            options(nostack, preserves_flags),
        )
    };
    (eax, ebx as u32, ecx, edx)
}

fn max_extended_leaf() -> u32 {
    cpuid(0x8000_0000, 0).0
}

/// Bit NX (no ejecutar) disponible.
pub fn has_nx() -> bool {
    max_extended_leaf() >= 0x8000_0001 && cpuid(0x8000_0001, 0).3 & (1 << 20) != 0
}

/// Páginas de 1 GiB disponibles.
pub fn has_gib_pages() -> bool {
    max_extended_leaf() >= 0x8000_0001 && cpuid(0x8000_0001, 0).3 & (1 << 26) != 0
}

pub fn rdmsr(msr: u32) -> u64 {
    let (lo, hi): (u32, u32);
    // SAFETY: sólo se usa con MSR arquitectónicos que existen en x86_64.
    unsafe { asm!("rdmsr", in("ecx") msr, out("eax") lo, out("edx") hi, options(nomem, nostack, preserves_flags)) };
    ((hi as u64) << 32) | lo as u64
}

pub fn wrmsr(msr: u32, value: u64) {
    // SAFETY: igual que `rdmsr`; el llamante elige un valor válido.
    unsafe {
        asm!("wrmsr", in("ecx") msr, in("eax") value as u32, in("edx") (value >> 32) as u32, options(nostack, preserves_flags))
    };
}

pub const IA32_EFER: u32 = 0xC000_0080;
pub const EFER_NXE: u64 = 1 << 11;

fn outb(port: u16, v: u8) {
    // SAFETY: E/S a la UART COM1.
    unsafe { asm!("out dx, al", in("dx") port, in("al") v, options(nomem, nostack, preserves_flags)) };
}

fn inb(port: u16) -> u8 {
    let v: u8;
    // SAFETY: E/S a la UART COM1.
    unsafe { asm!("in al, dx", in("dx") port, out("al") v, options(nomem, nostack, preserves_flags)) };
    v
}

/// Escribe en COM1 sin depender del firmware (para después de ExitBootServices).
pub fn serial_write(bytes: &[u8]) {
    for &b in bytes {
        if b == b'\n' {
            serial_byte(b'\r');
        }
        serial_byte(b);
    }
}

fn serial_byte(b: u8) {
    for _ in 0..100_000 {
        if inb(0x3FD) & 0x20 != 0 {
            break;
        }
        core::hint::spin_loop();
    }
    outb(0x3F8, b);
}

pub fn halt_forever() -> ! {
    loop {
        // SAFETY: detiene la CPU hasta la siguiente interrupción.
        unsafe { asm!("cli; hlt", options(nomem, nostack)) };
    }
}

/// Instala las tablas de páginas del núcleo y salta a su punto de entrada.
///
/// El código de esta función sigue en marcha justo después de cambiar CR3,
/// así que **debe** estar mapeado en las tablas nuevas: por eso el cargador
/// mapea la memoria física también en identidad (además de en el HHDM).
///
/// # Safety
/// `pml4` debe mapear este código, `stack_top` y `entry`; `boot_info` debe ser
/// una dirección válida en las tablas nuevas.
pub unsafe fn enter_kernel(pml4: u64, stack_top: u64, entry: u64, boot_info: u64) -> ! {
    // SAFETY: garantizado por quien llama.
    unsafe {
        asm!(
            "cli",
            "mov cr3, {pml4}",
            "mov rsp, {stack}",
            "xor ebp, ebp",
            "push 0",            // dirección de retorno falsa: pila alineada como tras un `call`
            "jmp {entry}",
            pml4 = in(reg) pml4,
            stack = in(reg) stack_top,
            entry = in(reg) entry,
            in("rdi") boot_info,
            options(noreturn),
        )
    }
}
