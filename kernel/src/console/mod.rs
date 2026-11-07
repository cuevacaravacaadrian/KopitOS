//! # Consola del núcleo
//!
//! Un multiplexor sencillo: todo lo que se escribe va a la **pantalla**
//! (framebuffer gráfico o, en arranques BIOS/PVH, el modo texto VGA) y al
//! **puerto serie**. El texto lleva un [`Tone`] semántico que cada destino
//! traduce a su manera: color del tema en la pantalla, código ANSI en la serie.

#[cfg(feature = "fbcon")]
mod fbcon;
#[cfg(target_arch = "x86_64")]
mod vgatext;

use crate::log::{self, FixedBuf, Level, Record};
use crate::sync::IrqLock;
use core::fmt::{self, Write};
use core::sync::atomic::{AtomicBool, Ordering};
use kopit_boot_proto::{BootInfo, FramebufferKind};
use kopit_theme::{Rgb, Theme};

/// Significado del texto (no su color).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tone {
    Normal,
    Muted,
    Accent,
    Success,
    Warning,
    Error,
    Info,
    Debug,
    Trace,
}

impl Tone {
    pub fn of_level(l: Level) -> Tone {
        match l {
            Level::Error => Tone::Error,
            Level::Warn => Tone::Warning,
            Level::Info => Tone::Info,
            Level::Debug => Tone::Debug,
            Level::Trace => Tone::Trace,
        }
    }

    pub fn color(self, t: &Theme) -> Rgb {
        match self {
            Tone::Normal => t.foreground,
            Tone::Muted => t.muted,
            Tone::Accent => t.accent,
            Tone::Success => t.success,
            Tone::Warning => t.warning,
            Tone::Error => t.error,
            Tone::Info => t.info,
            Tone::Debug => t.debug,
            Tone::Trace => t.trace,
        }
    }

    fn ansi(self) -> &'static str {
        match self {
            Tone::Normal => "\x1b[0m",
            Tone::Muted | Tone::Trace => "\x1b[90m",
            Tone::Accent => "\x1b[1;34m",
            Tone::Success | Tone::Info => "\x1b[32m",
            Tone::Warning => "\x1b[33m",
            Tone::Error => "\x1b[1;31m",
            Tone::Debug => "\x1b[36m",
        }
    }
}

enum Screen {
    None,
    #[cfg(feature = "fbcon")]
    Fb(fbcon::FbCon),
    #[cfg(target_arch = "x86_64")]
    Vga(vgatext::VgaText),
}

static SCREEN: IrqLock<Screen> = IrqLock::new(Screen::None);
static THEME: IrqLock<Theme> = IrqLock::new(kopit_theme::KOPIT_NOCHE);
static SERIAL_COLOR: AtomicBool = AtomicBool::new(true);

/// Datos de la pantalla para `info`.
#[derive(Clone, Copy, Debug)]
pub struct ScreenInfo {
    pub kind: &'static str,
    pub width: usize,
    pub height: usize,
    pub cols: usize,
    pub rows: usize,
    pub scale: usize,
    pub double_buffered: bool,
}

/// Elige la pantalla y registra los destinos del registro.
pub fn init(boot: &BootInfo, theme: Theme) {
    SERIAL_COLOR.store(crate::cmdline::get("serial.color") != Some("0"), Ordering::Relaxed);
    *THEME.lock() = theme;
    let fb = boot.framebuffer;
    let mut screen = SCREEN.lock();
    match fb.kind {
        #[cfg(feature = "fbcon")]
        FramebufferKind::RGB if fb.base != 0 && fb.bpp == 32 => {
            let scale = crate::cmdline::get("fbcon.scale").and_then(|s| s.parse().ok()).filter(|s| (1..=4).contains(s));
            let base = crate::mm::phys_to_virt(fb.base) as *mut u8;
            *screen = Screen::Fb(fbcon::FbCon::new(base, &fb, theme, scale));
        }
        #[cfg(target_arch = "x86_64")]
        FramebufferKind::EGA_TEXT => *screen = Screen::Vga(vgatext::VgaText::new()),
        #[cfg(target_arch = "x86_64")]
        FramebufferKind::NONE if boot.source != kopit_boot_proto::BootSource::KOPITBOOT_UEFI => {
            // BIOS / PVH: el firmware dejó el modo texto 80x25.
            *screen = Screen::Vga(vgatext::VgaText::new());
        }
        _ => {}
    }
    drop(screen);

    let screen_level = crate::cmdline::get("loglevel").and_then(Level::parse).unwrap_or(Level::Info);
    let serial_level = crate::cmdline::get("serial.loglevel").and_then(Level::parse).unwrap_or(Level::Debug);
    log::register_sink("serie", serial_sink, serial_level);
    log::register_sink("pantalla", screen_sink, screen_level);
}

fn with_screen<R>(f: impl FnOnce(&mut Screen, &Theme) -> R) -> R {
    let theme = *THEME.lock();
    let mut screen = SCREEN.lock();
    f(&mut screen, &theme)
}

fn screen_write(screen: &mut Screen, theme: &Theme, tone: Tone, s: &str) {
    match screen {
        Screen::None => {}
        #[cfg(feature = "fbcon")]
        Screen::Fb(fb) => fb.write(s, tone.color(theme)),
        #[cfg(target_arch = "x86_64")]
        Screen::Vga(v) => {
            let _ = theme;
            v.write(s, tone);
        }
    }
}

fn serial_write_toned(tone: Tone, s: &str) {
    let color = SERIAL_COLOR.load(Ordering::Relaxed) && tone != Tone::Normal;
    if color {
        crate::hal::serial_write(tone.ansi().as_bytes());
    }
    crate::hal::serial_write(s.as_bytes());
    if color {
        crate::hal::serial_write(b"\x1b[0m");
    }
}

/// Escribe texto con un tono en pantalla y serie.
pub fn write(tone: Tone, s: &str) {
    with_screen(|screen, theme| screen_write(screen, theme, tone, s));
    serial_write_toned(tone, s);
}

/// Sólo en pantalla (p. ej. el eco del teclado ya llega por serie).
pub fn write_screen(tone: Tone, s: &str) {
    with_screen(|screen, theme| screen_write(screen, theme, tone, s));
}

/// Destino de `kprint!`.
pub fn print(args: fmt::Arguments<'_>) {
    let mut buf = FixedBuf::<1024>::new();
    let _ = buf.write_fmt(args);
    write(Tone::Normal, buf.as_str());
}

fn serial_sink(rec: &Record<'_>) {
    let mut line = FixedBuf::<640>::new();
    if SERIAL_COLOR.load(Ordering::Relaxed) {
        let secs = rec.time_ns / 1_000_000_000;
        let micros = (rec.time_ns % 1_000_000_000) / 1000;
        let _ = write!(
            line,
            "\x1b[90m[{:>5}.{:06}]\x1b[0m {}{}\x1b[0m \x1b[90m{}:\x1b[0m {}\n",
            secs,
            micros,
            rec.level.ansi(),
            rec.level.tag(),
            rec.module,
            rec.message
        );
    } else {
        log::write_prefix(&mut line, rec);
        let _ = writeln!(line, "{}", rec.message);
    }
    crate::hal::serial_write(line.as_str().as_bytes());
}

/// Sangría colgante de las líneas largas (sólo la consola gráfica parte por palabras).
fn set_wrap_indent(screen: &mut Screen, cols: usize) {
    #[cfg(feature = "fbcon")]
    if let Screen::Fb(fb) = screen {
        fb.set_wrap_indent(cols);
    }
    let _ = (screen, cols);
}

fn screen_sink(rec: &Record<'_>) {
    with_screen(|screen, theme| {
        if matches!(screen, Screen::None) {
            return;
        }
        // Las continuaciones se alinean después de "   0.022 INFO ".
        set_wrap_indent(screen, 15);
        let mut stamp = FixedBuf::<24>::new();
        let ms = rec.time_ns / 1_000_000;
        let _ = write!(stamp, "{:>4}.{:03} ", ms / 1000, ms % 1000);
        screen_write(screen, theme, Tone::Muted, stamp.as_str());
        screen_write(screen, theme, Tone::of_level(rec.level), rec.level.tag());
        let mut module = FixedBuf::<48>::new();
        let _ = write!(module, " {}: ", rec.module);
        screen_write(screen, theme, Tone::Muted, module.as_str());
        let tone = if rec.level <= Level::Warn { Tone::of_level(rec.level) } else { Tone::Normal };
        screen_write(screen, theme, tone, rec.message);
        screen_write(screen, theme, Tone::Normal, "\n");
        set_wrap_indent(screen, 0);
    });
}

pub fn theme() -> Theme {
    *THEME.lock()
}

/// Cambia el tema en caliente (repinta la pantalla).
pub fn set_theme(theme: Theme) {
    *THEME.lock() = theme;
    let mut screen = SCREEN.lock();
    #[cfg(feature = "fbcon")]
    if let Screen::Fb(fb) = &mut *screen {
        fb.set_theme(theme);
    }
    #[cfg(target_arch = "x86_64")]
    if let Screen::Vga(v) = &mut *screen {
        v.clear();
    }
    let _ = &mut screen;
}

pub fn clear() {
    let mut screen = SCREEN.lock();
    match &mut *screen {
        Screen::None => {}
        #[cfg(feature = "fbcon")]
        Screen::Fb(fb) => fb.clear(),
        #[cfg(target_arch = "x86_64")]
        Screen::Vga(v) => v.clear(),
    }
    drop(screen);
    if SERIAL_COLOR.load(Ordering::Relaxed) {
        crate::hal::serial_write(b"\x1b[2J\x1b[H");
    }
}

/// Barra superior (derecha) y barra inferior.
pub fn set_status(title_right: &str, status_left: &str, status_right: &str) {
    #[cfg(feature = "fbcon")]
    if let Some(mut screen) = SCREEN.try_lock() {
        if let Screen::Fb(fb) = &mut *screen {
            fb.set_bars(title_right, status_left, status_right);
        }
    }
    let _ = (title_right, status_left, status_right);
}

pub fn cursor(visible: bool) {
    #[cfg(feature = "fbcon")]
    if let Screen::Fb(fb) = &mut *SCREEN.lock() {
        fb.set_cursor(visible);
    }
    let _ = visible;
}

/// Pasa la consola gráfica a doble búfer (necesita el montículo).
pub fn enable_double_buffering() -> bool {
    #[cfg(feature = "fbcon")]
    if let Screen::Fb(fb) = &mut *SCREEN.lock() {
        return fb.enable_shadow();
    }
    false
}

/// El framebuffer se movió a otra dirección virtual (tras la paginación).
pub fn relocate_framebuffer(virt: u64) {
    #[cfg(feature = "fbcon")]
    if let Screen::Fb(fb) = &mut *SCREEN.lock() {
        fb.relocate(virt as *mut u8);
    }
    let _ = virt;
}

/// Mueve la consola VGA al nuevo mapa (sigue en el HHDM, nada que hacer) — gancho
/// por simetría con el framebuffer.
pub fn screen_info() -> Option<ScreenInfo> {
    match &*SCREEN.lock() {
        Screen::None => None,
        #[cfg(feature = "fbcon")]
        Screen::Fb(fb) => Some(fb.info()),
        #[cfg(target_arch = "x86_64")]
        Screen::Vga(_) => Some(ScreenInfo {
            kind: "VGA texto",
            width: 80,
            height: 25,
            cols: 80,
            rows: 25,
            scale: 1,
            double_buffered: false,
        }),
    }
}

/// Muestra en pantalla la paleta del tema actual.
pub fn draw_palette() {
    #[cfg(feature = "fbcon")]
    if let Screen::Fb(fb) = &mut *SCREEN.lock() {
        fb.draw_palette();
        return;
    }
    write(Tone::Muted, "(la paleta necesita la consola gráfica)\n");
}

/// Pantalla de pánico. Fuerza los cerrojos: puede llamarse con ellos tomados.
pub fn panic_screen(title: &str, lines: &[&str]) {
    // SAFETY: en pánico no se vuelve; nadie más usa la pantalla.
    unsafe {
        SCREEN.force_unlock();
        THEME.force_unlock();
    }
    let theme = *THEME.lock();
    let mut screen = SCREEN.lock();
    match &mut *screen {
        Screen::None => {}
        #[cfg(feature = "fbcon")]
        Screen::Fb(fb) => fb.panic_screen(title, lines),
        #[cfg(target_arch = "x86_64")]
        Screen::Vga(v) => {
            v.write("\n", Tone::Normal);
            v.write(title, Tone::Error);
            v.write("\n", Tone::Normal);
            for l in lines {
                v.write(l, Tone::Normal);
                v.write("\n", Tone::Normal);
            }
        }
    }
    let _ = theme;
}
