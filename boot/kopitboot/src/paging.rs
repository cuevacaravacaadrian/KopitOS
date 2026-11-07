//! Tablas de páginas x86_64 de 4 niveles que KopitBoot entrega al núcleo.
//!
//! Espacio de direcciones al entrar en el núcleo:
//!
//! | Rango virtual                      | Contenido                              |
//! |------------------------------------|----------------------------------------|
//! | `0 .. max_phys`                    | identidad (sólo para el salto; el      |
//! |                                    | núcleo la elimina enseguida)           |
//! | `0xFFFF_8000_0000_0000 + física`   | HHDM: toda la memoria física           |
//! | `0xFFFF_FFFF_8000_0000 ..`         | núcleo, con permisos W^X por segmento  |
//!
//! Identidad y HHDM usan páginas de 1 GiB si la CPU las tiene (o de 2 MiB):
//! mapear 64 GiB cuesta unas pocas páginas de tablas.

use crate::firmware::allocate_zeroed_pages;
use crate::uefi::{Status, memtype};

const PRESENT: u64 = 1 << 0;
const WRITABLE: u64 = 1 << 1;
const HUGE: u64 = 1 << 7;
const NO_EXECUTE: u64 = 1 << 63;
const ADDR_MASK: u64 = 0x000F_FFFF_FFFF_F000;

const MIB2: u64 = 2 << 20;
const GIB: u64 = 1 << 30;

pub struct PageTables {
    pub pml4: u64,
    pub table_pages: usize,
    nx: bool,
    gib_pages: bool,
}

fn index(virt: u64, shift: u32) -> usize {
    ((virt >> shift) & 0x1FF) as usize
}

impl PageTables {
    pub fn new(nx: bool, gib_pages: bool) -> Result<Self, Status> {
        let pml4 = allocate_zeroed_pages(memtype::KOPIT_PAGETABLES, 1)?;
        Ok(Self { pml4, table_pages: 1, nx, gib_pages })
    }

    /// Entrada `index` de `table`; crea la tabla siguiente si no existe.
    fn next(&mut self, table: u64, idx: usize) -> Result<u64, Status> {
        let entry = (table + idx as u64 * 8) as *mut u64;
        // SAFETY: las tablas están en memoria propia mapeada en identidad por UEFI.
        unsafe {
            if *entry & PRESENT != 0 {
                return Ok(*entry & ADDR_MASK);
            }
            let t = allocate_zeroed_pages(memtype::KOPIT_PAGETABLES, 1)?;
            self.table_pages += 1;
            // Las entradas intermedias son permisivas: decide la hoja.
            *entry = t | PRESENT | WRITABLE;
            Ok(t)
        }
    }

    fn flags(&self, writable: bool, executable: bool) -> u64 {
        let mut f = PRESENT;
        if writable {
            f |= WRITABLE;
        }
        if !executable && self.nx {
            f |= NO_EXECUTE;
        }
        f
    }

    pub fn map_4k(&mut self, virt: u64, phys: u64, writable: bool, executable: bool) -> Result<(), Status> {
        let pdpt = self.next(self.pml4, index(virt, 39))?;
        let pd = self.next(pdpt, index(virt, 30))?;
        let pt = self.next(pd, index(virt, 21))?;
        let entry = (pt + index(virt, 12) as u64 * 8) as *mut u64;
        // SAFETY: tabla propia.
        unsafe { *entry = phys | self.flags(writable, executable) };
        Ok(())
    }

    fn map_2m(&mut self, virt: u64, phys: u64) -> Result<(), Status> {
        let pdpt = self.next(self.pml4, index(virt, 39))?;
        let pd = self.next(pdpt, index(virt, 30))?;
        let entry = (pd + index(virt, 21) as u64 * 8) as *mut u64;
        // SAFETY: tabla propia.
        unsafe { *entry = phys | self.flags(true, true) | HUGE };
        Ok(())
    }

    fn map_1g(&mut self, virt: u64, phys: u64) -> Result<(), Status> {
        let pdpt = self.next(self.pml4, index(virt, 39))?;
        let entry = (pdpt + index(virt, 30) as u64 * 8) as *mut u64;
        // SAFETY: tabla propia.
        unsafe { *entry = phys | self.flags(true, true) | HUGE };
        Ok(())
    }

    /// Mapea la memoria física `[0, size)` a partir de `virt_base`.
    pub fn map_physical(&mut self, virt_base: u64, size: u64) -> Result<(), Status> {
        let mut p = 0u64;
        while p < size {
            if self.gib_pages && p % GIB == 0 && p + GIB <= size {
                self.map_1g(virt_base + p, p)?;
                p += GIB;
            } else {
                self.map_2m(virt_base + p, p)?;
                p += MIB2;
            }
        }
        Ok(())
    }
}
