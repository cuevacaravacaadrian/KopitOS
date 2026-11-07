//! Envoltorios seguros (o casi) sobre los servicios UEFI.
//!
//! Todo el cargador habla con el firmware a través de este módulo. Guarda la
//! tabla de sistema en un `AtomicPtr` y sabe si los servicios de arranque
//! siguen vivos: después de `ExitBootServices` cualquier intento de usarlos
//! (incluida la memoria dinámica) se convierte en un error claro, no en un
//! cuelgue misterioso.

use crate::uefi::*;
use alloc::string::String;
use alloc::vec::Vec;
use core::alloc::{GlobalAlloc, Layout};
use core::ffi::c_void;
use core::fmt;
use core::ptr::{self, null_mut};
use core::sync::atomic::{AtomicBool, AtomicPtr, Ordering};
use kopit_boot_proto::SecureBootState;

static SYSTEM_TABLE: AtomicPtr<SystemTable> = AtomicPtr::new(null_mut());
static IMAGE: AtomicPtr<c_void> = AtomicPtr::new(null_mut());
static BOOT_SERVICES_ACTIVE: AtomicBool = AtomicBool::new(false);

pub fn init(image: Handle, st: *mut SystemTable) {
    IMAGE.store(image, Ordering::SeqCst);
    SYSTEM_TABLE.store(st, Ordering::SeqCst);
    BOOT_SERVICES_ACTIVE.store(true, Ordering::SeqCst);
}

pub fn image() -> Handle {
    IMAGE.load(Ordering::SeqCst)
}

pub fn system_table() -> &'static SystemTable {
    // SAFETY: el firmware nos dio un puntero válido para toda la vida del cargador.
    unsafe { &*SYSTEM_TABLE.load(Ordering::SeqCst) }
}

pub fn system_table_ptr() -> *mut SystemTable {
    SYSTEM_TABLE.load(Ordering::SeqCst)
}

pub fn boot_services_active() -> bool {
    BOOT_SERVICES_ACTIVE.load(Ordering::SeqCst)
}

pub fn boot_services() -> &'static BootServices {
    assert!(boot_services_active(), "servicios de arranque ya cerrados");
    // SAFETY: válidos hasta ExitBootServices (comprobado arriba).
    unsafe { &*system_table().boot_services }
}

pub fn runtime_services() -> &'static RuntimeServices {
    // SAFETY: los servicios de ejecución sobreviven a ExitBootServices.
    unsafe { &*system_table().runtime_services }
}

pub fn mark_boot_services_exited() {
    BOOT_SERVICES_ACTIVE.store(false, Ordering::SeqCst);
}

// --- Memoria -----------------------------------------------------------------

pub fn allocate_pages(kind: u32, pages: usize) -> Result<u64, Status> {
    let mut addr = 0u64;
    // SAFETY: llamada UEFI con argumentos válidos.
    unsafe { (boot_services().allocate_pages)(ALLOCATE_ANY_PAGES, kind, pages, &mut addr) }.ok()?;
    Ok(addr)
}

/// Páginas puestas a cero (UEFI no garantiza el contenido).
pub fn allocate_zeroed_pages(kind: u32, pages: usize) -> Result<u64, Status> {
    let addr = allocate_pages(kind, pages)?;
    // SAFETY: memoria recién asignada; UEFI la mapea 1:1 (identidad).
    unsafe { ptr::write_bytes(addr as *mut u8, 0, pages * 4096) };
    Ok(addr)
}

/// Asignador global sobre `AllocatePool`: permite usar `Vec`/`String` mientras
/// existan los servicios de arranque.
pub struct PoolAllocator;

unsafe impl GlobalAlloc for PoolAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if !boot_services_active() {
            return null_mut();
        }
        let bs = boot_services();
        let align = layout.align();
        let mut raw = null_mut();
        if align <= 8 {
            // SAFETY: llamada UEFI.
            let st = unsafe { (bs.allocate_pool)(memtype::LOADER_DATA, layout.size().max(1), &mut raw) };
            return if st.is_error() { null_mut() } else { raw };
        }
        // Alineación mayor: reservamos de más y guardamos el puntero original justo antes.
        let total = layout.size() + align + 8;
        // SAFETY: llamada UEFI.
        let st = unsafe { (bs.allocate_pool)(memtype::LOADER_DATA, total, &mut raw) };
        if st.is_error() {
            return null_mut();
        }
        let aligned = ((raw as usize + 8 + align - 1) & !(align - 1)) as *mut u8;
        // SAFETY: `aligned - 8` está dentro del bloque reservado.
        unsafe { (aligned as *mut *mut u8).sub(1).write(raw) };
        aligned
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if !boot_services_active() {
            return; // tras ExitBootServices la memoria del pool ya no es nuestra
        }
        let raw = if layout.align() <= 8 {
            ptr
        } else {
            // SAFETY: lo escribimos en `alloc`.
            unsafe { (ptr as *mut *mut u8).sub(1).read() }
        };
        // SAFETY: llamada UEFI con un puntero de AllocatePool.
        unsafe { (boot_services().free_pool)(raw) };
    }
}

// --- Consola de texto --------------------------------------------------------

fn con_out() -> *mut SimpleTextOutput {
    system_table().con_out
}

/// Escribe texto en la consola UEFI (convierte UTF-8 → UCS-2 y `\n` → `\r\n`).
pub fn output(s: &str) {
    if !boot_services_active() {
        crate::cpu::serial_write(s.as_bytes());
        return;
    }
    let out = con_out();
    let mut buf = [0u16; 128];
    let mut n = 0;
    let flush = |buf: &mut [u16; 128], n: &mut usize| {
        buf[*n] = 0;
        // SAFETY: cadena UCS-2 terminada en 0.
        unsafe { ((*out).output_string)(out, buf.as_ptr()) };
        *n = 0;
    };
    for c in s.chars() {
        if n >= 124 {
            flush(&mut buf, &mut n);
        }
        if c == '\n' {
            buf[n] = '\r' as u16;
            n += 1;
        }
        let code = c as u32;
        buf[n] = if code <= 0xFFFF { code as u16 } else { '?' as u16 };
        n += 1;
    }
    if n > 0 {
        flush(&mut buf, &mut n);
    }
}

pub struct Console;

impl fmt::Write for Console {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        output(s);
        Ok(())
    }
}

#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => {{
        use core::fmt::Write as _;
        let _ = write!($crate::firmware::Console, $($arg)*);
    }};
}

#[macro_export]
macro_rules! println {
    () => { $crate::print!("\n") };
    ($($arg:tt)*) => {{
        $crate::print!($($arg)*);
        $crate::print!("\n");
    }};
}

pub fn set_color(fg: usize, bg: usize) {
    if boot_services_active() {
        let out = con_out();
        // SAFETY: llamada UEFI.
        unsafe { ((*out).set_attribute)(out, color::attr(fg, bg)) };
    }
}

pub fn clear_screen() {
    let out = con_out();
    // SAFETY: llamada UEFI.
    unsafe { ((*out).clear_screen)(out) };
}

pub fn set_cursor(col: usize, row: usize) {
    let out = con_out();
    // SAFETY: llamada UEFI.
    unsafe { ((*out).set_cursor_position)(out, col, row) };
}

pub fn show_cursor(visible: bool) {
    let out = con_out();
    // SAFETY: llamada UEFI (puede no estar soportado: se ignora el error).
    unsafe { ((*out).enable_cursor)(out, visible as u8) };
}

/// Columnas y filas del modo de texto actual.
pub fn text_size() -> (usize, usize) {
    let out = con_out();
    let (mut cols, mut rows) = (80usize, 25usize);
    // SAFETY: llamada UEFI; `mode` es válido mientras exista la consola.
    unsafe {
        let mode = (*(*out).mode).mode;
        if mode >= 0 && ((*out).query_mode)(out, mode as usize, &mut cols, &mut rows).is_error() {
            return (80, 25);
        }
    }
    (cols, rows)
}

// --- Teclado y temporizadores --------------------------------------------------

pub fn read_key() -> Option<InputKey> {
    let input = system_table().con_in;
    let mut key = InputKey::default();
    // SAFETY: llamada UEFI.
    let st = unsafe { ((*input).read_key_stroke)(input, &mut key) };
    (!st.is_error()).then_some(key)
}

/// Vacía el búfer del teclado (teclas pulsadas antes de mostrar el menú).
pub fn flush_keys() {
    while read_key().is_some() {}
}

pub fn create_periodic_timer(period_ms: u64) -> Result<Event, Status> {
    let bs = boot_services();
    let mut ev: Event = null_mut();
    // SAFETY: llamadas UEFI; temporizador sin función de aviso.
    unsafe {
        (bs.create_event)(EVT_TIMER, TPL_CALLBACK, None, null_mut(), &mut ev).ok()?;
        (bs.set_timer)(ev, TIMER_PERIODIC, period_ms * 10_000).ok()?;
    }
    Ok(ev)
}

pub fn close_timer(ev: Event) {
    let bs = boot_services();
    // SAFETY: evento creado por nosotros.
    unsafe {
        (bs.set_timer)(ev, TIMER_CANCEL, 0);
        (bs.close_event)(ev);
    }
}

pub enum Wake {
    Key(InputKey),
    Tick,
}

/// Duerme hasta que llegue una tecla o un tic del temporizador.
pub fn wait_key_or(timer: Option<Event>) -> Wake {
    let bs = boot_services();
    // SAFETY: con_in es válido mientras existan los servicios de arranque.
    let key_event = unsafe { (*system_table().con_in).wait_for_key };
    let events = [key_event, timer.unwrap_or(null_mut())];
    let count = if timer.is_some() { 2 } else { 1 };
    loop {
        if let Some(k) = read_key() {
            return Wake::Key(k);
        }
        let mut index = 0usize;
        // SAFETY: eventos válidos.
        let st = unsafe { (bs.wait_for_event)(count, events.as_ptr(), &mut index) };
        if st.is_error() {
            stall_ms(50);
            continue;
        }
        if index == 1 {
            return Wake::Tick;
        }
    }
}

pub fn stall_ms(ms: usize) {
    // SAFETY: llamada UEFI.
    unsafe { (boot_services().stall)(ms * 1000) };
}

/// Desactiva el perro guardián (UEFI reinicia a los 5 minutos si no).
pub fn disable_watchdog() {
    // SAFETY: llamada UEFI.
    unsafe { (boot_services().set_watchdog_timer)(0, 0, 0, ptr::null()) };
}

// --- Protocolos y tablas -------------------------------------------------------

pub fn handle_protocol<T>(handle: Handle, guid: &Guid) -> Result<*mut T, Status> {
    let mut iface: *mut c_void = null_mut();
    // SAFETY: llamada UEFI.
    unsafe { (boot_services().handle_protocol)(handle, guid, &mut iface) }.ok()?;
    Ok(iface as *mut T)
}

pub fn locate_protocol<T>(guid: &Guid) -> Result<*mut T, Status> {
    let mut iface: *mut c_void = null_mut();
    // SAFETY: llamada UEFI.
    unsafe { (boot_services().locate_protocol)(guid, null_mut(), &mut iface) }.ok()?;
    Ok(iface as *mut T)
}

/// Dirección de una tabla de configuración (ACPI, SMBIOS...).
pub fn config_table(guid: &Guid) -> Option<u64> {
    let st = system_table();
    // SAFETY: tabla de `number_of_table_entries` elementos que provee el firmware.
    let tables = unsafe { core::slice::from_raw_parts(st.configuration_table, st.number_of_table_entries) };
    tables.iter().find(|t| t.vendor_guid == *guid).map(|t| t.vendor_table as u64)
}

const fn ucs2<const N: usize>(s: &str) -> [u16; N] {
    let b = s.as_bytes();
    let mut out = [0u16; N];
    let mut i = 0;
    while i < b.len() {
        out[i] = b[i] as u16;
        i += 1;
    }
    out
}

fn read_u8_variable(name: &[u16], guid: &Guid) -> Option<u8> {
    let mut value = 0u8;
    let mut size = 1usize;
    // SAFETY: llamada UEFI; el búfer es de 1 byte y así lo indicamos.
    let st = unsafe {
        (runtime_services().get_variable)(
            name.as_ptr(),
            guid,
            null_mut(),
            &mut size,
            &mut value as *mut u8 as *mut c_void,
        )
    };
    (!st.is_error() && size == 1).then_some(value)
}

/// Estado de Secure Boot (variables globales `SecureBoot` y `SetupMode`).
pub fn secure_boot_state() -> SecureBootState {
    const SECURE_BOOT: [u16; 11] = ucs2("SecureBoot");
    const SETUP_MODE: [u16; 10] = ucs2("SetupMode");
    match (read_u8_variable(&SECURE_BOOT, &GLOBAL_VARIABLE_GUID), read_u8_variable(&SETUP_MODE, &GLOBAL_VARIABLE_GUID))
    {
        (Some(1), _) => SecureBootState::ENABLED,
        (_, Some(1)) => SecureBootState::SETUP_MODE,
        (Some(0), _) => SecureBootState::DISABLED,
        _ => SecureBootState::UNKNOWN,
    }
}

pub fn get_time() -> Option<Time> {
    let mut t = Time::default();
    // SAFETY: llamada UEFI.
    let st = unsafe { (runtime_services().get_time)(&mut t, null_mut()) };
    (!st.is_error() && t.year >= 2000).then_some(t)
}

pub fn reset(kind: u32) -> ! {
    // SAFETY: llamada UEFI que no vuelve.
    unsafe { (runtime_services().reset_system)(kind, Status::SUCCESS, 0, ptr::null()) }
}

/// Nombre del fabricante del firmware (p. ej. "EDK II").
pub fn firmware_vendor() -> String {
    let st = system_table();
    let mut out = String::new();
    if st.firmware_vendor.is_null() {
        return out;
    }
    let mut p = st.firmware_vendor;
    // SAFETY: cadena UCS-2 terminada en 0 del firmware (limitamos la longitud).
    unsafe {
        for _ in 0..64 {
            let c = *p;
            if c == 0 {
                break;
            }
            out.push(char::from_u32(c as u32).unwrap_or('?'));
            p = p.add(1);
        }
    }
    out
}

/// Mapa de memoria UEFI en un búfer propio.
pub struct MemoryMap {
    pub buffer: *mut u8,
    pub capacity: usize,
    pub size: usize,
    pub key: usize,
    pub descriptor_size: usize,
}

impl MemoryMap {
    /// Reserva un búfer con holgura: entre esta llamada y ExitBootServices el
    /// mapa crece (nuestras propias reservas añaden descriptores).
    pub fn allocate() -> Result<Self, Status> {
        let bs = boot_services();
        let (mut size, mut key, mut desc_size, mut version) = (0usize, 0usize, 0usize, 0u32);
        // SAFETY: llamada de sondeo con búfer nulo: devuelve el tamaño necesario.
        unsafe { (bs.get_memory_map)(&mut size, null_mut(), &mut key, &mut desc_size, &mut version) };
        let capacity = size + desc_size.max(48) * 64;
        let pages = capacity.div_ceil(4096);
        let buffer = allocate_pages(memtype::LOADER_DATA, pages)? as *mut u8;
        Ok(Self { buffer, capacity: pages * 4096, size: 0, key: 0, descriptor_size: desc_size.max(48) })
    }

    /// Lee el mapa actual (sin reservar memoria: válido justo antes de salir).
    pub fn refresh(&mut self) -> Result<(), Status> {
        let bs = boot_services();
        let mut size = self.capacity;
        let mut version = 0u32;
        // SAFETY: búfer propio de `capacity` bytes.
        unsafe { (bs.get_memory_map)(&mut size, self.buffer, &mut self.key, &mut self.descriptor_size, &mut version) }
            .ok()?;
        self.size = size;
        Ok(())
    }

    pub fn descriptors(&self) -> impl Iterator<Item = MemoryDescriptor> + '_ {
        let count = if self.descriptor_size == 0 { 0 } else { self.size / self.descriptor_size };
        (0..count).map(move |i| {
            // SAFETY: dentro del tamaño devuelto por GetMemoryMap; lectura sin alinear por si acaso.
            unsafe { (self.buffer.add(i * self.descriptor_size) as *const MemoryDescriptor).read_unaligned() }
        })
    }

    pub fn len(&self) -> usize {
        self.size.checked_div(self.descriptor_size).unwrap_or(0)
    }
}

/// Cierra los servicios de arranque. Reintenta si el mapa cambió entre medias
/// (la especificación lo permite: hay que releer el mapa y volver a intentarlo).
pub fn exit_boot_services(map: &mut MemoryMap) -> Result<(), Status> {
    for _ in 0..8 {
        map.refresh()?;
        // SAFETY: llamada UEFI con la clave del mapa recién leído.
        let st = unsafe { (boot_services().exit_boot_services)(image(), map.key) };
        if !st.is_error() {
            mark_boot_services_exited();
            return Ok(());
        }
    }
    Err(Status::ABORTED)
}

/// Ruta de archivo en UCS-2 terminada en 0 (`/` → `\`, el separador de UEFI).
pub fn path_to_ucs2(s: &str) -> Vec<u16> {
    s.chars()
        .map(|c| match c {
            '/' => '\\' as u16,
            c if (c as u32) <= 0xFFFF => c as u16,
            _ => '?' as u16,
        })
        .chain(core::iter::once(0))
        .collect()
}
