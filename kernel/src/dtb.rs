//! # Device tree → KBP (ARM64 y RISC-V)
//!
//! En estas arquitecturas, sin UEFI, el firmware (OpenSBI) o QEMU describen
//! la máquina con un árbol de dispositivos. Aquí se traduce al mismo
//! `BootInfo` que construyen KopitBoot y los adaptadores Multiboot2/PVH:
//! `kernel_main` no distingue cómo arrancó.
//!
//! Fase 1: la MMU está en identidad (física = virtual), así que el "mapa
//! directo" empieza en 0.

use crate::mm::layout;
use crate::sync::RacyCell;
use kopit_boot_proto::*;
use kopit_fdt::Fdt;

static INFO: RacyCell<BootInfo> = RacyCell::new(BootInfo::empty());
static REGIONS: RacyCell<[MemoryRegion; 128]> = RacyCell::new([MemoryRegion::EMPTY; 128]);
static CMDLINE: RacyCell<[u8; 1024]> = RacyCell::new([0; 1024]);
static LOADER: RacyCell<[u8; 64]> = RacyCell::new([0; 64]);

/// El DTB como slice (vacío si la firma no es válida).
pub fn blob(dtb_phys: u64) -> &'static [u8] {
    if dtb_phys == 0 {
        return &[];
    }
    // SAFETY: memoria identidad; primero sólo la cabecera para leer el tamaño.
    let header = unsafe { core::slice::from_raw_parts(dtb_phys as *const u8, 40) };
    match kopit_fdt::total_size(header) {
        // SAFETY: el tamaño lo declara el propio DTB (QEMU/OpenSBI lo dejan entero en RAM).
        Ok(size) if size < 16 << 20 => unsafe { core::slice::from_raw_parts(dtb_phys as *const u8, size) },
        _ => &[],
    }
}

/// Primer `reg` de un nodo compatible (dirección MMIO de un dispositivo).
pub fn device_base(dtb_phys: u64, compats: &[&str]) -> Option<u64> {
    let fdt = Fdt::parse(blob(dtb_phys)).ok()?;
    let node = fdt.find_compatible(compats)?;
    node.reg().next().map(|(b, _)| b)
}

/// Propiedad `u32` de un nodo por nombre base (p. ej. `cpus` / `timebase-frequency`).
pub fn node_u32(dtb_phys: u64, node: &str, prop: &str) -> Option<u32> {
    let fdt = Fdt::parse(blob(dtb_phys)).ok()?;
    fdt.find(|n| n.base_name() == node).and_then(|n| n.prop_u32(prop))
}

/// Cadena de una propiedad del primer nodo con ese nombre base.
pub fn node_str(dtb_phys: u64, node: &str, prop: &str) -> Option<&'static str> {
    let data = blob(dtb_phys);
    let fdt = Fdt::parse(data).ok()?;
    let found = fdt.find(|n| n.base_name() == node).and_then(|n| n.prop_str(prop))?;
    // La cadena vive dentro de `data`, que es 'static.
    let off = found.as_ptr() as usize - data.as_ptr() as usize;
    core::str::from_utf8(&data[off..off + found.len()]).ok()
}

pub fn build(dtb_phys: u64, loader_name: &str) -> &'static BootInfo {
    // SAFETY: arranque, un solo hilo.
    let (info, regions, cmdline, loader) =
        unsafe { (&mut *INFO.get(), &mut *REGIONS.get(), &mut *CMDLINE.get(), &mut *LOADER.get()) };
    *info = BootInfo::empty();
    info.source = BootSource::DEVICE_TREE;
    info.dtb = dtb_phys;
    info.hhdm_base = 0;
    let (kstart, kend) = (layout::kernel_start(), layout::kernel_end());
    info.kernel = KernelImage { phys_base: kstart, virt_base: kstart, size: kend - kstart };

    let n = loader_name.len().min(loader.len());
    loader[..n].copy_from_slice(&loader_name.as_bytes()[..n]);
    info.loader_name = BootStr { ptr: loader.as_ptr() as u64, len: n as u64 };

    let data = blob(dtb_phys);
    let mut map = MemoryMapBuilder::new(regions);
    let mut max_ram = 0u64;
    if let Ok(fdt) = Fdt::parse(data) {
        let _ = fdt.for_each_memory(|base, size| {
            let _ = map.add(base, size, MemoryKind::USABLE);
            max_ram = max_ram.max(base + size);
        });
        for (base, size) in fdt.reservations() {
            let _ = map.add(base, size, MemoryKind::RESERVED);
        }
        let _ = fdt.for_each_reserved(|base, size| {
            let _ = map.add(base, size, MemoryKind::RESERVED);
        });
        if let Some(args) = fdt.bootargs() {
            let len = args.len().min(cmdline.len());
            cmdline[..len].copy_from_slice(&args.as_bytes()[..len]);
            info.cmdline = BootStr { ptr: cmdline.as_ptr() as u64, len: len as u64 };
        }
    }
    let _ = map.add(dtb_phys, data.len().max(4096) as u64, MemoryKind::RESERVED);
    let _ = map.add(info.kernel.phys_base, info.kernel.size, MemoryKind::KERNEL);
    let count = map.finish().unwrap_or(0);
    info.memory_map = MemoryMap { entries: regions.as_ptr() as u64, count: count as u64 };
    info.hhdm_limit = max_ram;
    info
}
