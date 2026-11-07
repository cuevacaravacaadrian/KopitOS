//! # Pánico del núcleo
//!
//! Un pánico es un error del que el núcleo no puede recuperarse. Objetivo:
//! dejar **toda la información posible** y no empeorar las cosas:
//!
//! 1. desactivar interrupciones;
//! 2. contarlo por la serie *sin cerrojos* (puede que el pánico ocurriera con
//!    el cerrojo de la serie tomado);
//! 3. traza de la pila por punteros de marco (`kopit addr2line` la traduce);
//! 4. recuadro rojo en pantalla;
//! 5. si es una prueba automática, salir de QEMU con código de error;
//! 6. detener la CPU.

use crate::hal::{self, Arch};
use crate::log::FixedBuf;
use core::fmt::Write;
use core::panic::PanicInfo;
use core::sync::atomic::{AtomicBool, Ordering};

static PANICKING: AtomicBool = AtomicBool::new(false);

pub fn is_panicking() -> bool {
    PANICKING.load(Ordering::Relaxed)
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    hal::disable_interrupts();
    if PANICKING.swap(true, Ordering::SeqCst) {
        // Pánico dentro del pánico: lo mínimo y parar.
        crate::log::emergency(format_args!("\n*** pánico anidado: {}\n", info.message()));
        hal::halt();
    }
    // SAFETY: no volveremos; nadie más usará estos cerrojos.
    unsafe {
        hal::Current::serial_force_unlock();
        crate::log::force_unlock();
    }

    let mut message = FixedBuf::<256>::new();
    let _ = write!(message, "{}", info.message());
    let mut location = FixedBuf::<128>::new();
    if let Some(l) = info.location() {
        let _ = write!(location, "{}:{}:{}", l.file(), l.line(), l.column());
    }
    crate::log::emergency(format_args!(
        "\n\x1b[1;31m══════ PÁNICO DEL NÚCLEO ══════\x1b[0m\n  {}\n  en {}\n  tras {} ms de funcionamiento\n  traza:",
        message.as_str(),
        location.as_str(),
        crate::time::uptime_ms()
    ));

    let mut frames = [0u64; 12];
    let mut count = 0;
    hal::backtrace(&mut |addr| {
        if count < frames.len() {
            frames[count] = addr;
            count += 1;
        }
    });
    let mut trace = FixedBuf::<256>::new();
    for f in &frames[..count] {
        crate::log::emergency(format_args!(" {:#x}", f));
        let _ = write!(trace, "{:x} ", f);
    }
    crate::log::emergency(format_args!("\n  (tradúcela con: cargo kopit addr2line <direcciones>)\n"));

    let mut uptime = FixedBuf::<64>::new();
    let _ = write!(uptime, "Tiempo en marcha: {} ms", crate::time::uptime_ms());
    let lines = [
        message.as_str(),
        "",
        location.as_str(),
        uptime.as_str(),
        "",
        "Traza:",
        trace.as_str(),
        "",
        "El sistema se ha detenido. Reinicia el equipo.",
    ];
    crate::console::panic_screen("■ PÁNICO DEL NÚCLEO", &lines);

    #[cfg(feature = "selftest")]
    crate::selftest::exit_on_panic();

    hal::halt()
}
