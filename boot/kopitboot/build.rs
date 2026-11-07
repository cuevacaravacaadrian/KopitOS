//! Recompila el cargador cuando cambia la huella del núcleo que debe integrar.
fn main() {
    println!("cargo:rerun-if-env-changed=KOPIT_KERNEL_SHA256");
}
