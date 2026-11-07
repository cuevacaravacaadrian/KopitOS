//! # Pruebas de humo integradas (`test=smoke`)
//!
//! `cargo kopit test` arranca cada imagen en QEMU con `test=smoke`. El núcleo
//! comprueba sus subsistemas de verdad, en hardware emulado, imprime
//! `KOPIT-SMOKE: OK` y sale de QEMU con un código de salida que la
//! herramienta interpreta (dispositivo `isa-debug-exit`). Si algo entra en
//! pánico, también sale de QEMU, con código de error: la integración continua
//! nunca se queda colgada.

use crate::console::Tone;
use crate::log::FixedBuf;
use crate::{console, hal, mm, time};
use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt::Write;

use crate::hal::{EXIT_FAILURE, EXIT_SUCCESS};

struct Report {
    passed: u32,
    failed: u32,
}

impl Report {
    fn check(&mut self, name: &str, ok: bool, detail: core::fmt::Arguments<'_>) {
        let mut line = FixedBuf::<256>::new();
        let _ = write!(line, "  {} {:<34} {}\n", if ok { "✓" } else { "✗" }, name, detail);
        console::write(if ok { Tone::Success } else { Tone::Error }, line.as_str());
        if ok {
            self.passed += 1;
        } else {
            self.failed += 1;
        }
    }
}

pub fn run_and_exit() -> ! {
    console::write(Tone::Accent, "\nPruebas de humo del núcleo:\n");
    let mut r = Report { passed: 0, failed: 0 };

    // 1. Excepciones: int3 debe volver.
    let before = crate::arch::breakpoint_count();
    hal::breakpoint();
    let after = crate::arch::breakpoint_count();
    r.check("excepción de depuración y retorno", after == before + 1, format_args!("{} → {}", before, after));

    // 2. Marcos físicos: asignar, escribir, liberar.
    let free0 = mm::pmm::stats().map(|s| s.free_bytes).unwrap_or(0);
    let mut ok = true;
    let mut frames = Vec::new();
    for order in 0..6 {
        match mm::pmm::alloc_frames(order) {
            Some(f) => {
                let p = mm::phys_to_virt(f) as *mut u64;
                // SAFETY: bloque recién asignado y mapeado en el HHDM.
                unsafe {
                    p.write_volatile(0xC0FFEE ^ f);
                    ok &= p.read_volatile() == 0xC0FFEE ^ f;
                }
                ok &= f % (4096 << order) == 0;
                frames.push((f, order));
            }
            None => ok = false,
        }
    }
    for (f, o) in frames {
        mm::pmm::free_frames(f, o);
    }
    let free1 = mm::pmm::stats().map(|s| s.free_bytes).unwrap_or(0);
    r.check("buddy: órdenes 0-5 alineados", ok && free0 == free1, format_args!("{} KiB libres", free1 >> 10));

    // 3. Montículo: Vec, Box, String, BTreeMap.
    let mut v: Vec<u32> = (0..50_000).collect();
    v.retain(|x| x % 3 == 0);
    let b = Box::new([0x5Au8; 3000]);
    let mut s = String::new();
    let _ = write!(s, "KopitOS {}", 2026);
    let mut m = BTreeMap::new();
    for i in 0..1000 {
        m.insert(i * 7 % 1000, i);
    }
    let heap_ok = v.len() == 16_667 && b[2999] == 0x5A && s == "KopitOS 2026" && m.len() == 1000;
    drop((v, b, s, m));
    r.check(
        "montículo: Vec/Box/String/BTreeMap",
        heap_ok,
        format_args!("{} KiB", mm::heap::stats().total_bytes() >> 10),
    );

    // 4. Paginación: W^X real.
    match crate::arch::paging_self_check() {
        Some((text_ro, data_nx, guard_unmapped)) => r.check(
            "paginación W^X + página de guarda",
            text_ro && data_nx && guard_unmapped,
            format_args!("texto r-x:{} datos nx:{} guarda:{}", text_ro, data_nx, guard_unmapped),
        ),
        None => r.check("paginación W^X + página de guarda", true, format_args!("(no aplica en esta arquitectura)")),
    }

    // 5. Temporizador: deben llegar tics.
    let t0 = time::ticks();
    let deadline = time::uptime_ms() + 500;
    while time::ticks() < t0 + 5 && time::uptime_ms() < deadline {
        hal::wait_for_interrupt();
    }
    let t1 = time::ticks();
    r.check("temporizador e interrupciones", t1 >= t0 + 5, format_args!("{} tics", t1 - t0));

    // 6. Plan de servicios para todos los dispositivos.
    let classes = ["desktop", "laptop", "server", "tablet", "phone", "iot"];
    let plans_ok = classes.iter().all(|c| kopit_init::plan(kopit_init::DEFAULT_SERVICES, c).is_ok());
    r.check("sys_init: planes de las 6 clases", plans_ok, format_args!(""));

    let mut summary = FixedBuf::<96>::new();
    let ok = r.failed == 0;
    let _ = write!(
        summary,
        "\nKOPIT-SMOKE: {} ({}/{} pruebas)\n",
        if ok { "OK" } else { "FALLO" },
        r.passed,
        r.passed + r.failed
    );
    console::write(if ok { Tone::Success } else { Tone::Error }, summary.as_str());
    crate::arch::qemu_exit(if ok { EXIT_SUCCESS } else { EXIT_FAILURE });
    hal::halt()
}

/// Llamado desde el pánico: en modo prueba, salir de QEMU con error.
pub fn exit_on_panic() {
    if crate::cmdline::get("test") == Some("smoke") {
        crate::log::emergency(format_args!("\nKOPIT-SMOKE: PÁNICO\n"));
        crate::arch::qemu_exit(EXIT_FAILURE);
    }
}
