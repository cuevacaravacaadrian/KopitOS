//! # `kopit` — el constructor de distribuciones de KopitOS
//!
//! ```text
//!  profiles/desktop.toml ─┐
//!                         ├─► cargo (núcleo) ─► SHA-256 ─► cargo (KopitBoot + huella)
//!  overlay/ (opcional) ───┘                                        │
//!                         ┌────────────────────────────────────────┘
//!                         ├─► FAT32 ─► GPT ──────────► kopitos-desktop-x86_64.img
//!                         └─► FAT16 ─► ISO9660 + El Torito + GPT ─► …iso (CD y USB)
//! ```
//!
//! Sin dependencias externas: FAT, GPT, ISO, SHA-256 y CRC32 están escritos
//! en este repositorio. Funciona igual en Windows, Linux y macOS.

mod compile;
mod fat;
mod gpt;
mod iso;
mod profile;
mod qemu;
mod symbols;
mod util;

use compile::{Built, Ctx};
use profile::Profile;
use std::path::{Path, PathBuf};
use std::time::Duration;
use util::*;

struct Args {
    cmd: String,
    profile: String,
    arch: String,
    release: bool,
    boot: String,
    headless: bool,
    cmdline: String,
    no_build: bool,
    cpu: String,
    memory: String,
    qemu_args: Vec<String>,
    from: String,
    name: Option<String>,
    timeout: u64,
    positional: Vec<String>,
}

fn help() {
    println!(
        "{BOLD}kopit{RESET} — constructor de distribuciones de KopitOS {}

{BOLD}Uso:{RESET} cargo kopit <orden> [opciones]

{BOLD}Órdenes:{RESET}
  build                 compila el núcleo y KopitBoot para un perfil
  image                 + imagen de disco GPT con la ESP en FAT32 (QEMU, USB con dd/Rufus)
  iso                   + ISO híbrida El Torito: arranca como CD y como USB
  run                   crea la imagen y arranca QEMU (ventana + consola serie aquí)
  test                  pruebas del anfitrión + pruebas de humo en QEMU (UEFI, ISO, PVH)
  profiles              lista los perfiles de profiles/
  new-distro <id>       crea una distribución derivada (--from <perfil> --name \"Nombre\")
  addr2line <dir...>    traduce direcciones de una traza de pánico a funciones
  doctor                comprueba QEMU, firmware UEFI y herramientas opcionales
  grub-iso              ISO con GRUB (Multiboot2); necesita grub-mkrescue (Linux)
  clean                 borra target/kopitos

{BOLD}Opciones:{RESET}
  --profile <id>        perfil (por defecto: desktop)
  --arch <arquitectura> x86_64 (por defecto), aarch64 o riscv64 (QEMU virt)
  --release             compilación optimizada
  --boot <modo>         run: uefi (disco), iso o pvh/direct (sin firmware UEFI)
  --headless            sin ventana: todo por la consola serie
  --cmdline \"...\"       parámetros extra para el núcleo (p. ej. \"theme=aurora loglevel=debug\")
  --cpu <modelo>        modelo de CPU de QEMU (por defecto: max)
  --memory <tamaño>     memoria de la máquina virtual (por defecto: 512M)
  --qemu-arg <arg>      argumento extra para QEMU (repetible)
  --no-build            run: no recompilar
  --timeout <s>         test: segundos por prueba (por defecto: 120)

{BOLD}Ejemplos:{RESET}
  cargo kopit run
  cargo kopit run --boot pvh --headless
  cargo kopit run --cmdline \"theme=ambar fbcon.scale=3\"
  cargo kopit iso --profile server --release
  cargo kopit new-distro mi-distro --from desktop --name \"Mi Distro\"
  cargo kopit run --arch riscv64
  cargo kopit test --arch aarch64
  cargo kopit test",
        env!("CARGO_PKG_VERSION")
    );
}

fn parse_args() -> Result<Args, String> {
    let mut it = std::env::args().skip(1);
    let mut a = Args {
        cmd: it.next().unwrap_or_else(|| "help".into()),
        profile: "desktop".into(),
        arch: "x86_64".into(),
        release: false,
        boot: "uefi".into(),
        headless: false,
        cmdline: String::new(),
        no_build: false,
        cpu: "max".into(),
        memory: "512M".into(),
        qemu_args: Vec::new(),
        from: "desktop".into(),
        name: None,
        timeout: 120,
        positional: Vec::new(),
    };
    while let Some(arg) = it.next() {
        let mut value = |n: &str| it.next().ok_or_else(|| format!("{n} necesita un valor"));
        match arg.as_str() {
            "--profile" | "-p" => a.profile = value("--profile")?,
            "--arch" => a.arch = value("--arch")?,
            "--release" | "-r" => a.release = true,
            "--boot" => a.boot = value("--boot")?,
            "--headless" => a.headless = true,
            "--cmdline" => a.cmdline = value("--cmdline")?,
            "--no-build" => a.no_build = true,
            "--cpu" => a.cpu = value("--cpu")?,
            "--memory" | "-m" => a.memory = value("--memory")?,
            "--qemu-arg" => a.qemu_args.push(value("--qemu-arg")?),
            "--from" => a.from = value("--from")?,
            "--name" => a.name = Some(value("--name")?),
            "--timeout" => a.timeout = value("--timeout")?.parse().map_err(|_| "--timeout necesita segundos")?,
            s if s.starts_with("--") => return Err(format!("opción desconocida: {s}")),
            _ => a.positional.push(arg),
        }
    }
    Ok(a)
}

fn workspace_root() -> PathBuf {
    let compiled = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf();
    if compiled.join("Cargo.toml").exists() { compiled } else { std::env::current_dir().unwrap() }
}

fn main() {
    enable_terminal();
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{RED}error:{RESET} {e}\n");
            help();
            std::process::exit(2);
        }
    };
    let result = match args.cmd.as_str() {
        "build" => prepare(&args, false).map(|_| ()),
        "image" | "img" => make_image(&args, false).map(|_| ()),
        "iso" => make_iso(&args, false).map(|_| ()),
        "run" => run(&args),
        "test" => test(&args),
        "profiles" => profiles(),
        "new-distro" => new_distro(&args),
        "addr2line" => addr2line(&args),
        "doctor" => doctor(&args),
        "grub-iso" => grub_iso(&args),
        "clean" => clean(),
        "help" | "--help" | "-h" | "ayuda" => {
            help();
            Ok(())
        }
        other => Err(format!("orden desconocida: «{other}» (prueba `cargo kopit help`)")),
    };
    if let Err(e) = result {
        eprintln!("\n{RED}error:{RESET} {e}");
        std::process::exit(1);
    }
}

// --- Construcción ----------------------------------------------------------------------

struct Prepared {
    ctx: Ctx,
    profile: Profile,
    built: Built,
    cmdline: String,
    epoch: i64,
}

fn prepare(args: &Args, quiet_title: bool) -> Result<Prepared, String> {
    let root = workspace_root();
    let profile = profile::load(&root.join("profiles"), &args.profile)?;
    let ctx = Ctx { root, arch: args.arch.clone(), release: args.release };
    if !quiet_title {
        title(&format!(
            "{} {} · perfil «{}» · {} · {}",
            profile.name,
            profile.version,
            profile.id,
            ctx.arch,
            if ctx.release { "release" } else { "debug" }
        ));
        if profile.chain.len() > 1 {
            info("Herencia", &profile.chain.join(" → "));
        }
    }
    let epoch = build_epoch();
    let (y, mo, d, ..) = civil(epoch);
    let built = compile::build(&ctx, &profile, &format!("{y:04}.{mo:02}.{d:02}"))?;
    let cmdline = profile.kernel_cmdline(&args.cmdline);
    Ok(Prepared { ctx, profile, built, cmdline, epoch })
}

fn boot_cfg(p: &Profile, cmdline: &str, test_mode: bool) -> String {
    let mut s = format!(
        "# KopitBoot · generado por distro_builder · perfil {}\n# No lo edites a mano: cambia profiles/{}.toml y vuelve a generar la imagen.\n",
        p.id, p.id
    );
    s += &format!("title={}\n", p.name);
    let mut entries = vec![format!("entry={}|\\kopit\\kernel.elf|{}", p.name, cmdline)];
    if p.safe_mode_entry {
        entries.push(format!("entry={} · modo seguro|\\kopit\\kernel.elf|{} safe loglevel=debug", p.name, cmdline));
    }
    if p.test_entry || test_mode {
        entries.push(format!("entry={} · pruebas automáticas|\\kopit\\kernel.elf|{} test=smoke", p.name, cmdline));
    }
    let default = if test_mode { entries.len() - 1 } else { 0 };
    s += &format!(
        "timeout={}\ndefault={}\nresolution={}\n",
        if test_mode { 0 } else { p.boot_timeout },
        default,
        p.resolution
    );
    for e in entries {
        s += &e;
        s.push('\n');
    }
    s
}

fn readme(p: &Profile, sha: &str, epoch: i64) -> String {
    let (y, mo, d, h, mi, _) = civil(epoch);
    format!(
        "{} {}\r\nPerfil: {} ({})\r\nGenerado: {y:04}-{mo:02}-{d:02} {h:02}:{mi:02} UTC\r\nSHA-256 del núcleo: {sha}\r\n\r\nArranca en cualquier PC con UEFI (desactiva CSM/arranque heredado si hace falta).\r\nMás información: README.md del repositorio de KopitOS.\r\n",
        p.name, p.version, p.id, p.device_class
    )
}

fn collect_overlay(dir: &Path, base: &Path, out: &mut Vec<fat::FatFile>) -> Result<(), String> {
    for e in std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?.flatten() {
        let p = e.path();
        if p.is_dir() {
            collect_overlay(&p, base, out)?;
        } else {
            let rel = p.strip_prefix(base).unwrap().to_string_lossy().replace('\\', "/");
            out.push(fat::FatFile { path: rel, data: std::fs::read(&p).map_err(|e| e.to_string())? });
        }
    }
    Ok(())
}

fn esp_files(pre: &Prepared, test_mode: bool) -> Result<Vec<fat::FatFile>, String> {
    let loader = pre.built.loader.clone().ok_or("esta arquitectura todavía no tiene cargador UEFI (fase 2)")?;
    let mut files = vec![
        fat::FatFile { path: "EFI/BOOT/BOOTX64.EFI".into(), data: loader },
        fat::FatFile { path: "kopit/kernel.elf".into(), data: pre.built.kernel_image.clone() },
        fat::FatFile {
            path: "kopit/boot.cfg".into(),
            data: boot_cfg(&pre.profile, &pre.cmdline, test_mode).into_bytes(),
        },
        fat::FatFile {
            path: "leeme.txt".into(),
            data: readme(&pre.profile, &pre.built.kernel_sha256, pre.epoch).into_bytes(),
        },
    ];
    if let Some(ov) = &pre.profile.overlay {
        if ov.is_dir() {
            collect_overlay(ov, ov, &mut files)?;
        }
    }
    Ok(files)
}

fn dos_time(epoch: i64) -> fat::DosTime {
    let (y, mo, d, h, mi, s) = civil(epoch);
    fat::DosTime::new(y, mo, d, h, mi, s)
}

fn volume_id(seed: &[u8]) -> u32 {
    let h = kopit_crypto::sha256::digest(seed);
    u32::from_le_bytes([h[0], h[1], h[2], h[3]])
}

fn out_name(pre: &Prepared, test_mode: bool, ext: &str) -> PathBuf {
    let dir = pre.ctx.out_dir(&pre.profile.id);
    let suffix = if test_mode { "-pruebas" } else { "" };
    dir.join(format!("kopitos-{}-{}{}.{}", pre.profile.id, pre.ctx.arch, suffix, ext))
}

fn make_image(args: &Args, test_mode: bool) -> Result<(Prepared, PathBuf), String> {
    let pre = prepare(args, test_mode)?;
    let files = esp_files(&pre, test_mode)?;
    let content: u64 = files.iter().map(|f| f.data.len() as u64).sum();
    let size_mib = (pre.profile.image_size_mib as u64).max(content / (1 << 20) + 16).max(40);
    let seed = format!("{}:{}", pre.profile.id, pre.built.kernel_sha256);
    let esp = fat::build(
        &files,
        &fat::FatOptions {
            kind: fat::FatKind::Fat32,
            size_bytes: size_mib << 20,
            label: "KOPITOS",
            volume_id: volume_id(seed.as_bytes()),
            hidden_sectors: 2048,
            time: dos_time(pre.epoch),
        },
    )?;
    let disk = gpt::disk_with_esp(&esp, seed.as_bytes());
    let path = out_name(&pre, test_mode, "img");
    write_file(&path, &disk)?;
    step("Imagen de disco", &format!("{} · GPT + ESP FAT32 de {} MiB", path.display(), size_mib));
    Ok((pre, path))
}

fn make_iso(args: &Args, test_mode: bool) -> Result<(Prepared, PathBuf), String> {
    let pre = prepare(args, test_mode)?;
    let files = esp_files(&pre, test_mode)?;
    let content: u64 = files.iter().map(|f| f.data.len() as u64).sum();
    let size_mib = (content / (1 << 20) + 3).max(4);
    let seed = format!("{}:{}:iso", pre.profile.id, pre.built.kernel_sha256);
    let esp = fat::build(
        &files,
        &fat::FatOptions {
            kind: fat::FatKind::Fat16,
            size_bytes: size_mib << 20,
            label: "KOPITOS",
            volume_id: volume_id(seed.as_bytes()),
            hidden_sectors: 0,
            time: dos_time(pre.epoch),
        },
    )?;
    let visible = vec![
        iso::IsoFile { path: "KOPIT/KERNEL.ELF".into(), data: pre.built.kernel_image.clone() },
        iso::IsoFile {
            path: "KOPIT/BOOT.CFG".into(),
            data: boot_cfg(&pre.profile, &pre.cmdline, test_mode).into_bytes(),
        },
        iso::IsoFile {
            path: "LEEME.TXT".into(),
            data: readme(&pre.profile, &pre.built.kernel_sha256, pre.epoch).into_bytes(),
        },
    ];
    let (y, mo, d, h, mi, s) = civil(pre.epoch);
    let vol = format!("KOPITOS_{}", pre.profile.id.to_ascii_uppercase().replace('-', "_"));
    let image = iso::build(
        visible,
        esp,
        &iso::IsoOptions {
            volume_id: &vol,
            time: (y, mo, d, h, mi, s),
            esp_path: "BOOT/EFIBOOT.IMG",
            seed: seed.as_bytes(),
        },
    )?;
    let path = out_name(&pre, test_mode, "iso");
    write_file(&path, &image)?;
    step("ISO híbrida", &format!("{} · {} · El Torito UEFI + GPT", path.display(), human(image.len() as u64)));
    Ok((pre, path))
}

// --- QEMU -----------------------------------------------------------------------------

fn run_config(
    args: &Args,
    boot: qemu::Boot,
    out: &Path,
    profile: &Profile,
    headless: bool,
    serial_log: Option<PathBuf>,
    cpu: &str,
) -> qemu::RunConfig {
    qemu::RunConfig {
        boot,
        name: format!("{} ({})", profile.name, profile.id),
        headless,
        serial_log,
        memory: args.memory.clone(),
        cpu: cpu.to_string(),
        vars_copy: out.join("OVMF_VARS.fd"),
        extra: args.qemu_args.clone(),
    }
}

fn run(args: &Args) -> Result<(), String> {
    let tools = qemu::tools(&args.arch)?;
    // ARM64 y RISC-V todavía no tienen KopitBoot: arrancan el núcleo directamente.
    let mode = if args.arch != "x86_64" { "direct" } else { args.boot.as_str() };
    let (pre, boot) = match mode {
        "uefi" | "disk" | "img" => {
            let (pre, img) = make_image(args, false)?;
            (pre, qemu::Boot::Disk(img))
        }
        "iso" | "cdrom" => {
            let (pre, iso) = make_iso(args, false)?;
            (pre, qemu::Boot::Cdrom(iso))
        }
        "pvh" | "direct" => {
            let pre = prepare(args, false)?;
            let kernel = pre.built.kernel_elf.clone();
            let cmdline = pre.cmdline.clone();
            (pre, qemu::Boot::Direct { kernel, cmdline })
        }
        other => return Err(format!("modo de arranque desconocido: {other} (uefi, iso, pvh)")),
    };
    let out = pre.ctx.out_dir(&pre.profile.id);
    let cfg = run_config(args, boot, &out, &pre.profile, args.headless, None, &args.cpu);
    let mut cmd = qemu::command(&tools, &args.arch, &cfg)?;
    title("Arrancando QEMU");
    info("Emulador", &tools.qemu.display().to_string());
    if let Some(c) = &tools.uefi_code {
        info("Firmware UEFI", &c.display().to_string());
    }
    info("Consola serie", "esta terminal (escribe órdenes aquí o en la ventana; Ctrl+C sale)");
    println!();
    let status = cmd.status().map_err(|e| format!("no se pudo lanzar QEMU: {e}"))?;
    match status.code() {
        Some(0) | Some(33) | None => Ok(()),
        Some(c) => Err(format!("QEMU terminó con el código {c}")),
    }
}

fn test(args: &Args) -> Result<(), String> {
    let root = workspace_root();
    title("Pruebas unitarias en el anfitrión");
    let out = compile::cargo(&root)
        .args(["test", "--workspace", "--exclude", "kopit-kernel", "--exclude", "kopitboot", "--quiet"])
        .output()
        .map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&out.stdout);
    let (mut passed, mut failed) = (0u32, 0u32);
    for l in text.lines().filter(|l| l.starts_with("test result:")) {
        let num = |key: &str| {
            l.split(';')
                .find_map(|p| {
                    p.trim().strip_suffix(key).and_then(|n| n.trim().split_whitespace().last()?.parse::<u32>().ok())
                })
                .unwrap_or(0)
        };
        passed += num("passed");
        failed += num("failed");
    }
    if !out.status.success() {
        println!("{}", String::from_utf8_lossy(&out.stderr));
        return Err(format!("fallan pruebas del anfitrión ({failed} fallidas)"));
    }
    step(
        "cargo test",
        &format!(
            "{passed} pruebas superadas ({} suites)",
            text.lines().filter(|l| l.starts_with("test result:")).count()
        ),
    );

    let tools = qemu::tools(&args.arch)?;
    title(&format!("Pruebas de humo en QEMU ({})", args.arch));
    let mut cases: Vec<(String, qemu::Boot, &str)> = Vec::new();
    let pre = if args.arch == "x86_64" {
        let (pre, img) = make_image(args, true)?;
        let (_, iso) = make_iso(args, true)?;
        cases.push(("UEFI · disco GPT · CPU max (x2APIC)".into(), qemu::Boot::Disk(img), "max"));
        cases.push(("UEFI · ISO híbrida · CPU qemu64 (xAPIC)".into(), qemu::Boot::Cdrom(iso), "qemu64"));
        let direct =
            qemu::Boot::Direct { kernel: pre.built.kernel_elf.clone(), cmdline: format!("{} test=smoke", pre.cmdline) };
        cases.push(("PVH · arranque directo (sin UEFI)".into(), direct, "max"));
        pre
    } else {
        let pre = prepare(args, true)?;
        let firmware = if args.arch == "riscv64" { "OpenSBI → modo S" } else { "EL1 · GICv2" };
        let direct =
            qemu::Boot::Direct { kernel: pre.built.kernel_elf.clone(), cmdline: format!("{} test=smoke", pre.cmdline) };
        cases.push((format!("{} · QEMU virt · {}", args.arch, firmware), direct, "max"));
        pre
    };
    let out = pre.ctx.out_dir(&pre.profile.id);
    let mut all_ok = true;
    for (i, (name, boot, cpu)) in cases.into_iter().enumerate() {
        let log = out.join(format!("serie-prueba-{i}.log"));
        let cfg = run_config(args, boot, &out, &pre.profile, true, Some(log.clone()), cpu);
        let cmd = qemu::command(&tools, &args.arch, &cfg)?;
        let r = qemu::smoke(cmd, &log, Duration::from_secs(args.timeout), qemu::success_code(&args.arch))?;
        all_ok &= r.passed;
        let mark = if r.passed { format!("{GREEN}✓{RESET}") } else { format!("{RED}✗{RESET}") };
        let why = if r.timed_out { " (tiempo agotado)".to_string() } else { format!(" (salida {:?})", r.exit_code) };
        println!(
            "  {mark} {name:<42}{DIM}{:>6.1} s  {}{}{RESET}",
            r.seconds,
            r.summary,
            if r.passed { String::new() } else { why }
        );
        if !r.passed {
            println!("      {DIM}registro: {}{RESET}", log.display());
        }
    }
    if all_ok { Ok(()) } else { Err("alguna prueba de humo falló".into()) }
}

// --- Perfiles y distribuciones derivadas -------------------------------------------------------

fn profiles() -> Result<(), String> {
    let dir = workspace_root().join("profiles");
    title("Perfiles disponibles");
    for (id, name, desc) in profile::list(&dir) {
        println!("  {BOLD}{id:<12}{RESET} {name:<26} {DIM}{desc}{RESET}");
    }
    Ok(())
}

fn new_distro(args: &Args) -> Result<(), String> {
    let id = args.positional.first().ok_or("indica el identificador: cargo kopit new-distro <id>")?;
    if !id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') {
        return Err("el identificador sólo admite minúsculas, dígitos y '-'".into());
    }
    let root = workspace_root();
    let dir = root.join("profiles");
    let path = dir.join(format!("{id}.toml"));
    if path.exists() {
        return Err(format!("ya existe {}", path.display()));
    }
    let base = profile::load(&dir, &args.from)?;
    let name = args.name.clone().unwrap_or_else(|| format!("{} ({})", base.name, id));
    let text = format!(
        "# Distribución derivada de «{from}», creada con `cargo kopit new-distro`.\n\
         # Sólo hace falta escribir lo que cambia: el resto se hereda.\n\n\
         [distro]\ninherits = \"{from}\"\nname = \"{name}\"\ndescription = \"Distribución basada en {base}\"\n\n\
         [ui]\ntheme = \"aurora\"          # kopit-noche, kopit-dia, aurora, ambar, matriz, alto-contraste\n\n\
         [ui.colors]                 # colores de marca (cualquier rol del tema)\naccent = \"#ff8800\"\n\n\
         [image]\noverlay = \"overlay/{id}\"   # archivos que se copian tal cual a la imagen\n",
        from = args.from,
        base = base.name,
    );
    write_file(&path, text.as_bytes())?;
    let motd = dir.join("overlay").join(id).join("kopit").join("motd.txt");
    write_file(&motd, format!("Bienvenido a {name}.\r\n").as_bytes())?;
    title(&format!("Distribución «{id}» creada"));
    step("Perfil", &path.display().to_string());
    step("Capa de archivos", &motd.parent().unwrap().display().to_string());
    println!("\n  Pruébala con: {BOLD}cargo kopit run --profile {id}{RESET}");
    Ok(())
}

// --- Herramientas -------------------------------------------------------------------------------

fn addr2line(args: &Args) -> Result<(), String> {
    let ctx = Ctx { root: workspace_root(), arch: args.arch.clone(), release: args.release };
    let elf = ctx.kernel_elf()?;
    let data = std::fs::read(&elf).map_err(|e| format!("{}: {e} (¿compilaste el núcleo?)", elf.display()))?;
    let addrs: Vec<u64> =
        args.positional.iter().filter_map(|a| u64::from_str_radix(a.trim_start_matches("0x"), 16).ok()).collect();
    if addrs.is_empty() {
        return Err("indica direcciones en hexadecimal: cargo kopit addr2line ffffffff80201234 ...".into());
    }
    for (a, sym) in symbols::resolve(&data, &addrs)? {
        match sym {
            Some((name, off)) => println!("  {a:#018x}  {BOLD}{name}{RESET}+{off:#x}"),
            None => println!("  {a:#018x}  {DIM}(desconocida){RESET}"),
        }
    }
    Ok(())
}

fn doctor(args: &Args) -> Result<(), String> {
    title("Herramientas");
    let rustc = std::process::Command::new("rustc")
        .arg("--version")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());
    match rustc {
        Ok(v) => step("Rust", &v),
        Err(_) => warn("rustc no encontrado"),
    }
    match qemu::find_qemu(&args.arch) {
        Some(q) => {
            step("QEMU", &q.display().to_string());
            let (code, vars) = qemu::find_firmware(&args.arch, &q);
            match code {
                Some(c) => step("Firmware UEFI", &c.display().to_string()),
                None => warn("firmware UEFI (OVMF) no encontrado: indica KOPIT_OVMF_CODE"),
            }
            if let Some(v) = vars {
                step("Variables UEFI", &v.display().to_string());
            }
        }
        None => warn("QEMU no encontrado: instálalo o indica KOPIT_QEMU"),
    }
    for tool in ["grub-mkrescue", "xorriso", "sbsign"] {
        match which(tool) {
            Some(p) => step(tool, &p.display().to_string()),
            None => info(tool, &format!("{DIM}(opcional, no encontrado){RESET}")),
        }
    }
    Ok(())
}

fn grub_iso(args: &Args) -> Result<(), String> {
    let grub = which("grub-mkrescue").ok_or(
        "grub-mkrescue no está instalado. En Debian/Ubuntu (o WSL): sudo apt install grub-pc-bin grub-efi-amd64-bin xorriso mtools",
    )?;
    let pre = prepare(args, false)?;
    let out = pre.ctx.out_dir(&pre.profile.id);
    let stage = out.join("grub-stage");
    let _ = std::fs::remove_dir_all(&stage);
    write_file(&stage.join("boot/kopit/kernel.elf"), &pre.built.kernel_image)?;
    let cfg = format!(
        "set timeout={}\nset default=0\n\nmenuentry \"{} (Multiboot2)\" {{\n    multiboot2 /boot/kopit/kernel.elf {}\n    boot\n}}\n\nmenuentry \"{} (Multiboot2, modo seguro)\" {{\n    multiboot2 /boot/kopit/kernel.elf {} safe loglevel=debug\n    boot\n}}\n",
        pre.profile.boot_timeout, pre.profile.name, pre.cmdline, pre.profile.name, pre.cmdline
    );
    write_file(&stage.join("boot/grub/grub.cfg"), cfg.as_bytes())?;
    let iso = out.join(format!("kopitos-{}-{}-grub.iso", pre.profile.id, pre.ctx.arch));
    let st = std::process::Command::new(grub).arg("-o").arg(&iso).arg(&stage).status().map_err(|e| e.to_string())?;
    if !st.success() {
        return Err("grub-mkrescue falló".into());
    }
    step("ISO con GRUB", &iso.display().to_string());
    Ok(())
}

fn clean() -> Result<(), String> {
    let dir = workspace_root().join("target").join("kopitos");
    if dir.exists() {
        std::fs::remove_dir_all(&dir).map_err(|e| e.to_string())?;
    }
    step("Limpio", &dir.display().to_string());
    Ok(())
}
