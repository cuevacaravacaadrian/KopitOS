//! # ISO 9660 + El Torito (UEFI), híbrida
//!
//! ```text
//!  sector 0-15   área de sistema: MBR protector + GPT  ← arranque como USB
//!  sector 16     descriptor primario (PVD)
//!  sector 17     descriptor de arranque El Torito      ← arranque como CD
//!  sector 18     terminador
//!  ...           tablas de rutas, directorios, catálogo de arranque
//!  ...           BOOT/EFIBOOT.IMG (FAT16 con \EFI\BOOT\BOOTX64.EFI) = la ESP
//!  ...           resto de archivos (KOPIT/KERNEL.ELF...) visibles al montar
//!  final         copia de respaldo de la GPT
//! ```
//!
//! La partición ESP de la GPT y la imagen El Torito son **los mismos bytes**:
//! no se duplica nada.

use crate::gpt;
use std::collections::BTreeMap;

const SECTOR: usize = 2048;

pub struct IsoFile {
    pub path: String,
    pub data: Vec<u8>,
}

pub struct IsoOptions<'a> {
    pub volume_id: &'a str,
    /// (año, mes, día, hora, minuto, segundo)
    pub time: (i64, u8, u8, u8, u8, u8),
    pub esp_path: &'a str,
    pub seed: &'a [u8],
}

fn both32(b: &mut [u8], v: u32) {
    b[0..4].copy_from_slice(&v.to_le_bytes());
    b[4..8].copy_from_slice(&v.to_be_bytes());
}

fn both16(b: &mut [u8], v: u16) {
    b[0..2].copy_from_slice(&v.to_le_bytes());
    b[2..4].copy_from_slice(&v.to_be_bytes());
}

fn padded(s: &str, len: usize) -> Vec<u8> {
    let mut v: Vec<u8> = s.bytes().take(len).collect();
    v.resize(len, b' ');
    v
}

fn d_name(s: &str) -> Result<String, String> {
    let up = s.to_ascii_uppercase();
    let (base, ext) = up.rsplit_once('.').unwrap_or((&up, ""));
    let ok = |p: &str| p.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_');
    if base.is_empty() || base.len() > 8 || ext.len() > 3 || !ok(base) || !ok(ext) {
        return Err(format!("«{s}» no es un nombre ISO 9660 de nivel 1 (8.3, A-Z 0-9 _)"));
    }
    Ok(up)
}

fn dir_record(name: &[u8], lba: u32, size: u32, is_dir: bool, date: [u8; 7]) -> Vec<u8> {
    let len = 33 + name.len() + (name.len() % 2 == 0) as usize;
    let mut r = vec![0u8; len];
    r[0] = len as u8;
    both32(&mut r[2..10], lba);
    both32(&mut r[10..18], size);
    r[18..25].copy_from_slice(&date);
    r[25] = if is_dir { 2 } else { 0 };
    both16(&mut r[28..32], 1);
    r[32] = name.len() as u8;
    r[33..33 + name.len()].copy_from_slice(name);
    r
}

#[derive(Default)]
struct Dir {
    dirs: BTreeMap<String, Dir>,
    files: BTreeMap<String, usize>,
    lba: u32,
    number: u16,
}

pub fn build(mut files: Vec<IsoFile>, esp_image: Vec<u8>, opt: &IsoOptions<'_>) -> Result<Vec<u8>, String> {
    files.insert(0, IsoFile { path: opt.esp_path.to_string(), data: esp_image });
    let (y, mo, d, h, mi, s) = opt.time;
    let date = [(y - 1900).clamp(0, 255) as u8, mo, d, h, mi, s, 0];
    let long_date = format!("{:04}{:02}{:02}{:02}{:02}{:02}00", y, mo, d, h, mi, s);

    // Árbol.
    let mut root = Dir::default();
    for (i, f) in files.iter().enumerate() {
        let parts: Vec<&str> = f.path.split(['/', '\\']).filter(|p| !p.is_empty()).collect();
        let (name, dirs) = parts.split_last().ok_or("ruta vacía")?;
        let mut cur = &mut root;
        for p in dirs {
            cur = cur.dirs.entry(d_name(p)?).or_default();
        }
        cur.files.insert(format!("{};1", d_name(name)?), i);
    }

    // Numeración de directorios por niveles (lo exige la tabla de rutas).
    let mut order: Vec<(Vec<String>, u16)> = Vec::new(); // (ruta, número del padre)
    {
        let mut level: Vec<(Vec<String>, u16)> = vec![(Vec::new(), 1)];
        while !level.is_empty() {
            let mut next = Vec::new();
            for (path, parent) in level {
                order.push((path.clone(), parent));
                let me = order.len() as u16;
                let mut node = &root;
                for p in &path {
                    node = &node.dirs[p];
                }
                for name in node.dirs.keys() {
                    let mut child = path.clone();
                    child.push(name.clone());
                    next.push((child, me));
                }
            }
            level = next;
        }
    }
    fn node_mut<'a>(root: &'a mut Dir, path: &[String]) -> &'a mut Dir {
        let mut n = root;
        for p in path {
            n = n.dirs.get_mut(p).unwrap();
        }
        n
    }

    // Tablas de rutas.
    let path_table_size: usize = order
        .iter()
        .map(|(p, _)| {
            let n = p.last().map(|s| s.len()).unwrap_or(1);
            8 + n + n % 2
        })
        .sum();
    let pt_sectors = path_table_size.div_ceil(SECTOR) as u32;
    let l_table = 19u32;
    let m_table = l_table + pt_sectors;
    let mut lba = m_table + pt_sectors;
    for (i, (path, _)) in order.iter().enumerate() {
        let n = node_mut(&mut root, path);
        n.lba = lba;
        n.number = (i + 1) as u16;
        lba += 1; // un sector por directorio (suficiente para la imagen)
    }
    let catalog = lba;
    lba += 1;
    let mut file_lba = vec![0u32; files.len()];
    for (i, f) in files.iter().enumerate() {
        file_lba[i] = lba;
        lba += (f.data.len().div_ceil(SECTOR)).max(1) as u32;
    }
    let iso_sectors = lba as usize;
    // Cola de 9 sectores ISO (36 de 512) para la GPT de respaldo.
    let total_sectors = iso_sectors + 9;
    let mut img = vec![0u8; total_sectors * SECTOR];

    // Descriptor primario.
    let root_rec = dir_record(&[0], root.lba, SECTOR as u32, true, date);
    {
        let pvd = &mut img[16 * SECTOR..17 * SECTOR];
        pvd[0] = 1;
        pvd[1..6].copy_from_slice(b"CD001");
        pvd[6] = 1;
        pvd[8..40].copy_from_slice(&padded("UEFI", 32));
        pvd[40..72].copy_from_slice(&padded(&opt.volume_id.to_ascii_uppercase(), 32));
        both32(&mut pvd[80..88], total_sectors as u32);
        both16(&mut pvd[120..124], 1);
        both16(&mut pvd[124..128], 1);
        both16(&mut pvd[128..132], SECTOR as u16);
        both32(&mut pvd[132..140], path_table_size as u32);
        pvd[140..144].copy_from_slice(&l_table.to_le_bytes());
        pvd[148..152].copy_from_slice(&m_table.to_be_bytes());
        pvd[156..190].copy_from_slice(&root_rec);
        pvd[190..318].copy_from_slice(&padded("", 128));
        pvd[318..446].copy_from_slice(&padded("KOPITOS", 128));
        pvd[446..574].copy_from_slice(&padded("KOPIT DISTRO_BUILDER", 128));
        pvd[574..702].copy_from_slice(&padded("KOPITOS DISTRO_BUILDER", 128));
        pvd[702..813].copy_from_slice(&padded("", 111));
        for at in [813usize, 830, 864] {
            pvd[at..at + 16].copy_from_slice(long_date.as_bytes());
        }
        pvd[847..863].copy_from_slice(b"0000000000000000");
        pvd[881] = 1;
    }
    // Descriptor de arranque El Torito y terminador.
    {
        let br = &mut img[17 * SECTOR..18 * SECTOR];
        br[1..6].copy_from_slice(b"CD001");
        br[6] = 1;
        br[7..30].copy_from_slice(b"EL TORITO SPECIFICATION");
        br[71..75].copy_from_slice(&catalog.to_le_bytes());
        let term = &mut img[18 * SECTOR..19 * SECTOR];
        term[0] = 255;
        term[1..6].copy_from_slice(b"CD001");
        term[6] = 1;
    }
    // Tablas de rutas (L: little-endian, M: big-endian).
    let (mut lt, mut mt) = (Vec::new(), Vec::new());
    for (path, parent) in &order {
        let node = node_mut(&mut root, path);
        let name: Vec<u8> = path.last().map(|s| s.as_bytes().to_vec()).unwrap_or(vec![0]);
        for (table, big) in [(&mut lt, false), (&mut mt, true)] {
            table.push(name.len() as u8);
            table.push(0);
            table.extend_from_slice(&if big { node.lba.to_be_bytes() } else { node.lba.to_le_bytes() });
            table.extend_from_slice(&if big { parent.to_be_bytes() } else { parent.to_le_bytes() });
            table.extend_from_slice(&name);
            if name.len() % 2 == 1 {
                table.push(0);
            }
        }
    }
    img[l_table as usize * SECTOR..l_table as usize * SECTOR + lt.len()].copy_from_slice(&lt);
    img[m_table as usize * SECTOR..m_table as usize * SECTOR + mt.len()].copy_from_slice(&mt);

    // Directorios.
    for (path, parent_number) in &order {
        // La raíz es su propio padre (número 1).
        let parent_lba = node_mut(&mut root, &order[*parent_number as usize - 1].0).lba;
        let node = node_mut(&mut root, path);
        let mut sector = Vec::new();
        sector.extend(dir_record(&[0], node.lba, SECTOR as u32, true, date));
        sector.extend(dir_record(&[1], parent_lba, SECTOR as u32, true, date));
        let mut children: Vec<(String, Vec<u8>)> = Vec::new();
        for (name, sub) in &node.dirs {
            children.push((name.clone(), dir_record(name.as_bytes(), sub.lba, SECTOR as u32, true, date)));
        }
        for (name, &i) in &node.files {
            children.push((
                name.clone(),
                dir_record(name.as_bytes(), file_lba[i], files[i].data.len() as u32, false, date),
            ));
        }
        children.sort_by(|a, b| a.0.cmp(&b.0));
        for (_, rec) in children {
            sector.extend(rec);
        }
        if sector.len() > SECTOR {
            return Err("demasiadas entradas en un directorio de la ISO".into());
        }
        let at = node.lba as usize * SECTOR;
        img[at..at + sector.len()].copy_from_slice(&sector);
    }

    // Catálogo de arranque: entrada de validación (EFI) + entrada inicial.
    {
        let esp_lba = file_lba[0];
        let esp_len = files[0].data.len();
        let cat = &mut img[catalog as usize * SECTOR..catalog as usize * SECTOR + 64];
        cat[0] = 1;
        cat[1] = 0xEF; // plataforma EFI
        cat[4..11].copy_from_slice(b"KOPITOS");
        cat[30] = 0x55;
        cat[31] = 0xAA;
        let sum = (0..16).fold(0u16, |a, i| a.wrapping_add(u16::from_le_bytes([cat[2 * i], cat[2 * i + 1]])));
        cat[28..30].copy_from_slice(&0u16.wrapping_sub(sum).to_le_bytes());
        cat[32] = 0x88; // arrancable
        cat[33] = 0; // sin emulación
        let count = esp_len / 512;
        cat[38..40].copy_from_slice(&(if count <= 0xFFFF { count as u16 } else { 0 }).to_le_bytes());
        cat[40..44].copy_from_slice(&esp_lba.to_le_bytes());
    }

    // Archivos.
    for (i, f) in files.iter().enumerate() {
        let at = file_lba[i] as usize * SECTOR;
        img[at..at + f.data.len()].copy_from_slice(&f.data);
    }

    // GPT híbrida: la ESP es la imagen El Torito.
    let esp_first = file_lba[0] as u64 * 4;
    let esp_last = esp_first + (files[0].data.len() as u64 / 512) - 1;
    let part = gpt::Partition {
        type_guid: gpt::guid_bytes(gpt::ESP_TYPE),
        unique_guid: gpt::guid_from_seed(&[opt.seed, b"iso-esp"].concat()),
        first_lba: esp_first,
        last_lba: esp_last,
        name: "KopitOS EFI".into(),
    };
    gpt::write(&mut img, gpt::guid_from_seed(&[opt.seed, b"iso"].concat()), &[part]);
    Ok(img)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn estructura_basica() {
        let files = vec![
            IsoFile { path: "KOPIT/KERNEL.ELF".into(), data: vec![1; 5000] },
            IsoFile { path: "KOPIT/BOOT.CFG".into(), data: b"x".to_vec() },
            IsoFile { path: "LEEME.TXT".into(), data: b"hola".to_vec() },
        ];
        let esp = vec![0xEE; 64 * 1024];
        let opt =
            IsoOptions { volume_id: "KOPITOS", time: (2026, 1, 1, 0, 0, 0), esp_path: "BOOT/EFIBOOT.IMG", seed: b"t" };
        let img = build(files, esp, &opt).unwrap();
        assert_eq!(&img[16 * SECTOR + 1..16 * SECTOR + 6], b"CD001");
        assert_eq!(&img[17 * SECTOR + 7..17 * SECTOR + 30], b"EL TORITO SPECIFICATION");
        assert_eq!(img[18 * SECTOR], 255);
        assert_eq!(&img[512..520], b"EFI PART");
        // El catálogo apunta a la ESP y la suma de la entrada de validación es 0.
        let cat = u32::from_le_bytes(img[17 * SECTOR + 71..17 * SECTOR + 75].try_into().unwrap()) as usize * SECTOR;
        let sum =
            (0..16).fold(0u16, |a, i| a.wrapping_add(u16::from_le_bytes([img[cat + 2 * i], img[cat + 2 * i + 1]])));
        assert_eq!(sum, 0);
        let esp_lba = u32::from_le_bytes(img[cat + 40..cat + 44].try_into().unwrap()) as usize;
        assert_eq!(img[esp_lba * SECTOR], 0xEE);
        // La partición GPT empieza justo en la ESP.
        assert_eq!(u64::from_le_bytes(img[1024 + 32..1024 + 40].try_into().unwrap()), esp_lba as u64 * 4);
        assert_eq!(img.len() % SECTOR, 0);
    }

    #[test]
    fn nombres_no_validos() {
        assert!(d_name("demasiado_largo.txt").is_err());
        assert_eq!(d_name("kernel.elf").unwrap(), "KERNEL.ELF");
    }
}
