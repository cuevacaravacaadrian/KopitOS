//! `\kopit\boot.cfg`: lo genera `distro_builder` a partir del perfil.
//!
//! ```text
//! title=KopitOS Desktop
//! timeout=3                      # segundos; 0 = sin menú; never = esperar
//! default=0
//! resolution=1280x800            # o auto / max / keep
//! entry=KopitOS|\kopit\kernel.elf|profile=desktop theme=kopit-noche
//! entry=Modo seguro|\kopit\kernel.elf|profile=desktop safe loglevel=debug
//! ```

use alloc::string::{String, ToString};
use alloc::vec::Vec;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Resolution {
    /// La mayor que quepa en 1920x1080.
    Auto,
    /// La mayor disponible.
    Max,
    /// No tocar el modo actual.
    Keep,
    Exact(u32, u32),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Action {
    Boot,
    Reboot,
    Shutdown,
}

#[derive(Clone, Debug)]
pub struct Entry {
    pub title: String,
    pub kernel: String,
    pub cmdline: String,
    pub action: Action,
}

pub struct Config {
    pub title: String,
    /// `None` = esperar a que el usuario elija.
    pub timeout: Option<u32>,
    pub default: usize,
    pub resolution: Resolution,
    pub entries: Vec<Entry>,
    pub from_file: bool,
}

pub const DEFAULT_KERNEL: &str = "\\kopit\\kernel.elf";

fn parse_resolution(v: &str) -> Resolution {
    match v {
        "auto" => Resolution::Auto,
        "max" => Resolution::Max,
        "keep" => Resolution::Keep,
        _ => v
            .split_once(['x', 'X'])
            .and_then(|(w, h)| Some(Resolution::Exact(w.trim().parse().ok()?, h.trim().parse().ok()?)))
            .unwrap_or(Resolution::Auto),
    }
}

impl Config {
    pub fn parse(text: &str, from_file: bool) -> Self {
        let mut cfg = Self {
            title: "KopitOS".to_string(),
            timeout: Some(5),
            default: 0,
            resolution: Resolution::Auto,
            entries: Vec::new(),
            from_file,
        };
        for raw in text.lines() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else { continue };
            let value = value.trim();
            match key.trim() {
                "title" => cfg.title = value.to_string(),
                "timeout" => cfg.timeout = if value == "never" { None } else { value.parse().ok().or(Some(5)) },
                "default" => cfg.default = value.parse().unwrap_or(0),
                "resolution" => cfg.resolution = parse_resolution(value),
                "entry" => {
                    let mut parts = value.splitn(3, '|');
                    let title = parts.next().unwrap_or("KopitOS").trim();
                    let kernel = parts.next().map(str::trim).filter(|k| !k.is_empty()).unwrap_or(DEFAULT_KERNEL);
                    let cmdline = parts.next().unwrap_or("").trim();
                    cfg.entries.push(Entry {
                        title: title.to_string(),
                        kernel: kernel.to_string(),
                        cmdline: cmdline.to_string(),
                        action: Action::Boot,
                    });
                }
                _ => {}
            }
        }
        if cfg.entries.is_empty() {
            cfg.entries.push(Entry {
                title: cfg.title.clone(),
                kernel: DEFAULT_KERNEL.to_string(),
                cmdline: String::new(),
                action: Action::Boot,
            });
        }
        cfg.default = cfg.default.min(cfg.entries.len() - 1);
        for (title, action) in [("Reiniciar el equipo", Action::Reboot), ("Apagar el equipo", Action::Shutdown)] {
            cfg.entries.push(Entry { title: title.to_string(), kernel: String::new(), cmdline: String::new(), action });
        }
        cfg
    }
}
