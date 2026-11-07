//! # Registro del núcleo (`klog`)
//!
//! ```text
//!  kinfo!("...")  ──►  Record { nivel, tiempo, módulo, mensaje }
//!                         │
//!                         ├──► anillo de 64 KiB  (dmesg: siempre, hasta DEPUR)
//!                         ├──► destino "serie"     (con colores ANSI)
//!                         └──► destino "pantalla"  (consola gráfica / VGA)
//! ```
//!
//! Decisiones:
//!
//! * **Sin memoria dinámica**: el mensaje se formatea en un búfer fijo de la
//!   pila. El registro funciona desde la primera instrucción de Rust, antes
//!   de que exista el montículo, y dentro de un pánico.
//! * **Cada destino tiene su nivel** (`loglevel=` para la pantalla,
//!   `serial.loglevel=` para la serie): la pantalla se queda limpia y la serie
//!   lo cuenta todo.
//! * **Reentrada segura**: si una excepción ocurre *mientras* se registra
//!   algo (el cerrojo ya está tomado por esta CPU), el mensaje sale por la ruta
//!   de emergencia directa a la UART en vez de bloquearse.

use crate::sync::IrqLock;
use core::fmt::{self, Write};
use core::sync::atomic::{AtomicU8, Ordering};

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[repr(u8)]
pub enum Level {
    Error = 1,
    Warn = 2,
    Info = 3,
    Debug = 4,
    Trace = 5,
}

impl Level {
    pub const fn tag(self) -> &'static str {
        match self {
            Level::Error => "ERROR",
            Level::Warn => "AVISO",
            Level::Info => "INFO ",
            Level::Debug => "DEPUR",
            Level::Trace => "TRAZA",
        }
    }

    pub const fn name(self) -> &'static str {
        match self {
            Level::Error => "error",
            Level::Warn => "aviso",
            Level::Info => "info",
            Level::Debug => "depuración",
            Level::Trace => "traza",
        }
    }

    pub fn parse(s: &str) -> Option<Level> {
        Some(match s {
            "error" | "err" => Level::Error,
            "warn" | "warning" | "aviso" => Level::Warn,
            "info" => Level::Info,
            "debug" | "depur" | "depuracion" => Level::Debug,
            "trace" | "traza" => Level::Trace,
            _ => return None,
        })
    }

    const fn from_u8(v: u8) -> Level {
        match v {
            1 => Level::Error,
            2 => Level::Warn,
            3 => Level::Info,
            4 => Level::Debug,
            _ => Level::Trace,
        }
    }

    /// Color ANSI para terminales serie.
    pub const fn ansi(self) -> &'static str {
        match self {
            Level::Error => "\x1b[1;31m",
            Level::Warn => "\x1b[33m",
            Level::Info => "\x1b[32m",
            Level::Debug => "\x1b[36m",
            Level::Trace => "\x1b[90m",
        }
    }
}

/// Texto de capacidad fija que implementa `fmt::Write` (trunca si no cabe).
#[derive(Clone, Copy)]
pub struct FixedBuf<const N: usize> {
    buf: [u8; N],
    len: usize,
}

impl<const N: usize> FixedBuf<N> {
    pub const fn new() -> Self {
        Self { buf: [0; N], len: 0 }
    }

    pub fn as_str(&self) -> &str {
        // SAFETY: sólo se copian cadenas UTF-8 completas (se corta en límites de carácter).
        unsafe { core::str::from_utf8_unchecked(&self.buf[..self.len]) }
    }

    pub fn clear(&mut self) {
        self.len = 0;
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn set(&mut self, s: &str) {
        self.clear();
        let _ = self.write_str(s);
    }
}

impl<const N: usize> fmt::Write for FixedBuf<N> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let room = N - self.len;
        let mut take = s.len().min(room);
        while take > 0 && !s.is_char_boundary(take) {
            take -= 1;
        }
        self.buf[self.len..self.len + take].copy_from_slice(&s.as_bytes()[..take]);
        self.len += take;
        Ok(())
    }
}

/// Una entrada del registro.
pub struct Record<'a> {
    pub level: Level,
    pub time_ns: u64,
    pub module: &'a str,
    pub message: &'a str,
}

pub type SinkFn = fn(&Record<'_>);

#[derive(Clone, Copy)]
struct Sink {
    name: &'static str,
    func: SinkFn,
    level: Level,
}

const RING_SIZE: usize = 64 * 1024;

/// Anillo de texto: siempre conserva lo último que se registró.
struct Ring {
    buf: [u8; RING_SIZE],
    write: usize,
    wrapped: bool,
}

impl Ring {
    const fn new() -> Self {
        Self { buf: [0; RING_SIZE], write: 0, wrapped: false }
    }

    fn push(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.buf[self.write] = b;
            self.write += 1;
            if self.write == RING_SIZE {
                self.write = 0;
                self.wrapped = true;
            }
        }
    }

    /// Recorre el contenido de lo más antiguo a lo más reciente.
    fn for_each_chunk(&self, mut f: impl FnMut(&[u8])) {
        if self.wrapped {
            let old = &self.buf[self.write..];
            // La primera línea puede estar cortada: se salta hasta el primer salto.
            let start = old.iter().position(|&b| b == b'\n').map(|p| p + 1).unwrap_or(old.len());
            f(&old[start..]);
        }
        f(&self.buf[..self.write]);
    }
}

struct Logger {
    ring: Ring,
    sinks: [Option<Sink>; 4],
    records: u64,
}

static LOGGER: IrqLock<Logger> = IrqLock::new(Logger { ring: Ring::new(), sinks: [None; 4], records: 0 });
/// Nivel máximo que interesa a alguien (filtro rápido sin cerrojo).
static MAX_LEVEL: AtomicU8 = AtomicU8::new(Level::Debug as u8);
/// Nivel que guarda el anillo de `dmesg`.
static RING_LEVEL: AtomicU8 = AtomicU8::new(Level::Debug as u8);

fn recompute_max(logger: &Logger) {
    let mut max = RING_LEVEL.load(Ordering::Relaxed);
    for s in logger.sinks.iter().flatten() {
        max = max.max(s.level as u8);
    }
    MAX_LEVEL.store(max, Ordering::Relaxed);
}

/// Añade (o sustituye, por nombre) un destino del registro.
pub fn register_sink(name: &'static str, func: SinkFn, level: Level) {
    let mut logger = LOGGER.lock();
    let slot = logger
        .sinks
        .iter()
        .position(|s| s.is_some_and(|s| s.name == name))
        .or_else(|| logger.sinks.iter().position(|s| s.is_none()));
    if let Some(i) = slot {
        logger.sinks[i] = Some(Sink { name, func, level });
    }
    recompute_max(&logger);
}

pub fn set_sink_level(name: &str, level: Level) -> bool {
    let mut logger = LOGGER.lock();
    let mut found = false;
    for s in logger.sinks.iter_mut().flatten() {
        if s.name == name {
            s.level = level;
            found = true;
        }
    }
    recompute_max(&logger);
    found
}

pub fn sink_levels(mut f: impl FnMut(&'static str, Level)) {
    let logger = LOGGER.lock();
    for s in logger.sinks.iter().flatten() {
        f(s.name, s.level);
    }
}

pub fn set_ring_level(level: Level) {
    RING_LEVEL.store(level as u8, Ordering::Relaxed);
    recompute_max(&LOGGER.lock());
}

pub fn record_count() -> u64 {
    LOGGER.lock().records
}

/// `kopit_kernel::mm::pmm` → `mm::pmm`.
pub fn short_module(module: &str) -> &str {
    module.strip_prefix("kopit_kernel::").unwrap_or(module)
}

/// Escribe el prefijo `[    1.234567] INFO  mm::pmm: `.
pub fn write_prefix(out: &mut impl fmt::Write, rec: &Record<'_>) {
    let secs = rec.time_ns / 1_000_000_000;
    let micros = (rec.time_ns % 1_000_000_000) / 1000;
    let _ = write!(out, "[{:>5}.{:06}] {} {}: ", secs, micros, rec.level.tag(), rec.module);
}

pub fn log(level: Level, module: &'static str, args: fmt::Arguments<'_>) {
    if level as u8 > MAX_LEVEL.load(Ordering::Relaxed) {
        return;
    }
    let mut msg = FixedBuf::<480>::new();
    let _ = msg.write_fmt(args);
    let rec = Record { level, time_ns: crate::time::uptime_ns(), module: short_module(module), message: msg.as_str() };

    // Fase 1 = una CPU: si el cerrojo está ocupado es porque *esta* CPU está
    // dentro del registro (una excepción en medio). No se puede esperar.
    let Some(mut logger) = LOGGER.try_lock() else {
        emergency(format_args!("{}{}\n", Prefix(&rec), rec.message));
        return;
    };
    logger.records += 1;
    if level as u8 <= RING_LEVEL.load(Ordering::Relaxed) {
        let mut line = FixedBuf::<600>::new();
        write_prefix(&mut line, &rec);
        let _ = line.write_str(rec.message);
        let _ = line.write_str("\n");
        logger.ring.push(line.as_str().as_bytes());
    }
    let sinks = logger.sinks;
    for s in sinks.iter().flatten() {
        if level <= s.level {
            (s.func)(&rec);
        }
    }
}

struct Prefix<'a, 'b>(&'a Record<'b>);

impl fmt::Display for Prefix<'_, '_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write_prefix(f, self.0);
        Ok(())
    }
}

/// Salida de emergencia: directa a la UART, sin pasar por el registro.
pub fn emergency(args: fmt::Arguments<'_>) {
    struct Raw;
    impl fmt::Write for Raw {
        fn write_str(&mut self, s: &str) -> fmt::Result {
            crate::hal::serial_write(s.as_bytes());
            Ok(())
        }
    }
    let _ = Raw.write_fmt(args);
}

/// Recorre el anillo (para `dmesg`).
pub fn dump(mut f: impl FnMut(&str)) {
    let logger = LOGGER.lock();
    logger.ring.for_each_chunk(|chunk| {
        let text = core::str::from_utf8(chunk).unwrap_or("");
        for line in text.split_inclusive('\n') {
            f(line);
        }
    });
}

/// Nivel actual de un destino.
pub fn level_of(name: &str) -> Option<Level> {
    let logger = LOGGER.lock();
    logger.sinks.iter().flatten().find(|s| s.name == name).map(|s| s.level)
}

/// Fuerza la liberación del cerrojo del registro (sólo en un pánico).
///
/// # Safety
/// No volverá a ejecutarse quien lo tenía.
pub unsafe fn force_unlock() {
    // SAFETY: lo garantiza quien llama.
    unsafe { LOGGER.force_unlock() }
}

pub fn max_level() -> Level {
    Level::from_u8(MAX_LEVEL.load(Ordering::Relaxed))
}

#[macro_export]
macro_rules! klog {
    ($lvl:expr, $($arg:tt)+) => {
        $crate::log::log($lvl, module_path!(), format_args!($($arg)+))
    };
}

#[macro_export]
macro_rules! kerror { ($($arg:tt)+) => { $crate::klog!($crate::log::Level::Error, $($arg)+) }; }
#[macro_export]
macro_rules! kwarn { ($($arg:tt)+) => { $crate::klog!($crate::log::Level::Warn, $($arg)+) }; }
#[macro_export]
macro_rules! kinfo { ($($arg:tt)+) => { $crate::klog!($crate::log::Level::Info, $($arg)+) }; }
#[macro_export]
macro_rules! kdebug { ($($arg:tt)+) => { $crate::klog!($crate::log::Level::Debug, $($arg)+) }; }
#[macro_export]
macro_rules! ktrace { ($($arg:tt)+) => { $crate::klog!($crate::log::Level::Trace, $($arg)+) }; }

/// Texto directo a la consola (pantalla + serie), sin pasar por el registro.
#[macro_export]
macro_rules! kprint {
    ($($arg:tt)*) => { $crate::console::print(format_args!($($arg)*)) };
}

#[macro_export]
macro_rules! kprintln {
    () => { $crate::console::print(format_args!("\n")) };
    ($($arg:tt)*) => { $crate::console::print(format_args!("{}\n", format_args!($($arg)*))) };
}
