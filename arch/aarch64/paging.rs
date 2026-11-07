//! Paginación en ARM64: en la fase 1 hay una tabla identidad fija (bloques de
//! 1 GiB, ver `mod.rs`). La fase 2 implementará TTBR0/TTBR1 con mitad alta y
//! W^X con la misma interfaz que x86_64.

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
