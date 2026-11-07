//! # IDT, interrupciones y excepciones
//!
//! * 256 "stubs" en ensamblador, generados con `.rept`, de **exactamente 16
//!   bytes** cada uno: la dirección del stub `n` es `base + 16·n`, sin tabla
//!   de punteros. Cada stub iguala la pila (empuja un código de error falso
//!   si la CPU no lo pone) y el número de vector.
//! * Un único punto común guarda los 15 registros generales y llama a
//!   [`kopit_trap_dispatch`] con un [`TrapFrame`] completo: el mismo marco que
//!   usará el planificador para cambiar de contexto.
//! * Los manejadores se registran en tiempo de ejecución ([`register`]): un
//!   controlador pide su vector sin tocar este archivo.

use super::cpu;
use super::gdt;
use crate::sync::RacyCell;
use core::arch::{asm, global_asm};
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

/// Estado de la CPU en el momento de la interrupción.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct TrapFrame {
    pub r15: u64,
    pub r14: u64,
    pub r13: u64,
    pub r12: u64,
    pub r11: u64,
    pub r10: u64,
    pub r9: u64,
    pub r8: u64,
    pub rbp: u64,
    pub rdi: u64,
    pub rsi: u64,
    pub rdx: u64,
    pub rcx: u64,
    pub rbx: u64,
    pub rax: u64,
    pub vector: u64,
    pub error_code: u64,
    pub rip: u64,
    pub cs: u64,
    pub rflags: u64,
    pub rsp: u64,
    pub ss: u64,
}

global_asm!(
    r#"
    .section .text.isr, "ax"
    .balign 16
    .global kopit_isr_stubs
kopit_isr_stubs:
    .set kopit_vec, 0
    .rept 256
        .balign 16
        /* Estos vectores ya traen código de error de la CPU */
        .if (kopit_vec == 8) || (kopit_vec == 10) || (kopit_vec == 11) || (kopit_vec == 12) || (kopit_vec == 13) || (kopit_vec == 14) || (kopit_vec == 17) || (kopit_vec == 21) || (kopit_vec == 29) || (kopit_vec == 30)
        .else
            pushq $0
        .endif
        pushq $kopit_vec
        jmp kopit_isr_common
        .set kopit_vec, kopit_vec + 1
    .endr

kopit_isr_common:
    pushq %rax
    pushq %rbx
    pushq %rcx
    pushq %rdx
    pushq %rsi
    pushq %rdi
    pushq %rbp
    pushq %r8
    pushq %r9
    pushq %r10
    pushq %r11
    pushq %r12
    pushq %r13
    pushq %r14
    pushq %r15
    movq %rsp, %rdi
    movq %rsp, %rbx
    andq $-16, %rsp
    cld
    call kopit_trap_dispatch
    movq %rbx, %rsp
    popq %r15
    popq %r14
    popq %r13
    popq %r12
    popq %r11
    popq %r10
    popq %r9
    popq %r8
    popq %rbp
    popq %rdi
    popq %rsi
    popq %rdx
    popq %rcx
    popq %rbx
    popq %rax
    addq $16, %rsp
    iretq
    .section .text
"#,
    options(att_syntax)
);

unsafe extern "C" {
    safe static kopit_isr_stubs: u8;
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Gate {
    offset_low: u16,
    selector: u16,
    ist: u8,
    attributes: u8,
    offset_mid: u16,
    offset_high: u32,
    zero: u32,
}

static IDT: RacyCell<[Gate; 256]> = RacyCell::new(
    [Gate { offset_low: 0, selector: 0, ist: 0, attributes: 0, offset_mid: 0, offset_high: 0, zero: 0 }; 256],
);

pub type Handler = fn(&mut TrapFrame);
static HANDLERS: [AtomicUsize; 256] = [const { AtomicUsize::new(0) }; 256];
static BREAKPOINTS: AtomicU64 = AtomicU64::new(0);
static SPURIOUS: AtomicU64 = AtomicU64::new(0);

pub fn init() {
    let base = &raw const kopit_isr_stubs as u64;
    // SAFETY: arranque, un solo hilo.
    let idt = unsafe { &mut *IDT.get() };
    for (v, gate) in idt.iter_mut().enumerate() {
        let addr = base + 16 * v as u64;
        let ist = match v {
            8 => gdt::IST_DOUBLE_FAULT,
            2 => gdt::IST_NMI,
            18 => gdt::IST_MACHINE_CHECK,
            _ => 0,
        };
        // 0x8E: puerta de interrupción presente (IF=0 al entrar), DPL 0.
        // int3 con DPL 3 para que en el futuro funcione desde el usuario (depuradores).
        let attributes = if v == 3 { 0xEE } else { 0x8E };
        *gate = Gate {
            offset_low: addr as u16,
            selector: gdt::KERNEL_CODE,
            ist,
            attributes,
            offset_mid: (addr >> 16) as u16,
            offset_high: (addr >> 32) as u32,
            zero: 0,
        };
    }
    #[repr(C, packed)]
    struct Pointer {
        limit: u16,
        base: u64,
    }
    let ptr = Pointer { limit: (core::mem::size_of::<[Gate; 256]>() - 1) as u16, base: idt.as_ptr() as u64 };
    // SAFETY: la IDT es estática y válida.
    unsafe { asm!("lidt [{}]", in(reg) &ptr, options(readonly, nostack, preserves_flags)) };
    register(3, on_breakpoint);
    register(2, on_nmi);
}

/// Instala un manejador para un vector.
pub fn register(vector: u8, handler: Handler) {
    HANDLERS[vector as usize].store(handler as usize, Ordering::Release);
}

pub fn breakpoint_count() -> u64 {
    BREAKPOINTS.load(Ordering::Relaxed)
}

pub fn spurious_count() -> u64 {
    SPURIOUS.load(Ordering::Relaxed)
}

/// Punto común en Rust de todas las interrupciones y excepciones.
#[unsafe(no_mangle)]
extern "C" fn kopit_trap_dispatch(frame: &mut TrapFrame) {
    let v = frame.vector as usize;
    let h = HANDLERS[v].load(Ordering::Acquire);
    if h != 0 {
        // SAFETY: sólo se guardan punteros `Handler` en la tabla.
        let f: Handler = unsafe { core::mem::transmute::<usize, Handler>(h) };
        f(frame);
        return;
    }
    match v {
        0..=31 => fatal_exception(frame),
        // Vectores del PIC 8259 (enmascarado): sólo pueden ser espurios.
        0x20..=0x2F => {
            SPURIOUS.fetch_add(1, Ordering::Relaxed);
        }
        _ => {
            SPURIOUS.fetch_add(1, Ordering::Relaxed);
            super::apic::eoi();
        }
    }
}

const EXCEPTIONS: [&str; 32] = [
    "#DE división por cero",
    "#DB depuración",
    "NMI interrupción no enmascarable",
    "#BP punto de ruptura",
    "#OF desbordamiento",
    "#BR fuera de rango",
    "#UD instrucción no válida",
    "#NM coprocesador no disponible",
    "#DF doble fallo",
    "segmento de coprocesador",
    "#TS TSS no válido",
    "#NP segmento no presente",
    "#SS fallo de segmento de pila",
    "#GP protección general",
    "#PF fallo de página",
    "reservada",
    "#MF error de coma flotante x87",
    "#AC comprobación de alineación",
    "#MC comprobación de máquina",
    "#XM excepción SIMD",
    "#VE excepción de virtualización",
    "#CP protección de flujo de control",
    "reservada",
    "reservada",
    "reservada",
    "reservada",
    "reservada",
    "reservada",
    "#HV inyección del hipervisor",
    "#VC comunicación con el VMM",
    "#SX seguridad",
    "reservada",
];

fn on_breakpoint(frame: &mut TrapFrame) {
    BREAKPOINTS.fetch_add(1, Ordering::Relaxed);
    kinfo!("punto de ruptura (int3) en {:#x}; se continúa", frame.rip - 1);
}

fn on_nmi(frame: &mut TrapFrame) {
    kwarn!("NMI recibida en {:#x} (posible error de hardware)", frame.rip);
}

/// Decodifica el código de error de un fallo de página.
fn page_fault_reason(code: u64) -> (&'static str, &'static str, &'static str) {
    (
        if code & 1 != 0 { "violación de permisos" } else { "página no presente" },
        if code & 0x10 != 0 {
            "ejecución"
        } else if code & 2 != 0 {
            "escritura"
        } else {
            "lectura"
        },
        if code & 4 != 0 { "usuario" } else { "núcleo" },
    )
}

fn fatal_exception(f: &mut TrapFrame) -> ! {
    let name = EXCEPTIONS[f.vector as usize];
    let e = |args: core::fmt::Arguments<'_>| crate::log::emergency(args);
    e(format_args!("\n\x1b[1;31m*** EXCEPCIÓN DE CPU: {} (vector {}) ***\x1b[0m\n", name, f.vector));
    e(format_args!("  RIP={:016x} CS={:04x} RFLAGS={:016x} código={:#x}\n", f.rip, f.cs, f.rflags, f.error_code));
    e(format_args!("  RSP={:016x} SS={:04x} CR3={:016x}\n", f.rsp, f.ss, cpu::read_cr3()));
    e(format_args!("  RAX={:016x} RBX={:016x} RCX={:016x} RDX={:016x}\n", f.rax, f.rbx, f.rcx, f.rdx));
    e(format_args!("  RSI={:016x} RDI={:016x} RBP={:016x} R8 ={:016x}\n", f.rsi, f.rdi, f.rbp, f.r8));
    e(format_args!("  R9 ={:016x} R10={:016x} R11={:016x} R12={:016x}\n", f.r9, f.r10, f.r11, f.r12));
    e(format_args!("  R13={:016x} R14={:016x} R15={:016x}\n", f.r13, f.r14, f.r15));
    match f.vector {
        14 => {
            let cr2 = cpu::read_cr2();
            let (what, access, mode) = page_fault_reason(f.error_code);
            let guard = crate::mm::layout::stack_guard();
            if (guard..guard + 4096).contains(&cr2) {
                panic!("desbordamiento de la pila del núcleo (fallo en la página de guarda {:#x})", cr2);
            }
            panic!("{} en {:#x}: {} en modo {} ({}) desde RIP {:#x}", name, cr2, access, mode, what, f.rip);
        }
        8 => {
            let cr2 = cpu::read_cr2();
            let guard = crate::mm::layout::stack_guard();
            if (guard..guard + 4096).contains(&cr2) {
                panic!("doble fallo por desbordamiento de la pila del núcleo (CR2={:#x})", cr2);
            }
            panic!("{} (RIP {:#x}, CR2 {:#x})", name, f.rip, cr2);
        }
        _ => panic!("{} en RIP {:#x} (código de error {:#x})", name, f.rip, f.error_code),
    }
}
