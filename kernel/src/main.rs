//! # El núcleo de KopitOS
//!
//! Núcleo **híbrido modular**: el núcleo contiene sólo lo que debe ser
//! privilegiado y rápido (memoria, interrupciones, planificación, IPC); los
//! controladores viven en crates aislados (`drivers/`) que no dependen de él y
//! que, en fases posteriores, se ejecutarán como procesos con capacidades.
//!
//! Mapa del código:
//!
//! | Módulo            | Qué hace                                                      |
//! |-------------------|---------------------------------------------------------------|
//! | [`init`]          | `kernel_main`: la secuencia de arranque por etapas            |
//! | [`hal`]           | contrato que cumple cada arquitectura (`arch/<cpu>/`)         |
//! | [`log`]           | registro del núcleo: niveles, anillo `dmesg`, destinos        |
//! | [`console`]       | consola gráfica (framebuffer) o de texto (VGA) + serie        |
//! | [`mm`]            | marcos físicos (buddy), paginación W^X, montículo, MMIO       |
//! | [`time`]          | reloj monotónico, tics y fecha                                |
//! | [`boot`]          | copia y validación del `BootInfo` (protocolo KBP)             |
//! | `acpi`, `pci`     | descubrimiento de hardware                                    |
//! | `shell`           | monitor interactivo del núcleo                                |

#![no_std]
#![no_main]
#![deny(unsafe_op_in_unsafe_fn)]

extern crate alloc;

#[macro_use]
pub mod log;
pub mod boot;
pub mod cmdline;
pub mod console;
pub mod hal;
pub mod init;
pub mod input;
pub mod mm;
pub mod panic;
pub mod sync;
pub mod time;
pub mod version;

#[cfg(feature = "acpi")]
pub mod acpi;
#[cfg(any(target_arch = "aarch64", target_arch = "riscv64"))]
pub mod dtb;
#[cfg(feature = "pci")]
pub mod pci;
#[cfg(feature = "selftest")]
pub mod selftest;
#[cfg(feature = "shell")]
pub mod shell;

// El código de cada CPU vive en `arch/` (fuera de este directorio, como en
// Linux) pero se compila dentro de este crate: así puede usar los cerrojos, el
// registro y la memoria del núcleo sin dependencias circulares entre crates.
#[cfg(target_arch = "x86_64")]
#[path = "../../arch/x86_64/mod.rs"]
pub mod arch;

#[cfg(target_arch = "aarch64")]
#[path = "../../arch/aarch64/mod.rs"]
pub mod arch;

#[cfg(target_arch = "riscv64")]
#[path = "../../arch/riscv64/mod.rs"]
pub mod arch;
