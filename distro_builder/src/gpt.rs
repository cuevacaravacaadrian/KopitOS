//! # Tablas de particiones GPT
//!
//! Disco = MBR protector (LBA 0) + cabecera GPT (LBA 1) + 128 entradas
//! (LBA 2–33) + particiones + copia de las entradas y de la cabecera al final.
//! Las cabeceras llevan CRC32 (si no cuadra, el firmware usa la copia).
//!
//! Los GUID se derivan del contenido (SHA-256): misma entrada → mismo disco,
//! bit a bit. Compilaciones reproducibles.

use kopit_crypto::{crc32, sha256};

/// Tipo "EFI System Partition".
pub const ESP_TYPE: &str = "C12A7328-F81F-11D2-BA4B-00A0C93EC93B";

/// GUID textual → 16 bytes (los tres primeros campos en little-endian).
pub fn guid_bytes(s: &str) -> [u8; 16] {
    let hex: Vec<u8> = s
        .chars()
        .filter(|c| *c != '-')
        .collect::<String>()
        .as_bytes()
        .chunks(2)
        .map(|p| u8::from_str_radix(std::str::from_utf8(p).unwrap(), 16).unwrap())
        .collect();
    let mut g = [0u8; 16];
    g[0..4].copy_from_slice(&[hex[3], hex[2], hex[1], hex[0]]);
    g[4..6].copy_from_slice(&[hex[5], hex[4]]);
    g[6..8].copy_from_slice(&[hex[7], hex[6]]);
    g[8..16].copy_from_slice(&hex[8..16]);
    g
}

/// GUID versión 4 determinista a partir de una semilla.
pub fn guid_from_seed(seed: &[u8]) -> [u8; 16] {
    let h = sha256::digest(seed);
    let mut g = [0u8; 16];
    g.copy_from_slice(&h[..16]);
    g[7] = (g[7] & 0x0F) | 0x40; // versión 4 (campo 3 en little-endian)
    g[8] = (g[8] & 0x3F) | 0x80; // variante RFC 4122
    g
}

pub struct Partition {
    pub type_guid: [u8; 16],
    pub unique_guid: [u8; 16],
    pub first_lba: u64,
    pub last_lba: u64,
    pub name: String,
}

const ENTRIES: usize = 128;
const ENTRY_SIZE: usize = 128;
const ENTRY_SECTORS: u64 = (ENTRIES * ENTRY_SIZE / 512) as u64;

fn header(
    my_lba: u64,
    alternate: u64,
    entries_lba: u64,
    last_lba: u64,
    disk_guid: &[u8; 16],
    entries_crc: u32,
) -> [u8; 512] {
    let mut h = [0u8; 512];
    h[0..8].copy_from_slice(b"EFI PART");
    h[8..12].copy_from_slice(&0x0001_0000u32.to_le_bytes());
    h[12..16].copy_from_slice(&92u32.to_le_bytes());
    h[24..32].copy_from_slice(&my_lba.to_le_bytes());
    h[32..40].copy_from_slice(&alternate.to_le_bytes());
    h[40..48].copy_from_slice(&(2 + ENTRY_SECTORS).to_le_bytes());
    h[48..56].copy_from_slice(&(last_lba - 1 - ENTRY_SECTORS).to_le_bytes());
    h[56..72].copy_from_slice(disk_guid);
    h[72..80].copy_from_slice(&entries_lba.to_le_bytes());
    h[80..84].copy_from_slice(&(ENTRIES as u32).to_le_bytes());
    h[84..88].copy_from_slice(&(ENTRY_SIZE as u32).to_le_bytes());
    h[88..92].copy_from_slice(&entries_crc.to_le_bytes());
    let crc = crc32::checksum(&h[..92]);
    h[16..20].copy_from_slice(&crc.to_le_bytes());
    h
}

/// Escribe MBR protector, GPT primaria y GPT de respaldo sobre `disk`.
pub fn write(disk: &mut [u8], disk_guid: [u8; 16], parts: &[Partition]) {
    assert!(disk.len() % 512 == 0);
    let sectors = disk.len() as u64 / 512;
    let last = sectors - 1;

    let mbr = &mut disk[..512];
    let pe = &mut mbr[446..462];
    pe.fill(0);
    pe[1..4].copy_from_slice(&[0x00, 0x02, 0x00]);
    pe[4] = 0xEE;
    pe[5..8].copy_from_slice(&[0xFF, 0xFF, 0xFF]);
    pe[8..12].copy_from_slice(&1u32.to_le_bytes());
    pe[12..16].copy_from_slice(&((sectors - 1).min(0xFFFF_FFFF) as u32).to_le_bytes());
    mbr[510] = 0x55;
    mbr[511] = 0xAA;

    let mut entries = vec![0u8; ENTRIES * ENTRY_SIZE];
    for (i, p) in parts.iter().enumerate() {
        let e = &mut entries[i * ENTRY_SIZE..(i + 1) * ENTRY_SIZE];
        e[0..16].copy_from_slice(&p.type_guid);
        e[16..32].copy_from_slice(&p.unique_guid);
        e[32..40].copy_from_slice(&p.first_lba.to_le_bytes());
        e[40..48].copy_from_slice(&p.last_lba.to_le_bytes());
        for (j, u) in p.name.encode_utf16().take(36).enumerate() {
            e[56 + j * 2..58 + j * 2].copy_from_slice(&u.to_le_bytes());
        }
    }
    let crc = crc32::checksum(&entries);
    let primary = header(1, last, 2, last, &disk_guid, crc);
    let backup_entries = last - ENTRY_SECTORS;
    let backup = header(last, 1, backup_entries, last, &disk_guid, crc);
    disk[512..1024].copy_from_slice(&primary);
    disk[1024..1024 + entries.len()].copy_from_slice(&entries);
    let be = backup_entries as usize * 512;
    disk[be..be + entries.len()].copy_from_slice(&entries);
    disk[last as usize * 512..].copy_from_slice(&backup);
}

/// Imagen de disco completa con una única ESP alineada a 1 MiB.
pub fn disk_with_esp(esp: &[u8], seed: &[u8]) -> Vec<u8> {
    let start = 2048u64;
    let esp_sectors = esp.len() as u64 / 512;
    let total = start + esp_sectors + 2048;
    let mut disk = vec![0u8; total as usize * 512];
    disk[start as usize * 512..start as usize * 512 + esp.len()].copy_from_slice(esp);
    let part = Partition {
        type_guid: guid_bytes(ESP_TYPE),
        unique_guid: guid_from_seed(&[seed, b"esp"].concat()),
        first_lba: start,
        last_lba: start + esp_sectors - 1,
        name: "KopitOS EFI".into(),
    };
    write(&mut disk, guid_from_seed(&[seed, b"disk"].concat()), &[part]);
    disk
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guid_de_la_esp() {
        assert_eq!(
            guid_bytes(ESP_TYPE),
            [0x28, 0x73, 0x2A, 0xC1, 0x1F, 0xF8, 0xD2, 0x11, 0xBA, 0x4B, 0x00, 0xA0, 0xC9, 0x3E, 0xC9, 0x3B]
        );
    }

    #[test]
    fn cabeceras_y_crc() {
        let disk = disk_with_esp(&vec![0u8; 1 << 20], b"prueba");
        assert_eq!(&disk[512..520], b"EFI PART");
        let last = disk.len() / 512 - 1;
        assert_eq!(&disk[last * 512..last * 512 + 8], b"EFI PART");
        // El CRC de la cabecera se calcula con su propio campo a cero.
        let mut h = disk[512..604].to_vec();
        let stored = u32::from_le_bytes(h[16..20].try_into().unwrap());
        h[16..20].fill(0);
        assert_eq!(crc32::checksum(&h), stored);
        // Primera partición: ESP en el LBA 2048.
        assert_eq!(u64::from_le_bytes(disk[1024 + 32..1024 + 40].try_into().unwrap()), 2048);
        assert_eq!(disk[446 + 4], 0xEE);
        // Reproducible.
        assert_eq!(disk, disk_with_esp(&vec![0u8; 1 << 20], b"prueba"));
    }
}
