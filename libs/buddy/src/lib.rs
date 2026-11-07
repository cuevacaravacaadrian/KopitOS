//! # Asignador de marcos físicos "buddy" por zonas
//!
//! ## Por qué buddy
//!
//! Un mapa de bits es simple pero buscar un hueco es O(n) y no sabe dar bloques
//! contiguos grandes con eficiencia. El sistema de compañeros (buddy) da:
//!
//! * asignación y liberación en **O(log n)** (como mucho `MAX_ORDER` pasos);
//! * bloques contiguos de 4 KiB a 4 MiB **alineados a su tamaño** (necesario
//!   para páginas enormes, DMA y para el montículo del núcleo);
//! * fusión automática al liberar, que combate la fragmentación.
//!
//! ## Cómo se guarda el estado (sin memoria dinámica)
//!
//! * **Listas libres intrusivas**: el enlace `prev/next` de cada bloque libre
//!   se escribe *dentro del propio bloque* (el núcleo lo hace a través del mapa
//!   directo HHDM). Un bloque libre no ocupa memoria extra.
//! * **Un bit por bloque y orden** ("este bloque está libre con este orden"):
//!   ≈ 2 bits por marco de 4 KiB, es decir 64 KiB de metadatos por GiB.
//!   Con él, comprobar si el compañero está libre es O(1).
//!
//! ## Zonas
//!
//! Igual que Linux, la memoria se reparte en DMA (< 16 MiB, para dispositivos
//! ISA antiguos), DMA32 (< 4 GiB, para dispositivos de 32 bits) y Normal. Las
//! peticiones normales empiezan por la zona alta para dejar libres las bajas,
//! que son un recurso escaso.
//!
//! El acceso a la memoria de los marcos se abstrae con [`FrameLinks`]: en el
//! núcleo escribe por el HHDM y en las pruebas del anfitrión, en un `HashMap`.

#![cfg_attr(not(test), no_std)]

pub const PAGE_SHIFT: u32 = 12;
pub const PAGE_SIZE: u64 = 1 << PAGE_SHIFT;
/// Órdenes 0..=10: bloques de 4 KiB (orden 0) a 4 MiB (orden 10).
pub const MAX_ORDER: usize = 11;
/// Tamaño del bloque más grande: las zonas se alinean a él.
pub const MAX_BLOCK: u64 = PAGE_SIZE << (MAX_ORDER - 1);
/// Marca de "sin enlace" en las listas.
pub const NIL: u64 = u64::MAX;

/// Enlace de lista doble almacenado al principio de cada bloque libre.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Link {
    pub prev: u64,
    pub next: u64,
}

/// Cómo leer y escribir el enlace guardado dentro de un bloque libre.
pub trait FrameLinks {
    /// # Safety
    /// `frame` debe ser el inicio de un bloque libre propiedad del asignador.
    unsafe fn load(&self, frame: u64) -> Link;
    /// # Safety
    /// `frame` debe ser el inicio de un bloque libre propiedad del asignador.
    unsafe fn store(&self, frame: u64, link: Link);
}

/// Una zona de memoria física gestionada con buddy.
pub struct Zone<'b> {
    name: &'static str,
    base: u64,
    end: u64,
    heads: [u64; MAX_ORDER],
    counts: [u64; MAX_ORDER],
    bitmap: &'b mut [u64],
    offsets: [usize; MAX_ORDER],
    free_frames: u64,
    managed_frames: u64,
}

/// Estadísticas de una zona.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ZoneStats {
    pub name: &'static str,
    pub base: u64,
    pub end: u64,
    pub managed_frames: u64,
    pub free_frames: u64,
    /// Bloques libres por orden (muestra la fragmentación).
    pub free_blocks: [u64; MAX_ORDER],
}

impl<'b> Zone<'b> {
    /// Palabras de 64 bits de mapa de bits que necesita una zona `[base, end)`.
    pub fn bitmap_words(base: u64, end: u64) -> usize {
        let frames = end.saturating_sub(base) >> PAGE_SHIFT;
        let mut bits = 0u64;
        for k in 0..MAX_ORDER {
            bits += frames.div_ceil(1 << k);
        }
        bits.div_ceil(64) as usize
    }

    /// Crea una zona vacía (sin memoria libre todavía: ver [`Zone::add_range`]).
    ///
    /// `base` debe estar alineada a [`MAX_BLOCK`] para que todos los bloques
    /// queden alineados a su tamaño en direcciones absolutas.
    pub fn new(name: &'static str, base: u64, end: u64, bitmap: &'b mut [u64]) -> Self {
        assert!(base % MAX_BLOCK == 0, "la base de la zona debe estar alineada a 4 MiB");
        assert!(end > base && end % PAGE_SIZE == 0);
        assert!(bitmap.len() >= Self::bitmap_words(base, end), "mapa de bits demasiado pequeño");
        bitmap.fill(0);
        let frames = (end - base) >> PAGE_SHIFT;
        let mut offsets = [0usize; MAX_ORDER];
        let mut acc = 0usize;
        for (k, off) in offsets.iter_mut().enumerate() {
            *off = acc;
            acc += frames.div_ceil(1 << k) as usize;
        }
        Self {
            name,
            base,
            end,
            heads: [NIL; MAX_ORDER],
            counts: [0; MAX_ORDER],
            bitmap,
            offsets,
            free_frames: 0,
            managed_frames: 0,
        }
    }

    pub fn name(&self) -> &'static str {
        self.name
    }

    pub fn contains(&self, addr: u64) -> bool {
        addr >= self.base && addr < self.end
    }

    pub fn base(&self) -> u64 {
        self.base
    }

    pub fn end(&self) -> u64 {
        self.end
    }

    pub fn free_frames(&self) -> u64 {
        self.free_frames
    }

    pub fn stats(&self) -> ZoneStats {
        ZoneStats {
            name: self.name,
            base: self.base,
            end: self.end,
            managed_frames: self.managed_frames,
            free_frames: self.free_frames,
            free_blocks: self.counts,
        }
    }

    #[inline]
    fn bit(&self, order: usize, addr: u64) -> usize {
        self.offsets[order] + ((addr - self.base) >> (PAGE_SHIFT + order as u32)) as usize
    }

    #[inline]
    fn is_free_at(&self, order: usize, addr: u64) -> bool {
        let b = self.bit(order, addr);
        self.bitmap[b / 64] & (1 << (b % 64)) != 0
    }

    #[inline]
    fn mark(&mut self, order: usize, addr: u64, free: bool) {
        let b = self.bit(order, addr);
        if free {
            self.bitmap[b / 64] |= 1 << (b % 64);
        } else {
            self.bitmap[b / 64] &= !(1 << (b % 64));
        }
    }

    fn push<L: FrameLinks>(&mut self, links: &L, order: usize, addr: u64) {
        let head = self.heads[order];
        // SAFETY: `addr` (y `head`) son bloques libres de esta zona.
        unsafe {
            links.store(addr, Link { prev: NIL, next: head });
            if head != NIL {
                let mut h = links.load(head);
                h.prev = addr;
                links.store(head, h);
            }
        }
        self.heads[order] = addr;
        self.counts[order] += 1;
        self.mark(order, addr, true);
    }

    fn unlink<L: FrameLinks>(&mut self, links: &L, order: usize, addr: u64) {
        // SAFETY: `addr` está en la lista libre `order`, y sus vecinos también.
        unsafe {
            let l = links.load(addr);
            if l.prev == NIL {
                self.heads[order] = l.next;
            } else {
                let mut p = links.load(l.prev);
                p.next = l.next;
                links.store(l.prev, p);
            }
            if l.next != NIL {
                let mut n = links.load(l.next);
                n.prev = l.prev;
                links.store(l.next, n);
            }
        }
        self.counts[order] -= 1;
        self.mark(order, addr, false);
    }

    /// Reserva un bloque de `2^order` marcos. Devuelve su dirección física.
    pub fn alloc<L: FrameLinks>(&mut self, links: &L, order: usize) -> Option<u64> {
        if order >= MAX_ORDER {
            return None;
        }
        let mut k = order;
        while k < MAX_ORDER && self.heads[k] == NIL {
            k += 1;
        }
        if k == MAX_ORDER {
            return None;
        }
        let block = self.heads[k];
        self.unlink(links, k, block);
        // Partir el bloque: la mitad alta de cada división vuelve a la lista.
        while k > order {
            k -= 1;
            self.push(links, k, block + (PAGE_SIZE << k));
        }
        self.free_frames -= 1 << order;
        Some(block)
    }

    /// Reserva un bloque que termine por debajo de `limit` (arranque temprano:
    /// sólo es accesible la memoria que el cargador ya mapeó). O(n) en el peor
    /// caso; el camino rápido es [`Zone::alloc`].
    pub fn alloc_below<L: FrameLinks>(&mut self, links: &L, order: usize, limit: u64) -> Option<u64> {
        if order >= MAX_ORDER {
            return None;
        }
        for k in order..MAX_ORDER {
            let mut cur = self.heads[k];
            while cur != NIL {
                if cur + (PAGE_SIZE << order) <= limit {
                    self.unlink(links, k, cur);
                    let mut kk = k;
                    while kk > order {
                        kk -= 1;
                        self.push(links, kk, cur + (PAGE_SIZE << kk));
                    }
                    self.free_frames -= 1 << order;
                    return Some(cur);
                }
                // SAFETY: `cur` está en la lista libre.
                cur = unsafe { links.load(cur) }.next;
            }
        }
        None
    }

    /// Devuelve un bloque de `2^order` marcos y lo fusiona con su compañero
    /// mientras éste también esté libre.
    pub fn free<L: FrameLinks>(&mut self, links: &L, addr: u64, order: usize) {
        debug_assert!(self.contains(addr), "{:#x} no pertenece a la zona {}", addr, self.name);
        debug_assert!((addr - self.base) % (PAGE_SIZE << order) == 0, "bloque mal alineado");
        debug_assert!(!self.is_free_at(order, addr), "doble liberación de {:#x}", addr);
        self.free_frames += 1 << order;
        let mut addr = addr;
        let mut k = order;
        while k + 1 < MAX_ORDER {
            let size = PAGE_SIZE << k;
            let buddy = self.base + ((addr - self.base) ^ size);
            if buddy + size > self.end || !self.is_free_at(k, buddy) {
                break;
            }
            self.unlink(links, k, buddy);
            addr = addr.min(buddy);
            k += 1;
        }
        self.push(links, k, addr);
    }

    /// Entrega al asignador la RAM libre `[start, end)` (arranque o "hot-add").
    /// Devuelve cuántos marcos se añadieron.
    pub fn add_range<L: FrameLinks>(&mut self, links: &L, start: u64, end: u64) -> u64 {
        let start = align_up(start.max(self.base), PAGE_SIZE);
        let end = align_down(end.min(self.end), PAGE_SIZE);
        let mut a = start;
        let mut added = 0;
        while a < end {
            // El bloque más grande que esté alineado en `a` y quepa hasta `end`.
            let mut k = MAX_ORDER - 1;
            while k > 0 {
                let size = PAGE_SIZE << k;
                if (a - self.base) % size == 0 && a + size <= end {
                    break;
                }
                k -= 1;
            }
            self.managed_frames += 1 << k;
            self.free(links, a, k);
            added += 1 << k;
            a += PAGE_SIZE << k;
        }
        added
    }
}

/// Índices de zona.
pub const ZONE_DMA: usize = 0;
pub const ZONE_DMA32: usize = 1;
pub const ZONE_NORMAL: usize = 2;
pub const ZONE_COUNT: usize = 3;
/// Límites `[inicio, fin)` de cada zona.
pub const ZONE_LIMITS: [(u64, u64); ZONE_COUNT] = [(0, 16 << 20), (16 << 20, 4 << 30), (4 << 30, u64::MAX)];
pub const ZONE_NAMES: [&str; ZONE_COUNT] = ["DMA", "DMA32", "Normal"];

/// Calcula la extensión `[base, end)` que debe tener cada zona para cubrir
/// toda la RAM libre `ranges`, alineada a [`MAX_BLOCK`].
pub fn zone_extents(ranges: impl Iterator<Item = (u64, u64)> + Clone) -> [Option<(u64, u64)>; ZONE_COUNT] {
    let mut out = [None; ZONE_COUNT];
    for (z, &(lo, hi)) in ZONE_LIMITS.iter().enumerate() {
        let mut min = u64::MAX;
        let mut max = 0;
        for (s, e) in ranges.clone() {
            let s = s.max(lo);
            let e = e.min(hi);
            if e > s {
                min = min.min(s);
                max = max.max(e);
            }
        }
        if max > min {
            let base = align_down(min, MAX_BLOCK).max(lo);
            let end = align_up(max, MAX_BLOCK).min(hi);
            out[z] = Some((base, end));
        }
    }
    out
}

/// Varias zonas buddy con una política de reparto común.
pub struct ZonedAllocator<'b, L: FrameLinks> {
    zones: [Option<Zone<'b>>; ZONE_COUNT],
    links: L,
}

impl<'b, L: FrameLinks> ZonedAllocator<'b, L> {
    pub fn new(links: L) -> Self {
        Self { zones: [None, None, None], links }
    }

    pub fn set_zone(&mut self, index: usize, zone: Zone<'b>) {
        self.zones[index] = Some(zone);
    }

    pub fn zone(&self, index: usize) -> Option<&Zone<'b>> {
        self.zones[index].as_ref()
    }

    /// Añade RAM libre, repartiéndola entre las zonas que toca.
    pub fn add_range(&mut self, start: u64, end: u64) -> u64 {
        let mut added = 0;
        for zone in self.zones.iter_mut().flatten() {
            let s = start.max(zone.base());
            let e = end.min(zone.end());
            if e > s {
                added += zone.add_range(&self.links, s, e);
            }
        }
        added
    }

    /// Política general: Normal → DMA32 → DMA.
    pub fn alloc(&mut self, order: usize) -> Option<u64> {
        for z in (0..ZONE_COUNT).rev() {
            if let Some(zone) = self.zones[z].as_mut() {
                if let Some(a) = zone.alloc(&self.links, order) {
                    return Some(a);
                }
            }
        }
        None
    }

    /// Un bloque que termine por debajo de `limit` (p. ej. DMA32 o arranque).
    pub fn alloc_below(&mut self, order: usize, limit: u64) -> Option<u64> {
        for z in (0..ZONE_COUNT).rev() {
            if let Some(zone) = self.zones[z].as_mut() {
                if zone.base() >= limit {
                    continue;
                }
                let found = if zone.end() <= limit {
                    zone.alloc(&self.links, order)
                } else {
                    zone.alloc_below(&self.links, order, limit)
                };
                if found.is_some() {
                    return found;
                }
            }
        }
        None
    }

    pub fn free(&mut self, addr: u64, order: usize) {
        for zone in self.zones.iter_mut().flatten() {
            if zone.contains(addr) {
                zone.free(&self.links, addr, order);
                return;
            }
        }
        debug_assert!(false, "liberando {:#x}, que no pertenece a ninguna zona", addr);
    }

    pub fn free_frames(&self) -> u64 {
        self.zones.iter().flatten().map(|z| z.free_frames()).sum()
    }

    pub fn managed_frames(&self) -> u64 {
        self.zones.iter().flatten().map(|z| z.stats().managed_frames).sum()
    }

    pub fn stats(&self) -> [Option<ZoneStats>; ZONE_COUNT] {
        [
            self.zones[0].as_ref().map(|z| z.stats()),
            self.zones[1].as_ref().map(|z| z.stats()),
            self.zones[2].as_ref().map(|z| z.stats()),
        ]
    }
}

pub const fn align_up(v: u64, a: u64) -> u64 {
    (v + a - 1) & !(a - 1)
}

pub const fn align_down(v: u64, a: u64) -> u64 {
    v & !(a - 1)
}

/// Orden mínimo para `pages` páginas.
pub const fn order_for_pages(pages: u64) -> usize {
    if pages <= 1 { 0 } else { (64 - (pages - 1).leading_zeros()) as usize }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::{HashMap, HashSet};

    /// "Memoria física" simulada: sólo guarda los enlaces.
    #[derive(Default)]
    struct FakeRam(RefCell<HashMap<u64, Link>>);

    impl FrameLinks for FakeRam {
        unsafe fn load(&self, frame: u64) -> Link {
            *self.0.borrow().get(&frame).expect("enlace leído de un bloque no libre")
        }
        unsafe fn store(&self, frame: u64, link: Link) {
            self.0.borrow_mut().insert(frame, link);
        }
    }

    fn zone(base: u64, end: u64) -> (Zone<'static>, FakeRam) {
        let words = Zone::bitmap_words(base, end);
        let bitmap: &'static mut [u64] = Box::leak(vec![0u64; words].into_boxed_slice());
        (Zone::new("prueba", base, end, bitmap), FakeRam::default())
    }

    #[test]
    fn orden_para_paginas() {
        assert_eq!(order_for_pages(1), 0);
        assert_eq!(order_for_pages(2), 1);
        assert_eq!(order_for_pages(3), 2);
        assert_eq!(order_for_pages(1024), 10);
    }

    #[test]
    fn todo_libre_se_fusiona_en_bloques_maximos() {
        let (mut z, ram) = zone(0, 16 << 20);
        z.add_range(&ram, 0, 16 << 20);
        let s = z.stats();
        assert_eq!(s.free_frames, 4096);
        assert_eq!(s.free_blocks[MAX_ORDER - 1], 4);
    }

    #[test]
    fn partir_y_volver_a_fusionar() {
        let (mut z, ram) = zone(0, 4 << 20);
        z.add_range(&ram, 0, 4 << 20);
        let a = z.alloc(&ram, 0).unwrap();
        let b = z.alloc(&ram, 3).unwrap();
        let c = z.alloc(&ram, 0).unwrap();
        assert_eq!(a % PAGE_SIZE, 0);
        assert_eq!(b % (PAGE_SIZE << 3), 0, "los bloques se alinean a su tamaño");
        assert_ne!(a, c);
        z.free(&ram, b, 3);
        z.free(&ram, a, 0);
        z.free(&ram, c, 0);
        assert_eq!(z.free_frames(), 1024);
        assert_eq!(z.stats().free_blocks[MAX_ORDER - 1], 1);
    }

    #[test]
    fn rango_con_huecos_y_bordes_raros() {
        let (mut z, ram) = zone(0, 8 << 20);
        let added = z.add_range(&ram, 0x3000, 0x0050_1000);
        assert_eq!(added, (0x0050_1000 - 0x3000) / PAGE_SIZE);
        // Nunca se entrega memoria fuera del rango añadido.
        let mut seen = HashSet::new();
        while let Some(f) = z.alloc(&ram, 0) {
            assert!((0x3000..0x0050_1000).contains(&f));
            assert!(seen.insert(f));
        }
        assert_eq!(seen.len() as u64, added);
    }

    #[test]
    fn estres_aleatorio_sin_solapes() {
        let (mut z, ram) = zone(0, 32 << 20);
        z.add_range(&ram, 0x1000, 32 << 20);
        let total = z.free_frames();
        let mut rng = 0x1234_5678_9abc_def0u64;
        let mut next = || {
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;
            rng
        };
        let mut live: Vec<(u64, usize)> = Vec::new();
        let mut owned: HashSet<u64> = HashSet::new();
        for _ in 0..20_000 {
            if live.is_empty() || next() % 3 != 0 {
                let order = (next() % 6) as usize;
                if let Some(a) = z.alloc(&ram, order) {
                    for p in 0..(1u64 << order) {
                        assert!(owned.insert(a + p * PAGE_SIZE), "marco entregado dos veces");
                    }
                    live.push((a, order));
                }
            } else {
                let i = (next() as usize) % live.len();
                let (a, order) = live.swap_remove(i);
                for p in 0..(1u64 << order) {
                    owned.remove(&(a + p * PAGE_SIZE));
                }
                z.free(&ram, a, order);
            }
        }
        for (a, order) in live.drain(..) {
            z.free(&ram, a, order);
        }
        assert_eq!(z.free_frames(), total);
    }

    #[test]
    fn zonas_y_politica() {
        let ranges = [(0x1000u64, 0x9F000u64), (0x10_0000, 0x1_4000_0000)];
        let ext = zone_extents(ranges.iter().copied());
        assert_eq!(ext[ZONE_DMA], Some((0, 16 << 20)));
        assert_eq!(ext[ZONE_DMA32], Some((16 << 20, 4 << 30)));
        assert_eq!(ext[ZONE_NORMAL], Some((4 << 30, 0x1_4000_0000)));

        let mut za = ZonedAllocator::new(FakeRam::default());
        for (i, e) in ext.iter().enumerate() {
            let (b, e) = e.unwrap();
            let words = Zone::bitmap_words(b, e);
            let bm: &'static mut [u64] = Box::leak(vec![0u64; words].into_boxed_slice());
            za.set_zone(i, Zone::new(ZONE_NAMES[i], b, e, bm));
        }
        for (s, e) in ranges {
            za.add_range(s, e);
        }
        let normal = za.alloc(0).unwrap();
        assert!(normal >= 4 << 30, "la política normal empieza por arriba");
        let low = za.alloc_below(0, 4 << 30).unwrap();
        assert!(low < 4 << 30);
        let tiny = za.alloc_below(0, 1 << 20).unwrap();
        assert!(tiny < 1 << 20);
        za.free(normal, 0);
        za.free(low, 0);
        za.free(tiny, 0);
        assert_eq!(za.free_frames(), za.managed_frames());
    }
}
