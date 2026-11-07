//! Puertos de E/S de x86 (`in`/`out`).

use core::arch::asm;

#[inline]
pub fn outb(port: u16, v: u8) {
    // SAFETY: E/S de puertos; los llamantes conocen el dispositivo.
    unsafe { asm!("out dx, al", in("dx") port, in("al") v, options(nomem, nostack, preserves_flags)) };
}

#[inline]
pub fn inb(port: u16) -> u8 {
    let v: u8;
    // SAFETY: ídem.
    unsafe { asm!("in al, dx", in("dx") port, out("al") v, options(nomem, nostack, preserves_flags)) };
    v
}

#[inline]
pub fn outw(port: u16, v: u16) {
    // SAFETY: ídem.
    unsafe { asm!("out dx, ax", in("dx") port, in("ax") v, options(nomem, nostack, preserves_flags)) };
}

#[inline]
pub fn outl(port: u16, v: u32) {
    // SAFETY: ídem.
    unsafe { asm!("out dx, eax", in("dx") port, in("eax") v, options(nomem, nostack, preserves_flags)) };
}

#[inline]
pub fn inl(port: u16) -> u32 {
    let v: u32;
    // SAFETY: ídem.
    unsafe { asm!("in eax, dx", in("dx") port, out("eax") v, options(nomem, nostack, preserves_flags)) };
    v
}

/// Pequeña pausa para controladores antiguos (escritura en el puerto POST 0x80).
#[inline]
pub fn io_wait() {
    outb(0x80, 0);
}

/// Implementación de los rasgos de `drivers/api` con puertos reales.
#[derive(Clone, Copy)]
pub struct Ports;

impl kopit_driver_api::PortIo for Ports {
    fn inb(&self, port: u16) -> u8 {
        inb(port)
    }
    fn outb(&self, port: u16, value: u8) {
        outb(port, value)
    }
    fn inl(&self, port: u16) -> u32 {
        inl(port)
    }
    fn outl(&self, port: u16, value: u32) {
        outl(port, value)
    }
}

/// Banco de registros de 8 bits a partir de un puerto base (UART 16550).
#[derive(Clone, Copy)]
pub struct PortRegisters {
    pub base: u16,
}

impl kopit_driver_api::RegisterIo for PortRegisters {
    fn read(&self, reg: u16) -> u8 {
        inb(self.base + reg)
    }
    fn write(&self, reg: u16, value: u8) {
        outb(self.base + reg, value)
    }
}
