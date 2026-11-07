//! # Entrada de usuario
//!
//! El teclado (IRQ 1) y el puerto serie (IRQ 4) producen el mismo tipo de
//! evento, [`Input`], en una cola sin cerrojos. El consumidor (la shell) no
//! sabe ni le importa de dónde vino la tecla: KopitOS se maneja igual con un
//! teclado físico que por una consola serie remota.

use crate::sync::{RacyCell, Spsc};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Input {
    Char(char),
    Ctrl(char),
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
    Delete,
    F(u8),
}

static QUEUE: Spsc<Input, 256> = Spsc::new();

/// Desde un manejador de interrupción.
pub fn push(i: Input) {
    let _ = QUEUE.push(i);
}

pub fn pop() -> Option<Input> {
    QUEUE.pop()
}

pub fn has_input() -> bool {
    !QUEUE.is_empty()
}

/// Estado del decodificador de la serie: UTF-8 y secuencias de escape ANSI.
struct SerialDecoder {
    utf8: [u8; 4],
    utf8_len: usize,
    utf8_need: usize,
    escape: u8, // 0 = nada, 1 = vio ESC, 2 = vio ESC [
}

static SERIAL: RacyCell<SerialDecoder> =
    RacyCell::new(SerialDecoder { utf8: [0; 4], utf8_len: 0, utf8_need: 0, escape: 0 });

/// Un byte recibido por el puerto serie (sólo lo llama su IRQ).
pub fn push_serial_byte(b: u8) {
    // SAFETY: sólo el manejador de la IRQ serie toca el decodificador.
    let d = unsafe { &mut *SERIAL.get() };
    match d.escape {
        1 => {
            d.escape = if b == b'[' || b == b'O' { 2 } else { 0 };
            if d.escape == 0 {
                push(Input::Escape);
            }
            return;
        }
        2 => {
            d.escape = 0;
            let ev = match b {
                b'A' => Input::Up,
                b'B' => Input::Down,
                b'C' => Input::Right,
                b'D' => Input::Left,
                b'H' => Input::Home,
                b'F' => Input::End,
                b'3' => Input::Delete, // ESC [ 3 ~ (el '~' se ignora abajo)
                _ => return,
            };
            push(ev);
            return;
        }
        _ => {}
    }
    if d.utf8_need > 0 {
        if b & 0xC0 == 0x80 {
            d.utf8[d.utf8_len] = b;
            d.utf8_len += 1;
            if d.utf8_len == d.utf8_need {
                if let Some(c) = core::str::from_utf8(&d.utf8[..d.utf8_len]).ok().and_then(|s| s.chars().next()) {
                    push(Input::Char(c));
                }
                d.utf8_need = 0;
            }
            return;
        }
        d.utf8_need = 0;
    }
    match b {
        0x1B => d.escape = 1,
        b'\r' | b'\n' => push(Input::Enter),
        0x7F | 0x08 => push(Input::Backspace),
        b'\t' => push(Input::Tab),
        b'~' => {}
        0x01..=0x1A => push(Input::Ctrl((b'a' + b - 1) as char)),
        0x20..=0x7E => push(Input::Char(b as char)),
        0xC0..=0xDF => start_utf8(d, b, 2),
        0xE0..=0xEF => start_utf8(d, b, 3),
        0xF0..=0xF7 => start_utf8(d, b, 4),
        _ => {}
    }
}

fn start_utf8(d: &mut SerialDecoder, b: u8, need: usize) {
    d.utf8[0] = b;
    d.utf8_len = 1;
    d.utf8_need = need;
}
