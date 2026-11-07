//! Utilidades: salida con colores, tamaños, fechas reproducibles, rutas.

use std::path::{Path, PathBuf};

pub const BOLD: &str = "\x1b[1m";
pub const DIM: &str = "\x1b[90m";
pub const GREEN: &str = "\x1b[32m";
pub const YELLOW: &str = "\x1b[33m";
pub const RED: &str = "\x1b[31m";
pub const BLUE: &str = "\x1b[34m";
pub const CYAN: &str = "\x1b[36m";
pub const RESET: &str = "\x1b[0m";

pub fn step(what: &str, detail: &str) {
    println!("  {GREEN}✓{RESET} {what:<26}{DIM}{detail}{RESET}");
}

pub fn info(what: &str, detail: &str) {
    println!("  {CYAN}•{RESET} {what:<26}{detail}");
}

pub fn warn(msg: &str) {
    println!("  {YELLOW}!{RESET} {msg}");
}

pub fn title(t: &str) {
    println!("\n{BOLD}{BLUE}▶ {t}{RESET}");
}

pub fn human(bytes: u64) -> String {
    match bytes {
        b if b >= 1 << 30 => format!("{:.1} GiB", b as f64 / (1u64 << 30) as f64),
        b if b >= 1 << 20 => format!("{:.1} MiB", b as f64 / (1u64 << 20) as f64),
        b if b >= 1 << 10 => format!("{:.1} KiB", b as f64 / 1024.0),
        b => format!("{b} B"),
    }
}

/// Activa las secuencias ANSI y UTF-8 en la consola de Windows (los hijos,
/// como QEMU, heredan la consola: así los colores del núcleo se ven bien).
#[cfg(windows)]
pub fn enable_terminal() {
    type Handle = *mut std::ffi::c_void;
    unsafe extern "system" {
        fn GetStdHandle(n: u32) -> Handle;
        fn GetConsoleMode(h: Handle, mode: *mut u32) -> i32;
        fn SetConsoleMode(h: Handle, mode: u32) -> i32;
        fn SetConsoleOutputCP(cp: u32) -> i32;
        fn SetConsoleCP(cp: u32) -> i32;
    }
    const STD_OUTPUT_HANDLE: u32 = -11i32 as u32;
    const STD_INPUT_HANDLE: u32 = -10i32 as u32;
    const ENABLE_VIRTUAL_TERMINAL_PROCESSING: u32 = 0x0004;
    const ENABLE_VIRTUAL_TERMINAL_INPUT: u32 = 0x0200;
    // SAFETY: llamadas documentadas de kernel32 con argumentos válidos.
    unsafe {
        SetConsoleOutputCP(65001);
        SetConsoleCP(65001);
        let out = GetStdHandle(STD_OUTPUT_HANDLE);
        let mut mode = 0;
        if GetConsoleMode(out, &mut mode) != 0 {
            SetConsoleMode(out, mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING);
        }
        let inp = GetStdHandle(STD_INPUT_HANDLE);
        if GetConsoleMode(inp, &mut mode) != 0 {
            SetConsoleMode(inp, mode | ENABLE_VIRTUAL_TERMINAL_INPUT);
        }
    }
}

#[cfg(not(windows))]
pub fn enable_terminal() {}

/// Segundos Unix de la compilación: `SOURCE_DATE_EPOCH` (reproducible) o ahora.
pub fn build_epoch() -> i64 {
    std::env::var("SOURCE_DATE_EPOCH").ok().and_then(|v| v.parse().ok()).unwrap_or_else(|| {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
    })
}

/// (año, mes, día, hora, minuto, segundo) de unos segundos Unix.
pub fn civil(t: i64) -> (i64, u8, u8, u8, u8, u8) {
    let days = t.div_euclid(86_400);
    let rem = t.rem_euclid(86_400);
    let z = days + 719_468;
    let era = (if z >= 0 { z } else { z - 146_096 }) / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u8;
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u8;
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    (y, m, d, (rem / 3600) as u8, (rem % 3600 / 60) as u8, (rem % 60) as u8)
}

/// Busca un ejecutable en el PATH.
pub fn which(name: &str) -> Option<PathBuf> {
    let exe = if cfg!(windows) && !name.ends_with(".exe") { format!("{name}.exe") } else { name.to_string() };
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).map(|d| d.join(&exe)).find(|p| p.is_file())
}

pub fn first_existing(candidates: &[PathBuf]) -> Option<PathBuf> {
    candidates.iter().find(|p| p.is_file()).cloned()
}

pub fn write_file(path: &Path, data: &[u8]) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {}", dir.display(), e))?;
    }
    std::fs::write(path, data).map_err(|e| format!("{}: {}", path.display(), e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fechas() {
        assert_eq!(civil(0), (1970, 1, 1, 0, 0, 0));
        assert_eq!(civil(1_767_225_600), (2026, 1, 1, 0, 0, 0));
        assert_eq!(civil(951_782_400), (2000, 2, 29, 0, 0, 0));
    }

    #[test]
    fn tamanos() {
        assert_eq!(human(512), "512 B");
        assert_eq!(human(3 << 20), "3.0 MiB");
    }
}
