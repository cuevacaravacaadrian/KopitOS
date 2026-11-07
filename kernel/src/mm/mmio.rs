//! # Ventanas de E/S mapeada en memoria
//!
//! Los registros de un dispositivo **no** son memoria normal: no deben pasar
//! por la caché (o deben agrupar escrituras, como un framebuffer). Por eso no
//! se usan a través del mapa directo, sino de ventanas propias con el tipo de
//! caché adecuado (PAT en x86_64).

use super::{MapError, MapFlags, PAGE_SIZE, align_down, align_up};
use crate::arch::paging::{AddressSpace, MMIO_WINDOW_BASE};
use core::sync::atomic::{AtomicU64, Ordering};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Cache {
    /// Registros de dispositivos.
    Uncached,
    /// Framebuffers: escrituras agrupadas, lecturas lentas.
    WriteCombining,
}

static NEXT: AtomicU64 = AtomicU64::new(MMIO_WINDOW_BASE);

fn flags(cache: Cache) -> MapFlags {
    let base = MapFlags::WRITE | MapFlags::GLOBAL;
    match cache {
        Cache::Uncached => base | MapFlags::NO_CACHE,
        Cache::WriteCombining => base | MapFlags::WRITE_COMBINE,
    }
}

/// Mapea en un espacio concreto (antes de activarlo).
pub fn map_into(space: &mut AddressSpace, phys: u64, size: u64, cache: Cache) -> Result<u64, MapError> {
    let start = align_down(phys, PAGE_SIZE);
    let len = align_up(phys + size.max(1), PAGE_SIZE) - start;
    // Cada ventana deja una página sin mapear detrás (guarda contra desbordes).
    let virt = NEXT.fetch_add(len + PAGE_SIZE, Ordering::Relaxed);
    space.map_range(virt, start, len, flags(cache))?;
    Ok(virt + (phys - start))
}

/// Mapea en el espacio del núcleo. Sin paginación propia (arquitecturas con la
/// MMU apagada en fase 1) devuelve la dirección del mapa directo.
pub fn map(phys: u64, size: u64, cache: Cache) -> Result<u64, MapError> {
    match super::paging::with_kernel_space(|s| map_into(s, phys, size, cache)) {
        Some(r) => r,
        None => Ok(super::phys_to_virt(phys)),
    }
}
