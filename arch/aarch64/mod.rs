//! # Arquitectura ARM64 (AArch64)
//!
//! Fase 1: QEMU `virt` arrancando el ELF directamente en **EL1**.
//!
//! * Entrada: QEMU deja el DTB al principio de la RAM (`0x4000_0000`) y salta a
//!   `_start` (enlazado en `0x4020_0000`).
//! * **MMU en identidad** desde el primer momento: con la MMU apagada toda la
//!   memoria es de tipo *Device* (sin caché, sin atómicos fiables). Una tabla de
//!   nivel 1 con bloques de 1 GiB basta: 0–1 GiB dispositivos, 1–4 GiB RAM normal.
//! * Excepciones: tabla de 16 vectores en `VBAR_EL1`.
//! * Interrupciones: **GICv2** + temporizador virtual genérico (PPI 27).
//! * Consola: PL011. Energía: PSCI (`hvc`). Salida de pruebas: semihosting.
//!
//! Fase 2: KopitBoot para ARM64 (UEFI), mitad alta en TTBR1, GICv3, SMP.

pub mod paging;

use crate::hal::Arch;
use crate::sync::{IrqLock, RacyCell};
use core::arch::{asm, global_asm};
use core::sync::atomic::{AtomicU64, Ordering};
use kopit_boot_proto::BootInfo;
use kopit_driver_api::{CharSink, MmioRegisters};
use kopit_drv_pl011::Pl011;

const DTB_ADDRESS: u64 = 0x4000_0000;

global_asm!(
    r#"
    .section .text._start, "ax"
    .global _start
_start:
    mrs x1, mpidr_el1
    and x1, x1, #0xff
    cbnz x1, .Lpark
    mov x1, #(3 << 20)
    msr cpacr_el1, x1
    isb
    adrp x1, __boot_stack_top
    add x1, x1, :lo12:__boot_stack_top
    mov sp, x1
    mov x29, #0
    mov x30, #0
    bl kopit_entry_aarch64
.Lpark:
    wfe
    b .Lpark

    .macro VECTOR index
    .balign 0x80
    sub sp, sp, #272
    stp x0, x1, [sp, #0]
    stp x2, x3, [sp, #16]
    stp x4, x5, [sp, #32]
    stp x6, x7, [sp, #48]
    stp x8, x9, [sp, #64]
    stp x10, x11, [sp, #80]
    stp x12, x13, [sp, #96]
    stp x14, x15, [sp, #112]
    stp x16, x17, [sp, #128]
    stp x18, x19, [sp, #144]
    stp x20, x21, [sp, #160]
    stp x22, x23, [sp, #176]
    stp x24, x25, [sp, #192]
    stp x26, x27, [sp, #208]
    stp x28, x29, [sp, #224]
    str x30, [sp, #240]
    mov x1, #\index
    b kopit_vector_common
    .endm

    .section .text.vectors, "ax"
    .balign 0x800
    .global kopit_vectors
kopit_vectors:
    VECTOR 0
    VECTOR 1
    VECTOR 2
    VECTOR 3
    VECTOR 4
    VECTOR 5
    VECTOR 6
    VECTOR 7
    VECTOR 8
    VECTOR 9
    VECTOR 10
    VECTOR 11
    VECTOR 12
    VECTOR 13
    VECTOR 14
    VECTOR 15

kopit_vector_common:
    mrs x2, elr_el1
    mrs x3, spsr_el1
    stp x2, x3, [sp, #248]
    mov x0, sp
    bl kopit_trap_aarch64
    ldp x2, x3, [sp, #248]
    msr elr_el1, x2
    msr spsr_el1, x3
    ldp x0, x1, [sp, #0]
    ldp x2, x3, [sp, #16]
    ldp x4, x5, [sp, #32]
    ldp x6, x7, [sp, #48]
    ldp x8, x9, [sp, #64]
    ldp x10, x11, [sp, #80]
    ldp x12, x13, [sp, #96]
    ldp x14, x15, [sp, #112]
    ldp x16, x17, [sp, #128]
    ldp x18, x19, [sp, #144]
    ldp x20, x21, [sp, #160]
    ldp x22, x23, [sp, #176]
    ldp x24, x25, [sp, #192]
    ldp x26, x27, [sp, #208]
    ldp x28, x29, [sp, #224]
    ldr x30, [sp, #240]
    add sp, sp, #272
    eret

    .section .bss.boot, "aw", @nobits
    .balign 4096
    .global __boot_stack_guard
    .global __boot_stack_bottom
    .global __boot_stack_top
__boot_stack_guard:
    .skip 4096
__boot_stack_bottom:
    .skip 65536
__boot_stack_top:
    .section .text
"#
);

#[repr(C)]
pub struct TrapFrame {
    pub x: [u64; 31],
    pub elr: u64,
    pub spsr: u64,
    _pad: u64,
}

#[repr(C, align(4096))]
struct PageTable([u64; 512]);

static L1: RacyCell<PageTable> = RacyCell::new(PageTable([0; 512]));
static UART: IrqLock<Option<Pl011>> = IrqLock::new(None);
static GICD: AtomicU64 = AtomicU64::new(0x0800_0000);
static GICC: AtomicU64 = AtomicU64::new(0x0801_0000);
static TIMER_FREQ: AtomicU64 = AtomicU64::new(62_500_000);
static TICK_INTERVAL: AtomicU64 = AtomicU64::new(0);
static BREAKPOINTS: AtomicU64 = AtomicU64::new(0);
static PSCI_SMC: AtomicU64 = AtomicU64::new(0);

macro_rules! sysreg {
    (read $r:literal) => {{
        let v: u64;
        // SAFETY: lectura de un registro de sistema en EL1.
        unsafe { asm!(concat!("mrs {}, ", $r), out(reg) v, options(nomem, nostack)) };
        v
    }};
    (write $r:literal, $v:expr) => {{
        // SAFETY: escritura de un registro de sistema en EL1.
        unsafe { asm!(concat!("msr ", $r, ", {}"), in(reg) $v, options(nostack)) };
    }};
}

/// Identidad 0–4 GiB con bloques de 1 GiB y la MMU activa (caché incluida).
fn enable_mmu() {
    const VALID_BLOCK: u64 = 0b01;
    const AF: u64 = 1 << 10;
    const INNER_SHAREABLE: u64 = 0b11 << 8;
    const ATTR_DEVICE: u64 = 0 << 2;
    const ATTR_NORMAL: u64 = 1 << 2;
    const XN: u64 = (1 << 54) | (1 << 53);
    // SAFETY: arranque, un solo núcleo; la tabla es estática y está alineada.
    let table = unsafe { &mut (*L1.get()).0 };
    table[0] = VALID_BLOCK | AF | ATTR_DEVICE | XN; // 0-1 GiB: GIC, UART, PCIe...
    for (i, e) in table.iter_mut().enumerate().take(4).skip(1) {
        *e = ((i as u64) << 30) | VALID_BLOCK | AF | ATTR_NORMAL | INNER_SHAREABLE; // RAM
    }
    let mair: u64 = 0x00 | (0xFF << 8); // attr0 = Device-nGnRnE, attr1 = Normal WB
    let tcr: u64 = 25 | (0b01 << 8) | (0b01 << 10) | (0b11 << 12) | (1 << 23) | (0b010 << 32);
    sysreg!(write "mair_el1", mair);
    sysreg!(write "tcr_el1", tcr);
    sysreg!(write "ttbr0_el1", table.as_ptr() as u64);
    // SAFETY: barreras e invalidación del TLB antes de activar la MMU.
    unsafe { asm!("dsb ish", "tlbi vmalle1", "dsb ish", "isb", options(nostack)) };
    let sctlr = sysreg!(read "sctlr_el1") | 1 | (1 << 2) | (1 << 12);
    sysreg!(write "sctlr_el1", sctlr);
    // SAFETY: sincroniza el cambio de contexto.
    unsafe { asm!("isb", options(nostack)) };
}

#[unsafe(no_mangle)]
extern "C" fn kopit_entry_aarch64() -> ! {
    enable_mmu();
    let info = crate::dtb::build(DTB_ADDRESS, "QEMU virt (arranque directo, EL1)");
    crate::init::kernel_main(info)
}

// --- GIC y temporizador ------------------------------------------------------------------

const TIMER_PPI: u32 = 27;

fn gicd(off: u64) -> *mut u32 {
    (GICD.load(Ordering::Relaxed) + off) as *mut u32
}

fn gicc(off: u64) -> *mut u32 {
    (GICC.load(Ordering::Relaxed) + off) as *mut u32
}

fn init_gic() {
    // SAFETY: registros MMIO del GICv2 (memoria de dispositivo).
    unsafe {
        gicd(0x000).write_volatile(1); // distribuidor activo
        gicd(0x100).write_volatile(1 << TIMER_PPI); // habilitar PPI 27
        let prio = gicd(0x400 + (TIMER_PPI as u64 & !3)) as *mut u8;
        prio.add((TIMER_PPI & 3) as usize).write_volatile(0xA0);
        gicc(0x004).write_volatile(0xFF); // máscara de prioridad: todo
        gicc(0x000).write_volatile(1); // interfaz de CPU activa
    }
}

fn rearm_timer() {
    sysreg!(write "cntv_tval_el0", TICK_INTERVAL.load(Ordering::Relaxed));
    sysreg!(write "cntv_ctl_el0", 1u64);
}

fn poll_uart() {
    if let Some(mut u) = UART.try_lock() {
        if let Some(uart) = u.as_mut() {
            while let Some(b) = uart.try_read_byte() {
                crate::input::push_serial_byte(b);
            }
        }
    }
}

#[unsafe(no_mangle)]
extern "C" fn kopit_trap_aarch64(f: &mut TrapFrame, index: u64) {
    match index {
        // IRQ desde EL1 (SP_ELx)
        5 => {
            // SAFETY: GICC_IAR / GICC_EOIR.
            let iar = unsafe { gicc(0x00C).read_volatile() };
            let id = iar & 0x3FF;
            if id == TIMER_PPI {
                crate::time::on_tick();
                poll_uart();
                rearm_timer();
            }
            if id < 1020 {
                // SAFETY: fin de interrupción.
                unsafe { gicc(0x010).write_volatile(iar) };
            }
        }
        // Excepción síncrona desde EL1
        4 => {
            let esr = sysreg!(read "esr_el1");
            let ec = esr >> 26;
            if ec == 0x3C {
                BREAKPOINTS.fetch_add(1, Ordering::Relaxed);
                f.elr += 4;
                kinfo!("punto de ruptura (brk) atendido; se continúa");
                return;
            }
            let far = sysreg!(read "far_el1");
            panic!("excepción ARM64: EC {:#x} (ESR {:#x}) en {:#x}, FAR {:#x}", ec, esr, f.elr, far);
        }
        other => panic!("vector de excepción inesperado {} en {:#x}", other, f.elr),
    }
}

// --- PSCI y semihosting ------------------------------------------------------------------

fn psci(function: u64) {
    if PSCI_SMC.load(Ordering::Relaxed) == 1 {
        // SAFETY: llamada PSCI al firmware.
        unsafe { asm!("smc #0", inout("x0") function => _, options(nostack)) };
    } else {
        // SAFETY: llamada PSCI al hipervisor (QEMU la atiende).
        unsafe { asm!("hvc #0", inout("x0") function => _, options(nostack)) };
    }
}

// --- HAL -------------------------------------------------------------------------------

pub struct ArchImpl;

impl Arch for ArchImpl {
    const NAME: &'static str = "aarch64";

    fn early_init(boot: &BootInfo) {
        let dtb = boot.dtb;
        let base = crate::dtb::device_base(dtb, &["arm,pl011"]).unwrap_or(0x0900_0000);
        // SAFETY: MMIO de la PL011 (bloque de dispositivo en la identidad).
        *UART.lock() = Some(Pl011::new(unsafe { MmioRegisters::new(base as usize, 0) }));
        if let Some(fdt_gic) = crate::dtb::device_base(dtb, &["arm,cortex-a15-gic", "arm,gic-400"]) {
            GICD.store(fdt_gic, Ordering::Relaxed);
            GICC.store(fdt_gic + 0x1_0000, Ordering::Relaxed);
        }
        if crate::dtb::node_str(dtb, "psci", "method") == Some("smc") {
            PSCI_SMC.store(1, Ordering::Relaxed);
        }
        let freq = sysreg!(read "cntfrq_el0");
        if freq != 0 {
            TIMER_FREQ.store(freq, Ordering::Relaxed);
        }
        crate::time::set_cycle_frequency(TIMER_FREQ.load(Ordering::Relaxed));
    }

    fn init_cpu() {
        unsafe extern "C" {
            safe static kopit_vectors: u8;
        }
        sysreg!(write "vbar_el1", &raw const kopit_vectors as u64);
        // SAFETY: sincroniza el cambio de VBAR.
        unsafe { asm!("isb", options(nostack)) };
    }

    fn init_platform(hz: u32) {
        init_gic();
        TICK_INTERVAL.store((TIMER_FREQ.load(Ordering::Relaxed) / hz.max(1) as u64).max(1), Ordering::Relaxed);
        rearm_timer();
        kinfo!("GICv2 + temporizador genérico: {} MHz", TIMER_FREQ.load(Ordering::Relaxed) / 1_000_000);
    }

    fn enable_interrupts() {
        // SAFETY: borra la máscara de IRQ (DAIF.I).
        unsafe { asm!("msr daifclr, #2", options(nomem, nostack)) };
    }

    fn disable_interrupts() -> bool {
        let was = Self::interrupts_enabled();
        // SAFETY: activa la máscara de IRQ.
        unsafe { asm!("msr daifset, #2", options(nomem, nostack)) };
        was
    }

    fn restore_interrupts(was_enabled: bool) {
        if was_enabled {
            Self::enable_interrupts();
        }
    }

    fn interrupts_enabled() -> bool {
        sysreg!(read "daif") & (1 << 7) == 0
    }

    fn wait_for_interrupt() {
        // SAFETY: espera a la siguiente interrupción.
        unsafe { asm!("msr daifclr, #2", "wfi", options(nomem, nostack)) };
    }

    fn halt() -> ! {
        loop {
            // SAFETY: parada.
            unsafe { asm!("msr daifset, #2", "wfi", options(nomem, nostack)) };
        }
    }

    fn cycles() -> u64 {
        sysreg!(read "cntvct_el0")
    }

    fn serial_write(bytes: &[u8]) {
        if let Some(u) = UART.lock().as_mut() {
            u.write_bytes(bytes);
        }
    }

    fn serial_read() -> Option<u8> {
        UART.lock().as_mut().and_then(|u| u.try_read_byte())
    }

    unsafe fn serial_force_unlock() {
        // SAFETY: lo garantiza quien llama.
        unsafe { UART.force_unlock() }
    }

    fn reboot() -> ! {
        psci(0x8400_0009);
        Self::halt()
    }

    fn power_off() -> ! {
        psci(0x8400_0008);
        Self::halt()
    }

    fn breakpoint() {
        // SAFETY: provoca una excepción BRK que el manejador atiende y salta.
        unsafe { asm!("brk #0") };
    }

    fn backtrace(f: &mut dyn FnMut(u64)) {
        let mut fp: u64;
        // SAFETY: lectura del puntero de marco (x29).
        unsafe { asm!("mov {}, x29", out(reg) fp, options(nomem, nostack)) };
        let (lo, hi) = (crate::mm::layout::bss_start(), crate::mm::layout::bss_end());
        for _ in 0..16 {
            if fp < lo || fp + 16 > hi || fp % 8 != 0 {
                break;
            }
            // SAFETY: dentro del .bss (pilas del núcleo), comprobado arriba.
            let (prev, lr) = unsafe { (*(fp as *const u64), *((fp + 8) as *const u64)) };
            if lr == 0 {
                break;
            }
            f(lr);
            if prev <= fp {
                break;
            }
            fp = prev;
        }
    }
}

// --- Servicios para el resto del núcleo ----------------------------------------------------

pub fn breakpoint_count() -> u64 {
    BREAKPOINTS.load(Ordering::Relaxed)
}

/// Salida de QEMU por semihosting (sólo en pruebas: QEMU se lanza con `-semihosting`).
pub fn qemu_exit(code: u32) {
    let status: u64 = if code == crate::hal::EXIT_SUCCESS { 0 } else { code as u64 };
    let block: [u64; 2] = [0x20026, status]; // ADP_Stopped_ApplicationExit
    // SAFETY: llamada de semihosting SYS_EXIT (0x18) con su bloque de parámetros.
    unsafe { asm!("hlt #0xf000", in("x0") 0x18u64, in("x1") block.as_ptr(), options(nostack)) };
}

pub fn describe_cpu(f: &mut dyn FnMut(&str, &str)) {
    let midr = sysreg!(read "midr_el1");
    let el = (sysreg!(read "currentel") >> 2) & 3;
    let mut b = crate::log::FixedBuf::<96>::new();
    let implementer = match midr >> 24 {
        0x41 => "Arm",
        0x51 => "Qualcomm",
        0x61 => "Apple",
        0x48 => "HiSilicon",
        0xC0 => "Ampere",
        _ => "desconocido",
    };
    let _ = core::fmt::Write::write_fmt(
        &mut b,
        format_args!(
            "{} · pieza {:#05x} · rev r{}p{}",
            implementer,
            (midr >> 4) & 0xFFF,
            (midr >> 20) & 0xF,
            midr & 0xF
        ),
    );
    f("procesador", b.as_str());
    b.clear();
    let _ = core::fmt::Write::write_fmt(&mut b, format_args!("EL{} · MMU identidad (bloques de 1 GiB)", el));
    f("nivel de excepción", b.as_str());
    b.clear();
    let _ = core::fmt::Write::write_fmt(&mut b, format_args!("{} MHz", TIMER_FREQ.load(Ordering::Relaxed) / 1_000_000));
    f("temporizador genérico", b.as_str());
    f("controlador de IRQ", "GICv2");
}

pub fn paging_self_check() -> Option<(bool, bool, bool)> {
    None
}

pub fn pci_config() -> Option<NoPci> {
    None
}

pub struct NoPci;

impl kopit_drv_pci::ConfigSpace for NoPci {
    fn read32(&self, _: u8, _: u8, _: u8, _: u16) -> u32 {
        u32::MAX
    }
    fn write32(&self, _: u8, _: u8, _: u8, _: u16, _: u32) {}
}
