//! # PCI en el núcleo
//!
//! Enumera el bus con el mecanismo de configuración de la arquitectura y
//! cruza cada dispositivo con el registro de controladores
//! (`kopit_drv_pci::PCI_DRIVERS`): es la **capa de compatibilidad** en
//! acción, visible con la orden `pci` de la shell.

use crate::sync::Once;
use alloc::vec::Vec;
use kopit_driver_api::{DriverDescriptor, DriverStatus, best_driver};
use kopit_drv_pci::{PCI_DRIVERS, PciDevice, enumerate};

static DEVICES: Once<Vec<PciDevice>> = Once::new();

pub fn scan() -> &'static [PciDevice] {
    DEVICES.call_once(|| {
        let mut v = Vec::new();
        if let Some(cfg) = crate::arch::pci_config() {
            enumerate(&cfg, |d| v.push(d));
        }
        v
    })
}

pub fn devices() -> &'static [PciDevice] {
    DEVICES.get().map(|v| v.as_slice()).unwrap_or(&[])
}

pub fn driver_for(d: &PciDevice) -> Option<&'static DriverDescriptor> {
    best_driver(PCI_DRIVERS, &d.identity())
}

pub fn driver_label(d: &PciDevice) -> (&'static str, &'static str) {
    match driver_for(d) {
        Some(drv) => match drv.status {
            DriverStatus::Available => (drv.name, "activo"),
            DriverStatus::Planned { phase: 2 } => (drv.name, "fase 2"),
            DriverStatus::Planned { phase: 3 } => (drv.name, "fase 3"),
            DriverStatus::Planned { .. } => (drv.name, "planificado"),
        },
        None => ("—", "sin controlador"),
    }
}

/// Registra cada dispositivo encontrado.
pub fn log_devices() {
    for d in devices() {
        let (drv, state) = driver_label(d);
        kdebug!(
            "{:02x}:{:02x}.{} {:04x}:{:04x} {} ({}) → {} [{}]",
            d.bus,
            d.device,
            d.function,
            d.vendor_id,
            d.device_id,
            d.class_name(),
            d.vendor_name(),
            drv,
            state
        );
    }
}
