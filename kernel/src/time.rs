//! # Tiempo
//!
//! * **Reloj monotónico** en nanosegundos desde el arranque, basado en el
//!   contador de ciclos de la CPU (TSC en x86_64, `time` en RISC-V,
//!   `CNTVCT_EL0` en ARM64) calibrado una vez. Leerlo no cuesta una
//!   interrupción: sirve para marcar el tiempo de cada línea del registro.
//! * **Tics**: los cuenta el temporizador periódico (base del futuro
//!   planificador; en fase 2 el núcleo será *tickless* en reposo para ahorrar
//!   energía).
//! * **Fecha civil**: hora del firmware (UEFI o CMOS) + tiempo transcurrido.

use crate::hal;
use core::sync::atomic::{AtomicI64, AtomicU32, AtomicU64, Ordering};
use kopit_boot_proto::WallClock;

static CYCLE_HZ: AtomicU64 = AtomicU64::new(0);
static BOOT_CYCLES: AtomicU64 = AtomicU64::new(0);
static TICKS: AtomicU64 = AtomicU64::new(0);
static TICK_HZ: AtomicU32 = AtomicU32::new(0);
/// Segundos Unix en el instante de `BOOT_CYCLES` (i64::MIN = desconocido).
static BOOT_UNIX: AtomicI64 = AtomicI64::new(i64::MIN);

/// Origen de tiempos: el contador de ciclos en la primera instrucción del núcleo.
pub fn mark_boot(cycles: u64) {
    BOOT_CYCLES.store(cycles, Ordering::Relaxed);
}

pub fn boot_cycles() -> u64 {
    BOOT_CYCLES.load(Ordering::Relaxed)
}

/// Frecuencia calibrada del contador de ciclos (la fija la arquitectura).
pub fn set_cycle_frequency(hz: u64) {
    CYCLE_HZ.store(hz, Ordering::Release);
}

pub fn cycle_frequency() -> u64 {
    CYCLE_HZ.load(Ordering::Acquire)
}

pub fn cycles_to_ns(cycles: u64) -> u64 {
    let hz = cycle_frequency();
    if hz == 0 {
        return 0;
    }
    ((cycles as u128 * 1_000_000_000) / hz as u128) as u64
}

/// Nanosegundos desde que el núcleo tomó el control (0 hasta calibrar).
pub fn uptime_ns() -> u64 {
    cycles_to_ns(hal::cycles().wrapping_sub(BOOT_CYCLES.load(Ordering::Relaxed)))
}

pub fn uptime_ms() -> u64 {
    uptime_ns() / 1_000_000
}

/// Llamado por el temporizador en cada tic.
pub fn on_tick() {
    TICKS.fetch_add(1, Ordering::Relaxed);
}

pub fn ticks() -> u64 {
    TICKS.load(Ordering::Relaxed)
}

pub fn set_tick_hz(hz: u32) {
    TICK_HZ.store(hz, Ordering::Relaxed);
}

pub fn tick_hz() -> u32 {
    TICK_HZ.load(Ordering::Relaxed)
}

/// Espera activa (antes de tener temporizador o en contextos sin interrupciones).
pub fn spin_ms(ms: u64) {
    let hz = cycle_frequency();
    if hz == 0 {
        for _ in 0..ms * 100_000 {
            core::hint::spin_loop();
        }
        return;
    }
    let target = hal::cycles() + hz / 1000 * ms;
    while hal::cycles() < target {
        core::hint::spin_loop();
    }
}

// --- Fecha civil ---------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DateTime {
    pub year: i64,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
}

/// Días desde 1970-01-01 (algoritmo de Howard Hinnant, válido para siempre).
pub fn days_from_civil(y: i64, m: u8, d: u8) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = (if y >= 0 { y } else { y - 399 }) / 400;
    let yoe = y - era * 400;
    let m = m as i64;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

pub fn civil_from_days(z: i64) -> (i64, u8, u8) {
    let z = z + 719_468;
    let era = (if z >= 0 { z } else { z - 146_096 }) / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u8;
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u8;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Registra la hora de arranque que dio el firmware.
pub fn set_wall_clock(w: &WallClock) {
    if w.valid == 0 || w.year < 2000 || !(1..=12).contains(&w.month) || w.day == 0 {
        return;
    }
    let days = days_from_civil(w.year as i64, w.month, w.day);
    let secs = days * 86_400 + w.hour as i64 * 3600 + w.minute as i64 * 60 + w.second as i64;
    // Corregimos por el tiempo que ya llevamos en marcha.
    let elapsed = (uptime_ns() / 1_000_000_000) as i64;
    BOOT_UNIX.store(secs - elapsed, Ordering::Relaxed);
}

pub fn has_wall_clock() -> bool {
    BOOT_UNIX.load(Ordering::Relaxed) != i64::MIN
}

pub fn unix_now() -> Option<i64> {
    let base = BOOT_UNIX.load(Ordering::Relaxed);
    (base != i64::MIN).then(|| base + (uptime_ns() / 1_000_000_000) as i64)
}

pub fn now() -> Option<DateTime> {
    let t = unix_now()?;
    let days = t.div_euclid(86_400);
    let rem = t.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    Some(DateTime {
        year,
        month,
        day,
        hour: (rem / 3600) as u8,
        minute: (rem % 3600 / 60) as u8,
        second: (rem % 60) as u8,
    })
}

/// Día de la semana en español (0 = lunes).
pub fn weekday_name(dt: &DateTime) -> &'static str {
    const NAMES: [&str; 7] = ["lun", "mar", "mié", "jue", "vie", "sáb", "dom"];
    // 1970-01-01 fue jueves (índice 3).
    let days = days_from_civil(dt.year, dt.month, dt.day);
    NAMES[((days + 3).rem_euclid(7)) as usize]
}
