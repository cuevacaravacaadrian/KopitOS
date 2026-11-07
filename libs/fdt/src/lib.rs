//! # Lector de árboles de dispositivos aplanados (FDT / DTB)
//!
//! En ARM64 y RISC-V sin UEFI, el firmware (o QEMU) describe el hardware con
//! un *device tree*: cuánta RAM hay, dónde está la UART, la línea de órdenes
//! (`/chosen/bootargs`)... Este crate lo lee sin memoria dinámica ni `unsafe`
//! para que el núcleo construya con él el mismo `BootInfo` que en x86_64.
//!
//! Formato (todo big-endian): cabecera → bloque de reservas → bloque de
//! estructura (tokens `BEGIN_NODE`/`PROP`/`END_NODE`) → bloque de cadenas.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

const MAGIC: u32 = 0xD00D_FEED;
const BEGIN_NODE: u32 = 1;
const END_NODE: u32 = 2;
const PROP: u32 = 3;
const NOP: u32 = 4;
const END: u32 = 9;
const MAX_DEPTH: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FdtError {
    TooShort,
    BadMagic,
    Truncated,
    Malformed,
}

fn be32(d: &[u8], o: usize) -> Result<u32, FdtError> {
    d.get(o..o + 4).map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]])).ok_or(FdtError::Truncated)
}

fn be64(d: &[u8], o: usize) -> Result<u64, FdtError> {
    Ok(((be32(d, o)? as u64) << 32) | be32(d, o + 4)? as u64)
}

/// Tamaño total de un DTB a partir de su cabecera (para crear el slice).
pub fn total_size(header: &[u8]) -> Result<usize, FdtError> {
    if header.len() < 8 {
        return Err(FdtError::TooShort);
    }
    if be32(header, 0)? != MAGIC {
        return Err(FdtError::BadMagic);
    }
    Ok(be32(header, 4)? as usize)
}

/// Un nodo del árbol durante el recorrido.
#[derive(Clone, Copy)]
pub struct Node<'a> {
    fdt: &'a Fdt<'a>,
    pub name: &'a str,
    pub depth: usize,
    /// Celdas de dirección/tamaño que usa el `reg` de este nodo (las define el padre).
    pub address_cells: u32,
    pub size_cells: u32,
    props_at: usize,
}

impl<'a> Node<'a> {
    /// Valor de una propiedad de este nodo.
    pub fn prop(&self, name: &str) -> Option<&'a [u8]> {
        let mut o = self.props_at;
        loop {
            match self.fdt.token(o).ok()? {
                PROP => {
                    let len = be32(self.fdt.data, o + 4).ok()? as usize;
                    let nameoff = be32(self.fdt.data, o + 8).ok()? as usize;
                    let value = self.fdt.data.get(o + 12..o + 12 + len)?;
                    if self.fdt.string(nameoff) == Some(name) {
                        return Some(value);
                    }
                    o += 12 + align4(len);
                }
                NOP => o += 4,
                _ => return None,
            }
        }
    }

    pub fn prop_str(&self, name: &str) -> Option<&'a str> {
        let v = self.prop(name)?;
        let v = v.strip_suffix(&[0]).unwrap_or(v);
        core::str::from_utf8(v).ok()
    }

    pub fn prop_u32(&self, name: &str) -> Option<u32> {
        self.prop(name).and_then(|v| be32(v, 0).ok())
    }

    /// ¿Aparece `compat` en la lista `compatible`?
    pub fn is_compatible(&self, compat: &str) -> bool {
        self.prop("compatible").map(|v| v.split(|&b| b == 0).any(|s| s == compat.as_bytes())).unwrap_or(false)
    }

    /// Pares (dirección, tamaño) de la propiedad `reg`.
    pub fn reg(&self) -> impl Iterator<Item = (u64, u64)> + 'a {
        let v = self.prop("reg").unwrap_or(&[]);
        let ac = self.address_cells as usize;
        let sc = self.size_cells as usize;
        let stride = (ac + sc) * 4;
        let count = if stride == 0 { 0 } else { v.len() / stride };
        (0..count).map(move |i| {
            let read = |o: usize, cells: usize| -> u64 {
                match cells {
                    1 => be32(v, o).unwrap_or(0) as u64,
                    2 => be64(v, o).unwrap_or(0),
                    _ => 0,
                }
            };
            let o = i * stride;
            (read(o, ac), read(o + ac * 4, sc))
        })
    }

    /// Nombre sin la parte de unidad (`memory@80000000` → `memory`).
    pub fn base_name(&self) -> &'a str {
        self.name.split('@').next().unwrap_or(self.name)
    }
}

/// Vista sobre un DTB.
pub struct Fdt<'a> {
    data: &'a [u8],
    struct_off: usize,
    strings_off: usize,
    strings_size: usize,
    rsv_off: usize,
    pub boot_cpu: u32,
}

impl<'a> Fdt<'a> {
    pub fn parse(data: &'a [u8]) -> Result<Self, FdtError> {
        let size = total_size(data)?;
        if data.len() < size || size < 40 {
            return Err(FdtError::Truncated);
        }
        let data = &data[..size];
        Ok(Self {
            data,
            struct_off: be32(data, 8)? as usize,
            strings_off: be32(data, 12)? as usize,
            rsv_off: be32(data, 16)? as usize,
            boot_cpu: be32(data, 28)?,
            strings_size: be32(data, 32)? as usize,
        })
    }

    pub fn size(&self) -> usize {
        self.data.len()
    }

    fn token(&self, o: usize) -> Result<u32, FdtError> {
        be32(self.data, o)
    }

    fn string(&self, off: usize) -> Option<&'a str> {
        if off >= self.strings_size {
            return None;
        }
        let s = self.data.get(self.strings_off + off..self.strings_off + self.strings_size)?;
        let end = s.iter().position(|&b| b == 0)?;
        core::str::from_utf8(&s[..end]).ok()
    }

    /// Zonas de memoria reservadas por el firmware (`/memreserve/`).
    pub fn reservations(&self) -> impl Iterator<Item = (u64, u64)> + 'a {
        let data = self.data;
        let start = self.rsv_off;
        (0..256usize)
            .map(move |i| (be64(data, start + i * 16).unwrap_or(0), be64(data, start + i * 16 + 8).unwrap_or(0)))
            .take_while(|&(a, s)| a != 0 || s != 0)
    }

    /// Recorre todos los nodos en orden (profundidad primero).
    pub fn for_each_node(&'a self, mut f: impl FnMut(&Node<'a>)) -> Result<(), FdtError> {
        // Celdas declaradas por cada nivel (las heredan los hijos).
        let mut cells = [(2u32, 1u32); MAX_DEPTH + 1];
        let mut depth = 0usize;
        let mut o = self.struct_off;
        loop {
            match self.token(o)? {
                BEGIN_NODE => {
                    let name_start = o + 4;
                    let rest = self.data.get(name_start..).ok_or(FdtError::Truncated)?;
                    let len = rest.iter().position(|&b| b == 0).ok_or(FdtError::Malformed)?;
                    let name = core::str::from_utf8(&rest[..len]).map_err(|_| FdtError::Malformed)?;
                    let props_at = name_start + align4(len + 1);
                    if depth >= MAX_DEPTH {
                        return Err(FdtError::Malformed);
                    }
                    let (ac, sc) = if depth == 0 { (2, 1) } else { cells[depth - 1] };
                    let node = Node { fdt: self, name, depth, address_cells: ac, size_cells: sc, props_at };
                    // Este nodo define las celdas de sus hijos.
                    cells[depth] =
                        (node.prop_u32("#address-cells").unwrap_or(2), node.prop_u32("#size-cells").unwrap_or(1));
                    f(&node);
                    depth += 1;
                    o = props_at;
                }
                PROP => {
                    let len = be32(self.data, o + 4)? as usize;
                    o += 12 + align4(len);
                }
                END_NODE => {
                    depth = depth.checked_sub(1).ok_or(FdtError::Malformed)?;
                    o += 4;
                }
                NOP => o += 4,
                END => return Ok(()),
                _ => return Err(FdtError::Malformed),
            }
        }
    }

    /// Primer nodo que cumpla `pred`.
    pub fn find(&'a self, mut pred: impl FnMut(&Node<'a>) -> bool) -> Option<Node<'a>> {
        let mut found = None;
        let _ = self.for_each_node(|n| {
            if found.is_none() && pred(n) {
                found = Some(*n);
            }
        });
        found
    }

    /// Primer nodo compatible con alguno de `compats`.
    pub fn find_compatible(&'a self, compats: &[&str]) -> Option<Node<'a>> {
        self.find(|n| compats.iter().any(|c| n.is_compatible(c)))
    }

    /// `/chosen/bootargs`: la línea de órdenes del núcleo.
    pub fn bootargs(&'a self) -> Option<&'a str> {
        self.find(|n| n.depth == 1 && n.name == "chosen").and_then(|n| n.prop_str("bootargs"))
    }

    /// Bancos de RAM (`memory@...` con `device_type = "memory"`).
    pub fn for_each_memory(&'a self, mut f: impl FnMut(u64, u64)) -> Result<(), FdtError> {
        self.for_each_node(|n| {
            if n.depth == 1 && (n.base_name() == "memory" || n.prop_str("device_type") == Some("memory")) {
                for (base, size) in n.reg() {
                    f(base, size);
                }
            }
        })
    }

    /// Regiones hijas de `/reserved-memory`.
    pub fn for_each_reserved(&'a self, mut f: impl FnMut(u64, u64)) -> Result<(), FdtError> {
        let mut inside = false;
        self.for_each_node(|n| {
            if n.depth == 1 {
                inside = n.name == "reserved-memory";
            } else if inside && n.depth == 2 {
                for (base, size) in n.reg() {
                    f(base, size);
                }
            }
        })
    }
}

const fn align4(v: usize) -> usize {
    (v + 3) & !3
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Constructor mínimo de DTB para las pruebas.
    struct Builder {
        structure: Vec<u8>,
        strings: Vec<u8>,
    }

    impl Builder {
        fn new() -> Self {
            Self { structure: Vec::new(), strings: Vec::new() }
        }
        fn begin(&mut self, name: &str) {
            self.structure.extend_from_slice(&BEGIN_NODE.to_be_bytes());
            self.structure.extend_from_slice(name.as_bytes());
            self.structure.push(0);
            while self.structure.len() % 4 != 0 {
                self.structure.push(0);
            }
        }
        fn end(&mut self) {
            self.structure.extend_from_slice(&END_NODE.to_be_bytes());
        }
        fn prop(&mut self, name: &str, value: &[u8]) {
            let off = self.strings.len() as u32;
            self.strings.extend_from_slice(name.as_bytes());
            self.strings.push(0);
            self.structure.extend_from_slice(&PROP.to_be_bytes());
            self.structure.extend_from_slice(&(value.len() as u32).to_be_bytes());
            self.structure.extend_from_slice(&off.to_be_bytes());
            self.structure.extend_from_slice(value);
            while self.structure.len() % 4 != 0 {
                self.structure.push(0);
            }
        }
        fn finish(mut self) -> Vec<u8> {
            self.structure.extend_from_slice(&END.to_be_bytes());
            let rsv_off = 40usize;
            let rsv = [0x8000_0000u64.to_be_bytes(), 0x20_0000u64.to_be_bytes(), [0; 8], [0; 8]].concat();
            let struct_off = rsv_off + rsv.len();
            let strings_off = struct_off + self.structure.len();
            let total = strings_off + self.strings.len();
            let mut out = Vec::new();
            for v in [
                MAGIC,
                total as u32,
                struct_off as u32,
                strings_off as u32,
                rsv_off as u32,
                17,
                16,
                0,
                self.strings.len() as u32,
                self.structure.len() as u32,
            ] {
                out.extend_from_slice(&v.to_be_bytes());
            }
            out.extend_from_slice(&rsv);
            out.extend_from_slice(&self.structure);
            out.extend_from_slice(&self.strings);
            out
        }
    }

    fn sample() -> Vec<u8> {
        let mut b = Builder::new();
        b.begin("");
        b.prop("#address-cells", &2u32.to_be_bytes());
        b.prop("#size-cells", &2u32.to_be_bytes());
        b.begin("chosen");
        b.prop("bootargs", b"loglevel=debug theme=aurora\0");
        b.end();
        b.begin("memory@80000000");
        b.prop("device_type", b"memory\0");
        b.prop("reg", &[0x8000_0000u64.to_be_bytes(), 0x0800_0000u64.to_be_bytes()].concat());
        b.end();
        b.begin("soc");
        b.prop("#address-cells", &1u32.to_be_bytes());
        b.prop("#size-cells", &1u32.to_be_bytes());
        b.begin("serial@10000000");
        b.prop("compatible", b"ns16550a\0");
        b.prop("reg", &[0x1000_0000u32.to_be_bytes(), 0x100u32.to_be_bytes()].concat());
        b.end();
        b.end();
        b.end();
        b.finish()
    }

    #[test]
    fn lee_memoria_bootargs_y_uart() {
        let data = sample();
        let fdt = Fdt::parse(&data).unwrap();
        assert_eq!(fdt.bootargs(), Some("loglevel=debug theme=aurora"));
        let mut mem = Vec::new();
        fdt.for_each_memory(|b, s| mem.push((b, s))).unwrap();
        assert_eq!(mem, vec![(0x8000_0000, 0x0800_0000)]);
        let uart = fdt.find_compatible(&["ns16550a"]).unwrap();
        assert_eq!(uart.reg().next(), Some((0x1000_0000, 0x100)));
        assert_eq!(fdt.reservations().collect::<Vec<_>>(), vec![(0x8000_0000, 0x20_0000)]);
    }

    #[test]
    fn firma_mala() {
        assert_eq!(Fdt::parse(&[0u8; 64]).err(), Some(FdtError::BadMagic));
    }
}
