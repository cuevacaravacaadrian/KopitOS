//! # Definiciones UEFI escritas a mano
//!
//! Sólo lo que KopitBoot necesita, copiado campo a campo de la especificación
//! UEFI 2.10. Escribirlas nosotros (en vez de usar un crate) tiene dos
//! ventajas: cero dependencias externas en la parte más sensible a la
//! seguridad (el cargador que firma Secure Boot) y entender *exactamente* qué
//! hace cada llamada. Los punteros a funciones que no usamos se declaran como
//! `usize` para conservar la disposición de las tablas.

#![allow(dead_code)]

use core::ffi::c_void;

pub type Handle = *mut c_void;
pub type Event = *mut c_void;
pub type Char16 = u16;

/// `EFI_STATUS`: el bit alto indica error.
#[repr(transparent)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Status(pub usize);

const ERR: usize = 1 << (usize::BITS - 1);

impl Status {
    pub const SUCCESS: Self = Self(0);
    pub const LOAD_ERROR: Self = Self(ERR | 1);
    pub const INVALID_PARAMETER: Self = Self(ERR | 2);
    pub const UNSUPPORTED: Self = Self(ERR | 3);
    pub const BUFFER_TOO_SMALL: Self = Self(ERR | 5);
    pub const NOT_READY: Self = Self(ERR | 6);
    pub const DEVICE_ERROR: Self = Self(ERR | 7);
    pub const OUT_OF_RESOURCES: Self = Self(ERR | 9);
    pub const NOT_FOUND: Self = Self(ERR | 14);
    pub const ABORTED: Self = Self(ERR | 21);
    pub const SECURITY_VIOLATION: Self = Self(ERR | 26);

    pub fn is_error(self) -> bool {
        self.0 & ERR != 0
    }

    pub fn ok(self) -> Result<(), Status> {
        if self.is_error() { Err(self) } else { Ok(()) }
    }

    pub fn describe(self) -> &'static str {
        match self.0 & !ERR {
            0 => "correcto",
            1 => "error de carga",
            2 => "parámetro no válido",
            3 => "no soportado",
            5 => "búfer demasiado pequeño",
            6 => "no está listo",
            7 => "error del dispositivo",
            9 => "sin recursos (memoria)",
            14 => "no encontrado",
            21 => "abortado",
            26 => "violación de seguridad",
            _ => "error UEFI",
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Guid(pub u32, pub u16, pub u16, pub [u8; 8]);

pub const LOADED_IMAGE_GUID: Guid = Guid(0x5B1B31A1, 0x9562, 0x11D2, [0x8E, 0x3F, 0x00, 0xA0, 0xC9, 0x69, 0x72, 0x3B]);
pub const SIMPLE_FILE_SYSTEM_GUID: Guid =
    Guid(0x964E5B22, 0x6459, 0x11D2, [0x8E, 0x39, 0x00, 0xA0, 0xC9, 0x69, 0x72, 0x3B]);
pub const GRAPHICS_OUTPUT_GUID: Guid =
    Guid(0x9042A9DE, 0x23DC, 0x4A38, [0x96, 0xFB, 0x7A, 0xDE, 0xD0, 0x80, 0x51, 0x6A]);
pub const ACPI_20_TABLE_GUID: Guid = Guid(0x8868E871, 0xE4F1, 0x11D3, [0xBC, 0x22, 0x00, 0x80, 0xC7, 0x3C, 0x88, 0x81]);
pub const ACPI_10_TABLE_GUID: Guid = Guid(0xEB9D2D30, 0x2D88, 0x11D3, [0x9A, 0x16, 0x00, 0x90, 0x27, 0x3F, 0xC1, 0x4D]);
pub const SMBIOS3_TABLE_GUID: Guid = Guid(0xF2FD1544, 0x9794, 0x4A2C, [0x99, 0x2E, 0xE5, 0xBB, 0xCF, 0x20, 0xE3, 0x94]);
pub const SMBIOS_TABLE_GUID: Guid = Guid(0xEB9D2D31, 0x2D88, 0x11D3, [0x9A, 0x16, 0x00, 0x90, 0x27, 0x3F, 0xC1, 0x4D]);
pub const GLOBAL_VARIABLE_GUID: Guid =
    Guid(0x8BE4DF61, 0x93CA, 0x11D2, [0xAA, 0x0D, 0x00, 0xE0, 0x98, 0x03, 0x2B, 0x8C]);

#[repr(C)]
pub struct TableHeader {
    pub signature: u64,
    pub revision: u32,
    pub header_size: u32,
    pub crc32: u32,
    pub reserved: u32,
}

#[repr(C)]
pub struct SystemTable {
    pub hdr: TableHeader,
    pub firmware_vendor: *const Char16,
    pub firmware_revision: u32,
    pub console_in_handle: Handle,
    pub con_in: *mut SimpleTextInput,
    pub console_out_handle: Handle,
    pub con_out: *mut SimpleTextOutput,
    pub standard_error_handle: Handle,
    pub std_err: *mut SimpleTextOutput,
    pub runtime_services: *mut RuntimeServices,
    pub boot_services: *mut BootServices,
    pub number_of_table_entries: usize,
    pub configuration_table: *const ConfigurationTable,
}

#[repr(C)]
pub struct ConfigurationTable {
    pub vendor_guid: Guid,
    pub vendor_table: *mut c_void,
}

// --- Consola de texto --------------------------------------------------------

#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct InputKey {
    pub scan_code: u16,
    pub unicode_char: u16,
}

pub const SCAN_UP: u16 = 0x01;
pub const SCAN_DOWN: u16 = 0x02;
pub const SCAN_ESC: u16 = 0x17;

#[repr(C)]
pub struct SimpleTextInput {
    pub reset: unsafe extern "efiapi" fn(*mut SimpleTextInput, u8) -> Status,
    pub read_key_stroke: unsafe extern "efiapi" fn(*mut SimpleTextInput, *mut InputKey) -> Status,
    pub wait_for_key: Event,
}

#[repr(C)]
pub struct SimpleTextOutputMode {
    pub max_mode: i32,
    pub mode: i32,
    pub attribute: i32,
    pub cursor_column: i32,
    pub cursor_row: i32,
    pub cursor_visible: u8,
}

#[repr(C)]
pub struct SimpleTextOutput {
    pub reset: unsafe extern "efiapi" fn(*mut SimpleTextOutput, u8) -> Status,
    pub output_string: unsafe extern "efiapi" fn(*mut SimpleTextOutput, *const Char16) -> Status,
    pub test_string: unsafe extern "efiapi" fn(*mut SimpleTextOutput, *const Char16) -> Status,
    pub query_mode: unsafe extern "efiapi" fn(*mut SimpleTextOutput, usize, *mut usize, *mut usize) -> Status,
    pub set_mode: unsafe extern "efiapi" fn(*mut SimpleTextOutput, usize) -> Status,
    pub set_attribute: unsafe extern "efiapi" fn(*mut SimpleTextOutput, usize) -> Status,
    pub clear_screen: unsafe extern "efiapi" fn(*mut SimpleTextOutput) -> Status,
    pub set_cursor_position: unsafe extern "efiapi" fn(*mut SimpleTextOutput, usize, usize) -> Status,
    pub enable_cursor: unsafe extern "efiapi" fn(*mut SimpleTextOutput, u8) -> Status,
    pub mode: *const SimpleTextOutputMode,
}

/// Colores de texto UEFI (`EFI_TEXT_ATTR`): fondo 0..=7, texto 0..=15.
pub mod color {
    pub const BLACK: usize = 0x0;
    pub const BLUE: usize = 0x1;
    pub const GREEN: usize = 0x2;
    pub const CYAN: usize = 0x3;
    pub const RED: usize = 0x4;
    pub const MAGENTA: usize = 0x5;
    pub const BROWN: usize = 0x6;
    pub const LIGHTGRAY: usize = 0x7;
    pub const DARKGRAY: usize = 0x8;
    pub const LIGHTBLUE: usize = 0x9;
    pub const LIGHTGREEN: usize = 0xA;
    pub const LIGHTCYAN: usize = 0xB;
    pub const LIGHTRED: usize = 0xC;
    pub const LIGHTMAGENTA: usize = 0xD;
    pub const YELLOW: usize = 0xE;
    pub const WHITE: usize = 0xF;

    pub const fn attr(fg: usize, bg: usize) -> usize {
        fg | (bg << 4)
    }
}

// --- Servicios de arranque ---------------------------------------------------

/// Tipos de memoria UEFI (`EFI_MEMORY_TYPE`).
pub mod memtype {
    pub const RESERVED: u32 = 0;
    pub const LOADER_CODE: u32 = 1;
    pub const LOADER_DATA: u32 = 2;
    pub const BOOT_SERVICES_CODE: u32 = 3;
    pub const BOOT_SERVICES_DATA: u32 = 4;
    pub const RUNTIME_SERVICES_CODE: u32 = 5;
    pub const RUNTIME_SERVICES_DATA: u32 = 6;
    pub const CONVENTIONAL: u32 = 7;
    pub const UNUSABLE: u32 = 8;
    pub const ACPI_RECLAIM: u32 = 9;
    pub const ACPI_NVS: u32 = 10;
    pub const MMIO: u32 = 11;
    pub const MMIO_PORT_SPACE: u32 = 12;
    pub const PAL_CODE: u32 = 13;
    pub const PERSISTENT: u32 = 14;
    pub const UNACCEPTED: u32 = 15;
    // Rango 0x8000_0000.. reservado por la especificación para el cargador del SO:
    // con tipos propios, el mapa de memoria final dice qué es cada cosa.
    pub const KOPIT_KERNEL: u32 = 0x8000_4B00;
    pub const KOPIT_BOOTINFO: u32 = 0x8000_4B01;
    pub const KOPIT_MODULE: u32 = 0x8000_4B02;
    pub const KOPIT_PAGETABLES: u32 = 0x8000_4B03;
}

pub const ALLOCATE_ANY_PAGES: u32 = 0;
pub const EVT_TIMER: u32 = 0x8000_0000;
pub const TPL_CALLBACK: usize = 8;
pub const TIMER_PERIODIC: u32 = 1;
pub const TIMER_CANCEL: u32 = 0;

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct MemoryDescriptor {
    pub kind: u32,
    pub physical_start: u64,
    pub virtual_start: u64,
    pub number_of_pages: u64,
    pub attribute: u64,
}

#[repr(C)]
pub struct BootServices {
    pub hdr: TableHeader,
    pub raise_tpl: usize,
    pub restore_tpl: usize,
    pub allocate_pages: unsafe extern "efiapi" fn(u32, u32, usize, *mut u64) -> Status,
    pub free_pages: unsafe extern "efiapi" fn(u64, usize) -> Status,
    pub get_memory_map: unsafe extern "efiapi" fn(*mut usize, *mut u8, *mut usize, *mut usize, *mut u32) -> Status,
    pub allocate_pool: unsafe extern "efiapi" fn(u32, usize, *mut *mut u8) -> Status,
    pub free_pool: unsafe extern "efiapi" fn(*mut u8) -> Status,
    pub create_event: unsafe extern "efiapi" fn(
        u32,
        usize,
        Option<unsafe extern "efiapi" fn(Event, *mut c_void)>,
        *mut c_void,
        *mut Event,
    ) -> Status,
    pub set_timer: unsafe extern "efiapi" fn(Event, u32, u64) -> Status,
    pub wait_for_event: unsafe extern "efiapi" fn(usize, *const Event, *mut usize) -> Status,
    pub signal_event: usize,
    pub close_event: unsafe extern "efiapi" fn(Event) -> Status,
    pub check_event: usize,
    pub install_protocol_interface: usize,
    pub reinstall_protocol_interface: usize,
    pub uninstall_protocol_interface: usize,
    pub handle_protocol: unsafe extern "efiapi" fn(Handle, *const Guid, *mut *mut c_void) -> Status,
    pub reserved: usize,
    pub register_protocol_notify: usize,
    pub locate_handle: usize,
    pub locate_device_path: usize,
    pub install_configuration_table: usize,
    pub load_image: usize,
    pub start_image: usize,
    pub exit: usize,
    pub unload_image: usize,
    pub exit_boot_services: unsafe extern "efiapi" fn(Handle, usize) -> Status,
    pub get_next_monotonic_count: usize,
    pub stall: unsafe extern "efiapi" fn(usize) -> Status,
    pub set_watchdog_timer: unsafe extern "efiapi" fn(usize, u64, usize, *const Char16) -> Status,
    pub connect_controller: usize,
    pub disconnect_controller: usize,
    pub open_protocol: usize,
    pub close_protocol: usize,
    pub open_protocol_information: usize,
    pub protocols_per_handle: usize,
    pub locate_handle_buffer: usize,
    pub locate_protocol: unsafe extern "efiapi" fn(*const Guid, *mut c_void, *mut *mut c_void) -> Status,
}

// --- Servicios de ejecución --------------------------------------------------

#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct Time {
    pub year: u16,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
    pub pad1: u8,
    pub nanosecond: u32,
    pub time_zone: i16,
    pub daylight: u8,
    pub pad2: u8,
}

pub const RESET_COLD: u32 = 0;
pub const RESET_SHUTDOWN: u32 = 2;

#[repr(C)]
pub struct RuntimeServices {
    pub hdr: TableHeader,
    pub get_time: unsafe extern "efiapi" fn(*mut Time, *mut c_void) -> Status,
    pub set_time: usize,
    pub get_wakeup_time: usize,
    pub set_wakeup_time: usize,
    pub set_virtual_address_map: usize,
    pub convert_pointer: usize,
    pub get_variable:
        unsafe extern "efiapi" fn(*const Char16, *const Guid, *mut u32, *mut usize, *mut c_void) -> Status,
    pub get_next_variable_name: usize,
    pub set_variable: usize,
    pub get_next_high_monotonic_count: usize,
    pub reset_system: unsafe extern "efiapi" fn(u32, Status, usize, *const c_void) -> !,
}

// --- Protocolos --------------------------------------------------------------

#[repr(C)]
pub struct LoadedImage {
    pub revision: u32,
    pub parent_handle: Handle,
    pub system_table: *mut SystemTable,
    pub device_handle: Handle,
    pub file_path: *mut c_void,
    pub reserved: *mut c_void,
    pub load_options_size: u32,
    pub load_options: *mut c_void,
    pub image_base: *mut c_void,
    pub image_size: u64,
    pub image_code_type: u32,
    pub image_data_type: u32,
    pub unload: usize,
}

#[repr(C)]
pub struct SimpleFileSystem {
    pub revision: u64,
    pub open_volume: unsafe extern "efiapi" fn(*mut SimpleFileSystem, *mut *mut File) -> Status,
}

pub const FILE_MODE_READ: u64 = 0x1;

#[repr(C)]
pub struct File {
    pub revision: u64,
    pub open: unsafe extern "efiapi" fn(*mut File, *mut *mut File, *const Char16, u64, u64) -> Status,
    pub close: unsafe extern "efiapi" fn(*mut File) -> Status,
    pub delete: usize,
    pub read: unsafe extern "efiapi" fn(*mut File, *mut usize, *mut u8) -> Status,
    pub write: usize,
    pub get_position: unsafe extern "efiapi" fn(*mut File, *mut u64) -> Status,
    pub set_position: unsafe extern "efiapi" fn(*mut File, u64) -> Status,
    pub get_info: usize,
    pub set_info: usize,
    pub flush: usize,
}

pub const PIXEL_RGB: u32 = 0;
pub const PIXEL_BGR: u32 = 1;
pub const PIXEL_BITMASK: u32 = 2;
pub const PIXEL_BLT_ONLY: u32 = 3;

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct GraphicsModeInfo {
    pub version: u32,
    pub horizontal_resolution: u32,
    pub vertical_resolution: u32,
    pub pixel_format: u32,
    pub red_mask: u32,
    pub green_mask: u32,
    pub blue_mask: u32,
    pub reserved_mask: u32,
    pub pixels_per_scan_line: u32,
}

#[repr(C)]
pub struct GraphicsMode {
    pub max_mode: u32,
    pub mode: u32,
    pub info: *const GraphicsModeInfo,
    pub size_of_info: usize,
    pub frame_buffer_base: u64,
    pub frame_buffer_size: usize,
}

#[repr(C)]
pub struct GraphicsOutput {
    pub query_mode:
        unsafe extern "efiapi" fn(*mut GraphicsOutput, u32, *mut usize, *mut *const GraphicsModeInfo) -> Status,
    pub set_mode: unsafe extern "efiapi" fn(*mut GraphicsOutput, u32) -> Status,
    pub blt: usize,
    pub mode: *const GraphicsMode,
}
