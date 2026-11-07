# KBP: protocolo de arranque de KopitOS (versión 1)

KBP (*Kopit Boot Protocol*) es el contrato binario entre cualquier cargador y el núcleo. La
definición de referencia está en [`boot/protocol/src/lib.rs`](../boot/protocol/src/lib.rs); este
documento la explica.

## Estado de la máquina al entrar en el núcleo

| | x86_64 (KopitBoot) | x86_64 (Multiboot2/PVH) | ARM64 / RISC-V (fase 1) |
|---|---|---|---|
| Modo | largo, 64 bits | 32 bits; el trampolín pasa a 64 | EL1 / modo S |
| Paginación | tablas del cargador: identidad + HHDM + núcleo | tablas del trampolín: 0–4 GiB en identidad y en HHDM, 2 GiB altos | identidad (ARM64: bloques de 1 GiB) |
| Interrupciones | desactivadas | desactivadas | desactivadas |
| Pila | 64 KiB en el HHDM (el núcleo cambia a la suya en `_start`) | la del núcleo (`.bss`) | la del núcleo |
| Argumento | `rdi = &BootInfo` (virtual) | protocolo + puntero físico | `a1` (RISC-V) o `0x4000_0000` (ARM64) = DTB |

## Estructura `BootInfo`

`#[repr(C)]`, todos los campos de ancho fijo.

| Campo | Tipo | Significado |
|-------|------|-------------|
| `magic` | `u64` | `"KOPITKBP"` en ASCII (little-endian) |
| `version` | `u32` | 1 |
| `size` | `u32` | `size_of::<BootInfo>()` según el cargador; el núcleo rechaza tamaños menores |
| `source` | `BootSource` (`u32`) | 1 KopitBoot, 2 Multiboot2, 3 PVH, 4 Device tree |
| `flags` | `u32` | reservado (0) |
| `hhdm_base` | `u64` | base virtual del mapa directo (`0xFFFF_8000_0000_0000` en x86_64; 0 en identidad) |
| `hhdm_limit` | `u64` | el mapa directo es válido para las direcciones físicas `[0, hhdm_limit)` |
| `kernel` | `KernelImage` | `phys_base`, `virt_base` y `size` de la imagen cargada |
| `memory_map` | `MemoryMap` | puntero (virtual) y número de `MemoryRegion` |
| `framebuffer` | `Framebuffer` | base física, tamaño, ancho, alto, *stride* en bytes, bpp, tipo y desplazamiento/bits por canal |
| `rsdp` | `u64` | dirección física del RSDP de ACPI (0 = no hay) |
| `smbios` | `u64` | dirección física de SMBIOS (0 = no hay) |
| `dtb` | `u64` | dirección física del device tree (0 = no hay) |
| `efi_system_table` | `u64` | tabla de sistema UEFI (servicios de ejecución) |
| `cmdline` | `BootStr` | línea de órdenes UTF-8 (puntero virtual + longitud) |
| `loader_name` | `BootStr` | nombre y versión del cargador |
| `modules` | `ModuleList` | módulos cargados (initrd, temas...) |
| `boot_time` | `WallClock` | fecha y hora del firmware (`valid = 1` si se conoce) |
| `loader_timestamp` | `u64` | contador de ciclos (TSC) al empezar el cargador: mide el arranque completo |
| `secure_boot` | `SecureBootState` | 0 no disponible, 1 desactivado, 2 activo, 3 modo configuración |
| `kernel_verification` | `KernelVerification` | 0 sin verificar, 1 SHA-256 verificado, 2 no coincide |
| `reserved` | `[u64; 8]` | ampliaciones futuras (a 0) |

### Tipos de región de memoria

| Valor | Tipo | El núcleo... |
|-------|------|--------------|
| 1 | libre | la reparte desde el primer momento |
| 2 | reservada | no la toca nunca |
| 3 | ACPI recuperable | la conserva mientras use las tablas |
| 4 | ACPI NVS | no la toca nunca |
| 5 | defectuosa | no la toca nunca |
| 6 | cargador (recuperable) | la recupera tras cambiar a sus propias tablas |
| 7 | núcleo | — |
| 8 | framebuffer | la mapea como *write-combining* |
| 9 | módulo | la entrega al subsistema correspondiente |
| 10 | firmware (runtime) | la conserva para los servicios de ejecución UEFI |
| 11 | MMIO | sólo a través de ventanas MMIO |
| 12 | persistente (NVDIMM) | reservada para KopitFS |

Garantías del mapa (`MemoryMapBuilder::finish`): ordenado por dirección, sin solapamientos (gana el
tipo más restrictivo), regiones libres alineadas a 4 KiB y regiones contiguas del mismo tipo
fusionadas.

## Compatibilidad

- Un núcleo **más nuevo** con un cargador **más viejo** lee sólo hasta `size`.
- Un cargador **más nuevo** puede añadir tipos de región o de arranque: un núcleo viejo los trata
  como «reservado» o «desconocido», nunca como comportamiento indefinido.
- Las ampliaciones van en `reserved` o detrás de una versión mayor.

## Archivos en la ESP (KopitBoot)

```text
\EFI\BOOT\BOOTX64.EFI     KopitBoot (lleva dentro la huella SHA-256 del núcleo)
\kopit\kernel.elf         el núcleo (ELF64, sin información de depuración)
\kopit\boot.cfg           menú: title, timeout, default, resolution, entry=título|ruta|línea de órdenes
\leeme.txt                información de la compilación
```
