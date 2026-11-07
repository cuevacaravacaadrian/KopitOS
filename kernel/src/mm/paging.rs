//! # Espacio de direcciones del núcleo
//!
//! El núcleo **no se fía** de las tablas que dejó el cargador: construye las
//! suyas y cambia a ellas. Ventajas:
//!
//! * permisos precisos por sección (W^X): un error que intente escribir en el
//!   código o ejecutar datos provoca un fallo de página en vez de corromper;
//! * desaparece el mapeo identidad de la memoria baja (un puntero nulo ya no
//!   apunta a memoria válida);
//! * página de guarda bajo la pila de arranque: un desbordamiento es un doble
//!   fallo con diagnóstico, no una corrupción silenciosa;
//! * el mapa directo cubre **toda** la RAM (el cargador puede haber mapeado
//!   sólo una parte), con las páginas más grandes posibles;
//! * el framebuffer pasa a una ventana *write-combining*.

use super::{MapError, MapFlags, PAGE_SIZE, align_up, hhdm_base, layout, mmio, pmm, set_hhdm_limit};
use crate::arch::paging::AddressSpace;
use crate::sync::IrqLock;
use kopit_boot_proto::BootInfo;

static KERNEL_SPACE: IrqLock<Option<AddressSpace>> = IrqLock::new(None);

#[derive(Clone, Copy, Debug)]
pub struct PagingReport {
    pub direct_map_bytes: u64,
    pub text_bytes: u64,
    pub rodata_bytes: u64,
    pub data_bytes: u64,
    pub framebuffer_virt: Option<u64>,
    pub hot_added_bytes: u64,
}

/// Construye y activa el espacio de direcciones del núcleo.
pub fn init(boot: &BootInfo) -> Result<PagingReport, MapError> {
    if !crate::arch::paging::ENABLED {
        return Err(MapError::Unsupported);
    }
    let mut space = AddressSpace::new().ok_or(MapError::OutOfMemory)?;
    let (kvirt, kphys) = (boot.kernel.virt_base, boot.kernel.phys_base);
    let phys_of = |v: u64| v - kvirt + kphys;

    let map_section = |space: &mut AddressSpace, start: u64, end: u64, flags: MapFlags| -> Result<u64, MapError> {
        let (start, end) = (start & !(PAGE_SIZE - 1), align_up(end, PAGE_SIZE));
        let mut v = start;
        while v < end {
            space.map_4k(v, phys_of(v), flags)?;
            v += PAGE_SIZE;
        }
        Ok(end - start)
    };

    // 1) La imagen del núcleo, sección por sección.
    let text = map_section(&mut space, layout::text_start(), layout::text_end(), MapFlags::EXEC | MapFlags::GLOBAL)?;
    let rodata = map_section(&mut space, layout::rodata_start(), layout::rodata_end(), MapFlags::GLOBAL)?;
    let guard = layout::stack_guard();
    let rw = MapFlags::WRITE | MapFlags::GLOBAL;
    let mut data = map_section(&mut space, layout::data_start(), guard, rw)?;
    data += map_section(&mut space, guard + PAGE_SIZE, layout::kernel_end(), rw)?;

    // 2) Mapa directo de toda la RAM (y al menos los primeros 4 GiB, donde
    //    viven las tablas ACPI y la memoria VGA heredada).
    let max = align_up(crate::boot::max_ram_address().max(4 << 30), 1 << 30);
    space.map_range(hhdm_base(), 0, max, MapFlags::WRITE | MapFlags::GLOBAL)?;

    // 3) Framebuffer en una ventana write-combining.
    let fb = boot.framebuffer;
    let fb_virt = if fb.is_present() && fb.kind == kopit_boot_proto::FramebufferKind::RGB {
        Some(mmio::map_into(&mut space, fb.base, fb.size, mmio::Cache::WriteCombining)?)
    } else {
        None
    };

    // 4) ¡Cambio de tablas! A partir de aquí sólo existe lo que hemos mapeado.
    // SAFETY: el nuevo espacio mapea el código, la pila y los datos en uso.
    unsafe { space.activate() };
    set_hhdm_limit(max);
    *KERNEL_SPACE.lock() = Some(space);
    if let Some(v) = fb_virt {
        crate::console::relocate_framebuffer(v);
    }
    let hot_added = pmm::extend_direct_map();
    Ok(PagingReport {
        direct_map_bytes: max,
        text_bytes: text,
        rodata_bytes: rodata,
        data_bytes: data,
        framebuffer_virt: fb_virt,
        hot_added_bytes: hot_added,
    })
}

/// Ejecuta `f` sobre el espacio del núcleo (si ya existe).
pub fn with_kernel_space<R>(f: impl FnOnce(&mut AddressSpace) -> R) -> Option<R> {
    KERNEL_SPACE.lock().as_mut().map(f)
}

/// Traducción virtual → (física, permisos) en el espacio activo.
pub fn translate(virt: u64) -> Option<(u64, MapFlags)> {
    with_kernel_space(|s| s.translate(virt)).flatten()
}

pub fn is_active() -> bool {
    KERNEL_SPACE.lock().is_some()
}
