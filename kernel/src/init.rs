//! # `kernel_main`: la secuencia de arranque
//!
//! Todas las entradas (KopitBoot/UEFI, Multiboot2, PVH, device tree) acaban
//! aquí con un `BootInfo` válido. El arranque va por **etapas**, cada una
//! sólo usa lo que dejaron listo las anteriores:
//!
//! | Etapa | Qué se inicializa                            | Qué permite                  |
//! |-------|----------------------------------------------|------------------------------|
//! | 0     | serie, CPU, reloj, BootInfo, consola, logs   | ver lo que pasa              |
//! | 1     | GDT/TSS/IDT (o equivalentes)                 | sobrevivir a excepciones     |
//! | 2     | marcos físicos → paginación → montículo      | `Box`, `Vec`, doble búfer    |
//! | 3     | ACPI, controladores de IRQ, temporizador     | interrupciones, tiempo       |
//! | 4     | PCI y registro de controladores              | saber qué hardware hay       |
//! | 5     | plan de servicios de sys_init                | (fase 3: espacio de usuario) |

use crate::console::{self, Tone};
use crate::hal::{self, Arch, Current};
use crate::log::FixedBuf;
use crate::{boot, cmdline, mm, time, version};
use core::fmt::{self, Write};
use kopit_boot_proto::{BootInfo, KernelVerification, MemoryKind};
use kopit_theme::Theme;

/// Mide cuánto tarda una etapa.
struct Stopwatch(u64);

impl Stopwatch {
    fn start() -> Self {
        Self(hal::cycles())
    }
}

impl fmt::Display for Stopwatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let us = time::cycles_to_ns(hal::cycles().wrapping_sub(self.0)) / 1000;
        if us >= 1000 { write!(f, "[{}.{:01} ms]", us / 1000, us % 1000 / 100) } else { write!(f, "[{} µs]", us) }
    }
}

const BANNER: [&str; 6] = [
    "██╗  ██╗ ██████╗ ██████╗ ██╗████████╗   ██████╗ ███████╗",
    "██║ ██╔╝██╔═══██╗██╔══██╗██║╚══██╔══╝  ██╔═══██╗██╔════╝",
    "█████╔╝ ██║   ██║██████╔╝██║   ██║     ██║   ██║███████╗",
    "██╔═██╗ ██║   ██║██╔═══╝ ██║   ██║     ██║   ██║╚════██║",
    "██║  ██╗╚██████╔╝██║     ██║   ██║     ╚██████╔╝███████║",
    "╚═╝  ╚═╝ ╚═════╝ ╚═╝     ╚═╝   ╚═╝      ╚═════╝ ╚══════╝",
];

fn banner() {
    console::write(Tone::Normal, "\n");
    for line in BANNER {
        console::write(Tone::Accent, "  ");
        console::write(Tone::Accent, line);
        console::write(Tone::Normal, "\n");
    }
    let mut sub = FixedBuf::<160>::new();
    let _ = write!(
        sub,
        "  {} {} «{}» · núcleo híbrido modular · {} · perfil {}\n\n",
        version::DISTRO_NAME,
        version::VERSION,
        version::CODENAME,
        version::ARCH,
        version::PROFILE
    );
    console::write(Tone::Muted, sub.as_str());
}

/// Tema de la línea de órdenes (`theme=`) con los ajustes `theme.<rol>=`.
pub fn theme_from_cmdline() -> Theme {
    let mut t = cmdline::get("theme").and_then(kopit_theme::find).copied().unwrap_or(*kopit_theme::default_theme());
    for (k, v) in cmdline::iter() {
        if let (Some(role), Some(v)) = (k.strip_prefix("theme."), v) {
            t.apply_override(role, v);
        }
    }
    t
}

fn report_boot(info: &BootInfo) {
    kinfo!("arranque: {} vía {}", boot::loader_name(), info.source.name());
    if !cmdline::raw().is_empty() {
        kinfo!("línea de órdenes: {}", cmdline::raw());
    }
    match info.kernel_verification {
        KernelVerification::VERIFIED => kinfo!("integridad del núcleo: SHA-256 verificado por el cargador ✓"),
        KernelVerification::MISMATCH => {
            kwarn!("integridad del núcleo: ¡la huella NO coincide! (Secure Boot desactivado)")
        }
        _ => kdebug!("integridad del núcleo: sin verificar (compilación local)"),
    }
    if info.source == kopit_boot_proto::BootSource::KOPITBOOT_UEFI {
        kinfo!("Secure Boot: {}", info.secure_boot.name());
    }
    kdebug!(
        "imagen del núcleo: {} KiB en física {:#x} → virtual {:#x}",
        info.kernel.size / 1024,
        info.kernel.phys_base,
        info.kernel.virt_base
    );
    let fb = info.framebuffer;
    if fb.is_present() && fb.kind == kopit_boot_proto::FramebufferKind::RGB {
        if let Some(s) = console::screen_info() {
            kinfo!(
                "pantalla: {}x{} · {} bpp · consola {}x{} (letra ×{})",
                s.width,
                s.height,
                fb.bpp,
                s.cols,
                s.rows,
                s.scale
            );
        }
    }
}

fn default_hz() -> u32 {
    match version::DEVICE_CLASS {
        "server" => 250,
        "iot" => 100,
        "phone" | "tablet" | "laptop" => 300,
        _ => 1000,
    }
}

/// Punto de entrada común del núcleo.
pub fn kernel_main(info: &'static BootInfo) -> ! {
    // ── Etapa 0: poder hablar ────────────────────────────────────────────────
    time::mark_boot(hal::cycles());
    mm::init_early(info);
    Current::early_init(info);
    if let Err(e) = boot::init(info) {
        crate::log::emergency(format_args!("\nKopitOS: BootInfo no válido ({}). Arranque detenido.\n", e));
        hal::halt();
    }
    let info = boot::info();
    console::init(info, theme_from_cmdline());
    banner();
    report_boot(info);
    if info.boot_time.valid != 0 {
        time::set_wall_clock(&info.boot_time);
    }
    #[cfg(target_arch = "x86_64")]
    if !time::has_wall_clock() {
        time::set_wall_clock(&crate::arch::rtc::read());
    }

    // ── Etapa 1: la CPU ──────────────────────────────────────────────────────
    let sw = Stopwatch::start();
    Current::init_cpu();
    kinfo!("CPU: tablas de descriptores y de excepciones instaladas {}", sw);

    // ── Etapa 2: memoria ─────────────────────────────────────────────────────
    boot::log_memory_map();
    let sw = Stopwatch::start();
    match mm::pmm::init(boot::memory_map(), info.hhdm_limit) {
        Ok(s) => {
            let zones = s.zones.iter().flatten().count();
            kinfo!(
                "memoria física: {} MiB libres de {} MiB instalados · {} zonas buddy · metadatos {} KiB {}",
                s.free_bytes >> 20,
                boot::installed_ram() >> 20,
                zones,
                s.bitmap_bytes >> 10,
                sw
            );
        }
        Err(e) => panic!("no se pudo iniciar la memoria física: {}", e),
    }
    let sw = Stopwatch::start();
    match mm::paging::init(info) {
        Ok(r) => {
            kinfo!(
                "paginación propia (W^X): texto {} KiB r-x · rodata {} KiB r-- · datos {} KiB rw- · mapa directo {} GiB {}",
                r.text_bytes >> 10,
                r.rodata_bytes >> 10,
                r.data_bytes >> 10,
                r.direct_map_bytes >> 30,
                sw
            );
            if r.hot_added_bytes > 0 {
                kinfo!("memoria alta añadida tras ampliar el mapa directo: {} MiB", r.hot_added_bytes >> 20);
            }
            if let Some(v) = r.framebuffer_virt {
                kdebug!("framebuffer movido a una ventana write-combining en {:#x}", v);
            }
        }
        Err(mm::MapError::Unsupported) => {
            kinfo!("paginación: se mantiene el mapa del cargador en esta arquitectura (fase 1)")
        }
        Err(e) => panic!("no se pudo construir el espacio de direcciones del núcleo: {}", e.message()),
    }
    mm::heap::init();
    if console::enable_double_buffering() {
        kdebug!("consola gráfica: doble búfer activado");
    }
    if mm::paging::is_active() {
        // El cargador ya no hace falta: sus tablas, su pila y BootInfo están copiados.
        let mut reclaimed = 0;
        for r in boot::memory_map().iter().filter(|r| r.kind == MemoryKind::BOOTLOADER_RECLAIMABLE) {
            reclaimed += mm::pmm::add_region(r.base, r.end());
        }
        if reclaimed > 0 {
            kinfo!("memoria del cargador recuperada: {} MiB", reclaimed >> 20);
        }
    }

    // ── Etapa 3: plataforma ──────────────────────────────────────────────────
    #[cfg(feature = "acpi")]
    {
        #[cfg_attr(not(target_arch = "x86_64"), allow(unused_mut))]
        let mut rsdp = info.rsdp;
        #[cfg(target_arch = "x86_64")]
        if rsdp == 0 {
            rsdp = crate::acpi::find_rsdp_in_bios().unwrap_or(0);
        }
        match crate::acpi::init(rsdp) {
            Ok(a) => {
                let enabled = a.cpus.iter().filter(|c| c.enabled).count();
                kinfo!(
                    "ACPI {}: {} tablas · {} CPU · {} IOAPIC · apagado S5 {}",
                    if a.revision >= 2 { "2.0+" } else { "1.0" },
                    a.tables.len(),
                    enabled,
                    a.ioapics.len(),
                    if a.sleep_s5.is_some() { "sí" } else { "no" }
                );
            }
            Err(e) => kwarn!("ACPI no disponible: {}", e),
        }
    }
    let hz = cmdline::get_u32("hz").filter(|h| (10..=10_000).contains(h)).unwrap_or_else(default_hz);
    let sw = Stopwatch::start();
    Current::init_platform(hz);
    time::set_tick_hz(hz);
    hal::enable_interrupts();
    kinfo!("interrupciones activas · temporizador a {} Hz {}", hz, sw);

    // ── Etapa 4: dispositivos ────────────────────────────────────────────────
    #[cfg(feature = "pci")]
    if !cmdline::flag("safe") {
        let sw = Stopwatch::start();
        let devs = crate::pci::scan();
        let known = devs.iter().filter(|d| crate::pci::driver_for(d).is_some()).count();
        kinfo!("PCI: {} dispositivos, {} con controlador conocido (orden «pci») {}", devs.len(), known, sw);
        crate::pci::log_devices();
    }

    // ── Etapa 5: plan del espacio de usuario ─────────────────────────────────
    match kopit_init::plan(kopit_init::DEFAULT_SERVICES, version::DEVICE_CLASS) {
        Ok(plan) => {
            let mut line = FixedBuf::<400>::new();
            for (i, wave) in plan.iter().enumerate() {
                let _ = write!(line, "{}[", if i > 0 { " → " } else { "" });
                for (j, s) in wave.iter().enumerate() {
                    let _ = write!(line, "{}{}", if j > 0 { " " } else { "" }, s.name);
                }
                let _ = write!(line, "]");
            }
            kinfo!("sys_init ({}): {} oleadas {}", version::DEVICE_CLASS, plan.len(), line.as_str());
        }
        Err(e) => kerror!("plan de servicios no válido: {:?}", e),
    }

    let kernel_ms = time::uptime_ms();
    if info.loader_timestamp != 0 && info.loader_timestamp < time::boot_cycles() {
        let loader_ms = time::cycles_to_ns(time::boot_cycles() - info.loader_timestamp) / 1_000_000;
        kinfo!(
            "✓ {} listo: núcleo {} ms + cargador {} ms (incluye el menú)",
            version::DISTRO_NAME,
            kernel_ms,
            loader_ms
        );
    } else {
        kinfo!("✓ {} listo en {} ms", version::DISTRO_NAME, kernel_ms);
    }
    refresh_status();

    #[cfg(feature = "selftest")]
    if cmdline::get("test") == Some("smoke") {
        crate::selftest::run_and_exit();
    }

    #[cfg(feature = "shell")]
    crate::shell::run();

    #[allow(unreachable_code)]
    loop {
        hal::wait_for_interrupt();
        refresh_status();
    }
}

/// Actualiza las barras de estado (reloj, memoria, CPU). Barato: sólo
/// repinta si el texto cambió.
pub fn refresh_status() {
    let mut title = FixedBuf::<96>::new();
    match time::now() {
        Some(dt) => {
            let _ = write!(
                title,
                "{} {:02}/{:02}/{} · {:02}:{:02}:{:02}",
                time::weekday_name(&dt),
                dt.day,
                dt.month,
                dt.year,
                dt.hour,
                dt.minute,
                dt.second
            );
        }
        None => {
            let s = time::uptime_ms() / 1000;
            let _ = write!(title, "en marcha {:02}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60);
        }
    }
    let mut left = FixedBuf::<192>::new();
    if let Some(p) = mm::pmm::stats() {
        let _ = write!(left, "RAM {}/{} MiB libres", p.free_bytes >> 20, boot::installed_ram() >> 20);
    }
    let _ = write!(left, " · {} CPU · {} Hz", cpu_count(), time::tick_hz());
    let up = time::uptime_ms() / 1000;
    let _ = write!(left, " · en marcha {:02}:{:02}:{:02}", up / 3600, up / 60 % 60, up % 60);
    let mut right = FixedBuf::<96>::new();
    let _ = write!(right, "tema {}", console::theme().id);
    console::set_status(title.as_str(), left.as_str(), right.as_str());
}

pub fn cpu_count() -> usize {
    #[cfg(feature = "acpi")]
    if let Some(a) = crate::acpi::info() {
        return a.cpus.iter().filter(|c| c.enabled).count().max(1);
    }
    1
}
