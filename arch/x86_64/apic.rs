//! # Controladores de interrupciones y temporizadores de x86_64
//!
//! * **PIC 8259**: herencia del PC de 1981. Se reprograma a los vectores
//!   0x20–0x2F (para que sus interrupciones espurias no se confundan con
//!   excepciones) y se enmascara entero.
//! * **APIC local**: uno por CPU. Da el temporizador y recibe las IRQ. Se usa
//!   en modo **x2APIC** (por MSR, más rápido y sin MMIO) si la CPU lo tiene; si
//!   no, en modo xAPIC a través de una ventana MMIO sin caché.
//! * **IOAPIC**: reparte las IRQ de los dispositivos. Su dirección y las
//!   redirecciones ISA (p. ej. IRQ 0 → GSI 2) salen de la tabla MADT de ACPI.
//! * **Calibración**: el PIT (frecuencia fija de 1 193 182 Hz) mide 10 ms;
//!   con eso se calcula la frecuencia del TSC y la del temporizador del APIC.

use super::cpu::{self, IA32_APIC_BASE};
use super::idt::{self, TrapFrame};
use super::io::{inb, io_wait, outb};
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

pub const VECTOR_IRQ_BASE: u8 = 0x30; // IRQ ISA n → vector 0x30 + n
pub const VECTOR_TIMER: u8 = 0x40;
pub const VECTOR_SPURIOUS: u8 = 0xFF;

static X2APIC: AtomicBool = AtomicBool::new(false);
static XAPIC_BASE: AtomicU64 = AtomicU64::new(0);
static IOAPIC_BASE: AtomicU64 = AtomicU64::new(0);
static IOAPIC_GSI_BASE: AtomicU64 = AtomicU64::new(0);
static LAPIC_TICKS_PER_MS: AtomicU64 = AtomicU64::new(0);

// --- PIC 8259 ----------------------------------------------------------------

fn disable_pic() {
    const PIC1: u16 = 0x20;
    const PIC2: u16 = 0xA0;
    outb(PIC1, 0x11);
    io_wait();
    outb(PIC2, 0x11);
    io_wait();
    outb(PIC1 + 1, 0x20); // vectores 0x20..0x27
    io_wait();
    outb(PIC2 + 1, 0x28); // vectores 0x28..0x2F
    io_wait();
    outb(PIC1 + 1, 4);
    io_wait();
    outb(PIC2 + 1, 2);
    io_wait();
    outb(PIC1 + 1, 1);
    io_wait();
    outb(PIC2 + 1, 1);
    io_wait();
    outb(PIC1 + 1, 0xFF); // todo enmascarado
    outb(PIC2 + 1, 0xFF);
}

// --- PIT: calibración ----------------------------------------------------------

const PIT_HZ: u64 = 1_193_182;

/// Mide la frecuencia del TSC con el canal 2 del PIT (unos 10 ms).
pub fn calibrate_tsc() -> u64 {
    let ticks = PIT_HZ / 100; // 10 ms
    let port61 = inb(0x61);
    outb(0x61, (port61 & !0x02) | 0x01); // puerta del canal 2 abierta, altavoz apagado
    outb(0x43, 0b1011_0000); // canal 2, byte bajo/alto, modo 0
    outb(0x42, ticks as u8);
    outb(0x42, (ticks >> 8) as u8);
    let start = cpu::rdtsc();
    let mut guard = 0u64;
    while inb(0x61) & 0x20 == 0 {
        guard += 1;
        if guard > 50_000_000 {
            break; // sin PIT (algunas máquinas modernas): usamos otra fuente
        }
        core::hint::spin_loop();
    }
    let end = cpu::rdtsc();
    outb(0x61, port61);
    if guard > 50_000_000 {
        return 0;
    }
    (end - start) * 100
}

// --- APIC local ------------------------------------------------------------------

const REG_ID: u32 = 0x20;
const REG_TPR: u32 = 0x80;
const REG_EOI: u32 = 0xB0;
const REG_SVR: u32 = 0xF0;
const REG_LVT_TIMER: u32 = 0x320;
const REG_TIMER_INIT: u32 = 0x380;
const REG_TIMER_CURRENT: u32 = 0x390;
const REG_TIMER_DIVIDE: u32 = 0x3E0;

fn lapic_read(reg: u32) -> u32 {
    if X2APIC.load(Ordering::Relaxed) {
        cpu::rdmsr(0x800 + (reg >> 4)) as u32
    } else {
        // SAFETY: ventana MMIO del APIC mapeada sin caché.
        unsafe { ((XAPIC_BASE.load(Ordering::Relaxed) + reg as u64) as *const u32).read_volatile() }
    }
}

fn lapic_write(reg: u32, v: u32) {
    if X2APIC.load(Ordering::Relaxed) {
        cpu::wrmsr(0x800 + (reg >> 4), v as u64);
    } else {
        // SAFETY: ídem.
        unsafe { ((XAPIC_BASE.load(Ordering::Relaxed) + reg as u64) as *mut u32).write_volatile(v) }
    }
}

/// Fin de interrupción (todas las IRQ del APIC lo necesitan).
pub fn eoi() {
    if X2APIC.load(Ordering::Relaxed) || XAPIC_BASE.load(Ordering::Relaxed) != 0 {
        lapic_write(REG_EOI, 0);
    }
}

pub fn lapic_id() -> u32 {
    let id = lapic_read(REG_ID);
    if X2APIC.load(Ordering::Relaxed) { id } else { id >> 24 }
}

pub fn mode_name() -> &'static str {
    if X2APIC.load(Ordering::Relaxed) { "x2APIC" } else { "xAPIC" }
}

fn on_timer(_: &mut TrapFrame) {
    crate::time::on_tick();
    eoi();
}

fn on_spurious(_: &mut TrapFrame) {
    // Sin EOI: así lo exige la especificación para el vector espurio.
}

fn init_lapic(lapic_phys: u64) -> Result<(), &'static str> {
    let f = cpu::features();
    if !f.apic {
        return Err("la CPU no tiene APIC local");
    }
    let mut base = cpu::rdmsr(IA32_APIC_BASE);
    base |= 1 << 11; // APIC activo
    if f.x2apic && !crate::cmdline::flag("nox2apic") {
        base |= 1 << 10;
        cpu::wrmsr(IA32_APIC_BASE, base);
        X2APIC.store(true, Ordering::Relaxed);
    } else {
        cpu::wrmsr(IA32_APIC_BASE, base);
        let phys = if lapic_phys != 0 { lapic_phys } else { base & 0xF_FFFF_F000 };
        let virt = crate::mm::mmio::map(phys, 4096, crate::mm::mmio::Cache::Uncached).map_err(|e| e.message())?;
        XAPIC_BASE.store(virt, Ordering::Relaxed);
    }
    lapic_write(REG_TPR, 0);
    lapic_write(REG_SVR, 0x100 | VECTOR_SPURIOUS as u32);
    idt::register(VECTOR_SPURIOUS, on_spurious);
    Ok(())
}

/// Calibra el temporizador del APIC contra el TSC (ya calibrado) y lo pone en periódico.
fn start_timer(hz: u32) {
    lapic_write(REG_TIMER_DIVIDE, 0b0011); // dividir entre 16
    lapic_write(REG_LVT_TIMER, 1 << 16); // enmascarado mientras medimos
    lapic_write(REG_TIMER_INIT, u32::MAX);
    crate::time::spin_ms(10);
    let elapsed = u32::MAX - lapic_read(REG_TIMER_CURRENT);
    lapic_write(REG_TIMER_INIT, 0);
    let per_ms = (elapsed as u64 / 10).max(1);
    LAPIC_TICKS_PER_MS.store(per_ms, Ordering::Relaxed);
    idt::register(VECTOR_TIMER, on_timer);
    let initial = (per_ms * 1000 / hz.max(1) as u64).max(1) as u32;
    lapic_write(REG_LVT_TIMER, VECTOR_TIMER as u32 | (1 << 17)); // periódico
    lapic_write(REG_TIMER_INIT, initial);
}

pub fn timer_frequency_hz() -> u64 {
    LAPIC_TICKS_PER_MS.load(Ordering::Relaxed) * 1000 * 16
}

// --- IOAPIC ------------------------------------------------------------------------

fn ioapic_write(reg: u32, v: u32) {
    let base = IOAPIC_BASE.load(Ordering::Relaxed);
    // SAFETY: ventana MMIO del IOAPIC: IOREGSEL (+0) e IOWIN (+0x10).
    unsafe {
        (base as *mut u32).write_volatile(reg);
        ((base + 0x10) as *mut u32).write_volatile(v);
    }
}

fn ioapic_read(reg: u32) -> u32 {
    let base = IOAPIC_BASE.load(Ordering::Relaxed);
    // SAFETY: ídem.
    unsafe {
        (base as *mut u32).write_volatile(reg);
        ((base + 0x10) as *const u32).read_volatile()
    }
}

fn init_ioapic() -> Result<u32, &'static str> {
    #[cfg_attr(not(feature = "acpi"), allow(unused_mut))]
    let (mut phys, mut gsi_base) = (0xFEC0_0000u64, 0u32);
    #[cfg(feature = "acpi")]
    if let Some(io) = crate::acpi::info().and_then(|a| a.ioapics.first()) {
        phys = io.address;
        gsi_base = io.gsi_base;
    }
    let virt = crate::mm::mmio::map(phys, 4096, crate::mm::mmio::Cache::Uncached).map_err(|e| e.message())?;
    IOAPIC_BASE.store(virt, Ordering::Relaxed);
    IOAPIC_GSI_BASE.store(gsi_base as u64, Ordering::Relaxed);
    let entries = ((ioapic_read(1) >> 16) & 0xFF) + 1;
    for i in 0..entries {
        ioapic_write(0x10 + 2 * i, 1 << 16); // enmascarada
        ioapic_write(0x11 + 2 * i, 0);
    }
    Ok(entries)
}

/// Encamina la IRQ ISA `irq` al vector `0x30 + irq` en esta CPU.
pub fn route_isa_irq(irq: u8, handler: idt::Handler) {
    if IOAPIC_BASE.load(Ordering::Relaxed) == 0 {
        return;
    }
    #[cfg_attr(not(feature = "acpi"), allow(unused_mut))]
    let (mut gsi, mut active_low, mut level) = (irq as u32, false, false);
    #[cfg(feature = "acpi")]
    if let Some(o) = crate::acpi::info().and_then(|a| a.overrides.iter().find(|o| o.irq == irq)) {
        gsi = o.gsi;
        active_low = o.active_low;
        level = o.level_triggered;
    }
    let pin = gsi - IOAPIC_GSI_BASE.load(Ordering::Relaxed) as u32;
    let vector = VECTOR_IRQ_BASE + irq;
    idt::register(vector, handler);
    let polarity = if active_low { 1 << 13 } else { 0 };
    let trigger = if level { 1 << 15 } else { 0 };
    let low = vector as u32 | polarity | trigger;
    // Destino físico de 8 bits (bits 56..63), también con x2APIC (sin remapeo de IRQ).
    let dest = (lapic_id() & 0xFF) << 24;
    ioapic_write(0x11 + 2 * pin, dest);
    ioapic_write(0x10 + 2 * pin, low);
}

pub fn init(hz: u32) {
    disable_pic();
    #[allow(unused_mut)]
    let mut lapic_phys = 0;
    #[cfg(feature = "acpi")]
    if let Some(a) = crate::acpi::info() {
        lapic_phys = a.lapic_address;
    }
    if let Err(e) = init_lapic(lapic_phys) {
        kerror!("APIC local: {}", e);
        return;
    }
    match init_ioapic() {
        Ok(n) => kdebug!("IOAPIC: {} entradas de redirección", n),
        Err(e) => kwarn!("IOAPIC no disponible: {}", e),
    }
    start_timer(hz);
    kinfo!("APIC: modo {} · id {} · temporizador {} MHz", mode_name(), lapic_id(), timer_frequency_hz() / 1_000_000);
}
