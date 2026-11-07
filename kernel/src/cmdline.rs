//! # Línea de órdenes del núcleo
//!
//! Formato: palabras separadas por espacios, `clave=valor` o banderas
//! sueltas; los valores pueden ir entre comillas (`distro="Mi Distro"`).
//! Se copia a memoria del núcleo nada más arrancar, porque la del cargador se
//! recupera después.
//!
//! Parámetros que entiende la fase 1:
//!
//! | Parámetro            | Efecto                                            |
//! |----------------------|---------------------------------------------------|
//! | `loglevel=`          | nivel en pantalla (error/aviso/info/depur/traza)   |
//! | `serial.loglevel=`   | nivel por el puerto serie                         |
//! | `serial.color=0`     | sin colores ANSI en la serie                      |
//! | `theme=`             | tema (`kopit-noche`, `aurora`, `ambar`...)         |
//! | `theme.<rol>=#rrggbb`| ajusta un color del tema                          |
//! | `fbcon.scale=`       | tamaño de letra (1-4, `auto`)                     |
//! | `kbd=es` / `kbd=us`  | distribución de teclado                           |
//! | `hz=`                | frecuencia del temporizador                       |
//! | `profile=`, `distro=`| identidad de la distribución                      |
//! | `test=smoke`         | pruebas de humo y salida de QEMU                  |
//! | `safe`               | modo seguro (sin PCI ni teclado PS/2, más registro)|

use crate::sync::RacyCell;

const MAX: usize = 4096;
static BUFFER: RacyCell<[u8; MAX]> = RacyCell::new([0; MAX]);
static LEN: RacyCell<usize> = RacyCell::new(0);

/// Copia la línea de órdenes (una vez, en el arranque).
pub fn init(s: &str) {
    let n = s.len().min(MAX);
    let mut n2 = n;
    while n2 > 0 && !s.is_char_boundary(n2) {
        n2 -= 1;
    }
    // SAFETY: arranque, un solo hilo.
    unsafe {
        (&mut *BUFFER.get())[..n2].copy_from_slice(&s.as_bytes()[..n2]);
        *LEN.get() = n2;
    }
}

pub fn raw() -> &'static str {
    // SAFETY: sólo se escribe en `init`, antes de cualquier lectura.
    unsafe {
        let len = *LEN.get();
        core::str::from_utf8_unchecked(&(&*BUFFER.get())[..len])
    }
}

/// Itera `(clave, valor)`; las banderas sueltas tienen valor `None`.
pub fn iter() -> impl Iterator<Item = (&'static str, Option<&'static str>)> {
    Tokens { rest: raw() }.map(|tok| match tok.split_once('=') {
        Some((k, v)) => (k, Some(v.trim_matches('"'))),
        None => (tok, None),
    })
}

struct Tokens {
    rest: &'static str,
}

impl Iterator for Tokens {
    type Item = &'static str;
    fn next(&mut self) -> Option<&'static str> {
        let s = self.rest.trim_start();
        if s.is_empty() {
            return None;
        }
        let mut in_quotes = false;
        let mut end = s.len();
        for (i, c) in s.char_indices() {
            match c {
                '"' => in_quotes = !in_quotes,
                c if c.is_whitespace() && !in_quotes => {
                    end = i;
                    break;
                }
                _ => {}
            }
        }
        self.rest = &s[end..];
        Some(&s[..end])
    }
}

pub fn get(key: &str) -> Option<&'static str> {
    // La última aparición gana: la línea editada en el menú puede sobrescribir.
    iter().filter(|(k, _)| *k == key).filter_map(|(_, v)| v).last()
}

/// `clave` suelta, o `clave=1/si/true/on`.
pub fn flag(key: &str) -> bool {
    iter().any(|(k, v)| k == key && v.is_none_or(|v| matches!(v, "1" | "si" | "sí" | "true" | "on" | "yes")))
}

pub fn get_u32(key: &str) -> Option<u32> {
    get(key).and_then(|v| v.parse().ok())
}
