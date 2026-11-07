# Crear una distribución basada en KopitOS

Una distribución derivada es **un archivo de perfil**, más, si quieres, una carpeta de archivos
propios. No hace falta tocar el código del núcleo ni hacer un *fork*.

## 1. Crearla

```sh
cargo kopit new-distro aurora-os --from desktop --name "Aurora OS"
```

Se crean dos cosas:

```text
profiles/aurora-os.toml                     el perfil (hereda de desktop)
profiles/overlay/aurora-os/kopit/motd.txt   archivos que se copian tal cual a la imagen
```

## 2. Personalizarla

`profiles/aurora-os.toml`: sólo se escribe lo que cambia respecto al perfil del que hereda.

```toml
[distro]
inherits = "desktop"
name = "Aurora OS"
description = "Mi distribución para el aula"
version = "1.0.0"

[kernel]
# Quita lo que no necesites: no se compila, no ocupa, no se puede atacar.
features = ["fbcon", "shell", "acpi", "pci", "ps2", "selftest"]
cmdline = "loglevel=aviso"

[boot]
timeout = 2
resolution = "1920x1080"

[ui]
theme = "aurora"
keymap = "es"
font_scale = "3"

[ui.colors]          # colores de marca sobre cualquier rol del tema
accent = "#ff8800"
bar = "#1a1033"

[image]
overlay = "overlay/aurora-os"
```

Roles de color disponibles: `fondo`, `texto`, `tenue`, `acento`, `barra`, `bar_text`, `panel`,
`error`, `aviso`, `info`, `debug`, `trace`, `ok`, `cursor` y `on_accent` (también en inglés).

## 3. Probarla y generarla

```sh
cargo kopit run --profile aurora-os                  # QEMU con UEFI
cargo kopit run --profile aurora-os --boot iso       # desde la ISO
cargo kopit test --profile aurora-os                 # pruebas de humo de TU distribución
cargo kopit image --profile aurora-os --release      # .img para grabar en un USB (dd / Rufus en modo DD)
cargo kopit iso --profile aurora-os --release        # .iso híbrida: CD o USB
```

Las imágenes quedan en `target/kopitos/x86_64/aurora-os/`.

## 4. Qué se decide dónde

| Quiero... | Dónde |
|-----------|-------|
| que un subsistema no exista en el binario | `kernel.features` (compilación) |
| cambiar el aspecto, el teclado o el nivel de registro | `ui.*` y `kernel.cmdline` (ejecución: también editable en el menú de KopitBoot con la tecla `e`) |
| añadir archivos a la imagen | `image.overlay` (nombres 8.3: `motd.txt`, `logo.bmp`...) |
| otra clase de dispositivo | `distro.device_class`: cambia los Hz del temporizador y el plan de servicios de sys_init |

## 5. Compilaciones reproducibles

```sh
SOURCE_DATE_EPOCH=1767225600 cargo kopit iso --profile aurora-os --release
```

Con la misma fecha y el mismo código fuente, la ISO sale idéntica bit a bit: fechas de FAT e ISO,
números de serie y GUID se derivan del contenido.

## 6. Próximamente

- `cargo kopit sign`: firmar KopitBoot con las claves de *tu* distribución para Secure Boot.
- Temas, fondos y logotipos como módulos firmados.
- Paquetes y actualizaciones A/B con canales propios.

Más contexto en [ARCHITECTURE.md §8](../ARCHITECTURE.md#8-distribuciones-derivadas).
