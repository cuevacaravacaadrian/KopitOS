//! # KBP — Kopit Boot Protocol (versión 1)
//!
//! Es el **único** contrato entre "lo que arranca" y el núcleo. Da igual quién
//! cargue a KopitOS (nuestro cargador UEFI *KopitBoot*, GRUB vía Multiboot2,
//! QEMU/Firecracker vía PVH o un árbol de dispositivos en ARM/RISC-V): todos
//! terminan rellenando un [`BootInfo`] idéntico y llamando a `kernel_main`.
//!
//! ## Decisiones de diseño
//!
//! * **`#[repr(C)]` y tipos de ancho fijo**: el cargador UEFI es un binario PE
//!   distinto del núcleo (ELF); la estructura se comparte por memoria, así que
//!   su disposición debe ser estable y no depender del compilador.
//! * **Enumerados como `u32` con constantes** ([`MemoryKind`], [`BootSource`]):
//!   un `enum` de Rust con un valor desconocido sería comportamiento indefinido.
//!   Con un `u32` envuelto, un cargador más nuevo puede añadir tipos y un núcleo
//!   más viejo simplemente los trata como "reservado".
//! * **Punteros = direcciones virtuales válidas al entrar al núcleo** (dentro
//!   del mapa directo HHDM o de la imagen del núcleo). Así el núcleo puede leer
//!   todo sin preparar nada antes.
//! * **Campos `size` y `reserved`**: permiten ampliar el protocolo sin romper
//!   compatibilidad (un núcleo lee sólo hasta `size`).
//! * **Sin dependencias y `no_std`**: lo usan el cargador, el núcleo y las
//!   pruebas del anfitrión.

#![cfg_attr(not(test), no_std)]

/// Firma mágica de [`BootInfo`]: los bytes ASCII `"KOPITKBP"`.
pub const KBP_MAGIC: u64 = u64::from_le_bytes(*b"KOPITKBP");
/// Versión del protocolo que describe este crate.
pub const KBP_VERSION: u32 = 1;

/// Base del mapa directo de memoria física (HHDM) en x86_64: PML4[256].
/// Toda la RAM física `p` es accesible en `HHDM_BASE_X86_64 + p`.
pub const HHDM_BASE_X86_64: u64 = 0xFFFF_8000_0000_0000;
/// Base virtual del núcleo en x86_64 (los últimos 2 GiB: `-mcmodel=kernel`).
pub const KERNEL_VMA_X86_64: u64 = 0xFFFF_FFFF_8000_0000;

/// Tamaño de página base usado por todo el protocolo.
pub const PAGE_SIZE: u64 = 4096;

// -----------------------------------------------------------------------------
// Tipos "enumerados" estables para el ABI
// -----------------------------------------------------------------------------

/// Quién arrancó al núcleo.
#[repr(transparent)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct BootSource(pub u32);

impl BootSource {
    pub const UNKNOWN: Self = Self(0);
    /// KopitBoot, nuestro cargador UEFI.
    pub const KOPITBOOT_UEFI: Self = Self(1);
    /// GRUB u otro cargador compatible con Multiboot2.
    pub const MULTIBOOT2: Self = Self(2);
    /// Arranque directo PVH (QEMU `-kernel`, Firecracker, Cloud Hypervisor).
    pub const PVH: Self = Self(3);
    /// Árbol de dispositivos (ARM64 / RISC-V sin UEFI).
    pub const DEVICE_TREE: Self = Self(4);

    pub const fn name(self) -> &'static str {
        match self.0 {
            1 => "KopitBoot (UEFI)",
            2 => "Multiboot2",
            3 => "PVH (arranque directo)",
            4 => "Device Tree",
            _ => "desconocido",
        }
    }
}

/// Tipo de una región de memoria física.
#[repr(transparent)]
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct MemoryKind(pub u32);

impl MemoryKind {
    /// RAM libre para el asignador de marcos.
    pub const USABLE: Self = Self(1);
    /// Reservada por el firmware o el hardware: no tocar nunca.
    pub const RESERVED: Self = Self(2);
    /// Tablas ACPI: se pueden liberar después de leerlas.
    pub const ACPI_RECLAIMABLE: Self = Self(3);
    /// Memoria ACPI NVS: el firmware la necesita incluso en suspensión.
    pub const ACPI_NVS: Self = Self(4);
    /// RAM defectuosa detectada por el firmware.
    pub const BAD: Self = Self(5);
    /// Datos del cargador (este `BootInfo`, tablas de páginas iniciales,
    /// servicios de arranque UEFI...). Se recupera cuando el núcleo ya no
    /// los necesita.
    pub const BOOTLOADER_RECLAIMABLE: Self = Self(6);
    /// La imagen del núcleo cargada en memoria.
    pub const KERNEL: Self = Self(7);
    /// El framebuffer de vídeo.
    pub const FRAMEBUFFER: Self = Self(8);
    /// Módulos cargados junto al núcleo (initrd, drivers, temas...).
    pub const MODULE: Self = Self(9);
    /// Código/datos de servicios de ejecución del firmware (UEFI runtime).
    pub const FIRMWARE_RUNTIME: Self = Self(10);
    /// E/S mapeada en memoria descrita por el firmware.
    pub const MMIO: Self = Self(11);
    /// Memoria persistente (NVDIMM).
    pub const PERSISTENT: Self = Self(12);

    pub const fn name(self) -> &'static str {
        match self.0 {
            1 => "libre",
            2 => "reservada",
            3 => "ACPI (recuperable)",
            4 => "ACPI NVS",
            5 => "defectuosa",
            6 => "cargador (recuperable)",
            7 => "núcleo",
            8 => "framebuffer",
            9 => "módulo",
            10 => "firmware (runtime)",
            11 => "MMIO",
            12 => "persistente",
            _ => "desconocida",
        }
    }

    /// ¿Es RAM "de verdad" (cuenta para el total de memoria instalada)?
    pub const fn is_ram(self) -> bool {
        matches!(self.0, 1 | 3 | 4 | 6 | 7 | 9 | 10 | 12)
    }

    /// Prioridad al resolver solapamientos: gana el tipo más restrictivo.
    const fn priority(self) -> u32 {
        match self.0 {
            1 => 0,         // libre: siempre pierde
            6 => 1,         // cargador recuperable
            3 => 2,         // ACPI recuperable
            9 | 7 | 8 => 3, // módulos, núcleo, framebuffer
            _ => 4,         // todo lo demás es intocable
        }
    }
}

/// Estado de Secure Boot que leyó el cargador del firmware.
#[repr(transparent)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SecureBootState(pub u32);

impl SecureBootState {
    pub const UNKNOWN: Self = Self(0);
    pub const DISABLED: Self = Self(1);
    pub const ENABLED: Self = Self(2);
    /// El firmware está en "modo configuración" (sin clave de plataforma).
    pub const SETUP_MODE: Self = Self(3);

    pub const fn name(self) -> &'static str {
        match self.0 {
            1 => "desactivado",
            2 => "activo",
            3 => "modo configuración",
            _ => "no disponible en este firmware",
        }
    }
}

/// Resultado de la verificación de integridad del núcleo por el cargador.
#[repr(transparent)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct KernelVerification(pub u32);

impl KernelVerification {
    /// El cargador no tenía huella con la que comparar.
    pub const NOT_CHECKED: Self = Self(0);
    /// SHA-256 del núcleo == huella integrada en el cargador firmado.
    pub const VERIFIED: Self = Self(1);
    /// No coincide (sólo se arranca así si Secure Boot está desactivado).
    pub const MISMATCH: Self = Self(2);

    pub const fn name(self) -> &'static str {
        match self.0 {
            1 => "verificado (SHA-256)",
            2 => "¡NO coincide!",
            _ => "sin verificar",
        }
    }
}

// -----------------------------------------------------------------------------
// Estructuras del protocolo
// -----------------------------------------------------------------------------

/// Una región contigua de memoria física.
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct MemoryRegion {
    pub base: u64,
    pub length: u64,
    pub kind: MemoryKind,
    /// Atributos (reservado; 0 por ahora).
    pub attributes: u32,
}

impl MemoryRegion {
    pub const EMPTY: Self = Self { base: 0, length: 0, kind: MemoryKind::RESERVED, attributes: 0 };

    pub const fn new(base: u64, length: u64, kind: MemoryKind) -> Self {
        Self { base, length, kind, attributes: 0 }
    }

    pub const fn end(&self) -> u64 {
        self.base.saturating_add(self.length)
    }
}

/// Cadena UTF-8 (no terminada en 0) pasada por el cargador.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct BootStr {
    pub ptr: u64,
    pub len: u64,
}

impl BootStr {
    pub const EMPTY: Self = Self { ptr: 0, len: 0 };

    /// # Safety
    /// `ptr..ptr+len` debe ser memoria legible y UTF-8 válido durante `'a`.
    pub unsafe fn as_str<'a>(&self) -> &'a str {
        if self.ptr == 0 || self.len == 0 {
            return "";
        }
        // SAFETY: lo garantiza quien llama.
        let bytes = unsafe { core::slice::from_raw_parts(self.ptr as *const u8, self.len as usize) };
        core::str::from_utf8(bytes).unwrap_or("")
    }
}

/// Lista de regiones de memoria.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct MemoryMap {
    pub entries: u64,
    pub count: u64,
}

impl MemoryMap {
    /// # Safety
    /// `entries` debe apuntar a `count` regiones válidas durante `'a`.
    pub unsafe fn regions<'a>(&self) -> &'a [MemoryRegion] {
        if self.entries == 0 {
            return &[];
        }
        // SAFETY: lo garantiza quien llama.
        unsafe { core::slice::from_raw_parts(self.entries as *const MemoryRegion, self.count as usize) }
    }
}

/// Tipo de framebuffer.
#[repr(transparent)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct FramebufferKind(pub u16);

impl FramebufferKind {
    pub const NONE: Self = Self(0);
    /// Píxeles empaquetados con posiciones de color explícitas.
    pub const RGB: Self = Self(1);
    /// Modo texto EGA/VGA (80x25 celdas de 2 bytes). `width`/`height` en celdas.
    pub const EGA_TEXT: Self = Self(2);
}

/// Framebuffer gráfico entregado por el firmware (GOP en UEFI, VBE en GRUB).
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct Framebuffer {
    /// Dirección **física** del primer píxel.
    pub base: u64,
    /// Tamaño en bytes.
    pub size: u64,
    pub width: u32,
    pub height: u32,
    /// Bytes por línea (puede ser mayor que `width * bytes_por_píxel`).
    pub stride: u32,
    pub bpp: u16,
    pub kind: FramebufferKind,
    pub red_shift: u8,
    pub red_bits: u8,
    pub green_shift: u8,
    pub green_bits: u8,
    pub blue_shift: u8,
    pub blue_bits: u8,
    pub _pad: [u8; 2],
}

impl Framebuffer {
    pub const NONE: Self = Self {
        base: 0,
        size: 0,
        width: 0,
        height: 0,
        stride: 0,
        bpp: 0,
        kind: FramebufferKind::NONE,
        red_shift: 0,
        red_bits: 0,
        green_shift: 0,
        green_bits: 0,
        blue_shift: 0,
        blue_bits: 0,
        _pad: [0; 2],
    };

    /// Formato típico de GOP: byte 0 = azul (`0x00RRGGBB` en un `u32`).
    pub const fn bgrx(base: u64, width: u32, height: u32, stride: u32) -> Self {
        Self {
            base,
            size: stride as u64 * height as u64,
            width,
            height,
            stride,
            bpp: 32,
            kind: FramebufferKind::RGB,
            red_shift: 16,
            red_bits: 8,
            green_shift: 8,
            green_bits: 8,
            blue_shift: 0,
            blue_bits: 8,
            _pad: [0; 2],
        }
    }

    pub const fn is_present(&self) -> bool {
        self.base != 0 && self.kind.0 != 0
    }
}

/// Un módulo cargado junto al núcleo (initrd, tema, controlador...).
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct Module {
    /// Dirección física.
    pub base: u64,
    pub size: u64,
    pub name: BootStr,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct ModuleList {
    pub entries: u64,
    pub count: u64,
}

impl ModuleList {
    pub const EMPTY: Self = Self { entries: 0, count: 0 };

    /// # Safety
    /// `entries` debe apuntar a `count` módulos válidos durante `'a`.
    pub unsafe fn modules<'a>(&self) -> &'a [Module] {
        if self.entries == 0 {
            return &[];
        }
        // SAFETY: lo garantiza quien llama.
        unsafe { core::slice::from_raw_parts(self.entries as *const Module, self.count as usize) }
    }
}

/// Fecha y hora de arranque según el firmware (UTC si se sabe).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct WallClock {
    pub year: u16,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
    /// 1 si la fecha es válida.
    pub valid: u8,
}

/// Información de la imagen del núcleo en memoria.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct KernelImage {
    /// Dirección física donde empieza la imagen.
    pub phys_base: u64,
    /// Dirección virtual a la que está enlazada.
    pub virt_base: u64,
    /// Bytes que ocupa en memoria (incluye `.bss`).
    pub size: u64,
}

/// La información completa que recibe `kernel_main`.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct BootInfo {
    /// Siempre [`KBP_MAGIC`].
    pub magic: u64,
    /// [`KBP_VERSION`] con el que se rellenó.
    pub version: u32,
    /// `size_of::<BootInfo>()` según el cargador.
    pub size: u32,
    pub source: BootSource,
    /// Reservado para banderas futuras.
    pub flags: u32,
    /// Base virtual del mapa directo de la memoria física.
    pub hhdm_base: u64,
    /// El HHDM es válido para direcciones físicas `[0, hhdm_limit)`.
    pub hhdm_limit: u64,
    pub kernel: KernelImage,
    pub memory_map: MemoryMap,
    pub framebuffer: Framebuffer,
    /// Dirección física del RSDP de ACPI (0 si no hay).
    pub rsdp: u64,
    /// Dirección física de la tabla SMBIOS (0 si no hay).
    pub smbios: u64,
    /// Dirección física del árbol de dispositivos (0 si no hay).
    pub dtb: u64,
    /// Dirección física de la tabla de sistema UEFI (0 si no hay).
    pub efi_system_table: u64,
    pub cmdline: BootStr,
    pub loader_name: BootStr,
    pub modules: ModuleList,
    pub boot_time: WallClock,
    /// Contador de ciclos (TSC) cuando arrancó el cargador: mide el arranque.
    pub loader_timestamp: u64,
    pub secure_boot: SecureBootState,
    pub kernel_verification: KernelVerification,
    pub reserved: [u64; 8],
}

impl BootInfo {
    pub const fn empty() -> Self {
        Self {
            magic: KBP_MAGIC,
            version: KBP_VERSION,
            size: core::mem::size_of::<Self>() as u32,
            source: BootSource::UNKNOWN,
            flags: 0,
            hhdm_base: 0,
            hhdm_limit: 0,
            kernel: KernelImage { phys_base: 0, virt_base: 0, size: 0 },
            memory_map: MemoryMap { entries: 0, count: 0 },
            framebuffer: Framebuffer::NONE,
            rsdp: 0,
            smbios: 0,
            dtb: 0,
            efi_system_table: 0,
            cmdline: BootStr::EMPTY,
            loader_name: BootStr::EMPTY,
            modules: ModuleList::EMPTY,
            boot_time: WallClock { year: 0, month: 0, day: 0, hour: 0, minute: 0, second: 0, valid: 0 },
            loader_timestamp: 0,
            secure_boot: SecureBootState::UNKNOWN,
            kernel_verification: KernelVerification::NOT_CHECKED,
            reserved: [0; 8],
        }
    }

    /// Comprueba firma y versión antes de fiarse de nada más.
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.magic != KBP_MAGIC {
            return Err("firma KBP incorrecta");
        }
        if self.version == 0 || self.version > KBP_VERSION {
            return Err("versión de KBP no soportada");
        }
        if (self.size as usize) < core::mem::size_of::<Self>() {
            return Err("BootInfo truncado");
        }
        Ok(())
    }
}

// -----------------------------------------------------------------------------
// Constructor de mapas de memoria (lo usan todos los cargadores)
// -----------------------------------------------------------------------------

/// Error al construir el mapa: no caben más regiones.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MapFull;

/// Construye un mapa de memoria **limpio** a partir de fuentes sucias
/// (firmware, Multiboot2, e820...) sin usar memoria dinámica.
///
/// Garantías de [`MemoryMapBuilder::finish`]:
/// 1. ordenado por dirección y sin solapamientos;
/// 2. ante un solapamiento gana el tipo más restrictivo (la RAM libre nunca
///    "pisa" algo reservado, el núcleo o un módulo);
/// 3. las regiones libres están alineadas a página (las migas se descartan);
/// 4. regiones contiguas del mismo tipo se fusionan.
pub struct MemoryMapBuilder<'a> {
    entries: &'a mut [MemoryRegion],
    len: usize,
}

impl<'a> MemoryMapBuilder<'a> {
    pub fn new(storage: &'a mut [MemoryRegion]) -> Self {
        Self { entries: storage, len: 0 }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Añade una región tal cual viene de la fuente.
    pub fn add(&mut self, base: u64, length: u64, kind: MemoryKind) -> Result<(), MapFull> {
        if length == 0 {
            return Ok(());
        }
        if self.len == self.entries.len() {
            return Err(MapFull);
        }
        self.entries[self.len] = MemoryRegion::new(base, length, kind);
        self.len += 1;
        Ok(())
    }

    /// Normaliza el mapa y devuelve cuántas regiones quedaron.
    pub fn finish(mut self) -> Result<usize, MapFull> {
        // 1) Cada región "fuerte" se resta de las regiones más débiles que pisa.
        //    Recorremos con índices porque restar puede partir una región en dos
        //    y añadir la mitad derecha al final (que también se revisa después).
        let mut strong = 0;
        while strong < self.len {
            let s = self.entries[strong];
            if s.length != 0 {
                let mut weak = 0;
                while weak < self.len {
                    let w = self.entries[weak];
                    if weak != strong
                        && w.length != 0
                        && w.kind.priority() < s.kind.priority()
                        && w.base < s.end()
                        && s.base < w.end()
                    {
                        self.subtract(weak, s.base, s.end())?;
                    }
                    weak += 1;
                }
            }
            strong += 1;
        }

        // 2) La RAM libre se recorta a páginas completas.
        for e in self.entries[..self.len].iter_mut() {
            if e.kind == MemoryKind::USABLE && e.length != 0 {
                let start = align_up(e.base, PAGE_SIZE);
                let end = align_down(e.end(), PAGE_SIZE);
                if end > start {
                    e.base = start;
                    e.length = end - start;
                } else {
                    e.length = 0;
                }
            }
        }

        // 3) Compactar (quitar huecos de longitud 0) y ordenar por dirección.
        let mut n = 0;
        for i in 0..self.len {
            if self.entries[i].length != 0 {
                self.entries[n] = self.entries[i];
                n += 1;
            }
        }
        self.len = n;
        // Inserción: los mapas son pequeños (decenas de entradas) y no hay `alloc`.
        for i in 1..n {
            let mut j = i;
            while j > 0 && self.entries[j - 1].base > self.entries[j].base {
                self.entries.swap(j - 1, j);
                j -= 1;
            }
        }

        // 4) Fusionar vecinas del mismo tipo (contiguas o solapadas).
        let mut out = 0;
        for i in 0..n {
            let cur = self.entries[i];
            if out > 0 {
                let last = &mut self.entries[out - 1];
                if last.kind == cur.kind && cur.base <= last.end() {
                    let end = last.end().max(cur.end());
                    last.length = end - last.base;
                    continue;
                }
            }
            self.entries[out] = cur;
            out += 1;
        }
        Ok(out)
    }

    /// Quita `[cut_start, cut_end)` de la región `idx`.
    fn subtract(&mut self, idx: usize, cut_start: u64, cut_end: u64) -> Result<(), MapFull> {
        let r = self.entries[idx];
        let (start, end) = (r.base, r.end());
        let left = cut_start > start;
        let right = cut_end < end;
        match (left, right) {
            (false, false) => self.entries[idx].length = 0,
            (true, false) => self.entries[idx].length = cut_start - start,
            (false, true) => {
                self.entries[idx].base = cut_end;
                self.entries[idx].length = end - cut_end;
            }
            (true, true) => {
                self.entries[idx].length = cut_start - start;
                self.add(cut_end, end - cut_end, r.kind)?;
            }
        }
        Ok(())
    }
}

pub const fn align_up(v: u64, align: u64) -> u64 {
    (v + align - 1) & !(align - 1)
}

pub const fn align_down(v: u64, align: u64) -> u64 {
    v & !(align - 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build(input: &[(u64, u64, MemoryKind)]) -> Vec<MemoryRegion> {
        let mut storage = [MemoryRegion::EMPTY; 64];
        let mut b = MemoryMapBuilder::new(&mut storage);
        for &(base, len, kind) in input {
            b.add(base, len, kind).unwrap();
        }
        let n = b.finish().unwrap();
        storage[..n].to_vec()
    }

    #[test]
    fn el_nucleo_parte_la_ram_libre() {
        let map =
            build(&[(0x0010_0000, 0x0FF0_0000, MemoryKind::USABLE), (0x0020_0000, 0x0010_0000, MemoryKind::KERNEL)]);
        assert_eq!(
            map,
            vec![
                MemoryRegion::new(0x0010_0000, 0x0010_0000, MemoryKind::USABLE),
                MemoryRegion::new(0x0020_0000, 0x0010_0000, MemoryKind::KERNEL),
                MemoryRegion::new(0x0030_0000, 0x0FD0_0000, MemoryKind::USABLE),
            ]
        );
    }

    #[test]
    fn lo_reservado_gana_y_se_alinea_a_pagina() {
        let map = build(&[(0x0, 0x9FC00, MemoryKind::USABLE), (0x9F000, 0x1000, MemoryKind::RESERVED)]);
        // 0x9F000..0xA0000 reservada; la RAM libre queda en 0..0x9F000.
        assert_eq!(map[0], MemoryRegion::new(0x0, 0x9F000, MemoryKind::USABLE));
        assert_eq!(map[1], MemoryRegion::new(0x9F000, 0x1000, MemoryKind::RESERVED));
    }

    #[test]
    fn fusiona_vecinas_y_ordena() {
        let map = build(&[
            (0x3000, 0x1000, MemoryKind::USABLE),
            (0x1000, 0x1000, MemoryKind::USABLE),
            (0x2000, 0x1000, MemoryKind::USABLE),
        ]);
        assert_eq!(map, vec![MemoryRegion::new(0x1000, 0x3000, MemoryKind::USABLE)]);
    }

    #[test]
    fn migas_de_menos_de_una_pagina_desaparecen() {
        let map = build(&[(0x1001, 0xFFE, MemoryKind::USABLE)]);
        assert!(map.is_empty());
    }

    #[test]
    fn firma_y_version() {
        let mut info = BootInfo::empty();
        assert!(info.validate().is_ok());
        info.magic = 0;
        assert!(info.validate().is_err());
    }
}
