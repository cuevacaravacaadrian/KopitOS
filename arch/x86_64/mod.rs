//! # Arquitectura x86_64
//!
//! | Archivo      | Contenido                                                    |
//! |--------------|--------------------------------------------------------------|
//! | `boot.rs`    | cabecera Multiboot2, nota PVH, trampolín 32→64, `_start`      |
//! | `boot32.rs`  | Multiboot2 / PVH → `BootInfo` (KBP)                           |
//! | `cpu.rs`     | CPUID (incl. núcleos P/E), MSR, CR*, PAT, NX                  |
//! | `gdt.rs`     | GDT + TSS con pilas IST                                       |
//! | `idt.rs`     | 256 stubs, `TrapFrame`, despachador, excepciones              |
//! | `apic.rs`    | PIC, APIC local (x2APIC/xAPIC), IOAPIC, PIT, temporizador     |
//! | `paging.rs`  | tablas de 4 niveles, páginas de 4K/2M/1G, PAT                 |
//! | `serial.rs`  | COM1                                                          |
//! | `keyboard.rs`| PS/2                                                          |
//! | `rtc.rs`     | reloj CMOS                                                    |
//! | `power.rs`   | reinicio / apagado / salida de QEMU                           |

pub mod apic;
pub mod boot;
pub mod boot32;
pub mod cpu;
pub mod gdt;
pub mod idt;
pub mod io;
#[cfg(feature = "ps2")]
pub mod keyboard;
pub mod paging;
pub mod power;
pub mod rtc;
pub mod serial;

use crate::hal::Arch;
use kopit_boot_proto::BootInfo;

pub struct ArchImpl;

impl Arch for ArchImpl {
    const NAME: &'static str = "x86_64";

    fn early_init(_boot: &BootInfo) {
        let f = cpu::detect();
        cpu::enable_features();
        serial::init();
        // Reloj: PIT (fiable en todas partes) o, si falta, la hoja 0x15 de CPUID.
        let hz = match apic::calibrate_tsc() {
            0 => f.tsc_hz_cpuid,
            hz => hz,
        };
        crate::time::set_cycle_frequency(if hz != 0 { hz } else { 2_000_000_000 });
    }

    fn init_cpu() {
        gdt::init();
        idt::init();
    }

    fn init_platform(hz: u32) {
        apic::init(hz);
        serial::enable_irq();
        #[cfg(feature = "ps2")]
        if !crate::cmdline::flag("safe") {
            let layout = crate::cmdline::get("kbd")
                .and_then(kopit_drv_ps2::Layout::from_name)
                .unwrap_or(kopit_drv_ps2::Layout::Es);
            match keyboard::init(layout) {
                Ok(()) => kinfo!("teclado PS/2 listo (distribución '{}')", layout.name()),
                Err(e) => kwarn!("teclado PS/2 no disponible: {:?}", e),
            }
        }
    }

    fn enable_interrupts() {
        // SAFETY: las tablas de interrupción ya están instaladas.
        unsafe { core::arch::asm!("sti", options(nomem, nostack)) };
    }

    fn disable_interrupts() -> bool {
        let was = Self::interrupts_enabled();
        // SAFETY: desactivar interrupciones siempre es seguro.
        unsafe { core::arch::asm!("cli", options(nomem, nostack)) };
        was
    }

    fn restore_interrupts(was_enabled: bool) {
        if was_enabled {
            Self::enable_interrupts();
        }
    }

    fn interrupts_enabled() -> bool {
        cpu::rflags() & (1 << 9) != 0
    }

    fn wait_for_interrupt() {
        // `sti; hlt` es atómico: una interrupción no puede colarse entre ambas.
        // SAFETY: espera a la siguiente interrupción.
        unsafe { core::arch::asm!("sti", "hlt", options(nomem, nostack)) };
    }

    fn halt() -> ! {
        loop {
            // SAFETY: detiene la CPU para siempre.
            unsafe { core::arch::asm!("cli", "hlt", options(nomem, nostack)) };
        }
    }

    fn cycles() -> u64 {
        cpu::rdtsc()
    }

    fn serial_write(bytes: &[u8]) {
        serial::write(bytes);
    }

    fn serial_read() -> Option<u8> {
        serial::read()
    }

    unsafe fn serial_force_unlock() {
        // SAFETY: lo garantiza quien llama.
        unsafe { serial::force_unlock() }
    }

    fn reboot() -> ! {
        power::reboot()
    }

    fn power_off() -> ! {
        power::power_off()
    }

    fn breakpoint() {
        // SAFETY: provoca #BP, que nuestro manejador atiende y del que vuelve.
        unsafe { core::arch::asm!("int3", options(nomem, nostack)) };
    }

    fn backtrace(f: &mut dyn FnMut(u64)) {
        let mut rbp: u64;
        // SAFETY: lectura del puntero de marco actual.
        unsafe { core::arch::asm!("mov {}, rbp", out(reg) rbp, options(nomem, nostack, preserves_flags)) };
        // Sólo se siguen marcos dentro de las pilas del núcleo (.bss): nunca se
        // lee memoria dudosa en medio de un pánico.
        let (lo, hi) = (crate::mm::layout::bss_start(), crate::mm::layout::bss_end());
        for _ in 0..16 {
            if rbp < lo || rbp + 16 > hi || rbp % 8 != 0 {
                break;
            }
            // SAFETY: `rbp` está dentro del .bss del núcleo (comprobado arriba).
            let (next, ret) = unsafe { (*(rbp as *const u64), *((rbp + 8) as *const u64)) };
            if ret == 0 {
                break;
            }
            f(ret);
            if next <= rbp {
                break;
            }
            rbp = next;
        }
    }
}

// --- Puntos de entrada -------------------------------------------------------------

/// Desde KopitBoot (`_start`).
#[unsafe(no_mangle)]
extern "C" fn kopit_entry_kbp(info: *const BootInfo) -> ! {
    // SAFETY: KopitBoot pasa un BootInfo válido en el mapa directo.
    let info = unsafe { &*info };
    crate::init::kernel_main(info)
}

/// Desde el trampolín de 32 bits (Multiboot2 o PVH).
#[unsafe(no_mangle)]
extern "C" fn kopit_boot32_entry64(protocol: u32, info_phys: u32) -> ! {
    let info = boot32::build(protocol, info_phys as u64);
    crate::init::kernel_main(info)
}

// --- Servicios para el resto del núcleo -----------------------------------------------

pub fn breakpoint_count() -> u64 {
    idt::breakpoint_count()
}

pub fn qemu_exit(code: u32) {
    power::qemu_exit(code);
}

pub fn describe_cpu(f: &mut dyn FnMut(&str, &str)) {
    cpu::describe(f);
    f("controlador de IRQ", apic::mode_name());
}

/// Comprueba W^X sobre el espacio activo: (texto sin escritura, datos sin
/// ejecución, página de guarda sin mapear).
pub fn paging_self_check() -> Option<(bool, bool, bool)> {
    use crate::mm::{MapFlags, layout, paging};
    if !paging::is_active() {
        return None;
    }
    let text = paging::translate(layout::text_start())?;
    let data = paging::translate(layout::data_start())?;
    let guard = paging::translate(layout::stack_guard());
    Some((
        !text.1.has(MapFlags::WRITE) && text.1.has(MapFlags::EXEC),
        data.1.has(MapFlags::WRITE) && !data.1.has(MapFlags::EXEC),
        guard.is_none(),
    ))
}

/// Espacio de configuración PCI por puertos `0xCF8/0xCFC` (mecanismo n.º 1).
#[derive(Clone, Copy)]
pub struct PortPciConfig;

impl kopit_drv_pci::ConfigSpace for PortPciConfig {
    fn read32(&self, bus: u8, device: u8, function: u8, offset: u16) -> u32 {
        io::outl(0xCF8, pci_address(bus, device, function, offset));
        io::inl(0xCFC)
    }
    fn write32(&self, bus: u8, device: u8, function: u8, offset: u16, value: u32) {
        io::outl(0xCF8, pci_address(bus, device, function, offset));
        io::outl(0xCFC, value);
    }
}

fn pci_address(bus: u8, device: u8, function: u8, offset: u16) -> u32 {
    0x8000_0000 | (bus as u32) << 16 | (device as u32) << 11 | (function as u32) << 8 | (offset as u32 & 0xFC)
}

pub fn pci_config() -> Option<PortPciConfig> {
    Some(PortPciConfig)
}
