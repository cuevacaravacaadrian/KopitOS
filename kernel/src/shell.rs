//! # Monitor del núcleo (`kopit>`)
//!
//! Una consola de diagnóstico que funciona igual con el teclado de la máquina
//! que por el puerto serie. Edición de línea, historial (↑/↓), autocompletado
//! (Tab) y órdenes en español con alias en inglés.
//!
//! Cuando exista el espacio de usuario (fase 3), esta consola seguirá
//! disponible como *modo de rescate* del núcleo.

use crate::console::{self, Tone};
use crate::hal;
use crate::input::{self, Input};
use crate::log::{FixedBuf, Level};
use crate::{boot, cmdline, mm, time, version};
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt::Write;

struct Command {
    name: &'static str,
    aliases: &'static [&'static str],
    help: &'static str,
    run: fn(&[&str]),
}

macro_rules! out {
    ($tone:expr, $($arg:tt)*) => {{
        let mut b = FixedBuf::<1024>::new();
        let _ = write!(b, $($arg)*);
        console::write($tone, b.as_str());
    }};
}

const COMMANDS: &[Command] = &[
    Command { name: "ayuda", aliases: &["help", "?"], help: "esta lista", run: cmd_help },
    Command { name: "info", aliases: &["uname", "sistema"], help: "resumen del sistema", run: cmd_info },
    Command { name: "mem", aliases: &["free", "memoria"], help: "memoria física y montículo", run: cmd_mem },
    Command { name: "mapa", aliases: &["memmap"], help: "mapa de memoria del firmware", run: cmd_map },
    Command { name: "dmesg", aliases: &["log"], help: "registro completo del núcleo", run: cmd_dmesg },
    Command {
        name: "nivel",
        aliases: &["loglevel"],
        help: "nivel del registro: nivel <error|aviso|info|depur|traza>",
        run: cmd_level,
    },
    Command { name: "cpu", aliases: &["lscpu"], help: "procesador y características", run: cmd_cpu },
    Command { name: "acpi", aliases: &[], help: "tablas ACPI, CPUs e IOAPIC", run: cmd_acpi },
    Command { name: "pci", aliases: &["lspci"], help: "dispositivos PCI y su controlador", run: cmd_pci },
    Command { name: "servicios", aliases: &["services"], help: "plan de arranque de sys_init", run: cmd_services },
    Command { name: "tema", aliases: &["theme"], help: "lista o cambia el tema: tema <nombre>", run: cmd_theme },
    Command { name: "colores", aliases: &["palette"], help: "paleta del tema actual", run: cmd_colors },
    Command {
        name: "teclado",
        aliases: &["kbd"],
        help: "distribución del teclado: teclado <es|us>",
        run: cmd_keyboard,
    },
    Command { name: "hora", aliases: &["date", "fecha"], help: "fecha y hora", run: cmd_date },
    Command { name: "uptime", aliases: &["tiempo"], help: "tiempo en marcha y tics", run: cmd_uptime },
    Command { name: "eco", aliases: &["echo"], help: "repite el texto", run: cmd_echo },
    Command { name: "limpiar", aliases: &["clear", "cls"], help: "limpia la pantalla", run: cmd_clear },
    Command { name: "prueba", aliases: &["test"], help: "diagnóstico: prueba <int3|alloc|pf|panico>", run: cmd_test },
    Command { name: "reiniciar", aliases: &["reboot"], help: "reinicia el equipo", run: cmd_reboot },
    Command {
        name: "apagar",
        aliases: &["poweroff", "shutdown"],
        help: "apaga el equipo (ACPI S5)",
        run: cmd_poweroff,
    },
];

fn find(name: &str) -> Option<&'static Command> {
    COMMANDS.iter().find(|c| c.name == name || c.aliases.contains(&name))
}

struct Shell {
    line: String,
    history: Vec<String>,
    browse: Option<usize>,
}

fn prompt() {
    console::write(Tone::Accent, "kopit");
    console::write(Tone::Muted, "@");
    console::write(Tone::Normal, version::ARCH);
    console::write(Tone::Accent, " ▸ ");
}

impl Shell {
    fn replace_line(&mut self, new: &str) {
        for _ in 0..self.line.chars().count() {
            console::write(Tone::Normal, "\u{8}");
        }
        self.line.clear();
        self.line.push_str(new);
        console::write(Tone::Normal, new);
    }

    fn complete(&mut self) {
        let prefix = self.line.as_str();
        if prefix.contains(' ') {
            return;
        }
        let matches: Vec<&str> = COMMANDS.iter().map(|c| c.name).filter(|n| n.starts_with(prefix)).collect();
        match matches.as_slice() {
            [one] => {
                let rest = &one[prefix.len()..];
                let mut s = String::from(rest);
                s.push(' ');
                self.line.push_str(&s);
                console::write(Tone::Normal, &s);
            }
            [] => {}
            many => {
                console::write(Tone::Normal, "\n");
                for m in many {
                    out!(Tone::Muted, "{}  ", m);
                }
                console::write(Tone::Normal, "\n");
                prompt();
                console::write(Tone::Normal, &self.line.clone());
            }
        }
    }

    fn handle(&mut self, ev: Input) {
        match ev {
            Input::Char(c) => {
                if self.line.len() < 512 {
                    self.line.push(c);
                    let mut b = [0u8; 4];
                    console::write(Tone::Normal, c.encode_utf8(&mut b));
                }
            }
            Input::Backspace => {
                if self.line.pop().is_some() {
                    console::write(Tone::Normal, "\u{8}");
                }
            }
            Input::Tab => self.complete(),
            Input::Up if !self.history.is_empty() => {
                let i = match self.browse {
                    None => self.history.len() - 1,
                    Some(i) => i.saturating_sub(1),
                };
                self.browse = Some(i);
                let h = self.history[i].clone();
                self.replace_line(&h);
            }
            Input::Down => {
                if let Some(i) = self.browse {
                    if i + 1 < self.history.len() {
                        self.browse = Some(i + 1);
                        let h = self.history[i + 1].clone();
                        self.replace_line(&h);
                    } else {
                        self.browse = None;
                        self.replace_line("");
                    }
                }
            }
            Input::Ctrl('c') => {
                console::write(Tone::Muted, "^C\n");
                self.line.clear();
                prompt();
            }
            Input::Ctrl('l') => {
                console::clear();
                prompt();
                console::write(Tone::Normal, &self.line.clone());
            }
            Input::Enter => {
                console::write(Tone::Normal, "\n");
                let line = core::mem::take(&mut self.line);
                self.browse = None;
                let trimmed = line.trim();
                if !trimmed.is_empty() {
                    if self.history.last().map(|h| h.as_str()) != Some(trimmed) {
                        if self.history.len() >= 32 {
                            self.history.remove(0);
                        }
                        self.history.push(String::from(trimmed));
                    }
                    execute(trimmed);
                }
                prompt();
            }
            _ => {}
        }
    }
}

fn execute(line: &str) {
    let args: Vec<&str> = line.split_whitespace().collect();
    match find(args[0]) {
        Some(c) => (c.run)(&args[1..]),
        None => out!(Tone::Error, "No existe la orden '{}'. Escribe 'ayuda' para ver las disponibles.\n", args[0]),
    }
}

/// Bucle principal: atiende la entrada y duerme hasta la siguiente interrupción.
pub fn run() -> ! {
    console::write(Tone::Muted, "\nMonitor del núcleo. Escribe 'ayuda' (o pulsa Tab) para ver las órdenes.\n\n");
    let mut shell = Shell { line: String::new(), history: Vec::new(), browse: None };
    prompt();
    console::cursor(true);
    let mut last_second = u64::MAX;
    loop {
        let mut handled = false;
        while let Some(ev) = input::pop() {
            shell.handle(ev);
            handled = true;
        }
        if handled {
            console::cursor(true);
        }
        let second = time::uptime_ms() / 1000;
        if second != last_second {
            last_second = second;
            crate::init::refresh_status();
        }
        hal::wait_for_interrupt();
    }
}

// --- Órdenes ------------------------------------------------------------------------

fn cmd_help(_: &[&str]) {
    console::write(Tone::Accent, "Órdenes del monitor de KopitOS:\n");
    for c in COMMANDS {
        out!(Tone::Normal, "  {:<10}", c.name);
        out!(Tone::Muted, " {}", c.help);
        if !c.aliases.is_empty() {
            out!(Tone::Trace, "  ({})", c.aliases.join(", "));
        }
        console::write(Tone::Normal, "\n");
    }
    console::write(Tone::Muted, "Teclas: ↑↓ historial · Tab completar · Ctrl+L limpiar · Ctrl+C cancelar\n");
}

fn kv(key: &str, value: core::fmt::Arguments<'_>) {
    out!(Tone::Muted, "  {:<20}", key);
    out!(Tone::Normal, "{}\n", value);
}

fn cmd_info(_: &[&str]) {
    let info = boot::info();
    out!(Tone::Accent, "{} {} «{}»\n", version::DISTRO_NAME, version::VERSION, version::CODENAME);
    kv("arquitectura", format_args!("{}", version::ARCH));
    kv("perfil", format_args!("{} ({})", version::PROFILE, version::DEVICE_CLASS));
    kv("compilación", format_args!("{}", version::BUILD_ID));
    kv("cargador", format_args!("{} · {}", boot::loader_name(), info.source.name()));
    kv("Secure Boot", format_args!("{}", info.secure_boot.name()));
    kv("integridad", format_args!("{}", info.kernel_verification.name()));
    kv("línea de órdenes", format_args!("{}", cmdline::raw()));
    kv("CPU", format_args!("{} lógicas", crate::init::cpu_count()));
    kv("RAM", format_args!("{} MiB instalados", boot::installed_ram() >> 20));
    if let Some(s) = console::screen_info() {
        kv(
            "pantalla",
            format_args!(
                "{} {}x{} · {}x{} celdas · ×{}{}",
                s.kind,
                s.width,
                s.height,
                s.cols,
                s.rows,
                s.scale,
                if s.double_buffered { " · doble búfer" } else { "" }
            ),
        );
    }
    kv("tema", format_args!("{}", console::theme().name));
    let up = time::uptime_ms();
    kv("en marcha", format_args!("{}.{:03} s", up / 1000, up % 1000));
}

fn cmd_mem(_: &[&str]) {
    let Some(s) = mm::pmm::stats() else {
        console::write(Tone::Warning, "El asignador de marcos no está listo.\n");
        return;
    };
    out!(
        Tone::Accent,
        "Memoria física: {} MiB libres de {} MiB gestionados\n",
        s.free_bytes >> 20,
        s.managed_bytes >> 20
    );
    for z in s.zones.iter().flatten() {
        out!(
            Tone::Normal,
            "  {:<7} {:#011x}-{:#011x}  {:>6} MiB libres / {:>6} MiB\n",
            z.name,
            z.base,
            z.end,
            (z.free_frames * 4096) >> 20,
            (z.managed_frames * 4096) >> 20
        );
        let mut orders = FixedBuf::<160>::new();
        for (o, n) in z.free_blocks.iter().enumerate() {
            let _ = write!(orders, "{}:{} ", o, n);
        }
        out!(Tone::Muted, "          bloques libres por orden (4 KiB·2^n): {}\n", orders.as_str());
    }
    out!(Tone::Muted, "  metadatos del buddy: {} KiB\n", s.bitmap_bytes >> 10);
    let h = mm::heap::stats();
    out!(Tone::Accent, "Montículo: {} KiB en uso\n", h.total_bytes() >> 10);
    for (i, size) in mm::heap::CLASSES.iter().enumerate() {
        if h.slab_pages[i] > 0 {
            out!(Tone::Normal, "  slab {:>4} B: {:>5} objetos · {} páginas\n", size, h.objects[i], h.slab_pages[i]);
        }
    }
    out!(Tone::Normal, "  bloques grandes: {} ({} páginas)\n", h.big_allocs, h.big_pages);
}

fn cmd_map(_: &[&str]) {
    out!(Tone::Accent, "Mapa de memoria ({} regiones):\n", boot::memory_map().len());
    for r in boot::memory_map() {
        let tone = if r.kind == kopit_boot_proto::MemoryKind::USABLE { Tone::Success } else { Tone::Normal };
        out!(tone, "  {:#014x}-{:#014x} {:>10} KiB  {}\n", r.base, r.end(), r.length >> 10, r.kind.name());
    }
}

fn cmd_dmesg(_: &[&str]) {
    crate::log::dump(|line| console::write(Tone::Normal, line));
}

fn cmd_level(args: &[&str]) {
    match args.first().and_then(|a| Level::parse(a)) {
        Some(l) => {
            crate::log::set_sink_level("pantalla", l);
            out!(Tone::Success, "Registro en pantalla: nivel {}\n", l.name());
        }
        None => crate::log::sink_levels(|name, l| out!(Tone::Normal, "  {:<10} {}\n", name, l.name())),
    }
}

fn cmd_cpu(_: &[&str]) {
    crate::arch::describe_cpu(&mut |k, v| kv(k, format_args!("{}", v)));
}

fn cmd_acpi(_: &[&str]) {
    #[cfg(feature = "acpi")]
    {
        let Some(a) = crate::acpi::info() else {
            console::write(Tone::Warning, "No hay información ACPI.\n");
            return;
        };
        out!(Tone::Accent, "ACPI revisión {} · OEM {}\n", a.revision, core::str::from_utf8(&a.oem_id).unwrap_or("?"));
        for t in &a.tables {
            out!(
                Tone::Normal,
                "  {}  {:#010x}  {:>6} B  rev {}  {}\n",
                t.signature_str(),
                t.phys,
                t.length,
                t.revision,
                t.oem_str()
            );
        }
        out!(Tone::Accent, "CPUs (MADT):\n");
        for c in &a.cpus {
            out!(
                Tone::Normal,
                "  ACPI {:>3} → APIC {:>3}  {}\n",
                c.acpi_id,
                c.apic_id,
                if c.enabled { "activa" } else { "desactivada" }
            );
        }
        for io in &a.ioapics {
            out!(Tone::Normal, "  IOAPIC {} en {:#x}, GSI desde {}\n", io.id, io.address, io.gsi_base);
        }
        for o in &a.overrides {
            out!(
                Tone::Muted,
                "  IRQ {} → GSI {}{}{}\n",
                o.irq,
                o.gsi,
                if o.active_low { " (activa baja)" } else { "" },
                if o.level_triggered { " (nivel)" } else { "" }
            );
        }
        return;
    }
    #[allow(unreachable_code)]
    console::write(Tone::Warning, "Este núcleo se compiló sin ACPI.\n");
}

fn cmd_pci(_: &[&str]) {
    #[cfg(feature = "pci")]
    {
        let devs = crate::pci::devices();
        out!(Tone::Accent, "{} dispositivos PCI:\n", devs.len());
        for d in devs {
            let (drv, state) = crate::pci::driver_label(d);
            out!(
                Tone::Muted,
                "  {:02x}:{:02x}.{} {:04x}:{:04x} ",
                d.bus,
                d.device,
                d.function,
                d.vendor_id,
                d.device_id
            );
            out!(Tone::Normal, "{} ", d.class_name());
            out!(Tone::Muted, "({})", d.vendor_name());
            let tone = if state == "activo" { Tone::Success } else { Tone::Warning };
            out!(tone, " → {} [{}]\n", drv, state);
        }
        console::write(Tone::Muted, "  [fase N] = controlador planificado en esa fase de la hoja de ruta\n");
        return;
    }
    #[allow(unreachable_code)]
    console::write(Tone::Warning, "Este núcleo se compiló sin PCI.\n");
}

fn cmd_services(args: &[&str]) {
    let class = args.first().copied().unwrap_or(version::DEVICE_CLASS);
    match kopit_init::plan(kopit_init::DEFAULT_SERVICES, class) {
        Ok(plan) => {
            out!(Tone::Accent, "Plan de arranque de sys_init para «{}» ({} oleadas en paralelo):\n", class, plan.len());
            for (i, wave) in plan.iter().enumerate() {
                out!(Tone::Muted, "  oleada {}: ", i + 1);
                for s in wave {
                    out!(if s.critical { Tone::Accent } else { Tone::Normal }, "{} ", s.name);
                }
                console::write(Tone::Normal, "\n");
            }
            console::write(Tone::Muted, "  (prueba: servicios server · servicios iot · servicios phone)\n");
        }
        Err(e) => out!(Tone::Error, "Plan no válido: {:?}\n", e),
    }
}

fn cmd_theme(args: &[&str]) {
    match args.first() {
        Some(id) => match kopit_theme::find(id) {
            Some(t) => {
                console::set_theme(*t);
                crate::init::refresh_status();
                out!(Tone::Success, "Tema cambiado a «{}».\n", t.name);
            }
            None => out!(Tone::Error, "No existe el tema '{}'.\n", id),
        },
        None => {
            let current = console::theme().id;
            for t in kopit_theme::THEMES.iter() {
                let mark = if t.id == current { "▸" } else { " " };
                out!(Tone::Normal, " {} {:<16}", mark, t.id);
                out!(Tone::Muted, "{}\n", t.name);
            }
            console::write(Tone::Muted, "Uso: tema <nombre>  (también theme=<nombre> en la línea de órdenes)\n");
        }
    }
}

fn cmd_colors(_: &[&str]) {
    console::draw_palette();
}

fn cmd_keyboard(args: &[&str]) {
    #[cfg(all(target_arch = "x86_64", feature = "ps2"))]
    {
        match args.first().and_then(|a| kopit_drv_ps2::Layout::from_name(a)) {
            Some(l) => {
                crate::arch::keyboard::set_layout(l);
                out!(Tone::Success, "Teclado: distribución '{}'.\n", l.name());
            }
            None => out!(
                Tone::Normal,
                "Distribución actual: {} (disponibles: es, us)\n",
                crate::arch::keyboard::layout().name()
            ),
        }
        return;
    }
    #[allow(unreachable_code)]
    {
        let _ = args;
        console::write(Tone::Warning, "No hay teclado PS/2 en esta plataforma.\n");
    }
}

fn cmd_date(_: &[&str]) {
    match time::now() {
        Some(dt) => out!(
            Tone::Normal,
            "{} {:02}/{:02}/{} {:02}:{:02}:{:02} (UTC según el firmware)\n",
            time::weekday_name(&dt),
            dt.day,
            dt.month,
            dt.year,
            dt.hour,
            dt.minute,
            dt.second
        ),
        None => console::write(Tone::Warning, "El firmware no proporcionó la hora.\n"),
    }
}

fn cmd_uptime(_: &[&str]) {
    let ms = time::uptime_ms();
    out!(
        Tone::Normal,
        "En marcha {}.{:03} s · {} tics a {} Hz · reloj de ciclos {} MHz\n",
        ms / 1000,
        ms % 1000,
        time::ticks(),
        time::tick_hz(),
        time::cycle_frequency() / 1_000_000
    );
}

fn cmd_echo(args: &[&str]) {
    out!(Tone::Normal, "{}\n", args.join(" "));
}

fn cmd_clear(_: &[&str]) {
    console::clear();
}

fn cmd_test(args: &[&str]) {
    match args.first().copied() {
        Some("int3") => {
            console::write(Tone::Muted, "Lanzando una excepción de punto de ruptura (int3)...\n");
            hal::breakpoint();
            console::write(Tone::Success, "✓ La CPU volvió de la excepción: la IDT funciona.\n");
        }
        Some("alloc") => {
            let before = mm::pmm::stats().map(|s| s.free_bytes).unwrap_or(0);
            let mut v: Vec<u64> = Vec::new();
            for i in 0..100_000u64 {
                v.push(i * i);
            }
            let sum: u64 = v.iter().sum();
            let boxed = alloc::boxed::Box::new([7u8; 8192]);
            out!(Tone::Normal, "Vec de 100 000 elementos (suma {}), Box de {} B\n", sum, boxed.len());
            drop(v);
            drop(boxed);
            let after = mm::pmm::stats().map(|s| s.free_bytes).unwrap_or(0);
            out!(Tone::Success, "✓ Memoria devuelta: libre antes {} KiB, después {} KiB\n", before >> 10, after >> 10);
        }
        Some("pf") => {
            console::write(Tone::Warning, "Escribiendo en la dirección 0 (debe provocar un fallo de página)...\n");
            // SAFETY: no lo es: es la prueba de que el fallo se detecta.
            unsafe { core::ptr::write_volatile(core::ptr::null_mut::<u64>(), 1) };
        }
        Some("panico") | Some("panic") => panic!("pánico solicitado desde el monitor"),
        _ => console::write(Tone::Normal, "Uso: prueba <int3|alloc|pf|panico>\n"),
    }
}

fn cmd_reboot(_: &[&str]) {
    console::write(Tone::Warning, "Reiniciando...\n");
    time::spin_ms(200);
    hal::reboot();
}

fn cmd_poweroff(_: &[&str]) {
    console::write(Tone::Warning, "Apagando...\n");
    time::spin_ms(200);
    hal::power_off();
}
