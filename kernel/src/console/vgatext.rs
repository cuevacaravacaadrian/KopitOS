//! # Consola de texto VGA (80x25)
//!
//! Para arranques BIOS (PVH desde QEMU/Firecracker, GRUB en modo texto) en los
//! que no hay framebuffer gráfico. Traduce Unicode a la página de códigos 437
//! del hardware VGA, que por suerte incluye las letras del español y las
//! líneas de caja.

use super::Tone;
use crate::arch::io::outb;

const COLS: usize = 80;
const ROWS: usize = 25;

pub struct VgaText {
    buf: *mut u16,
    x: usize,
    y: usize,
}

// SAFETY: la memoria VGA es global; la protege el cerrojo de la consola.
unsafe impl Send for VgaText {}

fn attr(tone: Tone) -> u8 {
    match tone {
        Tone::Normal => 0x07,
        Tone::Muted | Tone::Trace => 0x08,
        Tone::Accent => 0x09,
        Tone::Success | Tone::Info => 0x0A,
        Tone::Debug => 0x0B,
        Tone::Error => 0x0C,
        Tone::Warning => 0x0E,
    }
}

/// Unicode → CP437.
fn cp437(c: char) -> u8 {
    match c {
        ' '..='~' => c as u8,
        'Ç' => 0x80,
        'ü' => 0x81,
        'é' => 0x82,
        'â' => 0x83,
        'ä' => 0x84,
        'à' => 0x85,
        'ç' => 0x87,
        'ê' => 0x88,
        'ë' => 0x89,
        'è' => 0x8A,
        'ï' => 0x8B,
        'î' => 0x8C,
        'ì' => 0x8D,
        'Ä' => 0x8E,
        'É' => 0x90,
        'ô' => 0x93,
        'ö' => 0x94,
        'ò' => 0x95,
        'û' => 0x96,
        'ù' => 0x97,
        'Ö' => 0x99,
        'Ü' => 0x9A,
        'á' => 0xA0,
        'í' => 0xA1,
        'ó' => 0xA2,
        'ú' => 0xA3,
        'ñ' => 0xA4,
        'Ñ' => 0xA5,
        'ª' => 0xA6,
        'º' => 0xA7,
        '¿' => 0xA8,
        '¬' => 0xAA,
        '¡' => 0xAD,
        '«' => 0xAE,
        '»' => 0xAF,
        '░' => 0xB0,
        '▒' => 0xB1,
        '▓' => 0xB2,
        '│' => 0xB3,
        '┤' => 0xB4,
        '╗' => 0xBB,
        '╝' => 0xBC,
        '┐' => 0xBF,
        '└' => 0xC0,
        '┴' => 0xC1,
        '┬' => 0xC2,
        '├' => 0xC3,
        '─' | '—' | '–' => 0xC4,
        '┼' => 0xC5,
        '╚' => 0xC8,
        '╔' => 0xC9,
        '═' => 0xCD,
        '║' => 0xBA,
        '┘' => 0xD9,
        '┌' => 0xDA,
        '█' => 0xDB,
        '▄' => 0xDC,
        '▀' => 0xDF,
        '°' => 0xF8,
        'µ' => 0xE6,
        '±' => 0xF1,
        '²' => 0xFD,
        '×' => b'x',
        '·' => 0xFA,
        '✓' => 0xFB,
        '■' => 0xFE,
        '•' | '●' => 0x07,
        '▶' | '►' | '▸' => 0x10,
        '◀' | '◄' => 0x11,
        '↑' => 0x18,
        '↓' => 0x19,
        '→' => 0x1A,
        '←' => 0x1B,
        '▲' => 0x1E,
        '▼' => 0x1F,
        '…' => b'.',
        '✗' => b'x',
        'Á' => b'A',
        'Í' => b'I',
        'Ó' => b'O',
        'Ú' => b'U',
        _ => b'?',
    }
}

impl VgaText {
    pub fn new() -> Self {
        // Ocultar el cursor hardware (registro 0x0A del CRTC, bit 5).
        outb(0x3D4, 0x0A);
        outb(0x3D5, 0x20);
        let mut v = Self { buf: crate::mm::phys_to_virt(0xB8000) as *mut u16, x: 0, y: 0 };
        v.clear();
        v
    }

    fn put_at(&mut self, x: usize, y: usize, ch: u8, a: u8) {
        // SAFETY: dentro de la memoria de texto 80x25.
        unsafe { self.buf.add(y * COLS + x).write_volatile((a as u16) << 8 | ch as u16) };
    }

    pub fn clear(&mut self) {
        for y in 0..ROWS {
            for x in 0..COLS {
                self.put_at(x, y, b' ', 0x07);
            }
        }
        self.x = 0;
        self.y = 0;
    }

    fn newline(&mut self) {
        self.x = 0;
        if self.y + 1 < ROWS {
            self.y += 1;
            return;
        }
        // SAFETY: copia dentro de la memoria de texto (regiones solapadas: `copy`).
        unsafe { core::ptr::copy(self.buf.add(COLS), self.buf, COLS * (ROWS - 1)) };
        for x in 0..COLS {
            self.put_at(x, ROWS - 1, b' ', 0x07);
        }
    }

    pub fn write(&mut self, s: &str, tone: Tone) {
        let a = attr(tone);
        for c in s.chars() {
            match c {
                '\n' => self.newline(),
                '\r' => self.x = 0,
                '\u{8}' => {
                    if self.x > 0 {
                        self.x -= 1;
                        self.put_at(self.x, self.y, b' ', a);
                    }
                }
                c if (c as u32) < 0x20 => {}
                c => {
                    if self.x >= COLS {
                        self.newline();
                    }
                    self.put_at(self.x, self.y, cp437(c), a);
                    self.x += 1;
                }
            }
        }
    }
}
