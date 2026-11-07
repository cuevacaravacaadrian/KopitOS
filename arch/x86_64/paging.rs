//! # Paginación x86_64 (4 niveles)
//!
//! `PML4 → PDPT → PD → PT`, 512 entradas de 8 bytes por tabla. Las tablas se
//! piden al asignador de marcos y se modifican a través del mapa directo.
//!
//! Tipos de caché con PAT (ver `cpu::enable_features`):
//! `PWT=1` → write-combining (framebuffers), `PCD|PWT` → sin caché (MMIO).

use super::cpu;
use crate::mm::{MapError, MapFlags, PageSize, phys_to_virt, pmm};

/// Hay paginación propia del núcleo en esta arquitectura.
pub const ENABLED: bool = true;
/// Inicio de las ventanas MMIO (PML4[384]).
pub const MMIO_WINDOW_BASE: u64 = 0xFFFF_C000_0000_0000;

const PRESENT: u64 = 1 << 0;
const WRITABLE: u64 = 1 << 1;
const USER: u64 = 1 << 2;
const PWT: u64 = 1 << 3;
const PCD: u64 = 1 << 4;
const HUGE: u64 = 1 << 7;
const GLOBAL: u64 = 1 << 8;
const NO_EXECUTE: u64 = 1 << 63;
const ADDR: u64 = 0x000F_FFFF_FFFF_F000;

type Table = [u64; 512];

fn table(phys: u64) -> &'static mut Table {
    // SAFETY: las tablas son marcos propios accesibles por el mapa directo.
    unsafe { &mut *(phys_to_virt(phys) as *mut Table) }
}

fn index(virt: u64, level: u32) -> usize {
    ((virt >> (12 + 9 * level)) & 0x1FF) as usize
}

pub struct AddressSpace {
    root: u64,
}

impl AddressSpace {
    pub fn new() -> Option<Self> {
        Some(Self { root: pmm::alloc_zeroed(0)? })
    }

    pub fn root(&self) -> u64 {
        self.root
    }

    /// # Safety
    /// El espacio debe mapear el código y la pila que se están usando.
    pub unsafe fn activate(&self) {
        // SAFETY: lo garantiza quien llama.
        unsafe { cpu::write_cr3(self.root) };
    }

    fn entry_bits(flags: MapFlags) -> u64 {
        let mut e = PRESENT;
        if flags.has(MapFlags::WRITE) {
            e |= WRITABLE;
        }
        if flags.has(MapFlags::USER) {
            e |= USER;
        }
        if flags.has(MapFlags::GLOBAL) {
            e |= GLOBAL;
        }
        if !flags.has(MapFlags::EXEC) && cpu::features().nx {
            e |= NO_EXECUTE;
        }
        if flags.has(MapFlags::NO_CACHE) {
            e |= PCD | PWT;
        } else if flags.has(MapFlags::WRITE_COMBINE) && cpu::features().pat {
            e |= PWT;
        }
        e
    }

    /// Tabla del nivel siguiente (la crea si no existe).
    fn next_table(entry: &mut u64, user: bool) -> Result<u64, MapError> {
        if *entry & PRESENT != 0 {
            if *entry & HUGE != 0 {
                return Err(MapError::AlreadyMapped);
            }
            if user {
                *entry |= USER;
            }
            return Ok(*entry & ADDR);
        }
        let t = pmm::alloc_zeroed(0).ok_or(MapError::OutOfMemory)?;
        *entry = t | PRESENT | WRITABLE | if user { USER } else { 0 };
        Ok(t)
    }

    pub fn map(&mut self, virt: u64, phys: u64, size: PageSize, flags: MapFlags) -> Result<(), MapError> {
        let bytes = size.bytes();
        if virt % bytes != 0 || phys % bytes != 0 {
            return Err(MapError::Misaligned);
        }
        if size == PageSize::Huge && !cpu::features().gib_pages {
            return Err(MapError::Unsupported);
        }
        let user = flags.has(MapFlags::USER);
        let leaf_level = match size {
            PageSize::Small => 0,
            PageSize::Large => 1,
            PageSize::Huge => 2,
        };
        let mut t = self.root;
        for level in (leaf_level + 1..=3).rev() {
            t = Self::next_table(&mut table(t)[index(virt, level)], user)?;
        }
        let e = &mut table(t)[index(virt, leaf_level)];
        if *e & PRESENT != 0 {
            return Err(MapError::AlreadyMapped);
        }
        let mut bits = Self::entry_bits(flags);
        if leaf_level > 0 {
            bits |= HUGE;
        }
        *e = phys | bits;
        Ok(())
    }

    pub fn map_4k(&mut self, virt: u64, phys: u64, flags: MapFlags) -> Result<(), MapError> {
        self.map(virt, phys, PageSize::Small, flags)
    }

    /// Mapea un rango con las páginas más grandes que permitan la alineación.
    pub fn map_range(&mut self, virt: u64, phys: u64, len: u64, flags: MapFlags) -> Result<(), MapError> {
        let mut off = 0;
        while off < len {
            let (v, p, rest) = (virt + off, phys + off, len - off);
            let size = [PageSize::Huge, PageSize::Large, PageSize::Small]
                .into_iter()
                .find(|s| {
                    let b = s.bytes();
                    v % b == 0 && p % b == 0 && rest >= b && (*s != PageSize::Huge || cpu::features().gib_pages)
                })
                .unwrap_or(PageSize::Small);
            self.map(v, p, size, flags)?;
            off += size.bytes();
        }
        Ok(())
    }

    /// Física y permisos de una dirección virtual.
    pub fn translate(&self, virt: u64) -> Option<(u64, MapFlags)> {
        let mut t = self.root;
        for level in (0..=3).rev() {
            let e = table(t)[index(virt, level)];
            if e & PRESENT == 0 {
                return None;
            }
            if level == 0 || e & HUGE != 0 {
                let page = 1u64 << (12 + 9 * level);
                let mut f = MapFlags::READ;
                if e & WRITABLE != 0 {
                    f = f | MapFlags::WRITE;
                }
                if e & NO_EXECUTE == 0 {
                    f = f | MapFlags::EXEC;
                }
                if e & GLOBAL != 0 {
                    f = f | MapFlags::GLOBAL;
                }
                return Some(((e & ADDR & !(page - 1)) + (virt & (page - 1)), f));
            }
            t = e & ADDR;
        }
        None
    }

    /// Quita una página de 4 KiB e invalida el TLB.
    pub fn unmap_4k(&mut self, virt: u64) -> Option<u64> {
        let mut t = self.root;
        for level in (1..=3).rev() {
            let e = table(t)[index(virt, level)];
            if e & PRESENT == 0 || e & HUGE != 0 {
                return None;
            }
            t = e & ADDR;
        }
        let e = &mut table(t)[index(virt, 0)];
        if *e & PRESENT == 0 {
            return None;
        }
        let phys = *e & ADDR;
        *e = 0;
        cpu::invlpg(virt);
        Some(phys)
    }
}
