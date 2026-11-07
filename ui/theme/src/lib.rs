//! # Motor de temas
//!
//! Un [`Theme`] es un conjunto de **roles semánticos** (fondo, texto, acento,
//! barras, niveles de log...), no de colores sueltos: la consola del núcleo,
//! el futuro compositor y las aplicaciones piden "color de aviso", no
//! "amarillo". Por eso un tema nuevo cambia todo el sistema a la vez.
//!
//! Tres niveles de personalización, de menos a más:
//!
//! 1. **Temas integrados** ([`THEMES`]), elegibles con `theme=<id>`.
//! 2. **Ajustes sueltos** en la línea de órdenes: `theme.accent=#ff8800`
//!    ([`Theme::apply_override`]). Así una distribución derivada pone su color
//!    de marca sin recompilar nada (lo hace `distro_builder` desde el perfil).
//! 3. **Temas en archivo** (fase 3): el mismo formato clave=valor cargado como
//!    módulo, con fuentes y fondos.

#![cfg_attr(not(test), no_std)]

/// Color RGB de 8 bits por canal.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    pub const fn hex(v: u32) -> Self {
        Self { r: (v >> 16) as u8, g: (v >> 8) as u8, b: v as u8 }
    }

    /// Acepta `#rrggbb`, `rrggbb` o `#rgb`.
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim().trim_start_matches('#');
        let v = u32::from_str_radix(s, 16).ok()?;
        match s.len() {
            6 => Some(Self::hex(v)),
            3 => {
                let (r, g, b) = ((v >> 8) & 0xF, (v >> 4) & 0xF, v & 0xF);
                Some(Self { r: (r * 17) as u8, g: (g * 17) as u8, b: (b * 17) as u8 })
            }
            _ => None,
        }
    }

    /// Mezcla lineal: `alpha = 0` → `self`, `alpha = 255` → `other`.
    pub const fn mix(self, other: Rgb, alpha: u8) -> Rgb {
        let a = alpha as u32;
        let ia = 255 - a;
        Rgb {
            r: ((self.r as u32 * ia + other.r as u32 * a) / 255) as u8,
            g: ((self.g as u32 * ia + other.g as u32 * a) / 255) as u8,
            b: ((self.b as u32 * ia + other.b as u32 * a) / 255) as u8,
        }
    }

    /// Luminancia relativa aproximada (0..=255).
    pub const fn luma(self) -> u8 {
        ((self.r as u32 * 299 + self.g as u32 * 587 + self.b as u32 * 114) / 1000) as u8
    }
}

/// Roles de color de un tema.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Theme {
    pub id: &'static str,
    pub name: &'static str,
    pub dark: bool,
    pub background: Rgb,
    pub foreground: Rgb,
    pub muted: Rgb,
    pub accent: Rgb,
    pub on_accent: Rgb,
    pub bar_background: Rgb,
    pub bar_foreground: Rgb,
    pub panel: Rgb,
    pub error: Rgb,
    pub warning: Rgb,
    pub info: Rgb,
    pub debug: Rgb,
    pub trace: Rgb,
    pub success: Rgb,
    pub cursor: Rgb,
}

impl Theme {
    /// Aplica un ajuste `rol=#color`. Devuelve `false` si el rol o el color no existen.
    pub fn apply_override(&mut self, role: &str, value: &str) -> bool {
        let Some(c) = Rgb::parse(value) else { return false };
        let slot = match role {
            "background" | "fondo" => &mut self.background,
            "foreground" | "texto" => &mut self.foreground,
            "muted" | "tenue" => &mut self.muted,
            "accent" | "acento" => &mut self.accent,
            "on_accent" => &mut self.on_accent,
            "bar" | "barra" => &mut self.bar_background,
            "bar_text" => &mut self.bar_foreground,
            "panel" => &mut self.panel,
            "error" => &mut self.error,
            "warning" | "aviso" => &mut self.warning,
            "info" => &mut self.info,
            "debug" => &mut self.debug,
            "trace" => &mut self.trace,
            "success" | "ok" => &mut self.success,
            "cursor" => &mut self.cursor,
            _ => return false,
        };
        *slot = c;
        true
    }
}

/// Tema por defecto: los colores de la marca HyperEZ (#0f1424, #2f81f7, #7ee787).
pub const KOPIT_NOCHE: Theme = Theme {
    id: "kopit-noche",
    name: "Kopit Noche",
    dark: true,
    background: Rgb::hex(0x0F1424),
    foreground: Rgb::hex(0xD7DEEA),
    muted: Rgb::hex(0x6B7894),
    accent: Rgb::hex(0x2F81F7),
    on_accent: Rgb::hex(0xFFFFFF),
    bar_background: Rgb::hex(0x161D33),
    bar_foreground: Rgb::hex(0xC9D4E8),
    panel: Rgb::hex(0x131A2E),
    error: Rgb::hex(0xFF6B6B),
    warning: Rgb::hex(0xF2C14E),
    info: Rgb::hex(0x7EE787),
    debug: Rgb::hex(0x79C0FF),
    trace: Rgb::hex(0x8B949E),
    success: Rgb::hex(0x3FB950),
    cursor: Rgb::hex(0x2F81F7),
};

pub const KOPIT_DIA: Theme = Theme {
    id: "kopit-dia",
    name: "Kopit Día",
    dark: false,
    background: Rgb::hex(0xF6F8FA),
    foreground: Rgb::hex(0x1F2328),
    muted: Rgb::hex(0x6E7781),
    accent: Rgb::hex(0x0969DA),
    on_accent: Rgb::hex(0xFFFFFF),
    bar_background: Rgb::hex(0xE4E9F0),
    bar_foreground: Rgb::hex(0x24292F),
    panel: Rgb::hex(0xFFFFFF),
    error: Rgb::hex(0xCF222E),
    warning: Rgb::hex(0x9A6700),
    info: Rgb::hex(0x1A7F37),
    debug: Rgb::hex(0x0550AE),
    trace: Rgb::hex(0x6E7781),
    success: Rgb::hex(0x1A7F37),
    cursor: Rgb::hex(0x0969DA),
};

pub const AURORA: Theme = Theme {
    id: "aurora",
    name: "Aurora",
    dark: true,
    background: Rgb::hex(0x14102A),
    foreground: Rgb::hex(0xE9E3FF),
    muted: Rgb::hex(0x7D74A8),
    accent: Rgb::hex(0x2DD4BF),
    on_accent: Rgb::hex(0x0B1F1C),
    bar_background: Rgb::hex(0x231B45),
    bar_foreground: Rgb::hex(0xDCD3FF),
    panel: Rgb::hex(0x1B1538),
    error: Rgb::hex(0xFF7AA2),
    warning: Rgb::hex(0xFFC56B),
    info: Rgb::hex(0x9EF0C8),
    debug: Rgb::hex(0xB69CFF),
    trace: Rgb::hex(0x8F88B5),
    success: Rgb::hex(0x5EEAD4),
    cursor: Rgb::hex(0x2DD4BF),
};

pub const AMBAR: Theme = Theme {
    id: "ambar",
    name: "Ámbar (terminal retro)",
    dark: true,
    background: Rgb::hex(0x140C00),
    foreground: Rgb::hex(0xFFB000),
    muted: Rgb::hex(0x9A6A00),
    accent: Rgb::hex(0xFFB000),
    on_accent: Rgb::hex(0x140C00),
    bar_background: Rgb::hex(0x2A1A00),
    bar_foreground: Rgb::hex(0xFFC940),
    panel: Rgb::hex(0x1C1200),
    error: Rgb::hex(0xFF5A1F),
    warning: Rgb::hex(0xFFD24D),
    info: Rgb::hex(0xFFB000),
    debug: Rgb::hex(0xD08F00),
    trace: Rgb::hex(0x8A5E00),
    success: Rgb::hex(0xFFD24D),
    cursor: Rgb::hex(0xFFB000),
};

pub const MATRIZ: Theme = Theme {
    id: "matriz",
    name: "Matriz (fósforo verde)",
    dark: true,
    background: Rgb::hex(0x020A04),
    foreground: Rgb::hex(0x33FF66),
    muted: Rgb::hex(0x1A8A3A),
    accent: Rgb::hex(0x33FF66),
    on_accent: Rgb::hex(0x020A04),
    bar_background: Rgb::hex(0x06200E),
    bar_foreground: Rgb::hex(0x7DFF9F),
    panel: Rgb::hex(0x041408),
    error: Rgb::hex(0xFF4D4D),
    warning: Rgb::hex(0xD4FF4D),
    info: Rgb::hex(0x33FF66),
    debug: Rgb::hex(0x2BD957),
    trace: Rgb::hex(0x1A8A3A),
    success: Rgb::hex(0x7DFF9F),
    cursor: Rgb::hex(0x33FF66),
};

/// Pensado para baja visión: contraste máximo (WCAG AAA en todos los roles).
pub const ALTO_CONTRASTE: Theme = Theme {
    id: "alto-contraste",
    name: "Alto contraste",
    dark: true,
    background: Rgb::hex(0x000000),
    foreground: Rgb::hex(0xFFFFFF),
    muted: Rgb::hex(0xC0C0C0),
    accent: Rgb::hex(0xFFFF00),
    on_accent: Rgb::hex(0x000000),
    bar_background: Rgb::hex(0xFFFF00),
    bar_foreground: Rgb::hex(0x000000),
    panel: Rgb::hex(0x000000),
    error: Rgb::hex(0xFF6060),
    warning: Rgb::hex(0xFFFF00),
    info: Rgb::hex(0x00FFFF),
    debug: Rgb::hex(0x80C0FF),
    trace: Rgb::hex(0xC0C0C0),
    success: Rgb::hex(0x00FF00),
    cursor: Rgb::hex(0xFFFF00),
};

/// Todos los temas integrados.
pub static THEMES: [Theme; 6] = [KOPIT_NOCHE, KOPIT_DIA, AURORA, AMBAR, MATRIZ, ALTO_CONTRASTE];

pub fn find(id: &str) -> Option<&'static Theme> {
    THEMES.iter().find(|t| t.id.eq_ignore_ascii_case(id))
}

pub fn default_theme() -> &'static Theme {
    &THEMES[0]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colores_desde_texto() {
        assert_eq!(Rgb::parse("#2f81f7"), Some(Rgb { r: 0x2F, g: 0x81, b: 0xF7 }));
        assert_eq!(Rgb::parse("fff"), Some(Rgb { r: 255, g: 255, b: 255 }));
        assert_eq!(Rgb::parse("#12"), None);
        assert_eq!(Rgb::parse("zzzzzz"), None);
    }

    #[test]
    fn mezcla_y_luma() {
        let negro = Rgb::hex(0);
        let blanco = Rgb::hex(0xFFFFFF);
        assert_eq!(negro.mix(blanco, 255), blanco);
        assert_eq!(negro.mix(blanco, 0), negro);
        assert!(blanco.luma() > 250);
    }

    #[test]
    fn busqueda_y_ajustes() {
        let mut t = *find("AURORA").unwrap();
        assert!(t.apply_override("acento", "#ff8800"));
        assert_eq!(t.accent, Rgb::hex(0xFF8800));
        assert!(!t.apply_override("inventado", "#000"));
        assert!(find("no-existe").is_none());
    }

    #[test]
    fn todos_los_temas_se_leen() {
        for t in THEMES.iter() {
            // El texto debe distinguirse del fondo en todos los temas.
            let diff = (t.foreground.luma() as i32 - t.background.luma() as i32).abs();
            assert!(diff > 90, "contraste insuficiente en {}", t.id);
        }
    }
}
