//! # Teclado PS/2 (controlador i8042)
//!
//! Dos piezas independientes:
//!
//! * [`I8042`]: programa el controlador (desactiva puertos, autotest, activa
//!   la traducción a *scancode set 1* y la IRQ 1). Genérico sobre [`PortIo`].
//! * [`Decoder`]: máquina de estados pura que convierte scancodes en teclas
//!   con modificadores, Bloq Mayús, AltGr y **teclas muertas** (´ ` ^ ¨),
//!   con distribuciones `us` y `es`. Al ser pura, se prueba en el anfitrión.
//!
//! QEMU (y los portátiles con emulación USB→PS/2) envían scancodes según la
//! posición física de la tecla; la distribución se aplica aquí, en el sistema
//! invitado, igual que en un PC real.

#![cfg_attr(not(test), no_std)]

use kopit_driver_api::PortIo;

const DATA: u16 = 0x60;
const STATUS_CMD: u16 = 0x64;
const ST_OUTPUT_FULL: u8 = 1 << 0;
const ST_INPUT_FULL: u8 = 1 << 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ps2Error {
    Timeout,
    SelfTest(u8),
    PortTest(u8),
}

pub struct I8042<P: PortIo> {
    io: P,
}

impl<P: PortIo> I8042<P> {
    pub const fn new(io: P) -> Self {
        Self { io }
    }

    fn status(&self) -> u8 {
        self.io.inb(STATUS_CMD)
    }

    fn wait_input_clear(&self) -> Result<(), Ps2Error> {
        for _ in 0..200_000 {
            if self.status() & ST_INPUT_FULL == 0 {
                return Ok(());
            }
            core::hint::spin_loop();
        }
        Err(Ps2Error::Timeout)
    }

    fn read_wait(&self) -> Result<u8, Ps2Error> {
        for _ in 0..200_000 {
            if self.status() & ST_OUTPUT_FULL != 0 {
                return Ok(self.io.inb(DATA));
            }
            core::hint::spin_loop();
        }
        Err(Ps2Error::Timeout)
    }

    fn command(&self, cmd: u8) -> Result<(), Ps2Error> {
        self.wait_input_clear()?;
        self.io.outb(STATUS_CMD, cmd);
        Ok(())
    }

    fn write_data(&self, value: u8) -> Result<(), Ps2Error> {
        self.wait_input_clear()?;
        self.io.outb(DATA, value);
        Ok(())
    }

    /// Descarta lo que haya pendiente en el búfer de salida.
    pub fn flush(&self) {
        for _ in 0..64 {
            if self.status() & ST_OUTPUT_FULL == 0 {
                break;
            }
            let _ = self.io.inb(DATA);
        }
    }

    /// Inicializa el controlador y el teclado del puerto 1.
    pub fn init(&mut self) -> Result<(), Ps2Error> {
        self.command(0xAD)?; // desactivar puerto 1
        self.command(0xA7)?; // desactivar puerto 2 (ratón)
        self.flush();

        self.command(0x20)?; // leer el byte de configuración
        let mut config = self.read_wait()?;
        config &= !0b0000_0011; // sin IRQ mientras configuramos
        config |= 1 << 6; // traducción → scancode set 1
        self.command(0x60)?;
        self.write_data(config)?;

        self.command(0xAA)?; // autotest del controlador
        let r = self.read_wait()?;
        if r != 0x55 {
            return Err(Ps2Error::SelfTest(r));
        }
        // Algunos controladores reinician la configuración tras el autotest.
        self.command(0x60)?;
        self.write_data(config)?;

        self.command(0xAB)?; // probar el puerto 1
        let t = self.read_wait()?;
        if t != 0x00 {
            return Err(Ps2Error::PortTest(t));
        }

        self.command(0xAE)?; // activar puerto 1
        config |= 1 << 0; // IRQ 1
        config &= !(1 << 4); // reloj del puerto 1 activo
        self.command(0x60)?;
        self.write_data(config)?;

        self.write_data(0xF4)?; // teclado: empezar a enviar scancodes
        let _ack = self.read_wait(); // 0xFA (si no llega, no es grave)
        self.flush();
        Ok(())
    }

    /// Lee un scancode si hay uno esperando (manejador de la IRQ 1).
    pub fn read_scancode(&self) -> Option<u8> {
        (self.status() & ST_OUTPUT_FULL != 0).then(|| self.io.inb(DATA))
    }
}

// -----------------------------------------------------------------------------
// Decodificador
// -----------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Char(char),
    Enter,
    Backspace,
    Tab,
    Escape,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    Insert,
    Delete,
    F(u8),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Modifiers {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub altgr: bool,
    pub caps_lock: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyEvent {
    pub key: Key,
    pub mods: Modifiers,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    Us,
    Es,
}

impl Layout {
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "us" | "en" => Some(Layout::Us),
            "es" => Some(Layout::Es),
            _ => None,
        }
    }

    pub const fn name(self) -> &'static str {
        match self {
            Layout::Us => "us",
            Layout::Es => "es",
        }
    }
}

// Teclas muertas: caracteres de uso privado que nunca llegan al usuario.
const DEAD_ACUTE: char = '\u{E001}';
const DEAD_GRAVE: char = '\u{E002}';
const DEAD_CIRC: char = '\u{E003}';
const DEAD_DIAER: char = '\u{E004}';

type Rows = [(u8, &'static str)];

const US_NORMAL: &Rows =
    &[(0x02, "1234567890-="), (0x10, "qwertyuiop[]"), (0x1E, "asdfghjkl;'`"), (0x2B, "\\zxcvbnm,./")];
const US_SHIFT: &Rows =
    &[(0x02, "!@#$%^&*()_+"), (0x10, "QWERTYUIOP{}"), (0x1E, "ASDFGHJKL:\"~"), (0x2B, "|ZXCVBNM<>?")];

const ES_NORMAL: &Rows =
    &[(0x02, "1234567890'¡"), (0x10, "qwertyuiop\u{E002}+"), (0x1E, "asdfghjklñ\u{E001}º"), (0x2B, "çzxcvbnm,.-")];
const ES_SHIFT: &Rows =
    &[(0x02, "!\"·$%&/()=?¿"), (0x10, "QWERTYUIOP\u{E003}*"), (0x1E, "ASDFGHJKLÑ\u{E004}ª"), (0x2B, "ÇZXCVBNM;:_")];
const ES_ALTGR: &Rows = &[(0x02, "|@#~€¬"), (0x12, "€"), (0x1A, "[]"), (0x28, "{\\"), (0x2B, "}")];

fn lookup(rows: &Rows, code: u8) -> Option<char> {
    rows.iter().find_map(|&(start, s)| {
        let i = code.checked_sub(start)? as usize;
        s.chars().nth(i)
    })
}

fn combine(dead: char, c: char) -> Option<char> {
    let to = match dead {
        DEAD_ACUTE => "áéíóúÁÉÍÓÚ",
        DEAD_GRAVE => "àèìòùÀÈÌÒÙ",
        DEAD_CIRC => "âêîôûÂÊÎÔÛ",
        DEAD_DIAER => "äëïöüÄËÏÖÜ",
        _ => return None,
    };
    "aeiouAEIOU".chars().position(|x| x == c).and_then(|i| to.chars().nth(i))
}

fn dead_symbol(dead: char) -> char {
    match dead {
        DEAD_ACUTE => '´',
        DEAD_GRAVE => '`',
        DEAD_CIRC => '^',
        _ => '¨',
    }
}

/// Convierte scancodes (set 1) en eventos de teclado.
pub struct Decoder {
    layout: Layout,
    extended: bool,
    skip: u8,
    mods: Modifiers,
    dead: Option<char>,
}

impl Decoder {
    pub const fn new(layout: Layout) -> Self {
        Self {
            layout,
            extended: false,
            skip: 0,
            mods: Modifiers { shift: false, ctrl: false, alt: false, altgr: false, caps_lock: false },
            dead: None,
        }
    }

    pub fn layout(&self) -> Layout {
        self.layout
    }

    pub fn set_layout(&mut self, layout: Layout) {
        self.layout = layout;
        self.dead = None;
    }

    pub fn feed(&mut self, sc: u8) -> Option<KeyEvent> {
        if self.skip > 0 {
            self.skip -= 1;
            return None;
        }
        match sc {
            0xE0 => {
                self.extended = true;
                return None;
            }
            0xE1 => {
                self.skip = 5; // Pausa: E1 1D 45 E1 9D C5
                return None;
            }
            _ => {}
        }
        let ext = core::mem::take(&mut self.extended);
        let released = sc & 0x80 != 0;
        let code = sc & 0x7F;

        match (ext, code) {
            (false, 0x2A) | (false, 0x36) => {
                self.mods.shift = !released;
                return None;
            }
            (_, 0x1D) => {
                self.mods.ctrl = !released;
                return None;
            }
            (false, 0x38) => {
                self.mods.alt = !released;
                return None;
            }
            (true, 0x38) => {
                self.mods.altgr = !released;
                return None;
            }
            (false, 0x3A) => {
                if !released {
                    self.mods.caps_lock = !self.mods.caps_lock;
                }
                return None;
            }
            (true, 0x2A) | (true, 0x37) => return None, // Impr Pant
            _ => {}
        }
        if released {
            return None;
        }

        let key = if ext {
            match code {
                0x1C => Key::Enter,
                0x35 => Key::Char('/'),
                0x47 => Key::Home,
                0x48 => Key::Up,
                0x49 => Key::PageUp,
                0x4B => Key::Left,
                0x4D => Key::Right,
                0x4F => Key::End,
                0x50 => Key::Down,
                0x51 => Key::PageDown,
                0x52 => Key::Insert,
                0x53 => Key::Delete,
                _ => return None,
            }
        } else {
            match code {
                0x01 => Key::Escape,
                0x0E => Key::Backspace,
                0x0F => Key::Tab,
                0x1C => Key::Enter,
                0x39 => Key::Char(' '),
                0x3B..=0x44 => Key::F(code - 0x3A),
                0x57 => Key::F(11),
                0x58 => Key::F(12),
                0x37 => Key::Char('*'),
                0x47..=0x53 => Key::Char(match b"789-456+1230."[(code - 0x47) as usize] {
                    b'.' if self.layout == Layout::Es => ',',
                    c => c as char,
                }),
                _ => Key::Char(self.translate(code)?),
            }
        };

        // Una tecla muerta pendiente se combina con la siguiente.
        let key = match (self.dead.take(), key) {
            (Some(d), Key::Char(' ')) => Key::Char(dead_symbol(d)),
            (Some(d), Key::Char(c)) => Key::Char(combine(d, c).unwrap_or(c)),
            (_, k) => k,
        };
        if let Key::Char(c) = key {
            if matches!(c, DEAD_ACUTE | DEAD_GRAVE | DEAD_CIRC | DEAD_DIAER) {
                self.dead = Some(c);
                return None;
            }
        }
        Some(KeyEvent { key, mods: self.mods })
    }

    fn translate(&self, code: u8) -> Option<char> {
        let m = self.mods;
        let (normal, shift, altgr): (&Rows, &Rows, &Rows) = match self.layout {
            Layout::Us => (US_NORMAL, US_SHIFT, &[]),
            Layout::Es => (ES_NORMAL, ES_SHIFT, ES_ALTGR),
        };
        if code == 0x56 {
            return Some(match (self.layout, m.shift) {
                (Layout::Es, false) => '<',
                (Layout::Es, true) => '>',
                (Layout::Us, false) => '\\',
                (Layout::Us, true) => '|',
            });
        }
        if m.altgr {
            return lookup(altgr, code);
        }
        let base = lookup(normal, code)?;
        if base.is_alphabetic() {
            let upper = m.shift ^ m.caps_lock;
            return Some(if upper { lookup(shift, code).unwrap_or(base) } else { base });
        }
        Some(if m.shift { lookup(shift, code).unwrap_or(base) } else { base })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn type_all(d: &mut Decoder, codes: &[u8]) -> String {
        codes
            .iter()
            .filter_map(|&c| d.feed(c))
            .filter_map(|e| if let Key::Char(c) = e.key { Some(c) } else { None })
            .collect()
    }

    #[test]
    fn distribucion_espanola() {
        let mut d = Decoder::new(Layout::Es);
        // "ñ", tilde muerta + e = "é", Mayús + ¡ = "¿", AltGr + 2 = "@"
        let s = type_all(
            &mut d,
            &[0x27, 0xA7, 0x28, 0xA8, 0x12, 0x92, 0x2A, 0x0D, 0x8D, 0xAA, 0xE0, 0x38, 0x03, 0x83, 0xE0, 0xB8],
        );
        assert_eq!(s, "ñé¿@");
        // Tilde + espacio = el propio acento; diéresis + u = ü.
        let s = type_all(&mut d, &[0x28, 0x39, 0x2A, 0x28, 0xAA, 0x16]);
        assert_eq!(s, "´ü");
    }

    #[test]
    fn distribucion_us_y_bloq_mayus() {
        let mut d = Decoder::new(Layout::Us);
        assert_eq!(type_all(&mut d, &[0x1E, 0x2A, 0x1E, 0xAA, 0x1E]), "aAa");
        assert_eq!(type_all(&mut d, &[0x3A, 0xBA, 0x1E, 0x02]), "A1");
    }

    #[test]
    fn teclas_extendidas() {
        let mut d = Decoder::new(Layout::Us);
        assert_eq!(d.feed(0xE0), None);
        assert_eq!(d.feed(0x48).map(|e| e.key), Some(Key::Up));
        assert_eq!(d.feed(0x1C).map(|e| e.key), Some(Key::Enter));
        assert_eq!(d.feed(0x3B).map(|e| e.key), Some(Key::F(1)));
    }
}
