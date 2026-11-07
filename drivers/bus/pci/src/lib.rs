//! # Bus PCI / PCI Express
//!
//! Enumera el bus sin conocer el mecanismo de acceso al espacio de
//! configuración ([`ConfigSpace`]): en x86 puede ser por puertos `0xCF8/0xCFC`
//! (siempre disponible) o por ECAM (MMIO, tabla ACPI `MCFG`); en ARM/RISC-V,
//! ECAM descrito en el device tree.
//!
//! Además incluye el **registro de controladores conocidos** que usa la capa
//! de compatibilidad: para cada dispositivo encontrado, el núcleo muestra qué
//! controlador lo atenderá y en qué fase de la hoja de ruta está.

#![cfg_attr(not(test), no_std)]

use kopit_driver_api::{DeviceIdentity, DeviceMatch, DriverDescriptor, DriverStatus, Isolation, PciIdentity, caps};

/// Acceso al espacio de configuración (registros de 32 bits alineados).
pub trait ConfigSpace {
    fn read32(&self, bus: u8, device: u8, function: u8, offset: u16) -> u32;
    fn write32(&self, bus: u8, device: u8, function: u8, offset: u16, value: u32);
}

/// Un dispositivo encontrado en el bus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PciDevice {
    pub bus: u8,
    pub device: u8,
    pub function: u8,
    pub vendor_id: u16,
    pub device_id: u16,
    pub class: u8,
    pub subclass: u8,
    pub prog_if: u8,
    pub revision: u8,
    pub header_type: u8,
    pub interrupt_line: u8,
    pub interrupt_pin: u8,
    pub bars: [u32; 6],
}

impl PciDevice {
    pub fn identity(&self) -> DeviceIdentity<'static> {
        DeviceIdentity {
            pci: Some(PciIdentity {
                vendor: self.vendor_id,
                device: self.device_id,
                class: self.class,
                subclass: self.subclass,
                prog_if: self.prog_if,
            }),
            ..Default::default()
        }
    }

    pub fn class_name(&self) -> &'static str {
        class_name(self.class, self.subclass, self.prog_if)
    }

    pub fn vendor_name(&self) -> &'static str {
        vendor_name(self.vendor_id).unwrap_or("fabricante desconocido")
    }

    /// ¿Es un puente PCI-PCI? (sus buses secundarios también se recorren).
    pub fn is_bridge(&self) -> bool {
        self.header_type & 0x7F == 1
    }
}

fn read_device<C: ConfigSpace>(cfg: &C, bus: u8, device: u8, function: u8) -> Option<PciDevice> {
    let id = cfg.read32(bus, device, function, 0x00);
    let vendor_id = id as u16;
    if vendor_id == 0xFFFF || vendor_id == 0x0000 {
        return None;
    }
    let class_reg = cfg.read32(bus, device, function, 0x08);
    let hdr = cfg.read32(bus, device, function, 0x0C);
    let irq = cfg.read32(bus, device, function, 0x3C);
    let header_type = (hdr >> 16) as u8;
    let mut bars = [0u32; 6];
    let bar_count = if header_type & 0x7F == 0 {
        6
    } else if header_type & 0x7F == 1 {
        2
    } else {
        0
    };
    for (i, bar) in bars.iter_mut().enumerate().take(bar_count) {
        *bar = cfg.read32(bus, device, function, 0x10 + i as u16 * 4);
    }
    Some(PciDevice {
        bus,
        device,
        function,
        vendor_id,
        device_id: (id >> 16) as u16,
        class: (class_reg >> 24) as u8,
        subclass: (class_reg >> 16) as u8,
        prog_if: (class_reg >> 8) as u8,
        revision: class_reg as u8,
        header_type,
        interrupt_line: irq as u8,
        interrupt_pin: (irq >> 8) as u8,
        bars,
    })
}

/// Recorre todos los buses (0..=255). Barato: un acceso de configuración por
/// ranura vacía, y las funciones 1..7 sólo si el dispositivo es multifunción.
pub fn enumerate<C: ConfigSpace>(cfg: &C, mut f: impl FnMut(PciDevice)) -> usize {
    let mut count = 0;
    for bus in 0..=255u8 {
        for device in 0..32u8 {
            let Some(d0) = read_device(cfg, bus, device, 0) else { continue };
            count += 1;
            let multi = d0.header_type & 0x80 != 0;
            f(d0);
            if multi {
                for function in 1..8u8 {
                    if let Some(d) = read_device(cfg, bus, device, function) {
                        count += 1;
                        f(d);
                    }
                }
            }
        }
    }
    count
}

/// Nombre (en español) de una clase PCI.
pub fn class_name(class: u8, subclass: u8, prog_if: u8) -> &'static str {
    match (class, subclass, prog_if) {
        (0x00, _, _) => "Dispositivo anterior a las clases PCI",
        (0x01, 0x01, _) => "Controlador IDE",
        (0x01, 0x06, 0x01) => "Controlador SATA (AHCI)",
        (0x01, 0x06, _) => "Controlador SATA",
        (0x01, 0x08, 0x02) => "Controlador NVMe",
        (0x01, 0x00, _) => "Controlador SCSI",
        (0x01, _, _) => "Controlador de almacenamiento",
        (0x02, 0x00, _) => "Controlador Ethernet",
        (0x02, 0x80, _) => "Controlador de red",
        (0x02, _, _) => "Controlador de red",
        (0x03, 0x00, _) => "Controlador VGA",
        (0x03, _, _) => "Controlador de pantalla",
        (0x04, 0x01, _) => "Audio",
        (0x04, 0x03, _) => "Audio HD (Intel HDA)",
        (0x04, _, _) => "Multimedia",
        (0x05, _, _) => "Controlador de memoria",
        (0x06, 0x00, _) => "Puente de host",
        (0x06, 0x01, _) => "Puente ISA",
        (0x06, 0x04, _) => "Puente PCI-PCI",
        (0x06, _, _) => "Puente",
        (0x07, 0x00, _) => "Controlador serie",
        (0x07, _, _) => "Comunicaciones",
        (0x08, _, _) => "Periférico del sistema",
        (0x09, _, _) => "Dispositivo de entrada",
        (0x0C, 0x03, 0x00) => "USB 1.1 (UHCI)",
        (0x0C, 0x03, 0x10) => "USB 1.1 (OHCI)",
        (0x0C, 0x03, 0x20) => "USB 2.0 (EHCI)",
        (0x0C, 0x03, 0x30) => "USB 3.x (xHCI)",
        (0x0C, 0x03, _) => "Controlador USB",
        (0x0C, 0x05, _) => "Controlador SMBus",
        (0x0C, _, _) => "Controlador de bus serie",
        (0x0D, _, _) => "Controlador inalámbrico",
        (0x10, _, _) => "Cifrado",
        (0x11, _, _) => "Procesamiento de señal",
        (0x12, _, _) => "Acelerador de procesamiento",
        (0xFF, _, _) => "Dispositivo sin clase",
        _ => "Dispositivo",
    }
}

/// Fabricantes más habituales (IDs asignados por PCI-SIG).
pub fn vendor_name(vendor: u16) -> Option<&'static str> {
    Some(match vendor {
        0x8086 => "Intel",
        0x1022 => "AMD",
        0x1002 => "AMD/ATI",
        0x10DE => "NVIDIA",
        0x1AF4 => "Red Hat (virtio)",
        0x1B36 => "Red Hat (QEMU)",
        0x1234 => "QEMU/Bochs",
        0x15AD => "VMware",
        0x80EE => "VirtualBox",
        0x1414 => "Microsoft (Hyper-V)",
        0x10EC => "Realtek",
        0x14E4 => "Broadcom",
        0x168C => "Qualcomm Atheros",
        0x17CB => "Qualcomm",
        0x1B4B => "Marvell",
        0x144D => "Samsung",
        0x1C5C => "SK hynix",
        0x15B7 => "Western Digital",
        0x1987 => "Phison",
        0x106B => "Apple",
        0x1106 => "VIA",
        0x1912 => "Renesas",
        0x8087 => "Intel (inalámbrico)",
        _ => return None,
    })
}

const fn planned(
    name: &'static str,
    description: &'static str,
    matches: &'static [DeviceMatch],
    isolation: Isolation,
    capabilities: u32,
    phase: u8,
) -> DriverDescriptor {
    DriverDescriptor { name, description, matches, isolation, capabilities, status: DriverStatus::Planned { phase } }
}

/// Registro de controladores de bus PCI: los que existen y los planificados.
pub static PCI_DRIVERS: &[DriverDescriptor] = &[
    DriverDescriptor {
        name: "kopit-fb",
        description: "framebuffer del firmware (GOP/VBE)",
        matches: &[DeviceMatch::PciClass { class: 0x03, subclass: 0x00, prog_if: None }],
        isolation: Isolation::InKernel,
        capabilities: caps::FRAMEBUFFER,
        status: DriverStatus::Available,
    },
    DriverDescriptor {
        name: "pci-host",
        description: "puente de host / ISA",
        matches: &[
            DeviceMatch::PciClass { class: 0x06, subclass: 0x00, prog_if: None },
            DeviceMatch::PciClass { class: 0x06, subclass: 0x01, prog_if: None },
            DeviceMatch::PciClass { class: 0x06, subclass: 0x04, prog_if: None },
        ],
        isolation: Isolation::InKernel,
        capabilities: caps::PORT_IO,
        status: DriverStatus::Available,
    },
    planned(
        "virtio-blk",
        "disco virtio",
        &[DeviceMatch::PciId { vendor: 0x1AF4, device: 0x1001 }, DeviceMatch::PciId { vendor: 0x1AF4, device: 0x1042 }],
        Isolation::UserSpace,
        caps::MMIO | caps::DMA | caps::MSI,
        2,
    ),
    planned(
        "virtio-net",
        "red virtio",
        &[DeviceMatch::PciId { vendor: 0x1AF4, device: 0x1000 }, DeviceMatch::PciId { vendor: 0x1AF4, device: 0x1041 }],
        Isolation::Split,
        caps::MMIO | caps::DMA | caps::MSI,
        2,
    ),
    planned(
        "virtio-gpu",
        "GPU virtio (aceleración 3D: Vulkan vía Venus)",
        &[DeviceMatch::PciId { vendor: 0x1AF4, device: 0x1050 }],
        Isolation::Split,
        caps::MMIO | caps::DMA | caps::MSI,
        3,
    ),
    planned(
        "nvme",
        "almacenamiento NVMe",
        &[DeviceMatch::PciClass { class: 0x01, subclass: 0x08, prog_if: Some(0x02) }],
        Isolation::UserSpace,
        caps::MMIO | caps::DMA | caps::MSI,
        2,
    ),
    planned(
        "ahci",
        "discos SATA (AHCI)",
        &[DeviceMatch::PciClass { class: 0x01, subclass: 0x06, prog_if: Some(0x01) }],
        Isolation::UserSpace,
        caps::MMIO | caps::DMA | caps::IRQ,
        2,
    ),
    planned(
        "xhci",
        "USB 3.x",
        &[DeviceMatch::PciClass { class: 0x0C, subclass: 0x03, prog_if: Some(0x30) }],
        Isolation::UserSpace,
        caps::MMIO | caps::DMA | caps::MSI,
        2,
    ),
    planned(
        "e1000",
        "Intel PRO/1000",
        &[DeviceMatch::PciId { vendor: 0x8086, device: 0x100E }, DeviceMatch::PciId { vendor: 0x8086, device: 0x10D3 }],
        Isolation::Split,
        caps::MMIO | caps::DMA | caps::IRQ,
        2,
    ),
    planned(
        "hda",
        "audio Intel HD",
        &[DeviceMatch::PciClass { class: 0x04, subclass: 0x03, prog_if: None }],
        Isolation::UserSpace,
        caps::MMIO | caps::DMA | caps::MSI,
        3,
    ),
    planned(
        "bochs-vga",
        "pantalla QEMU/Bochs (cambio de modo)",
        &[DeviceMatch::PciId { vendor: 0x1234, device: 0x1111 }],
        Isolation::UserSpace,
        caps::MMIO,
        2,
    ),
];

#[cfg(test)]
mod tests {
    use super::*;
    use kopit_driver_api::best_driver;
    use std::collections::HashMap;

    /// Bus simulado: (bus, dev, func) → registros de configuración.
    struct FakeBus(HashMap<(u8, u8, u8), [u32; 64]>);

    impl ConfigSpace for FakeBus {
        fn read32(&self, b: u8, d: u8, f: u8, off: u16) -> u32 {
            self.0.get(&(b, d, f)).map(|r| r[off as usize / 4]).unwrap_or(0xFFFF_FFFF)
        }
        fn write32(&self, _: u8, _: u8, _: u8, _: u16, _: u32) {}
    }

    fn dev(vendor: u16, device: u16, class: u8, sub: u8, prog_if: u8, multi: bool) -> [u32; 64] {
        let mut r = [0u32; 64];
        r[0] = (device as u32) << 16 | vendor as u32;
        r[2] = (class as u32) << 24 | (sub as u32) << 16 | (prog_if as u32) << 8;
        r[3] = if multi { 0x80 << 16 } else { 0 };
        r
    }

    #[test]
    fn enumera_y_asocia_controladores() {
        let mut m = HashMap::new();
        m.insert((0, 0, 0), dev(0x8086, 0x29C0, 0x06, 0x00, 0, false));
        m.insert((0, 1, 0), dev(0x1234, 0x1111, 0x03, 0x00, 0, false));
        m.insert((0, 2, 0), dev(0x1AF4, 0x1000, 0x02, 0x00, 0, true));
        m.insert((0, 2, 3), dev(0x1B36, 0x000D, 0x0C, 0x03, 0x30, false));
        let bus = FakeBus(m);
        let mut found = Vec::new();
        assert_eq!(enumerate(&bus, |d| found.push(d)), 4);
        assert_eq!(found[1].class_name(), "Controlador VGA");
        assert_eq!(found[1].vendor_name(), "QEMU/Bochs");
        // La VGA de QEMU: gana el ID exacto (bochs-vga) sobre la clase genérica.
        assert_eq!(best_driver(PCI_DRIVERS, &found[1].identity()).unwrap().name, "bochs-vga");
        assert_eq!(best_driver(PCI_DRIVERS, &found[2].identity()).unwrap().name, "virtio-net");
        assert_eq!(best_driver(PCI_DRIVERS, &found[3].identity()).unwrap().name, "xhci");
    }
}
