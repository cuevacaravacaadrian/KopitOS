//! # Gestión de memoria
//!
//! Tres capas, de abajo arriba:
//!
//! 1. [`pmm`] — **marcos físicos**: asignador buddy por zonas (DMA, DMA32,
//!    Normal) sobre el mapa de memoria del cargador.
//! 2. [`paging`] — **espacio de direcciones del núcleo**: el núcleo construye
//!    sus propias tablas (no hereda las del cargador) con **W^X** real:
//!    `.text` ejecutable y de sólo lectura, `.rodata` sólo lectura, datos sin
//!    ejecución, página de guarda bajo la pila y un **mapa directo** (HHDM) de
//!    toda la RAM con páginas enormes. [`mmio`] añade ventanas sin caché o
//!    write-combining para dispositivos.
//! 3. [`heap`] — **montículo**: slabs para objetos pequeños y bloques buddy
//!    para los grandes, todo a través del mapa directo (como `kmalloc`).
//!
//! Disposición virtual en x86_64:
//!
//! ```text
//! 0x0000_0000_0000_0000 ┐ espacio de usuario (fase 3)
//! 0x0000_7FFF_FFFF_FFFF ┘
//! 0xFFFF_8000_0000_0000   HHDM: física p ↔ virtual 0xFFFF_8000_0000_0000 + p
//! 0xFFFF_C000_0000_0000   ventanas MMIO (framebuffer WC, APIC sin caché...)
//! 0xFFFF_FFFF_8000_0000   imagen del núcleo (-2 GiB, modelo de código "kernel")
//! ```

pub mod heap;
pub mod mmio;
pub mod paging;
pub mod pmm;

use core::ops::BitOr;
use core::sync::atomic::{AtomicU64, Ordering};
use kopit_boot_proto::BootInfo;

pub const PAGE_SIZE: u64 = 4096;

static HHDM_BASE: AtomicU64 = AtomicU64::new(0);
static HHDM_LIMIT: AtomicU64 = AtomicU64::new(0);
static KERNEL_PHYS: AtomicU64 = AtomicU64::new(0);
static KERNEL_VIRT: AtomicU64 = AtomicU64::new(0);
static KERNEL_SIZE: AtomicU64 = AtomicU64::new(0);

/// Datos mínimos para traducir direcciones desde la primera instrucción.
pub fn init_early(boot: &BootInfo) {
    HHDM_BASE.store(boot.hhdm_base, Ordering::Relaxed);
    HHDM_LIMIT.store(boot.hhdm_limit, Ordering::Relaxed);
    KERNEL_PHYS.store(boot.kernel.phys_base, Ordering::Relaxed);
    KERNEL_VIRT.store(boot.kernel.virt_base, Ordering::Relaxed);
    KERNEL_SIZE.store(boot.kernel.size, Ordering::Relaxed);
}

#[inline]
pub fn hhdm_base() -> u64 {
    HHDM_BASE.load(Ordering::Relaxed)
}

/// Hasta dónde llega el mapa directo actual.
pub fn hhdm_limit() -> u64 {
    HHDM_LIMIT.load(Ordering::Relaxed)
}

pub fn set_hhdm_limit(limit: u64) {
    HHDM_LIMIT.store(limit, Ordering::Relaxed);
}

/// Dirección virtual (en el HHDM) de una dirección física.
#[inline]
pub fn phys_to_virt(phys: u64) -> u64 {
    hhdm_base() + phys
}

/// Dirección física de una dirección virtual del núcleo (imagen o HHDM).
pub fn virt_to_phys(virt: u64) -> u64 {
    let kv = KERNEL_VIRT.load(Ordering::Relaxed);
    if virt >= kv && virt < kv + KERNEL_SIZE.load(Ordering::Relaxed) {
        return virt - kv + KERNEL_PHYS.load(Ordering::Relaxed);
    }
    let h = hhdm_base();
    if virt >= h { virt - h } else { virt }
}

pub fn kernel_phys_base() -> u64 {
    KERNEL_PHYS.load(Ordering::Relaxed)
}

/// Permisos de un mapeo, independientes de la arquitectura.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct MapFlags(pub u32);

impl MapFlags {
    /// Sólo lectura, sin ejecución (lo mínimo).
    pub const READ: Self = Self(0);
    pub const WRITE: Self = Self(1 << 0);
    pub const EXEC: Self = Self(1 << 1);
    pub const USER: Self = Self(1 << 2);
    /// Entrada global: sobrevive a cambios de CR3 (memoria del núcleo).
    pub const GLOBAL: Self = Self(1 << 3);
    pub const NO_CACHE: Self = Self(1 << 4);
    pub const WRITE_COMBINE: Self = Self(1 << 5);

    pub const fn union(self, o: Self) -> Self {
        Self(self.0 | o.0)
    }

    pub const fn has(self, o: Self) -> bool {
        self.0 & o.0 == o.0
    }
}

impl BitOr for MapFlags {
    type Output = Self;
    fn bitor(self, o: Self) -> Self {
        self.union(o)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PageSize {
    /// 4 KiB
    Small,
    /// 2 MiB
    Large,
    /// 1 GiB
    Huge,
}

impl PageSize {
    pub const fn bytes(self) -> u64 {
        match self {
            PageSize::Small => 4 << 10,
            PageSize::Large => 2 << 20,
            PageSize::Huge => 1 << 30,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MapError {
    OutOfMemory,
    AlreadyMapped,
    Misaligned,
    Unsupported,
}

impl MapError {
    pub const fn message(self) -> &'static str {
        match self {
            MapError::OutOfMemory => "sin memoria para tablas de páginas",
            MapError::AlreadyMapped => "la dirección ya estaba mapeada",
            MapError::Misaligned => "dirección mal alineada",
            MapError::Unsupported => "no soportado en esta arquitectura",
        }
    }
}

pub const fn align_up(v: u64, a: u64) -> u64 {
    (v + a - 1) & !(a - 1)
}

pub const fn align_down(v: u64, a: u64) -> u64 {
    v & !(a - 1)
}

/// Símbolos que define el guion del enlazador (`kernel/linker/<arch>.ld`).
pub mod layout {
    unsafe extern "C" {
        safe static __kernel_start: u8;
        safe static __kernel_end: u8;
        safe static __text_start: u8;
        safe static __text_end: u8;
        safe static __rodata_start: u8;
        safe static __rodata_end: u8;
        safe static __data_start: u8;
        safe static __bss_start: u8;
        safe static __bss_end: u8;
        safe static __boot_stack_guard: u8;
        safe static __boot_stack_bottom: u8;
        safe static __boot_stack_top: u8;
    }

    macro_rules! addr {
        ($($f:ident => $s:ident),* $(,)?) => {
            $(#[inline] pub fn $f() -> u64 { &raw const $s as u64 })*
        };
    }

    addr! {
        kernel_start => __kernel_start,
        kernel_end => __kernel_end,
        text_start => __text_start,
        text_end => __text_end,
        rodata_start => __rodata_start,
        rodata_end => __rodata_end,
        data_start => __data_start,
        bss_start => __bss_start,
        bss_end => __bss_end,
        stack_guard => __boot_stack_guard,
        stack_bottom => __boot_stack_bottom,
        stack_top => __boot_stack_top,
    }
}
