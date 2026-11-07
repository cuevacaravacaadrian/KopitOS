//! # Escritor de sistemas de archivos FAT16 / FAT32
//!
//! Genera la partición de sistema EFI (ESP) sin `mkfs.fat` ni `mtools`, igual
//! en Windows, Linux y macOS.
//!
//! * **FAT32** para la imagen de disco (la especificación UEFI lo exige en
//!   discos fijos) y **FAT16** para la imagen El Torito de la ISO (más pequeña).
//! * El tipo de FAT lo decide **el número de clústeres** (regla de Microsoft):
//!   < 65 525 → FAT16, ≥ 65 525 → FAT32. La geometría se calcula para caer en
//!   el lado correcto.
//! * Nombres 8.3 con las banderas de minúsculas de Windows NT: `kernel.elf` se
//!   guarda como `KERNEL  ELF` pero se ve en minúsculas en todos los sistemas.
//! * Fecha fija (reproducible) y número de serie derivado del contenido.

use std::collections::BTreeMap;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FatKind {
    Fat16,
    Fat32,
}

pub struct FatFile {
    pub path: String,
    pub data: Vec<u8>,
}

/// Fecha y hora en formato DOS.
#[derive(Clone, Copy)]
pub struct DosTime {
    pub date: u16,
    pub time: u16,
}

impl DosTime {
    pub fn new(year: i64, month: u8, day: u8, hour: u8, minute: u8, second: u8) -> Self {
        let y = (year.clamp(1980, 2107) - 1980) as u16;
        Self {
            date: (y << 9) | ((month as u16) << 5) | day as u16,
            time: ((hour as u16) << 11) | ((minute as u16) << 5) | (second as u16 / 2),
        }
    }
}

#[derive(Default)]
struct Dir {
    dirs: BTreeMap<String, Dir>,
    files: BTreeMap<String, usize>,
    display: BTreeMap<String, String>,
    cluster: u32,
}

/// Nombre 8.3: (11 bytes, banderas de minúsculas).
fn short_name(name: &str) -> Result<([u8; 11], u8), String> {
    let (base, ext) = match name.rsplit_once('.') {
        Some((b, e)) => (b, e),
        None => (name, ""),
    };
    let valid = |s: &str| s.chars().all(|c| c.is_ascii_alphanumeric() || "_-$~!#%&'(){}^@`".contains(c));
    if base.is_empty() || base.len() > 8 || ext.len() > 3 || !valid(base) || !valid(ext) {
        return Err(format!("«{name}» no es un nombre 8.3 válido (máx. 8+3 caracteres ASCII)"));
    }
    let mut out = [b' '; 11];
    for (i, c) in base.bytes().enumerate() {
        out[i] = c.to_ascii_uppercase();
    }
    for (i, c) in ext.bytes().enumerate() {
        out[8 + i] = c.to_ascii_uppercase();
    }
    let lower = |s: &str| {
        !s.is_empty() && s.chars().all(|c| !c.is_ascii_uppercase()) && s.chars().any(|c| c.is_ascii_lowercase())
    };
    let mut flags = 0;
    if lower(base) {
        flags |= 0x08;
    }
    if lower(ext) {
        flags |= 0x10;
    }
    Ok((out, flags))
}

fn put16(b: &mut [u8], o: usize, v: u16) {
    b[o..o + 2].copy_from_slice(&v.to_le_bytes());
}

fn put32(b: &mut [u8], o: usize, v: u32) {
    b[o..o + 4].copy_from_slice(&v.to_le_bytes());
}

/// Entrada de directorio de 32 bytes.
fn dir_entry(name: [u8; 11], attr: u8, case: u8, cluster: u32, size: u32, t: DosTime) -> [u8; 32] {
    let mut e = [0u8; 32];
    e[..11].copy_from_slice(&name);
    e[11] = attr;
    e[12] = case;
    put16(&mut e, 14, t.time);
    put16(&mut e, 16, t.date);
    put16(&mut e, 18, t.date);
    put16(&mut e, 20, (cluster >> 16) as u16);
    put16(&mut e, 22, t.time);
    put16(&mut e, 24, t.date);
    put16(&mut e, 26, cluster as u16);
    put32(&mut e, 28, size);
    e
}

struct Geometry {
    kind: FatKind,
    reserved: u32,
    fat_sectors: u32,
    root_dir_sectors: u32,
    sectors_per_cluster: u32,
    clusters: u32,
}

impl Geometry {
    fn compute(kind: FatKind, total_sectors: u32) -> Result<Self, String> {
        let (reserved, root_entries, min_clusters, max_clusters) = match kind {
            FatKind::Fat16 => (4u32, 512u32, 4_085u32, 65_524u32),
            FatKind::Fat32 => (32, 0, 65_525, 0x0FFF_FFF5),
        };
        let root_dir_sectors = root_entries * 32 / 512;
        for spc in [1u32, 2, 4, 8, 16, 32, 64] {
            // Fórmula de la especificación FAT de Microsoft.
            let tmp1 = total_sectors.saturating_sub(reserved + root_dir_sectors);
            let mut tmp2 = 256 * spc + 2;
            if kind == FatKind::Fat32 {
                tmp2 /= 2;
            }
            let fat_sectors = tmp1.div_ceil(tmp2);
            let data = total_sectors.saturating_sub(reserved + 2 * fat_sectors + root_dir_sectors);
            let clusters = data / spc;
            if clusters < min_clusters {
                break;
            }
            if clusters <= max_clusters {
                return Ok(Self { kind, reserved, fat_sectors, root_dir_sectors, sectors_per_cluster: spc, clusters });
            }
        }
        Err(format!(
            "{} sectores no permiten un {:?} válido (FAT32 necesita ≥ 33 MiB; FAT16, entre 2 MiB y 4 GiB)",
            total_sectors, kind
        ))
    }

    fn cluster_bytes(&self) -> usize {
        self.sectors_per_cluster as usize * 512
    }

    fn data_start(&self) -> u32 {
        self.reserved + 2 * self.fat_sectors + self.root_dir_sectors
    }

    fn cluster_offset(&self, cluster: u32) -> usize {
        (self.data_start() as usize + (cluster as usize - 2) * self.sectors_per_cluster as usize) * 512
    }
}

pub struct FatOptions<'a> {
    pub kind: FatKind,
    pub size_bytes: u64,
    pub label: &'a str,
    pub volume_id: u32,
    pub hidden_sectors: u32,
    pub time: DosTime,
}

pub fn build(files: &[FatFile], opt: &FatOptions<'_>) -> Result<Vec<u8>, String> {
    let total_sectors = u32::try_from(opt.size_bytes / 512).map_err(|_| "imagen demasiado grande")?;
    let g = Geometry::compute(opt.kind, total_sectors)?;
    let cluster_bytes = g.cluster_bytes();

    // 1) Árbol de directorios.
    let mut root = Dir::default();
    for (index, f) in files.iter().enumerate() {
        let parts: Vec<&str> = f.path.split(['/', '\\']).filter(|p| !p.is_empty()).collect();
        let (file, dirs) = parts.split_last().ok_or("ruta vacía")?;
        let mut d = &mut root;
        for part in dirs {
            short_name(part)?;
            d.display.insert(part.to_ascii_uppercase(), part.to_string());
            d = d.dirs.entry(part.to_ascii_uppercase()).or_default();
        }
        short_name(file)?;
        d.display.insert(file.to_ascii_uppercase(), file.to_string());
        d.files.insert(file.to_ascii_uppercase(), index);
    }

    // 2) Reparto de clústeres (consecutivos, sin fragmentar).
    let mut next = 2u32;
    let mut fat: Vec<u32> = vec![0; g.clusters as usize + 2];
    let mut alloc_chain = |bytes: usize, fat: &mut Vec<u32>| -> Result<u32, String> {
        let n = bytes.div_ceil(cluster_bytes).max(1) as u32;
        let first = next;
        if (first + n) as usize > fat.len() {
            return Err("la imagen se queda pequeña para estos archivos".into());
        }
        for c in first..first + n {
            fat[c as usize] = if c + 1 < first + n { c + 1 } else { 0x0FFF_FFFF };
        }
        next += n;
        Ok(first)
    };
    fn entries_of(d: &Dir, is_root: bool) -> usize {
        d.dirs.len() + d.files.len() + if is_root { 1 } else { 2 }
    }
    fn assign(
        d: &mut Dir,
        is_root: bool,
        kind: FatKind,
        alloc: &mut dyn FnMut(usize) -> Result<u32, String>,
    ) -> Result<(), String> {
        // En FAT16 la raíz tiene su propia zona fija; en FAT32 es un directorio más.
        if !(is_root && kind == FatKind::Fat16) {
            d.cluster = alloc(entries_of(d, is_root) * 32)?;
        }
        for sub in d.dirs.values_mut() {
            assign(sub, false, kind, alloc)?;
        }
        Ok(())
    }
    let mut file_clusters = vec![0u32; files.len()];
    {
        let mut alloc = |bytes: usize| alloc_chain(bytes, &mut fat);
        assign(&mut root, true, opt.kind, &mut alloc)?;
        for (i, f) in files.iter().enumerate() {
            if !f.data.is_empty() {
                file_clusters[i] = alloc(f.data.len())?;
            }
        }
    }
    let used = next - 2;

    // 3) Imagen.
    let mut img = vec![0u8; total_sectors as usize * 512];
    let bs = &mut img[..512];
    bs[0..3].copy_from_slice(&[0xEB, 0x58, 0x90]);
    bs[3..11].copy_from_slice(b"KOPITOS ");
    put16(bs, 11, 512);
    bs[13] = g.sectors_per_cluster as u8;
    put16(bs, 14, g.reserved as u16);
    bs[16] = 2;
    put16(bs, 17, if opt.kind == FatKind::Fat16 { 512 } else { 0 });
    if total_sectors < 65_536 {
        put16(bs, 19, total_sectors as u16);
    } else {
        put32(bs, 32, total_sectors);
    }
    bs[21] = 0xF8;
    put16(bs, 24, 63);
    put16(bs, 26, 255);
    put32(bs, 28, opt.hidden_sectors);
    let mut label = [b' '; 11];
    for (i, c) in opt.label.bytes().take(11).enumerate() {
        label[i] = c.to_ascii_uppercase();
    }
    let ext = match opt.kind {
        FatKind::Fat16 => {
            put16(bs, 22, g.fat_sectors as u16);
            36
        }
        FatKind::Fat32 => {
            put32(bs, 36, g.fat_sectors);
            put32(bs, 44, 2); // clúster del directorio raíz
            put16(bs, 48, 1); // sector FSInfo
            put16(bs, 50, 6); // copia del sector de arranque
            64
        }
    };
    bs[ext] = 0x80;
    bs[ext + 2] = 0x29;
    put32(bs, ext + 3, opt.volume_id);
    bs[ext + 7..ext + 18].copy_from_slice(&label);
    bs[ext + 18..ext + 26].copy_from_slice(if opt.kind == FatKind::Fat16 { b"FAT16   " } else { b"FAT32   " });
    // Si alguien intenta arrancar esto por BIOS: mensaje y parada (modo real).
    //   xor ax,ax · mov ds,ax · mov si,msg · bucle: lodsb · or al,al · jz fin
    //   mov ah,0Eh · int 10h · jmp bucle · fin: hlt · jmp fin
    let msg = b"KopitOS arranca con UEFI. Activa UEFI en el firmware.\r\n\0";
    let code_at = ext + 26;
    let stub: [u8; 21] = [
        0x31, 0xC0, 0x8E, 0xD8, 0xBE, 0x00, 0x00, 0xAC, 0x08, 0xC0, 0x74, 0x06, 0xB4, 0x0E, 0xCD, 0x10, 0xEB, 0xF5,
        0xF4, 0xEB, 0xFD,
    ];
    let msg_at = code_at + stub.len();
    bs[1] = (code_at - 2) as u8; // el salto inicial va al código
    bs[code_at..code_at + stub.len()].copy_from_slice(&stub);
    put16(bs, code_at + 5, (0x7C00 + msg_at) as u16);
    bs[msg_at..msg_at + msg.len()].copy_from_slice(msg);
    bs[510] = 0x55;
    bs[511] = 0xAA;

    if opt.kind == FatKind::Fat32 {
        let fsinfo = &mut img[512..1024];
        put32(fsinfo, 0, 0x4161_5252);
        put32(fsinfo, 484, 0x6141_7272);
        put32(fsinfo, 488, g.clusters - used);
        put32(fsinfo, 492, next);
        put32(fsinfo, 508, 0xAA55_0000);
        let (head, tail) = img.split_at_mut(6 * 512);
        tail[..1024].copy_from_slice(&head[..1024]); // copias en los sectores 6 y 7
    }

    // FATs (dos copias).
    let fat_bytes = g.fat_sectors as usize * 512;
    let mut fat_area = vec![0u8; fat_bytes];
    match opt.kind {
        FatKind::Fat16 => {
            put16(&mut fat_area, 0, 0xFFF8);
            put16(&mut fat_area, 2, 0xFFFF);
            for (c, &v) in fat.iter().enumerate().skip(2) {
                if v != 0 {
                    put16(&mut fat_area, c * 2, if v == 0x0FFF_FFFF { 0xFFFF } else { v as u16 });
                }
            }
        }
        FatKind::Fat32 => {
            put32(&mut fat_area, 0, 0x0FFF_FFF8);
            put32(&mut fat_area, 4, 0x0FFF_FFFF);
            for (c, &v) in fat.iter().enumerate().skip(2) {
                if v != 0 {
                    put32(&mut fat_area, c * 4, v);
                }
            }
        }
    }
    for copy in 0..2 {
        let at = (g.reserved as usize + copy * g.fat_sectors as usize) * 512;
        img[at..at + fat_bytes].copy_from_slice(&fat_area);
    }

    // Directorios y archivos.
    fn write_dir(
        img: &mut [u8],
        g: &Geometry,
        d: &Dir,
        parent: u32,
        is_root: bool,
        label: [u8; 11],
        files: &[FatFile],
        fc: &[u32],
        t: DosTime,
    ) -> Result<(), String> {
        let mut entries: Vec<[u8; 32]> = Vec::new();
        if is_root {
            entries.push(dir_entry(label, 0x08, 0, 0, 0, t));
        } else {
            let mut dot = [b' '; 11];
            dot[0] = b'.';
            entries.push(dir_entry(dot, 0x10, 0, d.cluster, 0, t));
            dot[1] = b'.';
            entries.push(dir_entry(dot, 0x10, 0, parent, 0, t));
        }
        for (key, sub) in &d.dirs {
            let (name, case) = short_name(&d.display[key])?;
            entries.push(dir_entry(name, 0x10, case, sub.cluster, 0, t));
        }
        for (key, &i) in &d.files {
            let (name, case) = short_name(&d.display[key])?;
            entries.push(dir_entry(name, 0x20, case, fc[i], files[i].data.len() as u32, t));
        }
        let at = if is_root && g.kind == FatKind::Fat16 {
            (g.reserved + 2 * g.fat_sectors) as usize * 512
        } else {
            g.cluster_offset(d.cluster)
        };
        for (n, e) in entries.iter().enumerate() {
            img[at + n * 32..at + n * 32 + 32].copy_from_slice(e);
        }
        // ".." de un hijo de la raíz apunta al clúster 0 (así lo pide la especificación).
        let me = if is_root { 0 } else { d.cluster };
        for sub in d.dirs.values() {
            write_dir(img, g, sub, me, false, label, files, fc, t)?;
        }
        Ok(())
    }
    write_dir(&mut img, &g, &root, 0, true, label, files, &file_clusters, opt.time)?;
    for (i, f) in files.iter().enumerate() {
        if !f.data.is_empty() {
            let at = g.cluster_offset(file_clusters[i]);
            img[at..at + f.data.len()].copy_from_slice(&f.data);
        }
    }
    Ok(img)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts(kind: FatKind, mib: u64) -> FatOptions<'static> {
        FatOptions {
            kind,
            size_bytes: mib << 20,
            label: "KOPITOS",
            volume_id: 0x1234_5678,
            hidden_sectors: 0,
            time: DosTime::new(2026, 1, 1, 0, 0, 0),
        }
    }

    fn files() -> Vec<FatFile> {
        vec![
            FatFile { path: "EFI/BOOT/BOOTX64.EFI".into(), data: vec![0xAB; 3000] },
            FatFile { path: "kopit/kernel.elf".into(), data: (0..100_000u32).map(|i| i as u8).collect() },
            FatFile { path: "kopit/boot.cfg".into(), data: b"timeout=3\n".to_vec() },
        ]
    }

    #[test]
    fn nombres_8_3() {
        assert_eq!(short_name("kernel.elf").unwrap(), (*b"KERNEL  ELF", 0x18));
        assert_eq!(short_name("BOOTX64.EFI").unwrap(), (*b"BOOTX64 EFI", 0));
        assert!(short_name("nombre-demasiado-largo.txt").is_err());
        assert!(short_name("ñ.txt").is_err());
    }

    #[test]
    fn geometria_segun_el_tipo() {
        let g = Geometry::compute(FatKind::Fat32, (64 << 20) / 512).unwrap();
        assert!(g.clusters >= 65_525);
        let g = Geometry::compute(FatKind::Fat16, (16 << 20) / 512).unwrap();
        assert!((4_085..65_525).contains(&g.clusters));
        assert!(Geometry::compute(FatKind::Fat32, (8 << 20) / 512).is_err());
    }

    /// Relee la imagen y comprueba que el archivo está donde dice el directorio.
    fn read_back(img: &[u8], path: &[&str]) -> Vec<u8> {
        let bps = u16::from_le_bytes([img[11], img[12]]) as usize;
        let spc = img[13] as usize;
        let reserved = u16::from_le_bytes([img[14], img[15]]) as usize;
        let root_entries = u16::from_le_bytes([img[17], img[18]]) as usize;
        let fat16 = root_entries > 0;
        let fat_sz = if fat16 {
            u16::from_le_bytes([img[22], img[23]]) as usize
        } else {
            u32::from_le_bytes(img[36..40].try_into().unwrap()) as usize
        };
        let root_secs = root_entries * 32 / bps;
        let data = reserved + 2 * fat_sz + root_secs;
        let cl = |c: usize| (data + (c - 2) * spc) * bps;
        let mut dir_at = if fat16 { (reserved + 2 * fat_sz) * bps } else { cl(2) };
        for (depth, part) in path.iter().enumerate() {
            let want = short_name(part).unwrap().0;
            let mut found = None;
            for n in 0..64 {
                let e = &img[dir_at + n * 32..dir_at + n * 32 + 32];
                if e[..11] == want {
                    let c = (u16::from_le_bytes([e[20], e[21]]) as usize) << 16
                        | u16::from_le_bytes([e[26], e[27]]) as usize;
                    found = Some((c, u32::from_le_bytes(e[28..32].try_into().unwrap()) as usize));
                    break;
                }
            }
            let (c, size) = found.expect("entrada no encontrada");
            if depth + 1 == path.len() {
                return img[cl(c)..cl(c) + size].to_vec();
            }
            dir_at = cl(c);
        }
        unreachable!()
    }

    #[test]
    fn fat32_y_fat16_se_releen() {
        for (kind, mib) in [(FatKind::Fat32, 40), (FatKind::Fat16, 8)] {
            let img = build(&files(), &opts(kind, mib)).unwrap();
            assert_eq!(img.len(), (mib << 20) as usize);
            assert_eq!(&img[510..512], &[0x55, 0xAA]);
            assert_eq!(read_back(&img, &["KOPIT", "KERNEL.ELF"]), files()[1].data);
            assert_eq!(read_back(&img, &["EFI", "BOOT", "BOOTX64.EFI"]), files()[0].data);
        }
    }
}
