//! # Adaptadores Multiboot2 y PVH → KBP
//!
//! Tras el trampolín de 32 bits, `kopit_boot32_entry64` recibe el protocolo y
//! la dirección física de la información del cargador. Aquí se traduce a un
//! [`BootInfo`] idéntico al de KopitBoot, así que `kernel_main` no sabe (ni
//! necesita saber) quién lo arrancó.
//!
//! El trampolín mapeó la física 0–4 GiB en el HHDM: todo lo que entregan GRUB
//! y QEMU está ahí.

use crate::mm::layout;
use crate::sync::RacyCell;
use kopit_boot_proto::*;

static INFO: RacyCell<BootInfo> = RacyCell::new(BootInfo::empty());
static REGIONS: RacyCell<[MemoryRegion; 256]> = RacyCell::new([MemoryRegion::EMPTY; 256]);
static CMDLINE: RacyCell<[u8; 1024]> = RacyCell::new([0; 1024]);
static LOADER: RacyCell<[u8; 64]> = RacyCell::new([0; 64]);
static MODULES: RacyCell<[Module; 8]> = RacyCell::new([Module { base: 0, size: 0, name: BootStr::EMPTY }; 8]);

const PROTOCOL_MULTIBOOT2: u32 = 1;
const PROTOCOL_PVH: u32 = 2;
const PVH_MAGIC: u32 = 0x336E_C578;

fn virt(phys: u64) -> u64 {
    HHDM_BASE_X86_64 + phys
}

fn r8(p: u64) -> u8 {
    // SAFETY: memoria del cargador mapeada en el HHDM por el trampolín.
    unsafe { (virt(p) as *const u8).read() }
}
fn r32(p: u64) -> u32 {
    // SAFETY: ídem.
    unsafe { (virt(p) as *const u32).read_unaligned() }
}
fn r64(p: u64) -> u64 {
    // SAFETY: ídem.
    unsafe { (virt(p) as *const u64).read_unaligned() }
}

/// Copia una cadena C (física) a un búfer propio. Devuelve su longitud.
fn copy_cstr(phys: u64, max: usize, dst: &mut [u8]) -> usize {
    let mut n = 0;
    while n < max.min(dst.len()) {
        let b = r8(phys + n as u64);
        if b == 0 {
            break;
        }
        dst[n] = b;
        n += 1;
    }
    // Recortar a un límite UTF-8 válido.
    while core::str::from_utf8(&dst[..n]).is_err() && n > 0 {
        n -= 1;
    }
    n
}

fn set_str(field: &mut BootStr, buf: &[u8], len: usize) {
    *field = BootStr { ptr: buf.as_ptr() as u64, len: len as u64 };
}

pub fn build(protocol: u32, info_phys: u64) -> &'static BootInfo {
    // SAFETY: arranque, un solo hilo; estos estáticos sólo se usan aquí.
    let (info, regions, cmdline, loader, modules) = unsafe {
        (&mut *INFO.get(), &mut *REGIONS.get(), &mut *CMDLINE.get(), &mut *LOADER.get(), &mut *MODULES.get())
    };
    *info = BootInfo::empty();
    info.hhdm_base = HHDM_BASE_X86_64;
    info.hhdm_limit = 4 << 30;
    let kstart = layout::kernel_start();
    let kend = layout::kernel_end();
    info.kernel = KernelImage { phys_base: kstart - KERNEL_VMA_X86_64, virt_base: kstart, size: kend - kstart };

    let mut map = MemoryMapBuilder::new(regions);
    let mut module_count = 0usize;
    match protocol {
        PROTOCOL_MULTIBOOT2 => {
            info.source = BootSource::MULTIBOOT2;
            parse_multiboot2(info_phys, info, &mut map, cmdline, loader, modules, &mut module_count);
        }
        PROTOCOL_PVH => {
            info.source = BootSource::PVH;
            parse_pvh(info_phys, info, &mut map, cmdline, modules, &mut module_count);
            let name = b"QEMU/Firecracker (PVH)";
            loader[..name.len()].copy_from_slice(name);
            set_str(&mut info.loader_name, loader, name.len());
        }
        _ => {}
    }
    let _ = map.add(info.kernel.phys_base, info.kernel.size, MemoryKind::KERNEL);
    for m in &modules[..module_count] {
        let _ = map.add(m.base, m.size, MemoryKind::MODULE);
    }
    let count = map.finish().unwrap_or(0);
    info.memory_map = MemoryMap { entries: regions.as_ptr() as u64, count: count as u64 };
    info.modules = ModuleList { entries: modules.as_ptr() as u64, count: module_count as u64 };
    info
}

#[allow(clippy::too_many_arguments)]
fn parse_multiboot2(
    base: u64,
    info: &mut BootInfo,
    map: &mut MemoryMapBuilder<'_>,
    cmdline: &'static mut [u8; 1024],
    loader: &'static mut [u8; 64],
    modules: &mut [Module; 8],
    module_count: &mut usize,
) {
    let total = r32(base) as u64;
    let _ = map.add(base, total, MemoryKind::BOOTLOADER_RECLAIMABLE);
    let mut off = 8u64;
    let mut have_new_rsdp = false;
    while off + 8 <= total {
        let tag = base + off;
        let (kind, size) = (r32(tag), r32(tag + 4) as u64);
        if kind == 0 || size < 8 {
            break;
        }
        match kind {
            1 => {
                let n = copy_cstr(tag + 8, (size - 8) as usize, cmdline);
                set_str(&mut info.cmdline, cmdline, n);
            }
            2 => {
                let n = copy_cstr(tag + 8, (size - 8) as usize, loader);
                set_str(&mut info.loader_name, loader, n);
            }
            3 if *module_count < modules.len() => {
                let (start, end) = (r32(tag + 8) as u64, r32(tag + 12) as u64);
                modules[*module_count] = Module { base: start, size: end.saturating_sub(start), name: BootStr::EMPTY };
                *module_count += 1;
            }
            6 => {
                let entry_size = r32(tag + 8) as u64;
                let mut e = tag + 16;
                while entry_size >= 24 && e + entry_size <= tag + size {
                    let (addr, len, t) = (r64(e), r64(e + 8), r32(e + 16));
                    let k = match t {
                        1 => MemoryKind::USABLE,
                        3 => MemoryKind::ACPI_RECLAIMABLE,
                        4 => MemoryKind::ACPI_NVS,
                        5 => MemoryKind::BAD,
                        _ => MemoryKind::RESERVED,
                    };
                    let _ = map.add(addr, len, k);
                    e += entry_size;
                }
            }
            8 => {
                let addr = r64(tag + 8);
                let pitch = r32(tag + 16);
                let (w, h) = (r32(tag + 20), r32(tag + 24));
                let (bpp, fb_type) = (r8(tag + 28), r8(tag + 29));
                let mut fb = Framebuffer::NONE;
                fb.base = addr;
                fb.stride = pitch;
                fb.width = w;
                fb.height = h;
                fb.bpp = bpp as u16;
                fb.size = pitch as u64 * h as u64;
                match fb_type {
                    1 => {
                        fb.kind = FramebufferKind::RGB;
                        fb.red_shift = r8(tag + 32);
                        fb.red_bits = r8(tag + 33);
                        fb.green_shift = r8(tag + 34);
                        fb.green_bits = r8(tag + 35);
                        fb.blue_shift = r8(tag + 36);
                        fb.blue_bits = r8(tag + 37);
                    }
                    2 => fb.kind = FramebufferKind::EGA_TEXT,
                    _ => {}
                }
                if fb.kind == FramebufferKind::RGB {
                    let _ = map.add(fb.base, fb.size, MemoryKind::FRAMEBUFFER);
                }
                info.framebuffer = fb;
            }
            12 => info.efi_system_table = r64(tag + 8),
            14 if !have_new_rsdp => info.rsdp = tag + 8,
            15 => {
                info.rsdp = tag + 8;
                have_new_rsdp = true;
            }
            _ => {}
        }
        off += (size + 7) & !7;
    }
}

fn parse_pvh(
    base: u64,
    info: &mut BootInfo,
    map: &mut MemoryMapBuilder<'_>,
    cmdline: &'static mut [u8; 1024],
    modules: &mut [Module; 8],
    module_count: &mut usize,
) {
    if r32(base) != PVH_MAGIC {
        return;
    }
    let version = r32(base + 4);
    let _ = map.add(base & !0xFFF, 4096, MemoryKind::BOOTLOADER_RECLAIMABLE);
    let cmdline_phys = r64(base + 24);
    if cmdline_phys != 0 {
        let n = copy_cstr(cmdline_phys, 1023, cmdline);
        set_str(&mut info.cmdline, cmdline, n);
    }
    info.rsdp = r64(base + 32);
    let (nr_modules, modlist) = (r32(base + 12) as usize, r64(base + 16));
    for i in 0..nr_modules.min(modules.len()) {
        let e = modlist + i as u64 * 32;
        modules[i] = Module { base: r64(e), size: r64(e + 8), name: BootStr::EMPTY };
        *module_count += 1;
    }
    if version >= 1 {
        let (memmap, entries) = (r64(base + 40), r32(base + 48) as u64);
        for i in 0..entries {
            let e = memmap + i * 24;
            let (addr, size, t) = (r64(e), r64(e + 8), r32(e + 16));
            let k = match t {
                1 => MemoryKind::USABLE,
                3 => MemoryKind::ACPI_RECLAIMABLE,
                4 => MemoryKind::ACPI_NVS,
                5 => MemoryKind::BAD,
                7 => MemoryKind::PERSISTENT,
                _ => MemoryKind::RESERVED,
            };
            let _ = map.add(addr, size, k);
        }
    }
}
