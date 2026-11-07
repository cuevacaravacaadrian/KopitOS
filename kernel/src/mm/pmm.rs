//! # Administrador de memoria física (marcos de 4 KiB)
//!
//! Envuelve `kopit-buddy` con lo propio del núcleo:
//!
//! * los enlaces de las listas libres se escriben **dentro** de los marcos
//!   libres, a través del mapa directo (HHDM);
//! * los metadatos (≈ 2 bits por marco) se colocan en la primera región libre
//!   que el cargador ya dejó mapeada, sin necesitar el montículo;
//! * **arranque en dos fases**: antes de que el núcleo tenga sus propias
//!   tablas, sólo es accesible la RAM que mapeó el cargador (`hhdm_limit`).
//!   La RAM por encima se apunta como *diferida* y se añade (hot-add) en
//!   cuanto el mapa directo la cubre;
//! * el primer MiB se reserva: allí irán los trampolines de arranque de las
//!   demás CPUs (SMP, fase 2), que deben estar en memoria baja.

use super::{PAGE_SIZE, align_up, phys_to_virt};
use crate::sync::IrqLock;
use kopit_boot_proto::{MemoryKind, MemoryRegion};
use kopit_buddy::{FrameLinks, Link, ZONE_COUNT, ZONE_NAMES, Zone, ZoneStats, ZonedAllocator, zone_extents};

/// Enlaces dentro de los marcos libres, vía HHDM.
pub struct HhdmLinks;

impl FrameLinks for HhdmLinks {
    unsafe fn load(&self, frame: u64) -> Link {
        // SAFETY: `frame` es un bloque libre del asignador, mapeado en el HHDM.
        unsafe { (phys_to_virt(frame) as *const Link).read() }
    }
    unsafe fn store(&self, frame: u64, link: Link) {
        // SAFETY: ídem.
        unsafe { (phys_to_virt(frame) as *mut Link).write(link) }
    }
}

const LOW_RESERVE: u64 = 1 << 20;
const MAX_DEFERRED: usize = 64;

struct Pmm {
    zones: ZonedAllocator<'static, HhdmLinks>,
    /// Sólo se reparte memoria por debajo de este límite (u64::MAX = sin límite).
    limit: u64,
    deferred: [(u64, u64); MAX_DEFERRED],
    deferred_count: usize,
    bitmap_bytes: u64,
}

static PMM: IrqLock<Option<Pmm>> = IrqLock::new(None);

#[derive(Clone, Copy, Debug)]
pub struct PmmStats {
    pub zones: [Option<ZoneStats>; ZONE_COUNT],
    pub free_bytes: u64,
    pub managed_bytes: u64,
    pub bitmap_bytes: u64,
    pub deferred_bytes: u64,
}

fn usable(map: &[MemoryRegion]) -> impl Iterator<Item = (u64, u64)> + Clone + '_ {
    map.iter()
        .filter(|r| r.kind == MemoryKind::USABLE)
        .map(|r| (r.base.max(LOW_RESERVE), r.end()))
        .filter(|(s, e)| e > s)
}

pub fn init(map: &[MemoryRegion], hhdm_limit: u64) -> Result<PmmStats, &'static str> {
    let extents = zone_extents(usable(map));
    let mut words = [0usize; ZONE_COUNT];
    for (z, e) in extents.iter().enumerate() {
        if let Some((b, e)) = e {
            words[z] = Zone::bitmap_words(*b, *e);
        }
    }
    let bitmap_bytes = align_up(words.iter().sum::<usize>() as u64 * 8, PAGE_SIZE);
    if bitmap_bytes == 0 {
        return Err("el mapa de memoria no tiene RAM libre");
    }

    // Hueco para los metadatos: memoria libre ya mapeada por el cargador.
    let storage = usable(map)
        .map(|(s, e)| (align_up(s, PAGE_SIZE), e.min(hhdm_limit)))
        .find(|&(s, e)| e > s && e - s >= bitmap_bytes)
        .map(|(s, _)| s)
        .ok_or("no hay hueco para los metadatos del asignador")?;
    // SAFETY: región libre y mapeada (por debajo de hhdm_limit), aún sin dueño.
    let mut rest: &'static mut [u64] = unsafe {
        let p = phys_to_virt(storage) as *mut u64;
        core::ptr::write_bytes(p, 0, (bitmap_bytes / 8) as usize);
        core::slice::from_raw_parts_mut(p, (bitmap_bytes / 8) as usize)
    };

    let mut zones = ZonedAllocator::new(HhdmLinks);
    for z in 0..ZONE_COUNT {
        if let Some((b, e)) = extents[z] {
            let (mine, tail) = rest.split_at_mut(words[z]);
            rest = tail;
            zones.set_zone(z, Zone::new(ZONE_NAMES[z], b, e, mine));
        }
    }

    let mut pmm = Pmm { zones, limit: hhdm_limit, deferred: [(0, 0); MAX_DEFERRED], deferred_count: 0, bitmap_bytes };
    let storage_end = storage + bitmap_bytes;
    for (s, e) in usable(map) {
        // Quitar el hueco de los metadatos.
        let pieces = [(s, e.min(storage)), (s.max(storage_end), e)];
        for (ps, pe) in pieces {
            if pe <= ps {
                continue;
            }
            pmm.add_or_defer(ps, pe);
        }
    }
    let stats = pmm.stats();
    *PMM.lock() = Some(pmm);
    Ok(stats)
}

impl Pmm {
    fn add_or_defer(&mut self, s: u64, e: u64) {
        let below_end = e.min(self.limit);
        if below_end > s {
            self.zones.add_range(s, below_end);
        }
        let above_start = s.max(self.limit);
        if e > above_start {
            if self.deferred_count < MAX_DEFERRED {
                self.deferred[self.deferred_count] = (above_start, e);
                self.deferred_count += 1;
            }
        }
    }

    fn stats(&self) -> PmmStats {
        let deferred_bytes = self.deferred[..self.deferred_count].iter().map(|(s, e)| e - s).sum();
        PmmStats {
            zones: self.zones.stats(),
            free_bytes: self.zones.free_frames() * PAGE_SIZE,
            managed_bytes: self.zones.managed_frames() * PAGE_SIZE,
            bitmap_bytes: self.bitmap_bytes,
            deferred_bytes,
        }
    }
}

/// El mapa directo ya cubre toda la RAM: entra la memoria diferida.
/// Devuelve los bytes añadidos.
pub fn extend_direct_map() -> u64 {
    let mut guard = PMM.lock();
    let Some(p) = guard.as_mut() else { return 0 };
    p.limit = u64::MAX;
    let mut added = 0;
    for i in 0..p.deferred_count {
        let (s, e) = p.deferred[i];
        added += p.zones.add_range(s, e) * PAGE_SIZE;
    }
    p.deferred_count = 0;
    added
}

/// Devuelve al asignador memoria libre (p. ej. la del cargador). Devuelve bytes.
pub fn add_region(start: u64, end: u64) -> u64 {
    let mut guard = PMM.lock();
    let Some(p) = guard.as_mut() else { return 0 };
    let before = p.zones.managed_frames();
    p.add_or_defer(start.max(LOW_RESERVE), end);
    (p.zones.managed_frames() - before) * PAGE_SIZE
}

/// Reserva `2^order` marcos contiguos.
pub fn alloc_frames(order: usize) -> Option<u64> {
    let mut guard = PMM.lock();
    let p = guard.as_mut()?;
    if p.limit == u64::MAX { p.zones.alloc(order) } else { p.zones.alloc_below(order, p.limit) }
}

/// Reserva por debajo de una dirección (DMA de 32 bits, trampolines...).
pub fn alloc_frames_below(order: usize, limit: u64) -> Option<u64> {
    let mut guard = PMM.lock();
    let p = guard.as_mut()?;
    p.zones.alloc_below(order, limit.min(p.limit))
}

pub fn alloc_frame() -> Option<u64> {
    alloc_frames(0)
}

/// Marcos puestos a cero.
pub fn alloc_zeroed(order: usize) -> Option<u64> {
    let f = alloc_frames(order)?;
    // SAFETY: bloque recién asignado y mapeado en el HHDM.
    unsafe { core::ptr::write_bytes(phys_to_virt(f) as *mut u8, 0, (PAGE_SIZE << order) as usize) };
    Some(f)
}

pub fn free_frames(addr: u64, order: usize) {
    if let Some(p) = PMM.lock().as_mut() {
        p.zones.free(addr, order);
    }
}

pub fn stats() -> Option<PmmStats> {
    PMM.lock().as_ref().map(|p| p.stats())
}
