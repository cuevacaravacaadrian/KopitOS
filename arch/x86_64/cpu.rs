//! Identificación de la CPU y registros de control.
//!
//! Además de lo clásico (fabricante, modelo, NX, páginas de 1 GiB, x2APIC...),
//! se detectan las **CPUs híbridas** (Intel Alder Lake y posteriores): la hoja
//! CPUID 0x1A dice si el núcleo actual es de alto rendimiento (P) o de alta
//! eficiencia (E). Es la base del planificador heterogéneo de la fase 2.

use crate::log::FixedBuf;
use crate::sync::Once;
use core::arch::asm;
use core::fmt::Write;

pub fn cpuid(leaf: u32, sub: u32) -> (u32, u32, u32, u32) {
    let (eax, ebx, ecx, edx): (u32, u64, u32, u32);
    // SAFETY: cpuid sólo lee; rbx lo reserva LLVM y se salva a mano.
    unsafe {
        asm!(
            "mov {tmp}, rbx",
            "cpuid",
            "xchg {tmp}, rbx",
            tmp = out(reg) ebx,
            inout("eax") leaf => eax,
            inout("ecx") sub => ecx,
            out("edx") edx,
            options(nostack, preserves_flags),
        )
    };
    (eax, ebx as u32, ecx, edx)
}

#[inline]
pub fn rdtsc() -> u64 {
    let (lo, hi): (u32, u32);
    // SAFETY: lectura del contador de ciclos.
    unsafe { asm!("rdtsc", out("eax") lo, out("edx") hi, options(nomem, nostack, preserves_flags)) };
    ((hi as u64) << 32) | lo as u64
}

pub fn rdmsr(msr: u32) -> u64 {
    let (lo, hi): (u32, u32);
    // SAFETY: los llamantes usan MSR que existen (comprobado por CPUID).
    unsafe { asm!("rdmsr", in("ecx") msr, out("eax") lo, out("edx") hi, options(nomem, nostack, preserves_flags)) };
    ((hi as u64) << 32) | lo as u64
}

pub fn wrmsr(msr: u32, v: u64) {
    // SAFETY: ídem.
    unsafe {
        asm!("wrmsr", in("ecx") msr, in("eax") v as u32, in("edx") (v >> 32) as u32, options(nostack, preserves_flags))
    };
}

macro_rules! control_register {
    ($read:ident, $write:ident, $reg:literal) => {
        pub fn $read() -> u64 {
            let v: u64;
            // SAFETY: lectura de un registro de control en ring 0.
            unsafe { asm!(concat!("mov {}, ", $reg), out(reg) v, options(nomem, nostack, preserves_flags)) };
            v
        }
        /// # Safety
        /// Cambiar un registro de control puede romper la ejecución.
        pub unsafe fn $write(v: u64) {
            // SAFETY: lo garantiza quien llama.
            unsafe { asm!(concat!("mov ", $reg, ", {}"), in(reg) v, options(nostack, preserves_flags)) };
        }
    };
}

control_register!(read_cr0, write_cr0, "cr0");
control_register!(read_cr3, write_cr3, "cr3");
control_register!(read_cr4, write_cr4, "cr4");

pub fn read_cr2() -> u64 {
    let v: u64;
    // SAFETY: lectura de CR2 (dirección del último fallo de página).
    unsafe { asm!("mov {}, cr2", out(reg) v, options(nomem, nostack, preserves_flags)) };
    v
}

pub fn invlpg(addr: u64) {
    // SAFETY: invalida una entrada del TLB.
    unsafe { asm!("invlpg [{}]", in(reg) addr, options(nostack, preserves_flags)) };
}

pub fn rflags() -> u64 {
    let r: u64;
    // SAFETY: lee RFLAGS por la pila.
    unsafe { asm!("pushfq", "pop {}", out(reg) r, options(nomem, preserves_flags)) };
    r
}

pub const IA32_APIC_BASE: u32 = 0x1B;
pub const IA32_EFER: u32 = 0xC000_0080;
pub const IA32_PAT: u32 = 0x277;

#[derive(Clone, Copy, Debug)]
pub struct Features {
    pub vendor: [u8; 12],
    pub brand: [u8; 48],
    pub family: u32,
    pub model: u32,
    pub stepping: u32,
    pub max_leaf: u32,
    pub apic: bool,
    pub x2apic: bool,
    pub tsc_deadline: bool,
    pub invariant_tsc: bool,
    pub nx: bool,
    pub gib_pages: bool,
    pub pat: bool,
    pub pge: bool,
    pub hypervisor: bool,
    pub hybrid: bool,
    /// 0x20 = núcleo E (Atom), 0x40 = núcleo P (Core).
    pub core_type: u8,
    pub initial_apic_id: u32,
    pub tsc_hz_cpuid: u64,
    pub sse2: bool,
    pub avx2: bool,
    pub avx512: bool,
}

static FEATURES: Once<Features> = Once::new();

pub fn detect() -> &'static Features {
    FEATURES.call_once(|| {
        let (max_leaf, b, c, d) = cpuid(0, 0);
        let mut vendor = [0u8; 12];
        vendor[0..4].copy_from_slice(&b.to_le_bytes());
        vendor[4..8].copy_from_slice(&d.to_le_bytes());
        vendor[8..12].copy_from_slice(&c.to_le_bytes());
        let (eax1, ebx1, ecx1, edx1) = cpuid(1, 0);
        let base_family = (eax1 >> 8) & 0xF;
        let family = if base_family == 0xF { base_family + ((eax1 >> 20) & 0xFF) } else { base_family };
        let model = if base_family == 0x6 || base_family == 0xF {
            ((eax1 >> 4) & 0xF) | ((eax1 >> 12) & 0xF0)
        } else {
            (eax1 >> 4) & 0xF
        };
        let max_ext = cpuid(0x8000_0000, 0).0;
        let ext = if max_ext >= 0x8000_0001 { cpuid(0x8000_0001, 0) } else { (0, 0, 0, 0) };
        let mut brand = [0u8; 48];
        if max_ext >= 0x8000_0004 {
            for (i, leaf) in (0x8000_0002u32..=0x8000_0004).enumerate() {
                let (a, b, c, d) = cpuid(leaf, 0);
                for (j, r) in [a, b, c, d].iter().enumerate() {
                    brand[i * 16 + j * 4..i * 16 + j * 4 + 4].copy_from_slice(&r.to_le_bytes());
                }
            }
        }
        let leaf7 = if max_leaf >= 7 { cpuid(7, 0) } else { (0, 0, 0, 0) };
        let hybrid = leaf7.3 & (1 << 15) != 0;
        let core_type = if hybrid && max_leaf >= 0x1A { (cpuid(0x1A, 0).0 >> 24) as u8 } else { 0 };
        let invariant_tsc = max_ext >= 0x8000_0007 && cpuid(0x8000_0007, 0).3 & (1 << 8) != 0;
        let tsc_hz_cpuid = if max_leaf >= 0x15 {
            let (den, num, hz, _) = cpuid(0x15, 0);
            if den != 0 && num != 0 && hz != 0 { hz as u64 * num as u64 / den as u64 } else { 0 }
        } else {
            0
        };
        Features {
            vendor,
            brand,
            family,
            model,
            stepping: eax1 & 0xF,
            max_leaf,
            apic: edx1 & (1 << 9) != 0,
            x2apic: ecx1 & (1 << 21) != 0,
            tsc_deadline: ecx1 & (1 << 24) != 0,
            invariant_tsc,
            nx: ext.3 & (1 << 20) != 0,
            gib_pages: ext.3 & (1 << 26) != 0,
            pat: edx1 & (1 << 16) != 0,
            pge: edx1 & (1 << 13) != 0,
            hypervisor: ecx1 & (1 << 31) != 0,
            hybrid,
            core_type,
            initial_apic_id: ebx1 >> 24,
            tsc_hz_cpuid,
            sse2: edx1 & (1 << 26) != 0,
            avx2: leaf7.1 & (1 << 5) != 0,
            avx512: leaf7.1 & (1 << 16) != 0,
        }
    })
}

pub fn features() -> &'static Features {
    detect()
}

impl Features {
    pub fn vendor_str(&self) -> &str {
        core::str::from_utf8(&self.vendor).unwrap_or("?")
    }
    pub fn brand_str(&self) -> &str {
        let end = self.brand.iter().position(|&b| b == 0).unwrap_or(48);
        core::str::from_utf8(&self.brand[..end]).unwrap_or("?").trim()
    }
}

/// Activa lo que el núcleo necesita: NX, protección de escritura en ring 0,
/// páginas globales y la tabla PAT con write-combining.
pub fn enable_features() {
    let f = features();
    if f.nx {
        wrmsr(IA32_EFER, rdmsr(IA32_EFER) | (1 << 11));
    }
    // SAFETY: CR0.WP sólo hace cumplir los permisos de sólo lectura en ring 0.
    unsafe { write_cr0(read_cr0() | (1 << 16)) };
    if f.pge {
        // SAFETY: activar páginas globales no invalida nada existente.
        unsafe { write_cr4(read_cr4() | (1 << 7)) };
    }
    if f.pat {
        // PAT: PA0=WB, PA1=WC (en vez de WT), PA2=UC-, PA3=UC (y se repite).
        // Con esto PWT=1 → write-combining y PCD|PWT → sin caché.
        wrmsr(IA32_PAT, 0x0007_0106_0007_0106);
    }
}

/// Texto descriptivo para la orden `cpu`.
pub fn describe(f: &mut dyn FnMut(&str, &str)) {
    let c = features();
    f("fabricante", c.vendor_str());
    f("modelo", c.brand_str());
    let mut b = FixedBuf::<96>::new();
    let _ = write!(b, "familia {:#x} · modelo {:#x} · revisión {}", c.family, c.model, c.stepping);
    f("identificación", b.as_str());
    b.clear();
    let flags = [
        ("apic", c.apic),
        ("x2apic", c.x2apic),
        ("nx", c.nx),
        ("pdpe1gb", c.gib_pages),
        ("pat", c.pat),
        ("pge", c.pge),
        ("tsc-invariante", c.invariant_tsc),
        ("tsc-deadline", c.tsc_deadline),
        ("sse2", c.sse2),
        ("avx2", c.avx2),
        ("avx512", c.avx512),
        ("hipervisor", c.hypervisor),
    ];
    for (name, on) in flags {
        if on {
            let _ = write!(b, "{} ", name);
        }
    }
    f("características", b.as_str());
    b.clear();
    if c.hybrid {
        let _ = write!(
            b,
            "sí — este núcleo es {}",
            match c.core_type {
                0x40 => "P (alto rendimiento)",
                0x20 => "E (alta eficiencia)",
                _ => "de tipo desconocido",
            }
        );
    } else {
        let _ = write!(b, "no (todos los núcleos iguales)");
    }
    f("CPU híbrida", b.as_str());
    b.clear();
    let _ = write!(b, "{} MHz (calibrado)", crate::time::cycle_frequency() / 1_000_000);
    f("reloj TSC", b.as_str());
    b.clear();
    let _ = write!(b, "{}", c.initial_apic_id);
    f("APIC ID", b.as_str());
}
