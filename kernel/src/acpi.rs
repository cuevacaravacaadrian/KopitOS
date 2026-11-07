//! # ACPI: el mapa del hardware que da el firmware
//!
//! Se leen las tablas que la fase 1 necesita:
//!
//! * **MADT** (`APIC`): cuántas CPUs hay, dónde están los IOAPIC y cómo se
//!   reasignan las IRQ heredadas del PC (p. ej. IRQ 0 → GSI 2).
//! * **FADT** (`FACP`): registros de energía (apagado S5) y de reinicio.
//! * **MCFG**: espacio de configuración PCIe mapeado en memoria (ECAM).
//! * **HPET**: temporizador de alta precisión (lo usará la fase 2).
//!
//! Todas las tablas se validan con su suma de comprobación antes de usarlas.
//! El intérprete AML completo llega en la fase 2; para apagar basta con
//! localizar el paquete `\_S5_` en la DSDT.

use crate::mm::phys_to_virt;
use crate::sync::Once;
use alloc::vec::Vec;

#[derive(Clone, Copy, Debug)]
pub struct TableInfo {
    pub signature: [u8; 4],
    pub phys: u64,
    pub length: u32,
    pub revision: u8,
    pub oem_id: [u8; 6],
}

#[derive(Clone, Copy, Debug)]
pub struct Cpu {
    pub acpi_id: u32,
    pub apic_id: u32,
    pub enabled: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct IoApic {
    pub id: u8,
    pub address: u64,
    pub gsi_base: u32,
}

/// Redirección de una IRQ ISA a otra entrada global (GSI).
#[derive(Clone, Copy, Debug)]
pub struct Override {
    pub irq: u8,
    pub gsi: u32,
    pub active_low: bool,
    pub level_triggered: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct GenericAddress {
    pub space: u8,
    pub width: u8,
    pub address: u64,
}

#[derive(Clone, Copy, Debug)]
pub struct Mcfg {
    pub base: u64,
    pub segment: u16,
    pub bus_start: u8,
    pub bus_end: u8,
}

pub struct AcpiInfo {
    pub revision: u8,
    pub oem_id: [u8; 6],
    pub tables: Vec<TableInfo>,
    pub lapic_address: u64,
    pub cpus: Vec<Cpu>,
    pub ioapics: Vec<IoApic>,
    pub overrides: Vec<Override>,
    pub pm1a_control: u32,
    pub pm1b_control: u32,
    pub sleep_s5: Option<(u16, u16)>,
    pub reset: Option<(GenericAddress, u8)>,
    pub century_register: u8,
    pub sci_interrupt: u16,
    pub mcfg: Vec<Mcfg>,
    pub hpet: Option<u64>,
}

static ACPI: Once<AcpiInfo> = Once::new();

pub fn info() -> Option<&'static AcpiInfo> {
    ACPI.get()
}

fn bytes<'a>(phys: u64, len: usize) -> &'a [u8] {
    // SAFETY: las tablas ACPI están en RAM cubierta por el mapa directo.
    unsafe { core::slice::from_raw_parts(phys_to_virt(phys) as *const u8, len) }
}

fn u16_at(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}
fn u64_at(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3], b[o + 4], b[o + 5], b[o + 6], b[o + 7]])
}

fn checksum_ok(b: &[u8]) -> bool {
    b.iter().fold(0u8, |a, &x| a.wrapping_add(x)) == 0
}

/// Tabla completa (cabecera de 36 bytes incluida) si la suma es correcta.
fn table<'a>(phys: u64) -> Option<&'a [u8]> {
    if phys == 0 {
        return None;
    }
    let head = bytes(phys, 36);
    let len = u32_at(head, 4) as usize;
    if !(36..=16 << 20).contains(&len) {
        return None;
    }
    let t = bytes(phys, len);
    checksum_ok(t).then_some(t)
}

pub fn init(rsdp_phys: u64) -> Result<&'static AcpiInfo, &'static str> {
    if rsdp_phys == 0 {
        return Err("el cargador no encontró el RSDP");
    }
    let rsdp = bytes(rsdp_phys, 20);
    if &rsdp[0..8] != b"RSD PTR " || !checksum_ok(rsdp) {
        return Err("RSDP no válido");
    }
    let revision = rsdp[15];
    let mut oem_id = [0u8; 6];
    oem_id.copy_from_slice(&rsdp[9..15]);
    // ACPI 2.0+: XSDT con punteros de 64 bits; si no, RSDT de 32.
    let (root, entry_size) = if revision >= 2 {
        let full = bytes(rsdp_phys, 36);
        (u64_at(full, 24), 8)
    } else {
        (u32_at(rsdp, 16) as u64, 4)
    };
    let root = table(root).ok_or("XSDT/RSDT no válida")?;

    let mut info = AcpiInfo {
        revision,
        oem_id,
        tables: Vec::new(),
        lapic_address: 0xFEE0_0000,
        cpus: Vec::new(),
        ioapics: Vec::new(),
        overrides: Vec::new(),
        pm1a_control: 0,
        pm1b_control: 0,
        sleep_s5: None,
        reset: None,
        century_register: 0,
        sci_interrupt: 0,
        mcfg: Vec::new(),
        hpet: None,
    };

    let count = (root.len() - 36) / entry_size;
    for i in 0..count {
        let o = 36 + i * entry_size;
        let phys = if entry_size == 8 { u64_at(root, o) } else { u32_at(root, o) as u64 };
        let Some(t) = table(phys) else { continue };
        let mut signature = [0u8; 4];
        signature.copy_from_slice(&t[0..4]);
        let mut oem = [0u8; 6];
        oem.copy_from_slice(&t[10..16]);
        info.tables.push(TableInfo { signature, phys, length: t.len() as u32, revision: t[8], oem_id: oem });
        match &signature {
            b"APIC" => parse_madt(t, &mut info),
            b"FACP" => parse_fadt(t, &mut info),
            b"MCFG" => parse_mcfg(t, &mut info),
            b"HPET" if t.len() >= 56 => info.hpet = Some(u64_at(t, 44)),
            _ => {}
        }
    }
    Ok(ACPI.call_once(|| info))
}

fn parse_madt(t: &[u8], info: &mut AcpiInfo) {
    info.lapic_address = u32_at(t, 36) as u64;
    let mut o = 44;
    while o + 2 <= t.len() {
        let (kind, len) = (t[o], t[o + 1] as usize);
        if len < 2 || o + len > t.len() {
            break;
        }
        let e = &t[o..o + len];
        match kind {
            0 if len >= 8 => {
                info.cpus.push(Cpu { acpi_id: e[2] as u32, apic_id: e[3] as u32, enabled: u32_at(e, 4) & 0b11 != 0 })
            }
            1 if len >= 12 => {
                info.ioapics.push(IoApic { id: e[2], address: u32_at(e, 4) as u64, gsi_base: u32_at(e, 8) })
            }
            2 if len >= 10 => {
                let flags = u16_at(e, 8);
                info.overrides.push(Override {
                    irq: e[3],
                    gsi: u32_at(e, 4),
                    active_low: flags & 0b11 == 0b11,
                    level_triggered: (flags >> 2) & 0b11 == 0b11,
                });
            }
            5 if len >= 12 => info.lapic_address = u64_at(e, 4),
            9 if len >= 16 => {
                info.cpus.push(Cpu { acpi_id: u32_at(e, 12), apic_id: u32_at(e, 4), enabled: u32_at(e, 8) & 0b11 != 0 })
            }
            _ => {}
        }
        o += len;
    }
}

fn parse_fadt(t: &[u8], info: &mut AcpiInfo) {
    if t.len() < 116 {
        return;
    }
    info.sci_interrupt = u16_at(t, 46);
    info.pm1a_control = u32_at(t, 64);
    info.pm1b_control = u32_at(t, 68);
    info.century_register = t[108];
    let flags = u32_at(t, 112);
    if t.len() >= 129 && flags & (1 << 10) != 0 {
        let gas = GenericAddress { space: t[116], width: t[117], address: u64_at(t, 120) };
        info.reset = Some((gas, t[128]));
    }
    let dsdt = if t.len() >= 148 && u64_at(t, 140) != 0 { u64_at(t, 140) } else { u32_at(t, 40) as u64 };
    if let Some(d) = table(dsdt) {
        info.sleep_s5 = find_s5(d);
    }
}

/// Busca `Name(\_S5_, Package(){ SLP_TYPa, SLP_TYPb, ... })` en la DSDT.
fn find_s5(dsdt: &[u8]) -> Option<(u16, u16)> {
    let pos = dsdt.windows(4).position(|w| w == b"_S5_")?;
    let named = (pos >= 1 && dsdt[pos - 1] == 0x08) || (pos >= 2 && dsdt[pos - 2] == 0x08 && dsdt[pos - 1] == b'\\');
    let mut i = pos + 4;
    if !named || *dsdt.get(i)? != 0x12 {
        return None; // no es un PackageOp
    }
    i += 1;
    let pkg_len_bytes = (dsdt.get(i)? >> 6) as usize;
    i += 1 + pkg_len_bytes + 1; // PkgLength + NumElements
    let mut value = || -> Option<u16> {
        let b = *dsdt.get(i)?;
        match b {
            0x0A => {
                let v = *dsdt.get(i + 1)?;
                i += 2;
                Some(v as u16)
            }
            0x00 | 0x01 => {
                i += 1;
                Some(b as u16)
            }
            _ => {
                i += 1;
                Some(b as u16)
            }
        }
    };
    let a = value()?;
    let b = value()?;
    Some((a, b))
}

fn parse_mcfg(t: &[u8], info: &mut AcpiInfo) {
    let mut o = 44;
    while o + 16 <= t.len() {
        info.mcfg.push(Mcfg {
            base: u64_at(t, o),
            segment: u16_at(t, o + 8),
            bus_start: t[o + 10],
            bus_end: t[o + 11],
        });
        o += 16;
    }
}

/// Busca el RSDP en la zona del BIOS (arranques PVH/Multiboot sin puntero).
#[cfg(target_arch = "x86_64")]
pub fn find_rsdp_in_bios() -> Option<u64> {
    let ebda = (unsafe { (phys_to_virt(0x40E) as *const u16).read_unaligned() } as u64) << 4;
    let candidates = [(ebda, 1024u64), (0xE0000, 0x20000)];
    for (start, len) in candidates {
        if start == 0 || start >= 0x10_0000 {
            continue;
        }
        let area = bytes(start, len as usize);
        let mut o = 0;
        while o + 20 <= area.len() {
            if &area[o..o + 8] == b"RSD PTR " && checksum_ok(&area[o..o + 20]) {
                return Some(start + o as u64);
            }
            o += 16;
        }
    }
    None
}

impl TableInfo {
    pub fn signature_str(&self) -> &str {
        core::str::from_utf8(&self.signature).unwrap_or("????")
    }
    pub fn oem_str(&self) -> &str {
        core::str::from_utf8(&self.oem_id).unwrap_or("").trim_end()
    }
}
