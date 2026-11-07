//! # Lector de ELF64
//!
//! Lo usan el cargador UEFI (para cargar `kernel.elf`) y `distro_builder`
//! (símbolos para `kopit addr2line`). Lee los campos con `from_le_bytes`
//! sobre sub-slices comprobados: nunca hay accesos desalineados ni `unsafe`,
//! y un archivo malformado produce un [`ElfError`], no un fallo de memoria
//! dentro del cargador.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

pub const PT_LOAD: u32 = 1;
pub const PT_NOTE: u32 = 4;
pub const PF_X: u32 = 1;
pub const PF_W: u32 = 2;
pub const PF_R: u32 = 4;
pub const EM_X86_64: u16 = 62;
pub const EM_AARCH64: u16 = 183;
pub const EM_RISCV: u16 = 243;
pub const ET_EXEC: u16 = 2;
pub const ET_DYN: u16 = 3;
const SHT_SYMTAB: u32 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ElfError {
    TooShort,
    BadMagic,
    Not64Bit,
    NotLittleEndian,
    BadHeader,
    OutOfBounds,
}

impl ElfError {
    pub const fn message(self) -> &'static str {
        match self {
            ElfError::TooShort => "archivo demasiado corto",
            ElfError::BadMagic => "no es un ELF (firma incorrecta)",
            ElfError::Not64Bit => "no es un ELF de 64 bits",
            ElfError::NotLittleEndian => "no es little-endian",
            ElfError::BadHeader => "cabecera ELF incoherente",
            ElfError::OutOfBounds => "una tabla apunta fuera del archivo",
        }
    }
}

fn u16_at(d: &[u8], o: usize) -> Result<u16, ElfError> {
    d.get(o..o + 2).map(|b| u16::from_le_bytes([b[0], b[1]])).ok_or(ElfError::OutOfBounds)
}

fn u32_at(d: &[u8], o: usize) -> Result<u32, ElfError> {
    d.get(o..o + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]])).ok_or(ElfError::OutOfBounds)
}

fn u64_at(d: &[u8], o: usize) -> Result<u64, ElfError> {
    d.get(o..o + 8)
        .map(|b| u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]))
        .ok_or(ElfError::OutOfBounds)
}

/// Cabecera de programa (segmento).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProgramHeader {
    pub p_type: u32,
    pub flags: u32,
    pub offset: u64,
    pub vaddr: u64,
    pub paddr: u64,
    pub filesz: u64,
    pub memsz: u64,
    pub align: u64,
}

impl ProgramHeader {
    pub fn is_load(&self) -> bool {
        self.p_type == PT_LOAD
    }
    pub fn readable(&self) -> bool {
        self.flags & PF_R != 0
    }
    pub fn writable(&self) -> bool {
        self.flags & PF_W != 0
    }
    pub fn executable(&self) -> bool {
        self.flags & PF_X != 0
    }
}

/// Un símbolo de la tabla `.symtab`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Symbol<'a> {
    pub name: &'a str,
    pub value: u64,
    pub size: u64,
    pub kind: u8,
}

/// Vista de sólo lectura sobre un archivo ELF64 little-endian.
#[derive(Clone, Copy)]
pub struct Elf64<'a> {
    data: &'a [u8],
    pub elf_type: u16,
    pub machine: u16,
    pub entry: u64,
    phoff: u64,
    phentsize: u16,
    phnum: u16,
    shoff: u64,
    shentsize: u16,
    shnum: u16,
}

impl<'a> Elf64<'a> {
    pub fn parse(data: &'a [u8]) -> Result<Self, ElfError> {
        if data.len() < 64 {
            return Err(ElfError::TooShort);
        }
        if &data[0..4] != b"\x7FELF" {
            return Err(ElfError::BadMagic);
        }
        if data[4] != 2 {
            return Err(ElfError::Not64Bit);
        }
        if data[5] != 1 {
            return Err(ElfError::NotLittleEndian);
        }
        let elf = Self {
            data,
            elf_type: u16_at(data, 16)?,
            machine: u16_at(data, 18)?,
            entry: u64_at(data, 24)?,
            phoff: u64_at(data, 32)?,
            shoff: u64_at(data, 40)?,
            phentsize: u16_at(data, 54)?,
            phnum: u16_at(data, 56)?,
            shentsize: u16_at(data, 58)?,
            shnum: u16_at(data, 60)?,
        };
        if elf.phnum > 0 && (elf.phentsize as usize) < 56 {
            return Err(ElfError::BadHeader);
        }
        let ph_end = elf.phoff.saturating_add(elf.phnum as u64 * elf.phentsize as u64);
        if ph_end > data.len() as u64 {
            return Err(ElfError::OutOfBounds);
        }
        Ok(elf)
    }

    pub fn data(&self) -> &'a [u8] {
        self.data
    }

    pub fn program_header_count(&self) -> usize {
        self.phnum as usize
    }

    pub fn program_header(&self, i: usize) -> Result<ProgramHeader, ElfError> {
        if i >= self.phnum as usize {
            return Err(ElfError::OutOfBounds);
        }
        let o = self.phoff as usize + i * self.phentsize as usize;
        let d = self.data;
        Ok(ProgramHeader {
            p_type: u32_at(d, o)?,
            flags: u32_at(d, o + 4)?,
            offset: u64_at(d, o + 8)?,
            vaddr: u64_at(d, o + 16)?,
            paddr: u64_at(d, o + 24)?,
            filesz: u64_at(d, o + 32)?,
            memsz: u64_at(d, o + 40)?,
            align: u64_at(d, o + 48)?,
        })
    }

    /// Itera los segmentos (los malformados se omiten).
    pub fn program_headers(&self) -> impl Iterator<Item = ProgramHeader> + 'a {
        let elf = *self;
        (0..self.phnum as usize).filter_map(move |i| elf.program_header(i).ok())
    }

    /// Bytes de un segmento dentro del archivo (`filesz` bytes desde `offset`).
    pub fn segment_data(&self, ph: &ProgramHeader) -> Result<&'a [u8], ElfError> {
        let start = ph.offset as usize;
        let end = start.checked_add(ph.filesz as usize).ok_or(ElfError::OutOfBounds)?;
        if ph.filesz > ph.memsz {
            return Err(ElfError::BadHeader);
        }
        self.data.get(start..end).ok_or(ElfError::OutOfBounds)
    }

    /// Rango virtual `[min, max)` que ocupan todos los `PT_LOAD`.
    pub fn load_range(&self) -> Option<(u64, u64)> {
        let mut lo = u64::MAX;
        let mut hi = 0;
        for ph in self.program_headers().filter(|p| p.is_load() && p.memsz > 0) {
            lo = lo.min(ph.vaddr);
            hi = hi.max(ph.vaddr.saturating_add(ph.memsz));
        }
        (hi > lo).then_some((lo, hi))
    }

    fn section(&self, i: usize) -> Result<(u32, u64, u64, u32, u64), ElfError> {
        let o = self.shoff as usize + i * self.shentsize as usize;
        let d = self.data;
        // (tipo, offset, tamaño, link, entsize)
        Ok((u32_at(d, o + 4)?, u64_at(d, o + 24)?, u64_at(d, o + 32)?, u32_at(d, o + 40)?, u64_at(d, o + 56)?))
    }

    /// Recorre la tabla de símbolos (vacía si el binario se compiló sin ella).
    pub fn for_each_symbol(&self, mut f: impl FnMut(Symbol<'a>)) -> Result<(), ElfError> {
        if self.shoff == 0 || self.shentsize < 64 {
            return Ok(());
        }
        for i in 0..self.shnum as usize {
            let (kind, off, size, link, entsize) = self.section(i)?;
            if kind != SHT_SYMTAB || entsize < 24 {
                continue;
            }
            let (_, str_off, str_size, _, _) = self.section(link as usize)?;
            let strtab = self.data.get(str_off as usize..(str_off + str_size) as usize).ok_or(ElfError::OutOfBounds)?;
            let count = size / entsize;
            for s in 0..count as usize {
                let o = off as usize + s * entsize as usize;
                let name_off = u32_at(self.data, o)? as usize;
                let info = *self.data.get(o + 4).ok_or(ElfError::OutOfBounds)?;
                let value = u64_at(self.data, o + 8)?;
                let sym_size = u64_at(self.data, o + 16)?;
                let name = strtab
                    .get(name_off..)
                    .and_then(|r| r.split(|&b| b == 0).next())
                    .and_then(|b| core::str::from_utf8(b).ok())
                    .unwrap_or("");
                f(Symbol { name, value, size: sym_size, kind: info & 0xF });
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ELF64 mínimo hecho a mano: cabecera + un PT_LOAD.
    fn tiny_elf() -> Vec<u8> {
        let mut d = vec![0u8; 0x100];
        d[0..4].copy_from_slice(b"\x7FELF");
        d[4] = 2;
        d[5] = 1;
        d[6] = 1;
        d[16..18].copy_from_slice(&ET_EXEC.to_le_bytes());
        d[18..20].copy_from_slice(&EM_X86_64.to_le_bytes());
        d[24..32].copy_from_slice(&0xFFFF_FFFF_8020_0000u64.to_le_bytes());
        d[32..40].copy_from_slice(&64u64.to_le_bytes());
        d[54..56].copy_from_slice(&56u16.to_le_bytes());
        d[56..58].copy_from_slice(&1u16.to_le_bytes());
        let p = 64;
        d[p..p + 4].copy_from_slice(&PT_LOAD.to_le_bytes());
        d[p + 4..p + 8].copy_from_slice(&(PF_R | PF_X).to_le_bytes());
        d[p + 8..p + 16].copy_from_slice(&0xF0u64.to_le_bytes());
        d[p + 16..p + 24].copy_from_slice(&0xFFFF_FFFF_8020_0000u64.to_le_bytes());
        d[p + 24..p + 32].copy_from_slice(&0x20_0000u64.to_le_bytes());
        d[p + 32..p + 40].copy_from_slice(&0x10u64.to_le_bytes());
        d[p + 40..p + 48].copy_from_slice(&0x2000u64.to_le_bytes());
        d
    }

    #[test]
    fn lee_cabecera_y_segmentos() {
        let data = tiny_elf();
        let elf = Elf64::parse(&data).unwrap();
        assert_eq!(elf.machine, EM_X86_64);
        assert_eq!(elf.entry, 0xFFFF_FFFF_8020_0000);
        let ph = elf.program_header(0).unwrap();
        assert!(ph.is_load() && ph.executable() && !ph.writable());
        assert_eq!(elf.segment_data(&ph).unwrap().len(), 0x10);
        assert_eq!(elf.load_range(), Some((0xFFFF_FFFF_8020_0000, 0xFFFF_FFFF_8020_2000)));
    }

    #[test]
    fn rechaza_basura_sin_entrar_en_panico() {
        assert_eq!(Elf64::parse(b"hola").err(), Some(ElfError::TooShort));
        let mut d = tiny_elf();
        d[0] = 0;
        assert_eq!(Elf64::parse(&d).err(), Some(ElfError::BadMagic));
        let mut d = tiny_elf();
        d[56..58].copy_from_slice(&500u16.to_le_bytes());
        assert_eq!(Elf64::parse(&d).err(), Some(ElfError::OutOfBounds));
    }
}
