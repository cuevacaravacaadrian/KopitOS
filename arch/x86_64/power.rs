//! Reinicio y apagado, de lo más estándar a lo más tosco.

use super::io::{inb, outb, outl, outw};
use core::arch::asm;

/// Reinicio: registro RESET de ACPI → controlador de teclado → triple fallo.
pub fn reboot() -> ! {
    crate::hal::disable_interrupts();
    #[cfg(feature = "acpi")]
    if let Some((gas, value)) = crate::acpi::info().and_then(|a| a.reset) {
        if gas.space == 1 {
            outb(gas.address as u16, value);
        } else if gas.space == 0 {
            // SAFETY: registro de reinicio indicado por el firmware.
            unsafe { (crate::mm::phys_to_virt(gas.address) as *mut u8).write_volatile(value) };
        }
        crate::time::spin_ms(50);
    }
    // Pulso de reinicio del 8042.
    for _ in 0..1000 {
        if inb(0x64) & 0x02 == 0 {
            break;
        }
    }
    outb(0x64, 0xFE);
    crate::time::spin_ms(50);
    // Último recurso: IDT vacía + excepción = triple fallo = reinicio.
    #[repr(C, packed)]
    struct Empty {
        limit: u16,
        base: u64,
    }
    let empty = Empty { limit: 0, base: 0 };
    // SAFETY: provocamos el reinicio a propósito.
    unsafe { asm!("lidt [{}]", "int3", in(reg) &empty, options(noreturn)) }
}

/// Apagado: ACPI S5 → puertos de apagado de hipervisores → parada.
pub fn power_off() -> ! {
    crate::hal::disable_interrupts();
    #[cfg(feature = "acpi")]
    if let Some(a) = crate::acpi::info() {
        if let Some((typ_a, typ_b)) = a.sleep_s5 {
            const SLP_EN: u16 = 1 << 13;
            if a.pm1a_control != 0 {
                outw(a.pm1a_control as u16, (typ_a << 10) | SLP_EN);
            }
            if a.pm1b_control != 0 {
                outw(a.pm1b_control as u16, (typ_b << 10) | SLP_EN);
            }
            crate::time::spin_ms(100);
        }
    }
    outw(0x604, 0x2000); // QEMU (q35/pc recientes)
    outw(0xB004, 0x2000); // QEMU antiguo / Bochs
    outw(0x4004, 0x3400); // VirtualBox
    crate::hal::halt()
}

/// Sale de QEMU con un código (dispositivo `isa-debug-exit` en 0xF4).
/// QEMU termina con `(código << 1) | 1`. Sin ese dispositivo no hace nada.
pub fn qemu_exit(code: u32) {
    outl(0xF4, code);
}
