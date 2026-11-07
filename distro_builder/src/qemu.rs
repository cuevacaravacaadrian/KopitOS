//! QEMU: localizar el emulador y el firmware, arrancar y hacer pruebas.

use crate::util::{first_existing, which};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub struct Tools {
    pub qemu: PathBuf,
    pub uefi_code: Option<PathBuf>,
    pub uefi_vars: Option<PathBuf>,
}

fn qemu_name(arch: &str) -> String {
    format!("qemu-system-{arch}")
}

pub fn find_qemu(arch: &str) -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("KOPIT_QEMU") {
        return Some(PathBuf::from(p));
    }
    let name = qemu_name(arch);
    if let Some(p) = which(&name) {
        return Some(p);
    }
    let exe = if cfg!(windows) { format!("{name}.exe") } else { name };
    let dirs = [
        r"C:\Program Files\qemu",
        r"C:\msys64\ucrt64\bin",
        r"C:\msys64\mingw64\bin",
        r"C:\msys64\clang64\bin",
        "/opt/homebrew/bin",
        "/usr/local/bin",
        "/usr/bin",
    ];
    first_existing(&dirs.iter().map(|d| Path::new(d).join(&exe)).collect::<Vec<_>>())
}

/// Firmware UEFI (OVMF/EDK II): `(código, plantilla de variables)`.
pub fn find_firmware(arch: &str, qemu: &Path) -> (Option<PathBuf>, Option<PathBuf>) {
    if let (Some(c), v) = (std::env::var_os("KOPIT_OVMF_CODE"), std::env::var_os("KOPIT_OVMF_VARS")) {
        return (Some(c.into()), v.map(Into::into));
    }
    let bin = qemu.parent().unwrap_or(Path::new("."));
    let mut share = vec![bin.join("../share/qemu"), bin.join("share"), PathBuf::from("/usr/share/qemu")];
    let (code_names, vars_names): (&[&str], &[&str]) = match arch {
        "x86_64" => (&["edk2-x86_64-code.fd"], &["edk2-i386-vars.fd", "edk2-x86_64-vars.fd"]),
        "aarch64" => (&["edk2-aarch64-code.fd"], &["edk2-arm-vars.fd"]),
        "riscv64" => (&["edk2-riscv-code.fd"], &["edk2-riscv-vars.fd"]),
        _ => (&[], &[]),
    };
    let mut code: Vec<PathBuf> = share.iter().flat_map(|d| code_names.iter().map(move |n| d.join(n))).collect();
    let mut vars: Vec<PathBuf> = share.iter().flat_map(|d| vars_names.iter().map(move |n| d.join(n))).collect();
    if arch == "x86_64" {
        for (c, v) in [
            ("/usr/share/OVMF/OVMF_CODE_4M.fd", "/usr/share/OVMF/OVMF_VARS_4M.fd"),
            ("/usr/share/OVMF/OVMF_CODE.fd", "/usr/share/OVMF/OVMF_VARS.fd"),
            ("/usr/share/edk2/x64/OVMF_CODE.4m.fd", "/usr/share/edk2/x64/OVMF_VARS.4m.fd"),
            ("/usr/share/edk2/ovmf/OVMF_CODE.fd", "/usr/share/edk2/ovmf/OVMF_VARS.fd"),
            ("/usr/share/edk2-ovmf/x64/OVMF_CODE.fd", "/usr/share/edk2-ovmf/x64/OVMF_VARS.fd"),
        ] {
            code.push(c.into());
            vars.push(v.into());
        }
    }
    share.clear();
    (first_existing(&code), first_existing(&vars))
}

pub fn tools(arch: &str) -> Result<Tools, String> {
    let qemu = find_qemu(arch).ok_or_else(|| {
        format!("no encuentro {} (instálalo o indica la ruta con la variable KOPIT_QEMU)", qemu_name(arch))
    })?;
    let (uefi_code, uefi_vars) = find_firmware(arch, &qemu);
    Ok(Tools { qemu, uefi_code, uefi_vars })
}

pub enum Boot {
    Disk(PathBuf),
    Cdrom(PathBuf),
    /// Núcleo cargado por QEMU sin firmware UEFI (PVH en x86_64, `virt` en ARM64/RISC-V).
    Direct {
        kernel: PathBuf,
        cmdline: String,
    },
}

pub struct RunConfig {
    pub boot: Boot,
    pub name: String,
    pub headless: bool,
    pub serial_log: Option<PathBuf>,
    pub memory: String,
    pub cpu: String,
    pub vars_copy: PathBuf,
    pub extra: Vec<String>,
}

/// ARM64 y RISC-V (fase 1): QEMU `virt` arrancando el ELF del núcleo directamente.
fn virt_command(t: &Tools, arch: &str, cfg: &RunConfig) -> Result<Command, String> {
    let Boot::Direct { kernel, cmdline } = &cfg.boot else {
        return Err(format!(
            "en {arch} la fase 1 sólo arranca en modo directo (--boot direct); KopitBoot ARM64/RISC-V llega en la fase 2"
        ));
    };
    let mut c = Command::new(&t.qemu);
    c.args(["-name", &cfg.name, "-machine", "virt", "-m", &cfg.memory]);
    match arch {
        "aarch64" => {
            let cpu = if cfg.cpu == "max" { "cortex-a72" } else { &cfg.cpu };
            c.args(["-cpu", cpu, "-semihosting"]);
        }
        _ => {
            c.args(["-bios", "default"]);
        }
    }
    c.arg("-kernel").arg(kernel).arg("-append").arg(cmdline);
    c.args(["-display", "none", "-no-reboot"]);
    match &cfg.serial_log {
        Some(p) => c.arg("-serial").arg(format!("file:{}", p.display())),
        None => c.args(["-serial", "stdio"]),
    };
    if cfg.headless {
        c.args(["-monitor", "none"]);
    }
    c.args(&cfg.extra);
    Ok(c)
}

/// Código de salida de QEMU cuando las pruebas de humo pasan.
pub fn success_code(arch: &str) -> i32 {
    // x86_64: isa-debug-exit sale con (0x10 << 1) | 1; RISC-V (sifive_test) y ARM64 (semihosting): 0.
    if arch == "x86_64" { 33 } else { 0 }
}

pub fn command(t: &Tools, arch: &str, cfg: &RunConfig) -> Result<Command, String> {
    if arch != "x86_64" {
        return virt_command(t, arch, cfg);
    }
    let mut c = Command::new(&t.qemu);
    c.args(["-name", &cfg.name, "-machine", "q35", "-m", &cfg.memory, "-cpu", &cfg.cpu]);
    match &cfg.boot {
        Boot::Disk(_) | Boot::Cdrom(_) => {
            let code =
                t.uefi_code.as_ref().ok_or("no encuentro el firmware UEFI (edk2/OVMF); indica KOPIT_OVMF_CODE")?;
            c.arg("-drive").arg(format!("if=pflash,format=raw,unit=0,readonly=on,file={}", code.display()));
            if let Some(vars) = &t.uefi_vars {
                if !cfg.vars_copy.exists() {
                    std::fs::copy(vars, &cfg.vars_copy).map_err(|e| format!("copiando las variables UEFI: {e}"))?;
                }
                c.arg("-drive").arg(format!("if=pflash,format=raw,unit=1,file={}", cfg.vars_copy.display()));
            }
        }
        Boot::Direct { .. } => {}
    }
    match &cfg.boot {
        Boot::Disk(img) => {
            c.arg("-drive").arg(format!("if=none,id=disco,format=raw,file={}", img.display()));
            c.args(["-device", "ide-hd,drive=disco,bootindex=0"]);
        }
        Boot::Cdrom(iso) => {
            c.arg("-drive").arg(format!("if=none,id=cd,media=cdrom,format=raw,file={}", iso.display()));
            c.args(["-device", "ide-cd,drive=cd,bootindex=0"]);
        }
        Boot::Direct { kernel, cmdline } => {
            c.arg("-kernel").arg(kernel).arg("-append").arg(cmdline);
        }
    }
    // Hardware de demostración para `pci`: red virtio y USB 3.
    c.args(["-nic", "user,model=virtio-net-pci", "-device", "qemu-xhci"]);
    c.args(["-device", "isa-debug-exit,iobase=0xf4,iosize=0x04", "-no-reboot"]);
    match &cfg.serial_log {
        Some(p) => {
            c.arg("-serial").arg(format!("file:{}", p.display()));
        }
        None => {
            c.args(["-serial", "stdio"]);
        }
    }
    if cfg.headless {
        c.args(["-display", "none", "-monitor", "none"]);
    }
    c.args(&cfg.extra);
    Ok(c)
}

/// Resultado de una prueba de humo.
pub struct SmokeResult {
    pub passed: bool,
    pub exit_code: Option<i32>,
    pub timed_out: bool,
    pub seconds: f64,
    pub summary: String,
}

/// Ejecuta QEMU sin ventana y espera `KOPIT-SMOKE` (o el tiempo límite).
pub fn smoke(mut cmd: Command, log: &Path, timeout: Duration, success: i32) -> Result<SmokeResult, String> {
    let _ = std::fs::remove_file(log);
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    let start = Instant::now();
    let mut child = cmd.spawn().map_err(|e| format!("no se pudo lanzar QEMU: {e}"))?;
    let mut timed_out = false;
    let status = loop {
        if let Some(s) = child.try_wait().map_err(|e| e.to_string())? {
            break Some(s);
        }
        if start.elapsed() > timeout {
            let _ = child.kill();
            let _ = child.wait();
            timed_out = true;
            break None;
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    let output = std::fs::read(log).map(|b| String::from_utf8_lossy(&b).to_string()).unwrap_or_default();
    let summary =
        output.lines().rev().find(|l| l.contains("KOPIT-SMOKE")).unwrap_or("(sin resultado)").trim().to_string();
    let code = status.and_then(|s| s.code());
    let passed = code == Some(success) && output.contains("KOPIT-SMOKE: OK");
    Ok(SmokeResult { passed, exit_code: code, timed_out, seconds: start.elapsed().as_secs_f64(), summary })
}
