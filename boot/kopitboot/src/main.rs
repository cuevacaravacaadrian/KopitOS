//! # KopitBoot — cargador UEFI de KopitOS
//!
//! Flujo completo, de la pulsación del botón de encendido a `kernel_main`:
//!
//! ```text
//!  Firmware UEFI ──► \EFI\BOOT\BOOTX64.EFI (KopitBoot, firmado para Secure Boot)
//!       │
//!       ├─ 1. lee \kopit\boot.cfg              (entradas, tiempo, resolución)
//!       ├─ 2. menú TUI                          (↑↓, Intro, editar línea de órdenes)
//!       ├─ 3. lee \kopit\kernel.elf             (protocolo Simple File System)
//!       ├─ 4. verifica su SHA-256               (huella integrada en ESTE binario)
//!       ├─ 5. carga los segmentos ELF           (memoria tipo KOPIT_KERNEL)
//!       ├─ 6. tablas de páginas                 (identidad + HHDM + núcleo W^X)
//!       ├─ 7. modo de vídeo GOP                 (framebuffer lineal)
//!       ├─ 8. ACPI, SMBIOS, hora, Secure Boot   (tablas de configuración)
//!       ├─ 9. ExitBootServices                  (a partir de aquí, sin firmware)
//!       ├─10. mapa de memoria → KBP             (sin reservar memoria)
//!       └─11. CR3 ← tablas nuevas; salto a _start(BootInfo*)
//! ```
//!
//! En x86_64 UEFI ya entrega la CPU en modo largo (64 bits): no hay que hacer
//! la transición desde 16/32 bits. Esa transición sí la hace el núcleo en sus
//! entradas Multiboot2/PVH (ver `arch/x86_64/boot.rs`).

#![no_std]
#![no_main]

#[cfg(not(target_arch = "x86_64"))]
compile_error!("KopitBoot sólo soporta x86_64 por ahora (ARM64 UEFI llega en la fase 2; ver ARCHITECTURE.md)");

extern crate alloc;

mod config;
mod cpu;
mod firmware;
mod fs;
mod menu;
mod paging;
mod uefi;
mod video;

use alloc::string::String;
use core::convert::Infallible;
use core::fmt::Write as _;
use kopit_boot_proto::*;
use uefi::{Handle, Status, SystemTable, color::*, memtype};

#[global_allocator]
static ALLOCATOR: firmware::PoolAllocator = firmware::PoolAllocator;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// Huella SHA-256 del núcleo, integrada por distro_builder al compilar.
/// Como forma parte de este binario, Secure Boot la protege con su firma.
const KERNEL_SHA256: Option<&str> = option_env!("KOPIT_KERNEL_SHA256");
/// Capacidad del bloque de línea de órdenes que se entrega al núcleo.
const CMDLINE_MAX: usize = 4096;
const KERNEL_STACK_PAGES: usize = 16;

struct BootError {
    what: &'static str,
    status: Option<Status>,
}

impl BootError {
    fn new(what: &'static str) -> Self {
        Self { what, status: None }
    }
    fn uefi(what: &'static str) -> impl FnOnce(Status) -> Self {
        move |s| Self { what, status: Some(s) }
    }
}

#[unsafe(export_name = "efi_main")]
extern "efiapi" fn efi_main(image: Handle, st: *mut SystemTable) -> Status {
    let t0 = cpu::rdtsc();
    firmware::init(image, st);
    firmware::disable_watchdog();
    match boot(t0) {
        Ok(never) => match never {},
        Err(e) => {
            firmware::set_color(LIGHTRED, BLACK);
            println!();
            println!("  KopitBoot no ha podido arrancar KopitOS:");
            match e.status {
                Some(s) => println!("    {} ({})", e.what, s.describe()),
                None => println!("    {}", e.what),
            }
            firmware::set_color(LIGHTGRAY, BLACK);
            println!("\n  Pulsa una tecla para volver al firmware.");
            firmware::flush_keys();
            let _ = firmware::wait_key_or(None);
            e.status.unwrap_or(Status::LOAD_ERROR)
        }
    }
}

/// Imprime una línea de progreso "  ✓ paso ........ detalle".
fn step(name: &str, detail: core::fmt::Arguments) {
    firmware::set_color(LIGHTGREEN, BLACK);
    print!("  ✓ ");
    firmware::set_color(LIGHTGRAY, BLACK);
    let mut s = String::new();
    let _ = s.write_fmt(detail);
    println!("{:<28}{}", name, s);
}

fn boot(t0: u64) -> Result<Infallible, BootError> {
    let volume = fs::Volume::boot_volume().map_err(BootError::uefi("no se pudo abrir el volumen de arranque"))?;
    let cfg = match volume.read("\\kopit\\boot.cfg") {
        Ok(bytes) => config::Config::parse(core::str::from_utf8(&bytes).unwrap_or(""), true),
        Err(_) => config::Config::parse("", false),
    };

    // --- Menú -------------------------------------------------------------------
    let secure_boot = firmware::secure_boot_state();
    let vendor = firmware::firmware_vendor();
    let status = menu::Status {
        secure_boot,
        firmware: &vendor,
        signed_hash: KERNEL_SHA256.is_some(),
        config_found: cfg.from_file,
    };
    let (entry, cmdline) = match menu::run(&cfg, &status) {
        menu::Choice::Boot { entry, cmdline } => (&cfg.entries[entry], cmdline),
        menu::Choice::Reboot => firmware::reset(uefi::RESET_COLD),
        menu::Choice::Shutdown => firmware::reset(uefi::RESET_SHUTDOWN),
    };

    firmware::set_color(LIGHTGRAY, BLACK);
    firmware::clear_screen();
    firmware::set_color(WHITE, BLACK);
    println!("\n  KopitBoot {} → {}\n", VERSION, entry.title);

    // --- Núcleo -------------------------------------------------------------------
    let data = volume.read(&entry.kernel).map_err(BootError::uefi("no se encontró el núcleo en la ESP"))?;
    step("Núcleo leído", format_args!("{} ({} KiB)", entry.kernel, data.len() / 1024));

    let digest = kopit_crypto::sha256::digest(&data);
    let verification = match KERNEL_SHA256.and_then(kopit_crypto::sha256::from_hex) {
        None => KernelVerification::NOT_CHECKED,
        Some(expected) if expected == digest => KernelVerification::VERIFIED,
        Some(_) => KernelVerification::MISMATCH,
    };
    let hex = kopit_crypto::sha256::to_hex(&digest);
    let short = core::str::from_utf8(&hex[..16]).unwrap_or("");
    match verification {
        KernelVerification::VERIFIED => {
            step("Integridad", format_args!("SHA-256 {}… coincide con la huella firmada", short))
        }
        KernelVerification::NOT_CHECKED => {
            step("Integridad", format_args!("SHA-256 {}… (sin huella integrada)", short))
        }
        _ => {
            firmware::set_color(LIGHTRED, BLACK);
            println!("  ✗ La huella SHA-256 del núcleo NO coincide con la integrada en el cargador.");
            if secure_boot == SecureBootState::ENABLED {
                return Err(BootError::new("núcleo modificado: con Secure Boot activo no se arranca"));
            }
            println!("    Secure Boot está desactivado: se continúa en 3 s (el núcleo lo sabrá).");
            firmware::set_color(LIGHTGRAY, BLACK);
            firmware::stall_ms(3000);
        }
    }

    let kernel = load_elf(&data)?;
    drop(data);
    step(
        "Segmentos ELF cargados",
        format_args!("{} KiB en física {:#x}, entrada {:#x}", kernel.size / 1024, kernel.phys_base, kernel.entry),
    );

    // --- Tablas del firmware ------------------------------------------------------
    let rsdp =
        firmware::config_table(&uefi::ACPI_20_TABLE_GUID).or_else(|| firmware::config_table(&uefi::ACPI_10_TABLE_GUID));
    let smbios =
        firmware::config_table(&uefi::SMBIOS3_TABLE_GUID).or_else(|| firmware::config_table(&uefi::SMBIOS_TABLE_GUID));
    let time = firmware::get_time();
    step(
        "Tablas del firmware",
        format_args!(
            "ACPI {} · SMBIOS {}",
            if rsdp.is_some() { "sí" } else { "no" },
            if smbios.is_some() { "sí" } else { "no" }
        ),
    );

    // --- Mapa de memoria preliminar: tamaño de la RAM y del HHDM ------------------
    let mut map = firmware::MemoryMap::allocate().map_err(BootError::uefi("no se pudo leer el mapa de memoria"))?;
    map.refresh().map_err(BootError::uefi("no se pudo leer el mapa de memoria"))?;
    // El HHDM cubre los primeros 4 GiB (ACPI, VGA...) y toda la RAM real. Las
    // regiones reservadas altas (p. ej. la ventana PCI de 64 bits de QEMU, cerca
    // de 1 TiB) no cuentan: el núcleo las mapeará como MMIO si las necesita.
    let mut max_phys = 4u64 << 30;
    for d in map.descriptors() {
        let end = d.physical_start + d.number_of_pages * 4096;
        let ram_like =
            !matches!(d.kind, memtype::MMIO | memtype::MMIO_PORT_SPACE | memtype::RESERVED | memtype::UNUSABLE);
        if ram_like {
            max_phys = max_phys.max(end);
        }
    }

    // --- Bloque de BootInfo (estructura + línea de órdenes + mapa) -------------------
    let capacity = map.len() * 2 + 64;
    let info_bytes = 1024 + CMDLINE_MAX + 64 + capacity * core::mem::size_of::<MemoryRegion>();
    let info_pages = info_bytes.div_ceil(4096);
    let info_phys = firmware::allocate_zeroed_pages(memtype::KOPIT_BOOTINFO, info_pages)
        .map_err(BootError::uefi("sin memoria para BootInfo"))?;
    let stack_phys = firmware::allocate_zeroed_pages(memtype::KOPIT_BOOTINFO, KERNEL_STACK_PAGES)
        .map_err(BootError::uefi("sin memoria para la pila del núcleo"))?;

    // --- Vídeo (lo último que se muestra en pantalla con la consola del firmware) ------
    let fb = video::setup(cfg.resolution);
    if let Some(f) = fb.as_ref() {
        max_phys = max_phys.max(f.base + f.size);
    }
    max_phys = align_up(max_phys, 1 << 30);

    // --- Tablas de páginas --------------------------------------------------------------
    let nx = cpu::has_nx();
    let mut pt = paging::PageTables::new(nx, cpu::has_gib_pages())
        .map_err(BootError::uefi("sin memoria para tablas de páginas"))?;
    pt.map_physical(0, max_phys).map_err(BootError::uefi("sin memoria para tablas de páginas"))?;
    pt.map_physical(HHDM_BASE_X86_64, max_phys).map_err(BootError::uefi("sin memoria para tablas de páginas"))?;
    for page in (0..kernel.size).step_by(4096) {
        let virt = kernel.virt_base + page;
        let (w, x) = kernel.permissions(virt);
        pt.map_4k(virt, kernel.phys_base + page, w, x)
            .map_err(BootError::uefi("sin memoria para tablas de páginas"))?;
    }
    if let Some(f) = fb.as_ref() {
        step("Vídeo (GOP)", format_args!("{}x{} · {} bpp", f.width, f.height, f.bpp));
    }
    step(
        "Tablas de páginas",
        format_args!(
            "{} GiB en HHDM · {} tablas · NX {}",
            max_phys >> 30,
            pt.table_pages,
            if nx { "sí" } else { "no" }
        ),
    );

    // --- Rellenar BootInfo (salvo el mapa, que sólo es definitivo tras salir) -----------
    let hhdm = |phys: u64| HHDM_BASE_X86_64 + phys;
    // SAFETY: bloque propio recién reservado, mapeado en identidad por UEFI.
    let (info, cmd_buf, name_buf, regions) = unsafe {
        let base = info_phys as *mut u8;
        (
            &mut *(base as *mut BootInfo),
            core::slice::from_raw_parts_mut(base.add(1024), CMDLINE_MAX),
            core::slice::from_raw_parts_mut(base.add(1024 + CMDLINE_MAX), 64),
            core::slice::from_raw_parts_mut(base.add(1024 + CMDLINE_MAX + 64) as *mut MemoryRegion, capacity),
        )
    };
    *info = BootInfo::empty();
    info.source = BootSource::KOPITBOOT_UEFI;
    info.hhdm_base = HHDM_BASE_X86_64;
    info.hhdm_limit = max_phys;
    info.kernel = KernelImage { phys_base: kernel.phys_base, virt_base: kernel.virt_base, size: kernel.size };
    info.framebuffer = fb.unwrap_or(Framebuffer::NONE);
    info.rsdp = rsdp.unwrap_or(0);
    info.smbios = smbios.unwrap_or(0);
    info.efi_system_table = firmware::system_table_ptr() as u64;
    let cmd = cmdline.as_bytes();
    let cmd_len = cmd.len().min(CMDLINE_MAX);
    cmd_buf[..cmd_len].copy_from_slice(&cmd[..cmd_len]);
    info.cmdline = BootStr { ptr: hhdm(info_phys + 1024), len: cmd_len as u64 };
    let name = alloc::format!("KopitBoot {}", VERSION);
    name_buf[..name.len()].copy_from_slice(name.as_bytes());
    info.loader_name = BootStr { ptr: hhdm(info_phys + 1024 + CMDLINE_MAX as u64), len: name.len() as u64 };
    if let Some(t) = time {
        info.boot_time = WallClock {
            year: t.year,
            month: t.month,
            day: t.day,
            hour: t.hour,
            minute: t.minute,
            second: t.second,
            valid: 1,
        };
    }
    info.loader_timestamp = t0;
    info.secure_boot = secure_boot;
    info.kernel_verification = verification;

    step("Saliendo del firmware", format_args!("ExitBootServices"));

    // --- Punto de no retorno ----------------------------------------------------------
    firmware::exit_boot_services(&mut map).map_err(BootError::uefi("ExitBootServices falló"))?;

    // A partir de aquí: nada de memoria dinámica ni consola del firmware.
    let mut builder = MemoryMapBuilder::new(regions);
    for d in map.descriptors() {
        let kind = match d.kind {
            memtype::CONVENTIONAL => MemoryKind::USABLE,
            memtype::LOADER_CODE | memtype::LOADER_DATA | memtype::BOOT_SERVICES_CODE | memtype::BOOT_SERVICES_DATA => {
                MemoryKind::BOOTLOADER_RECLAIMABLE
            }
            memtype::KOPIT_BOOTINFO | memtype::KOPIT_PAGETABLES => MemoryKind::BOOTLOADER_RECLAIMABLE,
            memtype::KOPIT_KERNEL => MemoryKind::KERNEL,
            memtype::KOPIT_MODULE => MemoryKind::MODULE,
            memtype::RUNTIME_SERVICES_CODE | memtype::RUNTIME_SERVICES_DATA => MemoryKind::FIRMWARE_RUNTIME,
            memtype::ACPI_RECLAIM => MemoryKind::ACPI_RECLAIMABLE,
            memtype::ACPI_NVS => MemoryKind::ACPI_NVS,
            memtype::UNUSABLE => MemoryKind::BAD,
            memtype::MMIO | memtype::MMIO_PORT_SPACE => MemoryKind::MMIO,
            memtype::PERSISTENT => MemoryKind::PERSISTENT,
            _ => MemoryKind::RESERVED,
        };
        if builder.add(d.physical_start, d.number_of_pages * 4096, kind).is_err() {
            break;
        }
    }
    if info.framebuffer.is_present() {
        let _ = builder.add(info.framebuffer.base, info.framebuffer.size, MemoryKind::FRAMEBUFFER);
    }
    let count = builder.finish().unwrap_or(0);
    info.memory_map =
        kopit_boot_proto::MemoryMap { entries: hhdm(info_phys + 1024 + CMDLINE_MAX as u64 + 64), count: count as u64 };

    if nx {
        cpu::wrmsr(cpu::IA32_EFER, cpu::rdmsr(cpu::IA32_EFER) | cpu::EFER_NXE);
    }
    let stack_top = hhdm(stack_phys + (KERNEL_STACK_PAGES * 4096) as u64);
    // SAFETY: las tablas mapean este código (identidad), la pila y el núcleo.
    unsafe { cpu::enter_kernel(pt.pml4, stack_top, kernel.entry, hhdm(info_phys)) }
}

/// Núcleo cargado en memoria física contigua.
struct LoadedKernel {
    entry: u64,
    phys_base: u64,
    virt_base: u64,
    size: u64,
    /// (inicio virtual, fin virtual, escribible, ejecutable) de cada PT_LOAD.
    segments: [(u64, u64, bool, bool); 8],
    segment_count: usize,
}

impl LoadedKernel {
    /// Permisos de una página: unión de los segmentos que la tocan (W^X real
    /// si el enlazador alinea los segmentos a página, como hace nuestro guion).
    fn permissions(&self, page: u64) -> (bool, bool) {
        let mut w = false;
        let mut x = false;
        for &(start, end, sw, sx) in &self.segments[..self.segment_count] {
            if page < end && page + 4096 > start {
                w |= sw;
                x |= sx;
            }
        }
        (w, x)
    }
}

fn load_elf(data: &[u8]) -> Result<LoadedKernel, BootError> {
    use kopit_elf::{EM_X86_64, ET_EXEC, Elf64};
    let elf = Elf64::parse(data).map_err(|e| BootError::new(e.message()))?;
    if elf.machine != EM_X86_64 || elf.elf_type != ET_EXEC {
        return Err(BootError::new("el núcleo no es un ELF64 ejecutable para x86_64"));
    }
    // Sólo segmentos de la mitad alta: es lo que KopitBoot carga (reubicándolos
    // en cualquier dirección física libre).
    let higher_half = |v: u64| v >= 0xFFFF_8000_0000_0000;
    let mut lo = u64::MAX;
    let mut hi = 0;
    let mut segments = [(0u64, 0u64, false, false); 8];
    let mut segment_count = 0;
    for ph in elf.program_headers().filter(|p| p.is_load() && p.memsz > 0 && higher_half(p.vaddr)) {
        lo = lo.min(ph.vaddr);
        hi = hi.max(ph.vaddr + ph.memsz);
        if segment_count < segments.len() {
            segments[segment_count] = (ph.vaddr, ph.vaddr + ph.memsz, ph.writable(), ph.executable());
            segment_count += 1;
        }
    }
    if lo >= hi || !(lo..hi).contains(&elf.entry) {
        return Err(BootError::new("el ELF del núcleo no tiene segmentos cargables válidos"));
    }
    let virt_base = align_down(lo, 4096);
    let size = align_up(hi, 4096) - virt_base;
    let phys_base = firmware::allocate_zeroed_pages(memtype::KOPIT_KERNEL, (size / 4096) as usize)
        .map_err(BootError::uefi("sin memoria para el núcleo"))?;
    for ph in elf.program_headers().filter(|p| p.is_load() && p.memsz > 0 && higher_half(p.vaddr)) {
        let bytes = elf.segment_data(&ph).map_err(|e| BootError::new(e.message()))?;
        let dst = phys_base + (ph.vaddr - virt_base);
        // SAFETY: destino dentro del bloque reservado (memsz >= filesz; el resto ya es 0 = .bss).
        unsafe { core::ptr::copy_nonoverlapping(bytes.as_ptr(), dst as *mut u8, bytes.len()) };
    }
    Ok(LoadedKernel { entry: elf.entry, phys_base, virt_base, size, segments, segment_count })
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    if firmware::boot_services_active() {
        firmware::set_color(LIGHTRED, BLACK);
    }
    println!("\n  KopitBoot: error interno: {}", info);
    cpu::halt_forever()
}
