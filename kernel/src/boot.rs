//! # Información de arranque
//!
//! El `BootInfo` que entregó el cargador vive en memoria *recuperable*: el
//! núcleo la devolverá al asignador. Por eso aquí se valida y se **copia** lo
//! que hará falta después (estructura, mapa de memoria, nombre del cargador).

use crate::sync::RacyCell;
use kopit_boot_proto::{BootInfo, MemoryKind, MemoryRegion};

const MAX_REGIONS: usize = 512;

static INFO: RacyCell<BootInfo> = RacyCell::new(BootInfo::empty());
static REGIONS: RacyCell<[MemoryRegion; MAX_REGIONS]> = RacyCell::new([MemoryRegion::EMPTY; MAX_REGIONS]);
static REGION_COUNT: RacyCell<usize> = RacyCell::new(0);
static LOADER: RacyCell<[u8; 64]> = RacyCell::new([0; 64]);
static LOADER_LEN: RacyCell<usize> = RacyCell::new(0);

/// Valida y copia el `BootInfo` (una sola vez, en el arranque).
pub fn init(info: &BootInfo) -> Result<(), &'static str> {
    info.validate()?;
    // SAFETY: arranque, un solo hilo; los punteros del BootInfo son válidos ahora.
    unsafe {
        *INFO.get() = *info;
        let src = info.memory_map.regions();
        let n = src.len().min(MAX_REGIONS);
        (&mut *REGIONS.get())[..n].copy_from_slice(&src[..n]);
        *REGION_COUNT.get() = n;
        let name = info.loader_name.as_str();
        let len = name.len().min(64);
        (&mut *LOADER.get())[..len].copy_from_slice(&name.as_bytes()[..len]);
        *LOADER_LEN.get() = len;
        crate::cmdline::init(info.cmdline.as_str());
    }
    Ok(())
}

pub fn info() -> &'static BootInfo {
    // SAFETY: sólo se escribe en `init`.
    unsafe { &*INFO.get() }
}

/// Mapa de memoria (copia propia del núcleo).
pub fn memory_map() -> &'static [MemoryRegion] {
    // SAFETY: sólo se escribe en `init`.
    unsafe { &(&*REGIONS.get())[..*REGION_COUNT.get()] }
}

pub fn loader_name() -> &'static str {
    // SAFETY: sólo se escribe en `init` (UTF-8 copiado de una &str).
    unsafe { core::str::from_utf8(&(&*LOADER.get())[..*LOADER_LEN.get()]).unwrap_or("?") }
}

/// RAM instalada (suma de regiones que son memoria de verdad).
pub fn installed_ram() -> u64 {
    memory_map().iter().filter(|r| r.kind.is_ram()).map(|r| r.length).sum()
}

/// Mayor dirección física de RAM.
pub fn max_ram_address() -> u64 {
    memory_map().iter().filter(|r| r.kind.is_ram()).map(|r| r.end()).max().unwrap_or(0)
}

/// Bytes de un tipo de región.
pub fn bytes_of(kind: MemoryKind) -> u64 {
    memory_map().iter().filter(|r| r.kind == kind).map(|r| r.length).sum()
}

/// Imprime el mapa de memoria en el registro.
pub fn log_memory_map() {
    kdebug!("mapa de memoria ({} regiones):", memory_map().len());
    for r in memory_map() {
        kdebug!("  {:#014x}-{:#014x} {:>9} KiB  {}", r.base, r.end(), r.length / 1024, r.kind.name());
    }
}
