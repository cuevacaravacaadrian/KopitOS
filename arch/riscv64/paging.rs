//! Paginación en RISC-V: en la fase 1 la MMU está apagada (`satp = 0`,
//! identidad). La fase 2 implementará Sv48 con la misma interfaz que x86_64.

use crate::mm::{MapError, MapFlags};

pub const ENABLED: bool = false;
pub const MMIO_WINDOW_BASE: u64 = 0;

pub struct AddressSpace;

impl AddressSpace {
    pub fn new() -> Option<Self> {
        None
    }

    /// # Safety
    /// Sin efecto en la fase 1.
    pub unsafe fn activate(&self) {}

    pub fn map_4k(&mut self, _: u64, _: u64, _: MapFlags) -> Result<(), MapError> {
        Err(MapError::Unsupported)
    }

    pub fn map_range(&mut self, _: u64, _: u64, _: u64, _: MapFlags) -> Result<(), MapError> {
        Err(MapError::Unsupported)
    }

    pub fn translate(&self, _: u64) -> Option<(u64, MapFlags)> {
        None
    }
}
