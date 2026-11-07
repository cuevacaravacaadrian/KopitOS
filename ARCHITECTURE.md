# Arquitectura de KopitOS

> **Estado:** fase 1, «Génesis». Arranque, núcleo base y tres arquitecturas funcionando y probadas en QEMU.
> Este documento separa siempre lo **hecho** (✅, con código y pruebas en este repositorio) de lo
> **diseñado** (🔜, hoja de ruta). Si algo no lleva ✅, todavía no existe.

---

## 1. Visión

KopitOS es un sistema operativo de propósito general escrito en **Rust** (con el ensamblador
imprescindible) que aspira a cuatro cosas a la vez:

1. **Un núcleo, todas las CPU.** El mismo `kernel_main` arranca en x86_64, ARM64 y RISC-V.
2. **Un núcleo, todos los dispositivos.** Servidor, sobremesa, portátil, tableta, teléfono e IoT
   comparten el código. Lo que cambia es el **perfil**: qué se compila, cómo arranca y cómo se ve.
3. **Distribuciones derivadas como ciudadanas de primera.** Crear «Mi Distro» basada en KopitOS
   es escribir un archivo de perfil, no hacer un *fork*.
4. **Controladores aislados.** Un controlador no conoce al núcleo: hoy se enlaza dentro y mañana
   corre como proceso con capacidades, sin reescribirlo.

## 2. Principios de diseño

| # | Principio | Consecuencia práctica |
|---|-----------|-----------------------|
| 1 | **Un contrato por frontera** | Arranque → núcleo: `BootInfo` (KBP). Núcleo → CPU: rasgo `hal::Arch`. Núcleo → controlador: `drivers/api`. Nada cruza una frontera sin pasar por su contrato. |
| 2 | **Lo que no se compila no existe** | Las funcionalidades son *features* de Cargo elegidas por el perfil. El núcleo IoT no contiene consola gráfica, PCI ni teclado: no ocupan memoria ni se pueden atacar. |
| 3 | **Algoritmos puros, probados fuera** | El asignador buddy, la fuente, FAT, GPT, ISO, SHA-256, el parser de DTB y el grafo de servicios son `no_std` y tienen pruebas en el anfitrión (`cargo test`). |
| 4 | **Probado en hardware emulado** | `cargo kopit test` arranca cada ruta de arranque en QEMU y ejecuta pruebas *dentro* del núcleo. |
| 5 | **Sin dependencias externas** | Ni el núcleo, ni el cargador, ni la herramienta de imágenes descargan crates. Todo es auditable y compila sin red. |
| 6 | **Fallar con información** | Pánicos con traza, excepciones con todos los registros, página de guarda contra desbordamientos, consola serie que funciona siempre. |
| 7 | **Español primero** | Mensajes, registro, órdenes y documentación en español (con alias en inglés en la shell). |

## 3. El sistema por capas

```text
 ┌─────────────────────────────────────────────────────────────────────────┐
 │ Distribuciones: profiles/*.toml  (desktop, laptop, server, tablet, ...)  │  distro_builder
 ├─────────────────────────────────────────────────────────────────────────┤
 │ Espacio de usuario (fase 3): sys_init (PID 1) · servicios · compositor  │  sys_init/, ui/
 ├─────────────────────────────────────────────────────────────────────────┤
 │ Controladores: drivers/api ← serie · framebuffer · PS/2 · PCI · ...      │  drivers/
 ├─────────────────────────────────────────────────────────────────────────┤
 │ Núcleo portable: init · mm (buddy, W^X, montículo) · log · consola ·     │  kernel/
 │                  tiempo · ACPI · PCI · shell · pruebas                   │
 ├───────────────── hal::Arch ─────────────────────────────────────────────┤
 │ arch/x86_64            │ arch/aarch64           │ arch/riscv64           │  arch/
 │ GDT/IDT/APIC/paginación│ VBAR/GICv2/MMU         │ stvec/SBI              │
 ├────────────────────────┴────────────────────────┴───────────────────────┤
 │ Arranque → BootInfo (KBP): KopitBoot UEFI · Multiboot2 · PVH · DTB       │  boot/
 └─────────────────────────────────────────────────────────────────────────┘
```

## 4. El repositorio

| Carpeta | Contenido |
|---------|-----------|
| `boot/protocol` | ✅ **KBP**: `BootInfo`, regiones de memoria y `MemoryMapBuilder`, que normaliza mapas sucios: solapamientos, alineación y fusión. |
| `boot/kopitboot` | ✅ **KopitBoot**, el cargador UEFI con *bindings* UEFI escritos a mano: menú, edición de la línea de órdenes, verificación SHA-256, GOP, tablas de páginas y salto al núcleo. |
| `kernel/` | ✅ El núcleo portable y sus guiones de enlazado (`linker/<arch>.ld`). |
| `arch/x86_64` | ✅ Trampolín 32→64, GDT/TSS/IST, IDT de 256 vectores, APIC/x2APIC/IOAPIC, paginación de 4 niveles, PAT, COM1, PS/2, CMOS y energía. |
| `arch/aarch64` | ✅ Entrada en EL1, MMU en identidad, vectores de excepción, GICv2, temporizador genérico, PL011 y PSCI. |
| `arch/riscv64` | ✅ Entrada en modo S desde OpenSBI, traps, temporizador SBI, UART 16550 por MMIO y SRST. |
| `drivers/` | ✅ API de controladores, UART 16550, PL011, framebuffer, i8042/PS-2 (distribuciones ES/US) y bus PCI con su registro de controladores. |
| `libs/` | ✅ `buddy`, `crypto` (SHA-256 y CRC32), `elf` y `fdt`. |
| `ui/` | ✅ Motor de temas (6 temas), fuente con escalado Scale2x y español completo, y núcleo del compositor (daños, capas, oclusión, elección de *backend*). |
| `sys_init/` | ✅ Motor del grafo de servicios con arranque por oleadas paralelas y planes por clase de dispositivo. |
| `distro_builder/` | ✅ La herramienta `kopit`: perfiles, compilación, FAT16/32, GPT, ISO híbrida, QEMU, pruebas de humo y `addr2line`. |
| `profiles/` | ✅ `base`, `desktop`, `laptop`, `server`, `tablet`, `phone`, `iot`. |

---

## 5. Arranque

### 5.1 Cuatro rutas, un destino

| Ruta | Quién arranca | Estado inicial de la CPU | Adaptador | Estado |
|------|---------------|--------------------------|-----------|--------|
| **KopitBoot** | firmware UEFI | 64 bits, paginación de identidad del firmware | ninguno: KopitBoot ya rellena KBP | ✅ probado (disco GPT e ISO) |
| **Multiboot2** | GRUB 2 | 32 bits, sin paginación | `arch/x86_64/boot.rs` + `boot32.rs` | ✅ implementado, cabecera validada; 🔜 prueba con GRUB en la integración continua |
| **PVH** | QEMU `-kernel`, Firecracker, Cloud Hypervisor | 32 bits, sin paginación | el mismo trampolín que Multiboot2 | ✅ probado |
| **Device tree** | OpenSBI (RISC-V), QEMU `virt` (ARM64) | 64 bits; en ARM64, MMU apagada | `kernel/src/dtb.rs` | ✅ probado |

Todas acaban en `init::kernel_main(&BootInfo)`. **Un único ELF** sirve para las tres rutas x86_64:
la cabecera Multiboot2 va en los primeros 32 KiB, la nota PVH en un segmento `PT_NOTE` y la
entrada de 64 bits `_start` en `e_entry`.

### 5.2 KopitBoot, paso a paso ✅

1. Lee `\kopit\boot.cfg`, que genera `distro_builder` a partir del perfil.
2. Muestra el **menú** (↑↓, Intro, `e` para editar la línea de órdenes, atajos 1-9 y cuenta atrás).
   El firmware lo replica en pantalla y por el puerto serie.
3. Lee `\kopit\kernel.elf` y **calcula su SHA-256**. La huella esperada está **dentro del propio
   binario del cargador** (ver §5.5).
4. Carga los segmentos ELF en memoria propia (tipo UEFI `0x80004B00`) en cualquier dirección física:
   el núcleo es reubicable físicamente.
5. Construye tablas de páginas de 4 niveles: identidad (para el salto), HHDM y núcleo con
   **permisos por segmento**. Usa páginas de 1 GiB si la CPU las tiene y, si no, de 2 MiB.
6. Elige el modo de vídeo GOP (exacto, `auto`, `max` o `keep`).
7. Recoge el RSDP de ACPI, SMBIOS, la hora UEFI y el estado de Secure Boot.
8. Llama a `ExitBootServices` (reintentando si el mapa cambia) y convierte el mapa UEFI a KBP sin
   reservar memoria.
9. Activa `EFER.NXE`, carga `CR3`, cambia de pila y salta a `_start(BootInfo*)`.

### 5.3 El trampolín de 32 a 64 bits ✅

GRUB y PVH entregan la CPU en modo protegido de 32 bits. El código de `arch/x86_64/boot.rs` sigue
estos pasos:

1. Comprueba que existen CPUID y el modo largo. Si no, escribe un mensaje por COM1 y VGA y se detiene.
2. Construye tablas de arranque en `.bss`: identidad de 0 a 4 GiB, el mismo rango en el HHDM y los
   2 GiB altos para el núcleo. Bastan **7 páginas** con páginas de 2 MiB.
3. Activa `CR4.PAE`, `CR3`, `EFER.LME` (+ `NXE`) y `CR0.PG|WP`, en ese orden.
4. Carga una GDT con código de 64 bits y hace un salto lejano: ya está en modo largo.
5. Salta a la mitad alta y llama a `kopit_boot32_entry64(protocolo, info)`.

El núcleo está enlazado en `0xFFFFFFFF80200000`, pero GRUB y QEMU lo cargan en la dirección física
`0x200000` (la LMA). Por eso el código de 32 bits referencia `símbolo - KERNEL_VMA`.

### 5.4 Protocolo KBP ✅

Especificación completa en [docs/protocolo-arranque.md](docs/protocolo-arranque.md). Claves:

- `#[repr(C)]`, firma `KOPITKBP`, versión y tamaño, para poder ampliarlo sin romper nada.
- Los «enumerados» son `u32` envueltos: un valor desconocido nunca es comportamiento indefinido.
- Los punteros son direcciones virtuales válidas al entrar (HHDM o imagen del núcleo).
- `MemoryMapBuilder` garantiza un mapa ordenado, sin solapes, alineado y fusionado.

### 5.5 Cadena de confianza ✅/🔜

```text
 Firmware UEFI ──verifica firma──► KopitBoot.efi ──verifica SHA-256──► kernel.elf
   (claves PK/KEK/db)              (la huella va dentro del binario)
```

- ✅ `distro_builder` compila el núcleo, calcula su SHA-256 y **compila el cargador con esa huella
  como constante**. Al firmar el cargador, la firma cubre también la huella del núcleo.
- ✅ Si la huella no coincide y Secure Boot está **activo**, KopitBoot **se niega a arrancar**. Con
  Secure Boot desactivado avisa, espera 3 s y se lo comunica al núcleo (`kernel_verification`).
- ✅ KopitBoot lee `SecureBoot` y `SetupMode` y muestra el estado en el menú y en el núcleo.
- 🔜 Fase 2: `cargo kopit sign`, que firma `BOOTX64.EFI` con las claves de la distribución
  (formato PE/Authenticode) e inscribe la clave en el firmware o en MOK.
- 🔜 Fase 2: módulos, temas y actualizaciones firmados con **Ed25519**. La clave pública también va
  dentro del cargador firmado.
- 🔜 Fase 3: arranque medido con TPM 2.0 (PCR 8-9) y atestación.

---

## 6. El núcleo

### 6.1 Etapas de `kernel_main` ✅

| Etapa | Inicializa | Habilita |
|-------|------------|----------|
| 0 | consola serie, CPU, calibración del reloj, copia de `BootInfo`, línea de órdenes, consola, registro | ver lo que pasa (unos 25 ms tras entrar) |
| 1 | GDT/TSS/IDT · `VBAR_EL1` · `stvec` | sobrevivir a excepciones |
| 2 | marcos físicos → paginación propia → montículo → doble búfer → recuperar memoria del cargador | `Box`, `Vec` y `String` |
| 3 | ACPI, controladores de interrupciones y temporizador | interrupciones y tiempo |
| 4 | PCI y registro de controladores | saber qué hardware hay |
| 5 | plan de sys_init para la clase de dispositivo | (fase 3: lanzar el espacio de usuario) |

Tiempo medido del núcleo en QEMU (TCG, sin aceleración): **unos 110 ms en x86_64 con UEFI**, unos
60 ms con PVH y unos 30 ms en ARM64/RISC-V.

### 6.2 HAL y arquitecturas ✅

`kernel/src/hal.rs` define el rasgo `Arch`: `early_init`, `init_cpu`, `init_platform(hz)`,
interrupciones, `wait_for_interrupt`, `cycles`, consola serie, `reboot`, `power_off`,
`breakpoint` y `backtrace`. Cada `arch/<cpu>/mod.rs` lo implementa y **el compilador comprueba que
no falte nada**. El código de `arch/` se compila dentro del crate del núcleo (como en Linux), así
que puede usar sus cerrojos y su registro sin dependencias circulares.

| | x86_64 | ARM64 | RISC-V 64 |
|---|---|---|---|
| Arranque | UEFI, Multiboot2, PVH ✅ | ELF directo en EL1 ✅ · UEFI 🔜 | OpenSBI ✅ · UEFI 🔜 |
| Paginación | 4 niveles, W^X, mitad alta ✅ | identidad con bloques de 1 GiB ✅ · TTBR1 🔜 | identidad ✅ · Sv48 🔜 |
| Excepciones | IDT, 256 vectores, IST ✅ | 16 vectores ✅ | `stvec` ✅ |
| Interrupciones | x2APIC/xAPIC + IOAPIC ✅ | GICv2 ✅ · GICv3 🔜 | temporizador SBI ✅ · PLIC 🔜 |
| Reloj | TSC calibrado con el PIT ✅ | `CNTVCT` ✅ | `time` ✅ |
| Consola | COM1 16550, VGA, framebuffer ✅ | PL011 ✅ | 16550 por MMIO ✅ |
| Energía | ACPI S5 + reinicio ✅ | PSCI ✅ | SBI SRST ✅ |
| SMP | 🔜 trampolín por debajo de 1 MiB (ya reservado) | 🔜 PSCI CPU_ON | 🔜 SBI HSM |

### 6.3 Memoria ✅

```text
0x0000_0000_0000_0000 ┐ usuario (fase 3)
0x0000_7FFF_FFFF_FFFF ┘
0xFFFF_8000_0000_0000   HHDM: física p ↔ virtual base + p (páginas de 1 GiB/2 MiB)
0xFFFF_C000_0000_0000   ventanas MMIO (framebuffer WC, APIC sin caché) + página de guarda
0xFFFF_FFFF_8000_0000   imagen del núcleo (-2 GiB, modelo de código «kernel»)
```

- **Marcos físicos:** buddy (órdenes de 4 KiB a 4 MiB) en tres **zonas**: DMA (< 16 MiB),
  DMA32 (< 4 GiB) y Normal. Los enlaces de las listas libres van *dentro* de los bloques libres y el
  estado ocupa unos 2 bits por marco. El arranque tiene dos fases: antes de las tablas propias sólo
  se reparte la RAM que mapeó el cargador, y el resto se añade después en caliente. El primer MiB
  queda reservado para los trampolines SMP.
- **Paginación propia con W^X:** `.text` r-x, `.rodata` r--, datos rw- NX, **página de guarda**
  bajo la pila y el mapa directo con páginas enormes. PAT reprogramada: `PWT` = *write-combining*
  y `PCD|PWT` = sin caché.
- **Recuperación:** tras cambiar de tablas, la memoria del cargador vuelve al asignador (unos
  33 MiB en QEMU).
- **Montículo:** slabs de 16 B a 2 KiB y bloques buddy para el resto, como `kmalloc`. No necesita
  cabeceras porque `dealloc` recibe el mismo `Layout`.
- 🔜 Fase 2: espacios de usuario, copia en escritura (CoW), `mmap` y KASLR. Fase 3: memoria
  predictiva (§9.2).

### 6.4 Interrupciones y tiempo ✅

- Stubs de 16 bytes generados con `.rept` (la dirección del stub *n* es `base + 16·n`), un
  `TrapFrame` completo con los 15 registros y manejadores registrables en tiempo de ejecución.
- Excepciones con volcado de todos los registros. Un fallo de página dice si fue lectura,
  escritura o ejecución, en modo usuario o núcleo, y detecta **desbordamiento de pila** (doble fallo
  en la pila IST).
- El TSC se calibra contra el PIT (10 ms) y el temporizador del APIC contra el TSC. La frecuencia
  depende del perfil: 1000 Hz en escritorio, 300 en portátil o teléfono, 250 en servidor y 100 en IoT.
- 🔜 Núcleo *tickless* en reposo (§9.1).

### 6.5 Registro del núcleo ✅

`kerror!`, `kwarn!`, `kinfo!`, `kdebug!` y `ktrace!` formatean en la pila, sin memoria dinámica.
Cada línea va al **anillo de 64 KiB** (`dmesg`) y a cada **destino** con su propio nivel (pantalla:
`loglevel=`; serie: `serial.loglevel=`). Si una excepción ocurre *dentro* del registro, el mensaje
sale por la ruta de emergencia directa a la UART en lugar de bloquearse.

### 6.6 Consola e interfaz ✅

- **Consola gráfica** con barra superior (distribución, versión, arquitectura, perfil y reloj),
  barra inferior (RAM, CPU, Hz, tiempo en marcha y tema) y **escala adaptativa** de la letra: ×1 en
  1024x768, ×2 en 1280–1920 y hasta ×4 en 4K. También tiene doble búfer, ajuste por palabras con
  sangría colgante y una pantalla de pánico.
- **Fuente propia:** 8x8 de dominio público, escalada con **Scale2x** al compilar. Las letras del
  español (á é í ó ú ü ñ ç ¿ ¡ « » …) y las cajas, bloques y flechas se componen a partir de la base.
- **Temas:** `kopit-noche` (colores de la marca HyperEZ), `kopit-dia`, `aurora`, `ambar`, `matriz`
  y `alto-contraste`. Se cambian **en caliente** (`tema aurora`) o con `theme.<rol>=#rrggbb`.
- **Consola VGA de texto** (CP437) para arranques BIOS/PVH.
- **Shell del núcleo:** más de 20 órdenes, historial, Tab y alias en inglés. Funciona igual con el
  teclado PS/2 (distribución española con teclas muertas) que por el puerto serie.

---

## 7. Controladores y capa de compatibilidad

### 7.1 El modelo ✅

Regla de oro: **un controlador no depende del núcleo**. Los crates de `drivers/` sólo dependen de
`drivers/api` y reciben el acceso al hardware a través de rasgos pequeños:

```rust
pub trait RegisterIo { fn read(&self, reg: u16) -> u8; fn write(&self, reg: u16, v: u8); }
pub trait PortIo     { fn inb(&self, port: u16) -> u8; /* ... */ }
pub struct MmioRegisters { /* base + (reg << shift), accesos volátiles */ }
```

La prueba de que funciona ✅: **el mismo** `kopit-drv-uart16550` atiende el COM1 de un PC (puertos
`0x3F8`) y la UART `ns16550a` de RISC-V (MMIO). Además, cada controlador se prueba en el anfitrión
con registros simulados.

### 7.2 Descriptores y emparejamiento ✅

Cada controlador publica un `DriverDescriptor`:

```rust
DriverDescriptor {
    name: "virtio-net",
    matches: &[PciId { vendor: 0x1AF4, device: 0x1000 }, PciId { vendor: 0x1AF4, device: 0x1041 }],
    isolation: Isolation::Split,          // ruta rápida en el núcleo, política fuera
    capabilities: caps::MMIO | caps::DMA | caps::MSI,
    status: DriverStatus::Planned { phase: 2 },
}
```

`best_driver` puntúa cada coincidencia (ID exacto 100, ACPI/DT 90, plataforma 80, clase con
`prog_if` 60, clase 50, fabricante 20) y elige la más específica. La orden `pci` muestra qué
controlador atenderá cada dispositivo y en qué fase está.

### 7.3 Aislamiento progresivo 🔜

| Fase | Dónde corre el controlador | Cómo accede al hardware |
|------|----------------------------|-------------------------|
| 1 ✅ | dentro del núcleo | los rasgos se implementan con `in`/`out` y accesos volátiles directos |
| 3 🔜 | **proceso de usuario** (`Isolation::UserSpace`) | **capacidades**: el núcleo concede esa ventana MMIO, ese puerto, esa IRQ y esa región DMA protegida por la **IOMMU** (VT-d/AMD-Vi/SMMU) |
| 3 🔜 | **mixto** (`Isolation::Split`) para red, almacenamiento y GPU | la ruta rápida (colas, DMA) se ejecuta en el núcleo; la configuración y la política, fuera |

Si un controlador de usuario falla, `devmgr` lo reinicia: el sistema sigue en pie. El código del
controlador **no cambia** entre fases, porque sólo cambia la implementación de los rasgos.

### 7.4 IPC de latencia cero (diseño) 🔜

- **Llamada síncrona rápida:** el mensaje viaja en registros y el núcleo cambia directamente al
  hilo servidor sin pasar por el planificador (*direct switch*, al estilo de seL4). La meta es
  menos de 1 µs de ida y vuelta.
- **Colas compartidas sin copias:** anillos SPSC en memoria compartida (el mismo diseño que
  `sync::Spsc` del núcleo, ya en uso para el teclado) con un **timbre** para despertar al otro lado.
  El trabajo por lotes no hace ninguna llamada al sistema.
- **Capacidades como único nombre:** un proceso sólo puede hablar con quien tiene una capacidad
  concedida. No hay espacio de nombres global.

### 7.5 Capa de compatibilidad 🔜

La compatibilidad se apila en este orden, de más barata a más costosa:

1. **virtio primero** (fase 2): blk, net, gpu, input, rng y vsock. Con ellos, KopitOS funciona
   completo en cualquier hipervisor (QEMU/KVM, Hyper-V, Firecracker y la nube).
2. **Controladores de clase** (fases 2–3): un controlador por *estándar*, no por modelo. AHCI,
   NVMe, xHCI (USB), HID, HDA, UVC y CDC cubren la mayoría del hardware real con poco código.
3. **Modelo de dispositivo unificado ACPI + DT** (fase 2): el gestor de dispositivos convierte
   tablas ACPI (`_HID`/`_CID`) y nodos de device tree (`compatible`) en el mismo
   `DeviceIdentity`. Así un controlador sirve en PC, servidor ARM o placa RISC-V.
4. **Anfitrión de controladores ajenos** (fase 4, investigación): un proceso aislado que ofrece un
   subconjunto de la API interna de otro sistema (al estilo de LinuxKPI en FreeBSD o DDE en
   Genode) para reutilizar controladores de Wi-Fi y GPU muy complejos. Al correr en espacio de
   usuario con IOMMU, un fallo ajeno no compromete el núcleo. Requiere revisar licencias controlador
   a controlador.
5. **Firmware de dispositivos**: paquetes de firmware firmados que instala `distro_builder` según
   el perfil.

### 7.6 Hoja de ruta de controladores

| Controlador | Estado | Aislamiento previsto |
|-------------|--------|----------------------|
| UART 16550 / PL011 | ✅ | núcleo (consola de emergencia) |
| Framebuffer GOP/VBE | ✅ | núcleo → compositor (fase 3) |
| i8042 / PS-2 | ✅ | usuario (fase 3) |
| PCI (puertos `0xCF8`) | ✅ | núcleo |
| PCIe ECAM (MCFG/DT) | 🔜 fase 2 | núcleo |
| virtio-blk / net / gpu / input | 🔜 fase 2 | mixto / usuario |
| NVMe · AHCI · xHCI | 🔜 fase 2 | usuario |
| HDA · USB HID · UVC | 🔜 fase 3 | usuario |
| Wi-Fi · GPU nativas | 🔜 fase 4 | anfitrión de controladores ajenos |

---

## 8. Distribuciones derivadas

### 8.1 Un perfil es una distribución ✅

Una distribución de KopitOS es un archivo `profiles/<id>.toml` (más, opcionalmente, una carpeta
de archivos). `distro_builder` lo convierte en una imagen arrancable:

```text
 profiles/mi-distro.toml ─┐
                          ├─► cargo (núcleo con SUS features) ─► SHA-256 ─► KopitBoot con la huella
 profiles/overlay/... ────┘                                                │
                                ┌──────────────────────────────────────────┘
                                ├─► FAT32 ─► GPT ─────────────────────► kopitos-mi-distro-x86_64.img
                                └─► FAT16 ─► ISO 9660 + El Torito + GPT ─► kopitos-mi-distro-x86_64.iso
```

### 8.2 Anatomía de un perfil ✅

| Sección / clave | Efecto | ¿Cuándo? |
|-----------------|--------|----------|
| `distro.inherits` | hereda de otro perfil (cadena de herencia) | — |
| `distro.name`, `version`, `description` | identidad, que aparece en el menú, las barras y `info` | compilación (`KOPIT_DISTRO_NAME`) |
| `distro.device_class` | `desktop`, `laptop`, `server`, `tablet`, `phone` o `iot` | compilación: Hz por defecto y plan de sys_init |
| `kernel.features` | qué subsistemas existen en el binario | **compilación** |
| `kernel.cmdline`, `kernel.timer_hz` | parámetros del núcleo | **ejecución** (`boot.cfg`) |
| `boot.timeout`, `resolution`, `safe_mode_entry`, `test_entry` | menú de KopitBoot | ejecución |
| `ui.theme`, `ui.keymap`, `ui.font_scale` | aspecto y teclado | ejecución |
| `ui.colors.<rol>` | colores de marca sobre el tema | ejecución (`theme.<rol>=`) |
| `image.size_mib`, `image.overlay` | ESP y archivos extra copiados tal cual | imagen |

### 8.3 Compilación frente a ejecución ✅

- **Compilación** (*features* de Cargo): lo que no se lista **no está** en el binario. El núcleo
  IoT (`shell, acpi, selftest`) se compila sin consola gráfica, PCI ni teclado.
- **Ejecución** (línea de órdenes): tema, colores, teclado, frecuencia y nivel de registro. Se pueden
  cambiar en el menú de KopitBoot (`e`) sin recompilar.

### 8.4 Crear una derivada en tres órdenes ✅

```sh
cargo kopit new-distro mi-distro --from desktop --name "Mi Distro"
#   → profiles/mi-distro.toml  (hereda de desktop: tema aurora, acento #ff8800)
#   → profiles/overlay/mi-distro/kopit/motd.txt
cargo kopit run --profile mi-distro       # pruébala en QEMU
cargo kopit iso --profile mi-distro --release   # ISO para USB o CD
```

Guía paso a paso: [docs/distribuciones.md](docs/distribuciones.md).

### 8.5 Hoja de ruta de distribuciones 🔜

| Fase | Capacidad |
|------|-----------|
| 2 | `cargo kopit sign` con claves propias de la distribución (Secure Boot) · temas y logotipos como **módulos** · `kopit diff-profile` |
| 3 | **paquetes** firmados (formato `.kpkg`: contenido + manifiesto + firma Ed25519) · conjuntos de servicios de sys_init por perfil · instalador gráfico/TUI generado desde el perfil |
| 3 | **actualizaciones A/B** atómicas sobre instantáneas de KopitFS con reversión automática si el arranque falla |
| 4 | **canales** (estable, beta, nocturno) y repositorios por distribución · reproducibilidad verificable (`SOURCE_DATE_EPOCH` ya está soportado ✅) |

---

## 9. Subsistemas futuros (diseño)

### 9.1 Planificador heterogéneo y energía 🔜

- **Topología real:** núcleos P/E (CPUID 0x1A en x86, ✅ ya se detecta y muestra en `cpu`),
  `capacity-dmips-mhz` en DT, clústeres y SMT.
- **Planificación consciente de la energía** (al estilo de EAS): cada hilo lleva una carga media y
  cada núcleo un modelo energía/frecuencia. Los hilos interactivos van a núcleos P y el trabajo de
  fondo a núcleos E. En CPUs Intel se usa la retroalimentación del hardware (HFI/Thread Director).
- **Núcleo sin tic en reposo:** el temporizador sólo se programa para el próximo evento real.
- **Gobernador por perfil:** `desktop` prioriza latencia, `laptop`/`phone` energía y `server` rendimiento
  estable. Se alimenta de la misma clase de dispositivo que ya decide los Hz ✅.

### 9.2 Memoria predictiva 🔜

Heurísticas con presupuesto acotado (nada de modelos pesados en el núcleo):

- **Conjunto de trabajo** por proceso, muestreando los bits *accessed* de las tablas de páginas.
- **Predicción de faltas de página** con una cadena de Markov de pocos estados por región: si
  después de la página *A* suele venir la *B*, se pre-carga *B*.
- **Compresión en memoria** (al estilo de zram) antes de recurrir al almacenamiento, y
  **pre-calentado** del arranque a partir de la traza del arranque anterior.
- Toda decisión se puede auditar con contadores en `mem`.

### 9.3 KopitFS 🔜

- **Copia en escritura** sobre árboles B+, con sumas de comprobación (BLAKE3) en cada bloque y nodo:
  la corrupción se detecta siempre.
- **Instantáneas** baratas, que son la base de las actualizaciones A/B.
- **Redundancia nativa:** espejo o códigos de borrado entre dispositivos, con reparación automática
  al leer.
- Compresión transparente (LZ4/Zstd) y escritura por transacciones, sin necesidad de `fsck`.

### 9.4 Compositor y GPU 🔜 (el núcleo de la escena ya existe ✅)

- `ui/compositor` ✅: rectángulos, **daños** fusionados, capas por `z`, **oclusión** por capas opacas
  y elección de *backend* según la clase de dispositivo. Tiene pruebas en el anfitrión.
- *Backends* 🔜: software (siempre disponible), **Vulkan** (virtio-gpu Venus y GPUs nativas),
  **Metal** (hardware Apple) y **OpenGL** (compatibilidad), detrás del rasgo `RenderBackend`.
- **Interfaz adaptativa:** misma aplicación, distinta densidad. La escala de la consola ✅ ya se
  adapta a la resolución; el compositor aplicará la misma política por clase de dispositivo.

### 9.5 sys_init y espacio de usuario 🔜 (el motor ya existe ✅)

- ✅ Grafo de servicios validado entero **antes** de arrancar (dependencias que faltan, ciclos).
  Se arranca por **oleadas paralelas** y cada clase de dispositivo tiene su plan (`servicios server`).
- 🔜 Fase 3: procesos en ring 3 / EL0 / modo U, ELF de usuario, llamadas al sistema por capacidades
  y sys_init como PID 1 con estos mismos manifiestos.

---

## 10. Seguridad

| Medida | Estado |
|--------|--------|
| Rust con `#![deny(unsafe_op_in_unsafe_fn)]`, cada `unsafe` justificado con `// SAFETY:` | ✅ |
| W^X real en el núcleo, NX, CR0.WP y página de guarda en la pila | ✅ (x86_64) |
| Verificación SHA-256 del núcleo con la huella dentro del cargador firmado | ✅ |
| Consulta del estado de Secure Boot; negarse a arrancar un núcleo modificado si está activo | ✅ |
| Lo no compilado no existe (perfiles mínimos) | ✅ |
| Firma propia de distribuciones, módulos firmados con Ed25519 | 🔜 fase 2 |
| KASLR, SMEP/SMAP, pilas sombra (CET) | 🔜 fase 2 |
| Controladores en espacio de usuario + IOMMU | 🔜 fase 3 |
| Arranque medido (TPM 2.0) | 🔜 fase 3 |

---

## 11. Hoja de ruta por fases

| Fase | Nombre | Contenido | Criterio de «hecho» |
|------|--------|-----------|---------------------|
| **1** ✅ | **Génesis** | KopitBoot, Multiboot2, PVH, DT · `kernel_main` · consola gráfica · buddy · W^X · montículo · IDT/GDT · APIC · ACPI · PCI · shell · temas · perfiles · IMG/ISO · x86_64 + ARM64 + RISC-V | `cargo kopit test` en verde en las 3 arquitecturas ✅ |
| 2 | Raíces | SMP · planificador con hilos · KopitBoot ARM64/RISC-V · Sv48/TTBR1 · virtio · NVMe/AHCI/xHCI · PCIe ECAM · firma de distribuciones · KASLR | arranca desde NVMe en 4 núcleos y monta un disco |
| 3 | Tronco | espacio de usuario · IPC · controladores aislados · sys_init como PID 1 · KopitFS · compositor por software · paquetes | escritorio mínimo con una aplicación aislada |
| 4 | Copa | GPU (Vulkan) · audio · red completa · instalador gráfico · canales de actualización · anfitrión de controladores ajenos | instalar KopitOS en un portátil real y actualizarlo A/B |
| 5 | Bosque | planificador heterogéneo con energía · memoria predictiva · tableta y teléfono (táctil, sensores) | un día de batería medido |

## 12. Pruebas y calidad ✅

- `cargo kopit test` ejecuta 59 pruebas unitarias en el anfitrión (buddy con estrés aleatorio,
  FAT16/32 que se relee, GPT con CRC, ISO, SHA-256 contra vectores oficiales, DTB, perfiles con
  herencia, decodificador de teclado, compositor...) y, después, las **pruebas de humo dentro del
  núcleo** en QEMU para cada ruta de arranque: UEFI desde disco (x2APIC), UEFI desde ISO (xAPIC),
  PVH, ARM64 y RISC-V.
- Las pruebas de humo comprueban la excepción de depuración con retorno, el buddy alineado, el
  montículo con colecciones de `alloc`, W^X y la página de guarda, la llegada de tics y los planes de
  sys_init de las 6 clases. Salen de QEMU con un código (dispositivo `isa-debug-exit`,
  `sifive_test` o *semihosting*), así que la integración continua nunca se queda colgada.
- `.github/workflows/ci.yml` ejecuta todo esto en Linux y valida la construcción en Windows.

## 13. Decisiones registradas

1. **Rust + ensamblador puntual.** Seguridad de memoria en el 95 % del código. El ensamblador queda
   para lo que no se puede expresar de otro modo: el trampolín, los stubs de interrupción y los
   vectores.
2. **`arch/` dentro del crate del núcleo** (con `#[path]`) en lugar de crates aparte: evita
   dependencias circulares y el contrato lo impone el rasgo `hal::Arch`.
3. **Cargador propio y no Limine o GRUB como ruta principal:** control total de la cadena de
   confianza (la huella del núcleo dentro del binario firmado) y de la experiencia (menú en
   español). GRUB y PVH siguen soportados como alternativas.
4. **Núcleo de mitad alta reubicable físicamente:** KopitBoot lo coloca donde haya memoria y GRUB
   y QEMU en `0x200000`. El núcleo nunca supone `física = virtual - constante`.
5. **Sin crates externos:** reproducibilidad, auditoría y compilación sin red. El coste, escribir
   FAT/GPT/ISO/SHA-256, se paga una vez y queda probado.
6. **Fase 1 en un solo núcleo, pero preparada para SMP:** los cerrojos usan atómicos
   Acquire/Release, el primer MiB está reservado para los trampolines y el `TrapFrame` ya sirve
   para cambiar de contexto.
