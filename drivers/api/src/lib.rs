//! # API de controladores de KopitOS
//!
//! ## Regla de oro: un controlador no conoce al núcleo
//!
//! Los crates de `drivers/` **sólo** dependen de éste. Nunca importan nada del
//! núcleo. Todo lo que necesitan del mundo exterior (leer un registro, mapear
//! MMIO, recibir una interrupción) les llega a través de rasgos pequeños
//! ([`RegisterIo`], [`PortIo`]...). Consecuencias:
//!
//! * **Fase 1 (hoy)**: se enlazan dentro del núcleo; los rasgos se implementan
//!   con instrucciones `in/out` o accesos volátiles directos.
//! * **Fase 3**: el *mismo código* compila como un proceso de espacio de
//!   usuario; los rasgos se implementan con *capacidades* (el núcleo le
//!   concede sólo esa ventana MMIO, ese puerto y esa interrupción), y un fallo
//!   del controlador ya no tumba el sistema.
//! * Se pueden **probar en el anfitrión** con registros simulados.
//!
//! ## Capa de compatibilidad
//!
//! Cada controlador publica un [`DriverDescriptor`] con las coincidencias que
//! sabe atender (IDs PCI, clases, ACPI `_HID`, `compatible` de device tree).
//! El gestor de dispositivos cruza lo que encuentra el bus con el registro de
//! descriptores. Un descriptor puede señalar un controlador nativo, un
//! controlador en espacio de usuario o (fase 4) una capa que reutiliza
//! controladores de otros sistemas: ver `ARCHITECTURE.md`.

#![cfg_attr(not(test), no_std)]

/// Acceso a un banco de registros de 8 bits (UART, PS/2, CMOS...).
pub trait RegisterIo {
    fn read(&self, reg: u16) -> u8;
    fn write(&self, reg: u16, value: u8);
}

/// Acceso a puertos de E/S de x86 (en otras arquitecturas no existe).
pub trait PortIo {
    fn inb(&self, port: u16) -> u8;
    fn outb(&self, port: u16, value: u8);
    fn inl(&self, port: u16) -> u32;
    fn outl(&self, port: u16, value: u32);
}

/// Registros mapeados en memoria: `base + (reg << shift)`, accesos volátiles.
///
/// Es la forma en que ARM64 y RISC-V exponen las UART 16550 y PL011.
#[derive(Clone, Copy, Debug)]
pub struct MmioRegisters {
    base: usize,
    shift: u8,
}

impl MmioRegisters {
    /// # Safety
    /// `base` debe ser una dirección virtual mapeada (sin caché) de un banco de
    /// registros válido mientras se use el valor.
    pub const unsafe fn new(base: usize, shift: u8) -> Self {
        Self { base, shift }
    }

    pub fn base(&self) -> usize {
        self.base
    }

    pub fn read32(&self, offset: usize) -> u32 {
        // SAFETY: garantizado por `new`.
        unsafe { core::ptr::read_volatile((self.base + offset) as *const u32) }
    }

    pub fn write32(&self, offset: usize, value: u32) {
        // SAFETY: garantizado por `new`.
        unsafe { core::ptr::write_volatile((self.base + offset) as *mut u32, value) }
    }
}

impl RegisterIo for MmioRegisters {
    fn read(&self, reg: u16) -> u8 {
        // SAFETY: garantizado por `new`.
        unsafe { core::ptr::read_volatile((self.base + ((reg as usize) << self.shift)) as *const u8) }
    }
    fn write(&self, reg: u16, value: u8) {
        // SAFETY: garantizado por `new`.
        unsafe { core::ptr::write_volatile((self.base + ((reg as usize) << self.shift)) as *mut u8, value) }
    }
}

/// Algo que acepta bytes (consolas, puertos serie, registros del sistema).
pub trait CharSink {
    fn write_bytes(&mut self, bytes: &[u8]);
}

/// Cómo reconoce un controlador "su" dispositivo.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeviceMatch {
    /// Fabricante y modelo PCI exactos.
    PciId { vendor: u16, device: u16 },
    /// Cualquier dispositivo PCI de un fabricante.
    PciVendor(u16),
    /// Clase y subclase PCI (`prog_if = None` → cualquiera).
    PciClass { class: u8, subclass: u8, prog_if: Option<u8> },
    /// Identificador de hardware ACPI (`_HID`), p. ej. `"PNP0501"`.
    Acpi(&'static str),
    /// Cadena `compatible` de device tree, p. ej. `"arm,pl011"`.
    DeviceTree(&'static str),
    /// Dispositivo de plataforma fijo (PS/2, RTC...).
    Platform(&'static str),
}

/// Dónde se ejecuta el controlador.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Isolation {
    /// Dentro del núcleo (sólo lo imprescindible para arrancar).
    InKernel,
    /// Proceso aislado con capacidades.
    UserSpace,
    /// Ruta rápida en el núcleo y política en espacio de usuario (red, GPU).
    Split,
}

impl Isolation {
    pub const fn name(self) -> &'static str {
        match self {
            Isolation::InKernel => "núcleo",
            Isolation::UserSpace => "espacio de usuario",
            Isolation::Split => "mixto",
        }
    }
}

/// Estado de implementación (la hoja de ruta, visible desde el propio sistema).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DriverStatus {
    Available,
    Planned { phase: u8 },
}

/// Capacidades que el controlador solicita (lo que el núcleo le concederá).
pub mod caps {
    pub const PORT_IO: u32 = 1 << 0;
    pub const MMIO: u32 = 1 << 1;
    pub const IRQ: u32 = 1 << 2;
    pub const MSI: u32 = 1 << 3;
    pub const DMA: u32 = 1 << 4;
    pub const FRAMEBUFFER: u32 = 1 << 5;
}

/// Ficha de un controlador.
#[derive(Clone, Copy, Debug)]
pub struct DriverDescriptor {
    pub name: &'static str,
    pub description: &'static str,
    pub matches: &'static [DeviceMatch],
    pub isolation: Isolation,
    pub capabilities: u32,
    pub status: DriverStatus,
}

/// Lo que se sabe de un dispositivo encontrado en un bus.
#[derive(Clone, Copy, Debug, Default)]
pub struct DeviceIdentity<'a> {
    pub pci: Option<PciIdentity>,
    pub acpi_hid: Option<&'a str>,
    pub dt_compatible: Option<&'a str>,
    pub platform: Option<&'a str>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PciIdentity {
    pub vendor: u16,
    pub device: u16,
    pub class: u8,
    pub subclass: u8,
    pub prog_if: u8,
}

impl DeviceMatch {
    /// Puntuación de la coincidencia: cuanto más específica, mayor.
    pub fn score(&self, dev: &DeviceIdentity<'_>) -> Option<u8> {
        match (*self, dev.pci) {
            (DeviceMatch::PciId { vendor, device }, Some(p)) if p.vendor == vendor && p.device == device => Some(100),
            (DeviceMatch::PciClass { class, subclass, prog_if }, Some(p))
                if p.class == class && p.subclass == subclass && prog_if.is_none_or(|pi| pi == p.prog_if) =>
            {
                Some(if prog_if.is_some() { 60 } else { 50 })
            }
            (DeviceMatch::PciVendor(v), Some(p)) if p.vendor == v => Some(20),
            (DeviceMatch::Acpi(h), _) if dev.acpi_hid == Some(h) => Some(90),
            (DeviceMatch::DeviceTree(c), _) if dev.dt_compatible == Some(c) => Some(90),
            (DeviceMatch::Platform(n), _) if dev.platform == Some(n) => Some(80),
            _ => None,
        }
    }
}

/// Elige el mejor controlador para un dispositivo (la coincidencia más específica).
pub fn best_driver<'r>(registry: &'r [DriverDescriptor], dev: &DeviceIdentity<'_>) -> Option<&'r DriverDescriptor> {
    registry
        .iter()
        .filter_map(|d| d.matches.iter().filter_map(|m| m.score(dev)).max().map(|s| (s, d)))
        .max_by_key(|&(s, d)| (s, matches!(d.status, DriverStatus::Available)))
        .map(|(_, d)| d)
}

#[cfg(test)]
mod tests {
    use super::*;

    const REG: &[DriverDescriptor] = &[
        DriverDescriptor {
            name: "virtio-net",
            description: "red virtio",
            matches: &[
                DeviceMatch::PciId { vendor: 0x1AF4, device: 0x1000 },
                DeviceMatch::PciId { vendor: 0x1AF4, device: 0x1041 },
            ],
            isolation: Isolation::Split,
            capabilities: caps::MMIO | caps::DMA | caps::MSI,
            status: DriverStatus::Planned { phase: 2 },
        },
        DriverDescriptor {
            name: "ethernet-generico",
            description: "cualquier tarjeta Ethernet",
            matches: &[DeviceMatch::PciClass { class: 2, subclass: 0, prog_if: None }],
            isolation: Isolation::UserSpace,
            capabilities: caps::MMIO,
            status: DriverStatus::Planned { phase: 3 },
        },
    ];

    #[test]
    fn gana_la_coincidencia_mas_especifica() {
        let dev = DeviceIdentity {
            pci: Some(PciIdentity { vendor: 0x1AF4, device: 0x1000, class: 2, subclass: 0, prog_if: 0 }),
            ..Default::default()
        };
        assert_eq!(best_driver(REG, &dev).unwrap().name, "virtio-net");
        let other = DeviceIdentity {
            pci: Some(PciIdentity { vendor: 0x8086, device: 0x100E, class: 2, subclass: 0, prog_if: 0 }),
            ..Default::default()
        };
        assert_eq!(best_driver(REG, &other).unwrap().name, "ethernet-generico");
        let none = DeviceIdentity { platform: Some("rtc"), ..Default::default() };
        assert!(best_driver(REG, &none).is_none());
    }
}
