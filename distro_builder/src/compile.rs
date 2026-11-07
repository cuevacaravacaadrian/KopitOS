//! Compilación del núcleo y del cargador para un perfil.
//!
//! Orden (importa): **núcleo → huella SHA-256 → cargador**. La huella entra en
//! el binario del cargador como constante de compilación; al firmar el
//! cargador para Secure Boot, la firma cubre también la huella del núcleo.

use crate::profile::Profile;
use crate::util;
use std::path::{Path, PathBuf};
use std::process::Command;

pub struct Ctx {
    pub root: PathBuf,
    pub arch: String,
    pub release: bool,
}

impl Ctx {
    pub fn kernel_triple(&self) -> Result<&'static str, String> {
        match self.arch.as_str() {
            "x86_64" => Ok("x86_64-unknown-none"),
            "aarch64" => Ok("aarch64-unknown-none"),
            "riscv64" => Ok("riscv64gc-unknown-none-elf"),
            other => Err(format!("arquitectura desconocida: {other} (x86_64, aarch64, riscv64)")),
        }
    }

    pub fn loader_triple(&self) -> Option<&'static str> {
        (self.arch == "x86_64").then_some("x86_64-unknown-uefi")
    }

    pub fn cargo_profile_dir(&self) -> &'static str {
        if self.release { "release" } else { "debug" }
    }

    pub fn kernel_elf(&self) -> Result<PathBuf, String> {
        Ok(self.root.join("target").join(self.kernel_triple()?).join(self.cargo_profile_dir()).join("kopit-kernel"))
    }

    pub fn out_dir(&self, profile: &str) -> PathBuf {
        self.root.join("target").join("kopitos").join(&self.arch).join(profile)
    }
}

pub fn cargo(root: &Path) -> Command {
    let mut c = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()));
    c.current_dir(root);
    c
}

fn run(mut cmd: Command, what: &str) -> Result<(), String> {
    let status = cmd.status().map_err(|e| format!("no se pudo ejecutar cargo ({what}): {e}"))?;
    if status.success() { Ok(()) } else { Err(format!("falló la compilación de {what}")) }
}

/// `llvm-objcopy` de la propia toolchain (componente llvm-tools).
fn objcopy() -> Option<PathBuf> {
    let sysroot = Command::new("rustc").args(["--print", "sysroot"]).output().ok()?;
    let sysroot = PathBuf::from(String::from_utf8(sysroot.stdout).ok()?.trim());
    let vv = Command::new("rustc").arg("-vV").output().ok()?;
    let host = String::from_utf8(vv.stdout).ok()?.lines().find_map(|l| l.strip_prefix("host: ").map(str::to_string))?;
    let exe = if cfg!(windows) { "llvm-objcopy.exe" } else { "llvm-objcopy" };
    let p = sysroot.join("lib").join("rustlib").join(host).join("bin").join(exe);
    p.is_file().then_some(p)
}

pub struct Built {
    /// ELF con símbolos (para addr2line y PVH).
    pub kernel_elf: PathBuf,
    /// Lo que va dentro de la imagen (sin información de depuración).
    pub kernel_image: Vec<u8>,
    pub kernel_sha256: String,
    pub loader: Option<Vec<u8>>,
}

pub fn build(ctx: &Ctx, profile: &Profile, build_id: &str) -> Result<Built, String> {
    let triple = ctx.kernel_triple()?;
    let mut cmd = cargo(&ctx.root);
    cmd.args(["build", "-p", "kopit-kernel", "--target", triple]);
    if !profile.kernel_features.is_empty() {
        cmd.args(["--no-default-features", "--features", &profile.kernel_features.join(",")]);
    }
    if ctx.release {
        cmd.arg("--release");
    }
    cmd.env("KOPIT_PROFILE", &profile.id)
        .env("KOPIT_DISTRO_NAME", &profile.name)
        .env("KOPIT_DEVICE_CLASS", &profile.device_class)
        .env("KOPIT_BUILD_ID", build_id);
    run(cmd, "el núcleo")?;
    let elf = ctx.kernel_elf()?;
    let full = std::fs::read(&elf).map_err(|e| format!("{}: {e}", elf.display()))?;

    // Imagen sin depuración (más pequeña y rápida de cargar).
    let stripped_path = ctx.out_dir(&profile.id).join("kernel.stripped.elf");
    std::fs::create_dir_all(stripped_path.parent().unwrap()).ok();
    let kernel_image = match objcopy() {
        Some(oc) => {
            let ok = Command::new(oc)
                .arg("--strip-debug")
                .arg(&elf)
                .arg(&stripped_path)
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
            if ok { std::fs::read(&stripped_path).unwrap_or(full.clone()) } else { full.clone() }
        }
        None => full.clone(),
    };
    let digest = kopit_crypto::sha256::digest(&kernel_image);
    let sha = String::from_utf8(kopit_crypto::sha256::to_hex(&digest).to_vec()).unwrap();
    util::step(
        "Núcleo",
        &format!(
            "{} → {} (SHA-256 {}…)",
            util::human(full.len() as u64),
            util::human(kernel_image.len() as u64),
            &sha[..16]
        ),
    );

    let loader = match ctx.loader_triple() {
        Some(lt) => {
            let mut cmd = cargo(&ctx.root);
            cmd.args(["build", "-p", "kopitboot", "--target", lt]);
            if ctx.release {
                cmd.arg("--release");
            }
            cmd.env("KOPIT_KERNEL_SHA256", &sha);
            run(cmd, "KopitBoot")?;
            let p = ctx.root.join("target").join(lt).join(ctx.cargo_profile_dir()).join("kopitboot.efi");
            let data = std::fs::read(&p).map_err(|e| format!("{}: {e}", p.display()))?;
            util::step(
                "KopitBoot (UEFI)",
                &format!("{} · huella del núcleo integrada", util::human(data.len() as u64)),
            );
            Some(data)
        }
        None => None,
    };
    Ok(Built { kernel_elf: elf, kernel_image, kernel_sha256: sha, loader })
}
