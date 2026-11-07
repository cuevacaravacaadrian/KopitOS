//! Script de compilación del núcleo: elige el guion del enlazador según la
//! arquitectura y registra los datos de la compilación que el núcleo muestra.

use std::env;

fn main() {
    let arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    let os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let dir = env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");

    // Sólo en objetivos bare-metal: en el anfitrión (cargo check) no se enlaza.
    if os == "none" {
        let script = format!("{dir}/linker/{arch}.ld");
        println!("cargo:rustc-link-arg-bins=-T{script}");
        println!("cargo:rerun-if-changed={script}");
    }

    // Datos de la distribución que inyecta distro_builder (ver profiles/*.toml).
    for var in ["KOPIT_PROFILE", "KOPIT_DISTRO_NAME", "KOPIT_DEVICE_CLASS", "KOPIT_BUILD_ID"] {
        println!("cargo:rerun-if-env-changed={var}");
    }
    println!("cargo:rerun-if-changed=build.rs");
}
