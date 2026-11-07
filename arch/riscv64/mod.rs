//! # Arquitectura RISC-V 64 (RV64GC)
//!
//! Fase 1: QEMU `virt` con OpenSBI (firmware de máquina, modo M). El núcleo
//! corre en **modo S** con la MMU en identidad (`satp = 0`).
//!
//! * Entrada: OpenSBI salta a `_start` con `a0 = hart`, `a1 = DTB`.
//! * Consola: la UART `ns16550a` con **el mismo controlador** 16550 que el COM1
//!   de un PC, aquí por MMIO: la prueba de que `drivers/` es portable.
//! * Temporizador: extensión TIME de SBI; interrupción de temporizador de
//!   supervisor (STIP) cada `1/hz` segundos.
//! * Apagado y reinicio: extensión SRST de SBI.
//!
//! Fase 2: Sv48 con mitad alta (igual que x86_64), PLIC, SMP con HSM.

pub mod paging;

use crate::hal::Arch;
use crate::sync::IrqLock;
use core::arch::{asm, global_asm};
use core::sync::atomic::{AtomicU64, Ordering};
use kopit_boot_proto::BootInfo;
use kopit_driver_api::{CharSink, MmioRegisters};
use kopit_drv_uart16550::Uart16550;

global_asm!(
    r#"
    .section .text._start, "ax"
    .global _start
_start:
    la sp, __boot_stack_top
    li s0, 0
    call kopit_entry_riscv
.Lpark:
    wfi
    j .Lpark

    .section .text.trap, "ax"
    .balign 4
    .global kopit_trap_entry
kopit_trap_entry:
    addi sp, sp, -272
    sd x1, 8(sp)
    sd x3, 24(sp)
    sd x4, 32(sp)
    sd x5, 40(sp)
    sd x6, 48(sp)
    sd x7, 56(sp)
    sd x8, 64(sp)
    sd x9, 72(sp)
    sd x10, 80(sp)
    sd x11, 88(sp)
    sd x12, 96(sp)
    sd x13, 104(sp)
    sd x14, 112(sp)
    sd x15, 120(sp)
    sd x16, 128(sp)
    sd x17, 136(sp)
    sd x18, 144(sp)
    sd x19, 152(sp)
    sd x20, 160(sp)
    sd x21, 168(sp)
    sd x22, 176(sp)
    sd x23, 184(sp)
    sd x24, 192(sp)
    sd x25, 200(sp)
    sd x26, 208(sp)
    sd x27, 216(sp)
    sd x28, 224(sp)
    sd x29, 232(sp)
    sd x30, 240(sp)
    sd x31, 248(sp)
    csrr t0, sepc
    sd t0, 256(sp)
    csrr t1, sstatus
    sd t1, 264(sp)
    mv a0, sp
    call kopit_trap_riscv
    ld t0, 256(sp)
    csrw sepc, t0
    ld t1, 264(sp)
    csrw sstatus, t1
    ld x1, 8(sp)
    ld x3, 24(sp)
    ld x4, 32(sp)
    ld x5, 40(sp)
    ld x6, 48(sp)
    ld x7, 56(sp)
    ld x8, 64(sp)
    ld x9, 72(sp)
    ld x10, 80(sp)
    ld x11, 88(sp)
    ld x12, 96(sp)
    ld x13, 104(sp)
    ld x14, 112(sp)
    ld x15, 120(sp)
    ld x16, 128(sp)
    ld x17, 136(sp)
    ld x18, 144(sp)
    ld x19, 152(sp)
    ld x20, 160(sp)
    ld x21, 168(sp)
    ld x22, 176(sp)
    ld x23, 184(sp)
    ld x24, 192(sp)
    ld x25, 200(sp)
    ld x26, 208(sp)
    ld x27, 216(sp)
    ld x28, 224(sp)
    ld x29, 232(sp)
    ld x30, 240(sp)
    ld x31, 248(sp)
    addi sp, sp, 272
    sret

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

/// Registros guardados por `kopit_trap_entry` (x0..x31, sepc, sstatus).
#[repr(C)]
pub struct TrapFrame {
    pub x: [u64; 32],
    pub sepc: u64,
    pub sstatus: u64,
}

static DTB: AtomicU64 = AtomicU64::new(0);
static UART: IrqLock<Option<Uart16550<MmioRegisters>>> = IrqLock::new(None);
static TIMEBASE: AtomicU64 = AtomicU64::new(10_000_000);
static TICK_INTERVAL: AtomicU64 = AtomicU64::new(0);
static BREAKPOINTS: AtomicU64 = AtomicU64::new(0);
static TEST_DEVICE: AtomicU64 = AtomicU64::new(0x10_0000);

#[unsafe(no_mangle)]
extern "C" fn kopit_entry_riscv(_hart: u64, dtb: u64) -> ! {
    DTB.store(dtb, Ordering::Relaxed);
    let info = crate::dtb::build(dtb, "OpenSBI (RISC-V)");
    crate::init::kernel_main(info)
}

// --- SBI -------------------------------------------------------------------------------

fn sbi_call(eid: u64, fid: u64, a0: u64, a1: u64) -> (i64, u64) {
    let (error, value): (i64, u64);
    // SAFETY: llamada al firmware (OpenSBI) según la especificación SBI.
    unsafe {
        asm!("ecall", inlateout("a0") a0 => error, inlateout("a1") a1 => value, in("a6") fid, in("a7") eid, options(nostack))
    };
    (error, value)
}

const SBI_TIME: u64 = 0x5449_4D45;
const SBI_SRST: u64 = 0x5352_5354;

fn set_timer(deadline: u64) {
    let (err, _) = sbi_call(SBI_TIME, 0, deadline, 0);
    if err != 0 {
        sbi_call(0x00, 0, deadline, 0); // extensión heredada
    }
}

fn read_time() -> u64 {
    let t: u64;
    // SAFETY: lectura del contador `time`.
    unsafe { asm!("csrr {}, time", out(reg) t, options(nomem, nostack)) };
    t
}

// --- Traps -----------------------------------------------------------------------------

const EXCEPTIONS: [&str; 16] = [
    "dirección de instrucción desalineada",
    "fallo de acceso a instrucción",
    "instrucción ilegal",
    "punto de ruptura",
    "lectura desalineada",
    "fallo de acceso en lectura",
    "escritura desalineada",
    "fallo de acceso en escritura",
    "ecall desde U",
    "ecall desde S",
    "reservada",
    "ecall desde M",
    "fallo de página de instrucción",
    "fallo de página en lectura",
    "reservada",
    "fallo de página en escritura",
];

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
extern "C" fn kopit_trap_riscv(f: &mut TrapFrame) {
    let (scause, stval): (u64, u64);
    // SAFETY: lectura de CSR de supervisor.
    unsafe {
        asm!("csrr {}, scause", out(reg) scause, options(nomem, nostack));
        asm!("csrr {}, stval", out(reg) stval, options(nomem, nostack));
    }
    let interrupt = scause >> 63 == 1;
    let code = scause & 0xFF;
    if interrupt {
        if code == 5 {
            // Temporizador de supervisor.
            crate::time::on_tick();
            poll_uart();
            set_timer(read_time() + TICK_INTERVAL.load(Ordering::Relaxed));
        }
        return;
    }
    if code == 3 {
        BREAKPOINTS.fetch_add(1, Ordering::Relaxed);
        // SAFETY: `sepc` apunta a la instrucción ebreak (2 o 4 bytes).
        let half = unsafe { (f.sepc as *const u16).read() };
        f.sepc += if half & 0b11 == 0b11 { 4 } else { 2 };
        kinfo!("punto de ruptura (ebreak) atendido; se continúa");
        return;
    }
    let name = EXCEPTIONS.get(code as usize).copied().unwrap_or("desconocida");
    panic!("excepción RISC-V: {} (scause {:#x}) en {:#x}, stval {:#x}", name, scause, f.sepc, stval);
}

// --- HAL -------------------------------------------------------------------------------

pub struct ArchImpl;

impl Arch for ArchImpl {
    const NAME: &'static str = "riscv64";

    fn early_init(boot: &BootInfo) {
        let dtb = boot.dtb;
        let base = crate::dtb::device_base(dtb, &["ns16550a", "ns16550"]).unwrap_or(0x1000_0000);
        // SAFETY: MMIO de la UART (identidad, sin MMU).
        let mut uart = Uart16550::new(unsafe { MmioRegisters::new(base as usize, 0) });
        uart.assume_present();
        *UART.lock() = Some(uart);
        if let Some(tb) = crate::dtb::node_u32(dtb, "cpus", "timebase-frequency") {
            TIMEBASE.store(tb as u64, Ordering::Relaxed);
        }
        if let Some(t) = crate::dtb::device_base(dtb, &["sifive,test1", "sifive,test0"]) {
            TEST_DEVICE.store(t, Ordering::Relaxed);
        }
        crate::time::set_cycle_frequency(TIMEBASE.load(Ordering::Relaxed));
    }

    fn init_cpu() {
        // SAFETY: instala el vector de traps (modo directo).
        unsafe {
            let entry: u64;
            asm!("la {}, kopit_trap_entry", out(reg) entry);
            asm!("csrw stvec, {}", in(reg) entry);
        }
    }

    fn init_platform(hz: u32) {
        let interval = TIMEBASE.load(Ordering::Relaxed) / hz.max(1) as u64;
        TICK_INTERVAL.store(interval.max(1), Ordering::Relaxed);
        set_timer(read_time() + interval);
        // SAFETY: activa la interrupción de temporizador de supervisor (sie.STIE).
        unsafe { asm!("csrs sie, {}", in(reg) 1u64 << 5) };
        kinfo!("temporizador SBI: base de tiempos {} MHz", TIMEBASE.load(Ordering::Relaxed) / 1_000_000);
    }

    fn enable_interrupts() {
        // SAFETY: sstatus.SIE.
        unsafe { asm!("csrsi sstatus, 2", options(nomem, nostack)) };
    }

    fn disable_interrupts() -> bool {
        let prev: u64;
        // SAFETY: borra sstatus.SIE devolviendo el valor anterior.
        unsafe { asm!("csrrci {}, sstatus, 2", out(reg) prev, options(nomem, nostack)) };
        prev & 2 != 0
    }

    fn restore_interrupts(was_enabled: bool) {
        if was_enabled {
            Self::enable_interrupts();
        }
    }

    fn interrupts_enabled() -> bool {
        let s: u64;
        // SAFETY: lectura de sstatus.
        unsafe { asm!("csrr {}, sstatus", out(reg) s, options(nomem, nostack)) };
        s & 2 != 0
    }

    fn wait_for_interrupt() {
        // SAFETY: wfi despierta con la interrupción pendiente aunque SIE la retrase.
        unsafe { asm!("csrsi sstatus, 2", "wfi", options(nomem, nostack)) };
    }

    fn halt() -> ! {
        loop {
            // SAFETY: parada.
            unsafe { asm!("csrci sstatus, 2", "wfi", options(nomem, nostack)) };
        }
    }

    fn cycles() -> u64 {
        read_time()
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
        sbi_call(SBI_SRST, 0, 1, 0);
        Self::halt()
    }

    fn power_off() -> ! {
        sbi_call(SBI_SRST, 0, 0, 0);
        sbi_call(0x08, 0, 0, 0);
        Self::halt()
    }

    fn breakpoint() {
        // SAFETY: provoca la excepción 3, que el manejador atiende y salta.
        unsafe { asm!("ebreak") };
    }

    fn backtrace(f: &mut dyn FnMut(u64)) {
        let mut fp: u64;
        // SAFETY: lectura del puntero de marco (s0).
        unsafe { asm!("mv {}, s0", out(reg) fp, options(nomem, nostack)) };
        let (lo, hi) = (crate::mm::layout::bss_start(), crate::mm::layout::bss_end());
        for _ in 0..16 {
            if fp < lo + 16 || fp > hi || fp % 8 != 0 {
                break;
            }
            // SAFETY: dentro del .bss (pilas del núcleo), comprobado arriba.
            let (ra, prev) = unsafe { (*((fp - 8) as *const u64), *((fp - 16) as *const u64)) };
            if ra == 0 {
                break;
            }
            f(ra);
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

/// Dispositivo `sifive,test` de QEMU: 0x5555 = éxito, (código << 16) | 0x3333 = fallo.
pub fn qemu_exit(code: u32) {
    let v: u32 = if code == crate::hal::EXIT_SUCCESS { 0x5555 } else { (code << 16) | 0x3333 };
    // SAFETY: registro MMIO del dispositivo de pruebas de QEMU (identidad).
    unsafe { (TEST_DEVICE.load(Ordering::Relaxed) as *mut u32).write_volatile(v) };
}

pub fn describe_cpu(f: &mut dyn FnMut(&str, &str)) {
    let dtb = DTB.load(Ordering::Relaxed);
    f("ISA", crate::dtb::node_str(dtb, "cpu", "riscv,isa").unwrap_or("rv64gc"));
    f("MMU", crate::dtb::node_str(dtb, "cpu", "mmu-type").unwrap_or("?"));
    let mut b = crate::log::FixedBuf::<64>::new();
    let _ = core::fmt::Write::write_fmt(&mut b, format_args!("{} MHz", TIMEBASE.load(Ordering::Relaxed) / 1_000_000));
    f("base de tiempos", b.as_str());
    f("firmware", "OpenSBI (modo M) · núcleo en modo S");
}

pub fn paging_self_check() -> Option<(bool, bool, bool)> {
    None
}

/// PCIe por ECAM llega en la fase 2.
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
