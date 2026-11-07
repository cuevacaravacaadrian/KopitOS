//! # Capa de abstracción del hardware (HAL)
//!
//! Cada arquitectura implementa [`Arch`] en `arch/<cpu>/mod.rs`. El núcleo
//! portable sólo llama a las funciones de este módulo, así que portar
//! KopitOS a otra CPU es implementar este rasgo (más la paginación de
//! `arch::paging`). El compilador comprueba que no falte nada.

use kopit_boot_proto::BootInfo;

pub trait Arch {
    /// Nombre de la arquitectura ("x86_64", "aarch64", "riscv64").
    const NAME: &'static str;

    /// Paso 0, sin memoria dinámica: consola serie, características de la CPU.
    fn early_init(boot: &BootInfo);
    /// Tablas de la CPU: GDT/TSS/IDT (x86_64), VBAR_EL1 (ARM64), stvec (RISC-V).
    fn init_cpu();
    /// Controladores de interrupciones, temporizador a `hz` y dispositivos de entrada.
    fn init_platform(hz: u32);

    fn enable_interrupts();
    /// Desactiva interrupciones y devuelve si estaban activas.
    fn disable_interrupts() -> bool;
    fn restore_interrupts(was_enabled: bool);
    fn interrupts_enabled() -> bool;
    /// Activa interrupciones y duerme hasta la siguiente, de forma atómica.
    fn wait_for_interrupt();
    fn halt() -> !;

    /// Contador de ciclos de alta resolución (TSC, CNTVCT, `time`).
    fn cycles() -> u64;

    fn serial_write(bytes: &[u8]);
    fn serial_read() -> Option<u8>;
    /// Para el pánico: libera el cerrojo de la consola serie aunque esté tomado.
    ///
    /// # Safety
    /// Sólo con el resto de CPUs detenidas y sin vuelta atrás.
    unsafe fn serial_force_unlock();

    fn reboot() -> !;
    fn power_off() -> !;
    /// Provoca una excepción de punto de ruptura (prueba de la tabla de excepciones).
    fn breakpoint();
    /// Recorre la pila por punteros de marco: llama a `f` con cada dirección de retorno.
    fn backtrace(f: &mut dyn FnMut(u64));
}

pub type Current = crate::arch::ArchImpl;

/// Códigos para salir de QEMU en las pruebas automáticas (`arch::qemu_exit`).
pub const EXIT_SUCCESS: u32 = 0x10;
pub const EXIT_FAILURE: u32 = 0x11;

// Atajos para no escribir `<Current as Arch>::...` en todo el núcleo.

pub const NAME: &str = <Current as Arch>::NAME;

#[inline]
pub fn cycles() -> u64 {
    <Current as Arch>::cycles()
}

#[inline]
pub fn enable_interrupts() {
    <Current as Arch>::enable_interrupts()
}

#[inline]
pub fn disable_interrupts() -> bool {
    <Current as Arch>::disable_interrupts()
}

#[inline]
pub fn restore_interrupts(was_enabled: bool) {
    <Current as Arch>::restore_interrupts(was_enabled)
}

#[inline]
pub fn interrupts_enabled() -> bool {
    <Current as Arch>::interrupts_enabled()
}

#[inline]
pub fn wait_for_interrupt() {
    <Current as Arch>::wait_for_interrupt()
}

pub fn halt() -> ! {
    <Current as Arch>::halt()
}

pub fn serial_write(bytes: &[u8]) {
    <Current as Arch>::serial_write(bytes)
}

pub fn serial_read() -> Option<u8> {
    <Current as Arch>::serial_read()
}

pub fn reboot() -> ! {
    <Current as Arch>::reboot()
}

pub fn power_off() -> ! {
    <Current as Arch>::power_off()
}

pub fn breakpoint() {
    <Current as Arch>::breakpoint()
}

pub fn backtrace(f: &mut dyn FnMut(u64)) {
    <Current as Arch>::backtrace(f)
}

/// Ejecuta `f` con las interrupciones desactivadas.
pub fn without_interrupts<R>(f: impl FnOnce() -> R) -> R {
    let was = disable_interrupts();
    let r = f();
    restore_interrupts(was);
    r
}
