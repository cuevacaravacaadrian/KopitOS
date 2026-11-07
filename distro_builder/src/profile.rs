//! # Perfiles de distribución (`profiles/*.toml`)
//!
//! Un perfil describe una distribución completa: qué funcionalidades lleva el
//! núcleo (en compilación), con qué línea de órdenes arranca (en ejecución),
//! cómo se ve (tema, colores, teclado) y cómo es su imagen. Las
//! **distribuciones derivadas** son perfiles que heredan de otro
//! (`inherits = "desktop"`) y cambian sólo lo que quieren.
//!
//! El formato es un subconjunto de TOML (secciones, cadenas, enteros,
//! booleanos y listas) con un analizador propio: la herramienta no necesita
//! descargar nada para funcionar.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Str(String),
    Int(i64),
    Bool(bool),
    List(Vec<Value>),
}

impl Value {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }
}

/// Claves planas `seccion.clave` → valor.
pub type Document = BTreeMap<String, Value>;

fn strip_comment(line: &str) -> &str {
    let mut in_str = None;
    for (i, c) in line.char_indices() {
        match (c, in_str) {
            ('"', None) | ('\'', None) => in_str = Some(c),
            (q, Some(open)) if q == open => in_str = None,
            ('#', None) => return &line[..i],
            _ => {}
        }
    }
    line
}

fn parse_string(s: &str) -> Result<(String, &str), String> {
    let quote = s.chars().next().unwrap();
    let mut out = String::new();
    let mut chars = s[1..].char_indices();
    while let Some((i, c)) = chars.next() {
        match c {
            c if c == quote => return Ok((out, &s[1 + i + 1..])),
            '\\' if quote == '"' => match chars.next().map(|(_, e)| e) {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('"') => out.push('"'),
                Some('\\') => out.push('\\'),
                other => return Err(format!("escape no válido: \\{}", other.unwrap_or(' '))),
            },
            c => out.push(c),
        }
    }
    Err("cadena sin cerrar".into())
}

fn parse_value(s: &str) -> Result<(Value, &str), String> {
    let s = s.trim_start();
    match s.chars().next() {
        Some('"') | Some('\'') => parse_string(s).map(|(v, rest)| (Value::Str(v), rest)),
        Some('[') => {
            let mut items = Vec::new();
            let mut rest = s[1..].trim_start();
            loop {
                if let Some(r) = rest.strip_prefix(']') {
                    return Ok((Value::List(items), r));
                }
                let (v, r) = parse_value(rest)?;
                items.push(v);
                rest = r.trim_start();
                if let Some(r) = rest.strip_prefix(',') {
                    rest = r.trim_start();
                } else if !rest.starts_with(']') {
                    return Err("se esperaba ',' o ']' en la lista".into());
                }
            }
        }
        Some(_) => {
            let end = s.find(|c: char| c == ',' || c == ']' || c.is_whitespace()).unwrap_or(s.len());
            let (tok, rest) = s.split_at(end);
            let v = match tok {
                "true" => Value::Bool(true),
                "false" => Value::Bool(false),
                t => Value::Int(t.replace('_', "").parse().map_err(|_| format!("valor no válido: {t}"))?),
            };
            Ok((v, rest))
        }
        None => Err("falta el valor".into()),
    }
}

pub fn parse(text: &str) -> Result<Document, String> {
    let mut doc = Document::new();
    let mut section = String::new();
    let mut pending = String::new();
    for (n, raw) in text.lines().enumerate() {
        let line = strip_comment(raw).trim();
        let line = if pending.is_empty() {
            line.to_string()
        } else {
            pending.push(' ');
            pending.push_str(line);
            std::mem::take(&mut pending)
        };
        if line.is_empty() {
            continue;
        }
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            section = name.trim().to_string();
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            return Err(format!("línea {}: se esperaba `clave = valor`", n + 1));
        };
        // Listas en varias líneas: acumular hasta cerrar el corchete.
        if value.matches('[').count() > value.matches(']').count() {
            pending = line.clone();
            continue;
        }
        let (v, rest) = parse_value(value).map_err(|e| format!("línea {}: {}", n + 1, e))?;
        if !rest.trim().is_empty() {
            return Err(format!("línea {}: texto sobrante `{}`", n + 1, rest.trim()));
        }
        let key = key.trim().trim_matches('"');
        let full = if section.is_empty() { key.to_string() } else { format!("{section}.{key}") };
        doc.insert(full, v);
    }
    if !pending.is_empty() {
        return Err("lista sin cerrar al final del archivo".into());
    }
    Ok(doc)
}

/// Perfil resuelto (con la herencia aplicada).
#[derive(Clone, Debug)]
pub struct Profile {
    pub id: String,
    pub name: String,
    pub description: String,
    pub version: String,
    pub device_class: String,
    pub kernel_features: Vec<String>,
    pub cmdline: String,
    pub timer_hz: Option<i64>,
    pub boot_timeout: i64,
    pub resolution: String,
    pub safe_mode_entry: bool,
    pub test_entry: bool,
    pub theme: String,
    pub colors: Vec<(String, String)>,
    pub keymap: String,
    pub font_scale: String,
    pub image_size_mib: i64,
    pub overlay: Option<PathBuf>,
    pub chain: Vec<String>,
}

fn load_doc(dir: &Path, id: &str, chain: &mut Vec<String>) -> Result<Document, String> {
    if chain.iter().any(|c| c == id) {
        return Err(format!("herencia circular: {} → {}", chain.join(" → "), id));
    }
    chain.push(id.to_string());
    let path = dir.join(format!("{id}.toml"));
    let text = fs::read_to_string(&path).map_err(|e| format!("no se pudo leer {}: {}", path.display(), e))?;
    let doc = parse(&text).map_err(|e| format!("{}: {}", path.display(), e))?;
    match doc.get("distro.inherits").and_then(Value::as_str) {
        Some(parent) => {
            let mut base = load_doc(dir, parent, chain)?;
            base.extend(doc);
            Ok(base)
        }
        None => Ok(doc),
    }
}

pub fn load(dir: &Path, id: &str) -> Result<Profile, String> {
    let mut chain = Vec::new();
    let doc = load_doc(dir, id, &mut chain)?;
    chain.reverse();
    let s = |k: &str, d: &str| doc.get(k).and_then(Value::as_str).unwrap_or(d).to_string();
    let i = |k: &str| match doc.get(k) {
        Some(Value::Int(v)) => Some(*v),
        _ => None,
    };
    let b = |k: &str, d: bool| match doc.get(k) {
        Some(Value::Bool(v)) => *v,
        _ => d,
    };
    let list = |k: &str| match doc.get(k) {
        Some(Value::List(v)) => v.iter().filter_map(|x| x.as_str().map(str::to_string)).collect(),
        _ => Vec::new(),
    };
    let colors = doc
        .iter()
        .filter_map(|(k, v)| Some((k.strip_prefix("ui.colors.")?.to_string(), v.as_str()?.to_string())))
        .collect();
    Ok(Profile {
        id: id.to_string(),
        name: s("distro.name", "KopitOS"),
        description: s("distro.description", ""),
        version: s("distro.version", env!("CARGO_PKG_VERSION")),
        device_class: s("distro.device_class", "desktop"),
        kernel_features: list("kernel.features"),
        cmdline: s("kernel.cmdline", ""),
        timer_hz: i("kernel.timer_hz"),
        boot_timeout: i("boot.timeout").unwrap_or(3),
        resolution: s("boot.resolution", "auto"),
        safe_mode_entry: b("boot.safe_mode_entry", true),
        test_entry: b("boot.test_entry", true),
        theme: s("ui.theme", "kopit-noche"),
        colors,
        keymap: s("ui.keymap", "es"),
        font_scale: s("ui.font_scale", "auto"),
        image_size_mib: i("image.size_mib").unwrap_or(64),
        overlay: doc.get("image.overlay").and_then(Value::as_str).map(|p| dir.join(p)),
        chain,
    })
}

impl Profile {
    /// Línea de órdenes del núcleo derivada del perfil.
    pub fn kernel_cmdline(&self, extra: &str) -> String {
        let mut parts =
            vec![format!("profile={}", self.id), format!("theme={}", self.theme), format!("kbd={}", self.keymap)];
        if let Some(hz) = self.timer_hz {
            parts.push(format!("hz={hz}"));
        }
        if self.font_scale != "auto" {
            parts.push(format!("fbcon.scale={}", self.font_scale));
        }
        for (role, color) in &self.colors {
            parts.push(format!("theme.{role}={color}"));
        }
        if !self.cmdline.is_empty() {
            parts.push(self.cmdline.clone());
        }
        if !extra.trim().is_empty() {
            parts.push(extra.trim().to_string());
        }
        parts.join(" ")
    }
}

/// (id, nombre, descripción) de todos los perfiles.
pub fn list(dir: &Path) -> Vec<(String, String, String)> {
    let mut out = Vec::new();
    if let Ok(entries) = fs::read_dir(dir) {
        for e in entries.flatten() {
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()) != Some("toml") {
                continue;
            }
            let id = p.file_stem().unwrap().to_string_lossy().to_string();
            if let Ok(prof) = load(dir, &id) {
                out.push((id, prof.name, prof.description));
            }
        }
    }
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn analiza_el_subconjunto_de_toml() {
        let doc = parse(
            r#"
            # comentario
            [distro]
            name = "Mi Distro"   # otro comentario
            version = '1.0'
            [kernel]
            features = [
                "fbcon",
                "shell",
            ]
            timer_hz = 1_000
            [boot]
            safe = false
            "#,
        )
        .unwrap();
        assert_eq!(doc["distro.name"], Value::Str("Mi Distro".into()));
        assert_eq!(doc["distro.version"], Value::Str("1.0".into()));
        assert_eq!(doc["kernel.features"], Value::List(vec![Value::Str("fbcon".into()), Value::Str("shell".into())]));
        assert_eq!(doc["kernel.timer_hz"], Value::Int(1000));
        assert_eq!(doc["boot.safe"], Value::Bool(false));
    }

    #[test]
    fn errores_con_numero_de_linea() {
        assert!(parse("[a]\nclave").unwrap_err().contains("línea 2"));
        assert!(parse("x = \"abierta").is_err());
        assert!(parse("x = [1, 2").is_err());
    }

    #[test]
    fn herencia_de_perfiles() {
        let dir = std::env::temp_dir().join(format!("kopit-perfiles-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("base.toml"), "[distro]\nname = \"Base\"\n[ui]\ntheme = \"aurora\"\nkeymap = \"us\"\n")
            .unwrap();
        fs::write(dir.join("hija.toml"), "[distro]\ninherits = \"base\"\nname = \"Hija\"\n[ui]\nkeymap = \"es\"\n[ui.colors]\naccent = \"#ff8800\"\n").unwrap();
        fs::write(dir.join("ciclo.toml"), "[distro]\ninherits = \"ciclo\"\n").unwrap();
        let p = load(&dir, "hija").unwrap();
        assert_eq!(p.name, "Hija");
        assert_eq!(p.theme, "aurora");
        assert_eq!(p.keymap, "es");
        assert_eq!(p.chain, vec!["base", "hija"]);
        assert!(p.kernel_cmdline("").contains("theme.accent=#ff8800"));
        assert!(load(&dir, "ciclo").unwrap_err().contains("circular"));
        fs::remove_dir_all(&dir).ok();
    }
}
