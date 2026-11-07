//! Identidad de esta compilación. `distro_builder` inyecta el perfil y el
//! nombre de la distribución en tiempo de compilación (variables `KOPIT_*`).

pub const NAME: &str = "KopitOS";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const CODENAME: &str = "Génesis";
pub const ARCH: &str = crate::hal::NAME;

const fn or(v: Option<&'static str>, default: &'static str) -> &'static str {
    match v {
        Some(s) => s,
        None => default,
    }
}

/// Perfil con el que se compiló (`profiles/<id>.toml`).
pub const PROFILE: &str = or(option_env!("KOPIT_PROFILE"), "desarrollo");
/// Nombre comercial de la distribución (las derivadas ponen el suyo).
pub const DISTRO_NAME: &str = or(option_env!("KOPIT_DISTRO_NAME"), "KopitOS");
/// Clase de dispositivo: desktop, laptop, server, tablet, phone, iot.
pub const DEVICE_CLASS: &str = or(option_env!("KOPIT_DEVICE_CLASS"), "desktop");
/// Identificador de la compilación (fecha reproducible o "local").
pub const BUILD_ID: &str = or(option_env!("KOPIT_BUILD_ID"), "local");
