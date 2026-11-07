//! Menú de arranque en modo texto (funciona igual en pantalla y por puerto serie,
//! porque el firmware replica la consola UEFI en ambos).

use crate::config::{Action, Config};
use crate::firmware::{self as fw, Wake};
use crate::uefi::{SCAN_DOWN, SCAN_ESC, SCAN_UP, color::*};
use crate::{VERSION, print};
use alloc::string::String;
use kopit_boot_proto::SecureBootState;

pub enum Choice {
    Boot { entry: usize, cmdline: String },
    Reboot,
    Shutdown,
}

pub struct Status<'a> {
    pub secure_boot: SecureBootState,
    pub firmware: &'a str,
    pub signed_hash: bool,
    pub config_found: bool,
}

struct Layout {
    cols: usize,
    rows: usize,
    width: usize,
    left: usize,
}

impl Layout {
    fn new() -> Self {
        let (cols, rows) = fw::text_size();
        let width = cols.saturating_sub(6).clamp(40, 76);
        Self { cols, rows, width, left: (cols.saturating_sub(width)) / 2 }
    }
}

fn repeat(c: char, n: usize) -> String {
    core::iter::repeat_n(c, n).collect()
}

fn line(l: &Layout, row: usize, fg: usize, bg: usize, text: &str) {
    fw::set_cursor(l.left, row);
    fw::set_color(fg, bg);
    // Se recorta al ancho del menú: nunca se parte en dos líneas.
    let shown: String = text.chars().take(l.width).collect();
    let len = shown.chars().count();
    print!("{}{}", shown, repeat(' ', l.width.saturating_sub(len)));
    fw::set_color(LIGHTGRAY, BLACK);
}

fn draw_header(l: &Layout, cfg: &Config) {
    let inner = l.width - 2;
    fw::set_cursor(l.left, 1);
    fw::set_color(LIGHTCYAN, BLACK);
    print!("╔{}╗", repeat('═', inner));
    let left = String::from("  KopitBoot ") + VERSION;
    let right = String::from(cfg.title.as_str()) + "  ";
    let gap = inner.saturating_sub(left.chars().count() + right.chars().count());
    fw::set_cursor(l.left, 2);
    print!("║");
    fw::set_color(WHITE, BLACK);
    print!("{}{}", left, repeat(' ', gap));
    fw::set_color(YELLOW, BLACK);
    print!("{}", right);
    fw::set_color(LIGHTCYAN, BLACK);
    print!("║");
    fw::set_cursor(l.left, 3);
    print!("╚{}╝", repeat('═', inner));
    fw::set_color(LIGHTGRAY, BLACK);
}

fn draw_entries(l: &Layout, cfg: &Config, selected: usize) {
    for (i, e) in cfg.entries.iter().enumerate() {
        let row = 5 + i;
        if row >= l.rows.saturating_sub(5) {
            break;
        }
        let marker = if i == selected { "►" } else { " " };
        let number = if i < 9 { char::from(b'1' + i as u8) } else { ' ' };
        let text = alloc::format!(" {} {}  {}", marker, number, e.title);
        match (i == selected, e.action) {
            (true, _) => line(l, row, WHITE, BLUE, &text),
            (false, Action::Boot) => line(l, row, LIGHTGRAY, BLACK, &text),
            (false, _) => line(l, row, DARKGRAY, BLACK, &text),
        }
    }
}

fn draw_footer(l: &Layout, remaining: Option<u32>, status: &Status) {
    let base = l.rows.saturating_sub(5);
    if !status.config_found {
        line(l, base - 1, YELLOW, BLACK, "Sin \\kopit\\boot.cfg: entradas por defecto.");
    }
    line(l, base, DARKGRAY, BLACK, "↑↓ elegir · Intro arrancar · e editar la línea de órdenes · 1-9 atajo");
    match remaining {
        Some(s) => line(l, base + 1, LIGHTGREEN, BLACK, &alloc::format!("Arranque automático en {} s...", s)),
        None => line(l, base + 1, DARKGRAY, BLACK, "Arranque automático detenido."),
    }
    let sb_color = match status.secure_boot {
        SecureBootState::ENABLED => LIGHTGREEN,
        SecureBootState::DISABLED => YELLOW,
        _ => DARKGRAY,
    };
    let integrity = if status.signed_hash { "huella SHA-256" } else { "sin huella" };
    let sb = match status.secure_boot {
        SecureBootState::ENABLED => "activo",
        SecureBootState::DISABLED => "desactivado",
        SecureBootState::SETUP_MODE => "modo configuración",
        _ => "no disponible",
    };
    let text = alloc::format!("Secure Boot: {} · Firmware: {} · Núcleo: {}", sb, status.firmware, integrity);
    line(l, base + 3, sb_color, BLACK, &text);
}

/// Edita la línea de órdenes de una entrada (como la tecla `e` de GRUB).
fn edit_cmdline(l: &Layout, initial: &str) -> Option<String> {
    let mut text = String::from(initial);
    let row = l.rows.saturating_sub(3);
    fw::show_cursor(true);
    loop {
        let shown: String = {
            let max = l.width.saturating_sub(4);
            let chars: alloc::vec::Vec<char> = text.chars().collect();
            let start = chars.len().saturating_sub(max);
            chars[start..].iter().collect()
        };
        line(l, row - 1, LIGHTCYAN, BLACK, "Línea de órdenes (Intro aceptar · Esc cancelar):");
        line(l, row, WHITE, BLUE, &alloc::format!("> {}_", shown));
        match fw::wait_key_or(None) {
            Wake::Key(k) if k.scan_code == SCAN_ESC => {
                fw::show_cursor(false);
                return None;
            }
            Wake::Key(k) if k.unicode_char == 0x0D => {
                fw::show_cursor(false);
                return Some(text);
            }
            Wake::Key(k) if k.unicode_char == 0x08 => {
                text.pop();
            }
            Wake::Key(k) if k.unicode_char >= 0x20 => {
                if let Some(c) = char::from_u32(k.unicode_char as u32) {
                    if text.len() < 1024 {
                        text.push(c);
                    }
                }
            }
            _ => {}
        }
    }
}

pub fn run(cfg: &Config, status: &Status) -> Choice {
    let finish = |i: usize, cmdline: Option<String>| match cfg.entries[i].action {
        Action::Boot => Choice::Boot { entry: i, cmdline: cmdline.unwrap_or_else(|| cfg.entries[i].cmdline.clone()) },
        Action::Reboot => Choice::Reboot,
        Action::Shutdown => Choice::Shutdown,
    };

    // timeout=0: arranque directo salvo que haya una tecla pulsada.
    if cfg.timeout == Some(0) && fw::read_key().is_none() {
        return finish(cfg.default, None);
    }

    let l = Layout::new();
    let mut selected = cfg.default;
    let mut remaining = cfg.timeout;
    let mut cmdlines: alloc::vec::Vec<Option<String>> = cfg.entries.iter().map(|_| None).collect();
    fw::flush_keys();
    fw::show_cursor(false);
    fw::set_color(LIGHTGRAY, BLACK);
    fw::clear_screen();
    draw_header(&l, cfg);
    draw_entries(&l, cfg, selected);
    draw_footer(&l, remaining, status);

    let timer = fw::create_periodic_timer(1000).ok();
    let n = cfg.entries.len();
    let result = loop {
        match fw::wait_key_or(if remaining.is_some() { timer } else { None }) {
            Wake::Tick => {
                if let Some(r) = remaining {
                    if r <= 1 {
                        break finish(selected, cmdlines[selected].take());
                    }
                    remaining = Some(r - 1);
                    draw_footer(&l, remaining, status);
                }
            }
            Wake::Key(k) => {
                if remaining.take().is_some() {
                    draw_footer(&l, None, status);
                }
                match (k.scan_code, k.unicode_char) {
                    (SCAN_UP, _) => selected = (selected + n - 1) % n,
                    (SCAN_DOWN, _) => selected = (selected + 1) % n,
                    (_, 0x0D) => break finish(selected, cmdlines[selected].take()),
                    (_, c) if (u16::from(b'1')..=u16::from(b'9')).contains(&c) => {
                        let i = (c - u16::from(b'1')) as usize;
                        if i < n {
                            break finish(i, cmdlines[i].take());
                        }
                    }
                    (_, c)
                        if (c == u16::from(b'e') || c == u16::from(b'E'))
                            && cfg.entries[selected].action == Action::Boot =>
                    {
                        let current =
                            cmdlines[selected].clone().unwrap_or_else(|| cfg.entries[selected].cmdline.clone());
                        if let Some(edited) = edit_cmdline(&l, &current) {
                            cmdlines[selected] = Some(edited);
                        }
                        fw::clear_screen();
                        draw_header(&l, cfg);
                        draw_footer(&l, None, status);
                    }
                    _ => {}
                }
                draw_entries(&l, cfg, selected);
            }
        }
    };
    if let Some(t) = timer {
        fw::close_timer(t);
    }
    let _ = l.cols;
    result
}
