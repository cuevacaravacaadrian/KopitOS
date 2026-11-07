//! COM1 (0x3F8): consola serie del núcleo, con el controlador genérico 16550.

use super::idt::TrapFrame;
use super::io::PortRegisters;
use crate::sync::IrqLock;
use kopit_driver_api::CharSink;
use kopit_drv_uart16550::Uart16550;

static COM1: IrqLock<Uart16550<PortRegisters>> = IrqLock::new(Uart16550::new(PortRegisters { base: 0x3F8 }));

pub fn init() -> bool {
    COM1.lock().init(115_200)
}

pub fn write(bytes: &[u8]) {
    COM1.lock().write_bytes(bytes);
}

pub fn read() -> Option<u8> {
    COM1.lock().try_read_byte()
}

/// # Safety
/// Sólo desde el pánico.
pub unsafe fn force_unlock() {
    // SAFETY: lo garantiza quien llama.
    unsafe { COM1.force_unlock() }
}

fn on_irq(_: &mut TrapFrame) {
    while let Some(b) = read() {
        crate::input::push_serial_byte(b);
    }
    super::apic::eoi();
}

/// IRQ 4: recepción por interrupción (la shell funciona por serie).
pub fn enable_irq() {
    COM1.lock().enable_rx_interrupt();
    super::apic::route_isa_irq(4, on_irq);
}
