//! Lectura de archivos del volumen del que arrancó KopitBoot (la ESP).

use crate::firmware::{self, handle_protocol, path_to_ucs2};
use crate::uefi::*;
use alloc::vec::Vec;
use core::ptr::null_mut;

pub struct Volume {
    root: *mut File,
}

impl Volume {
    /// El sistema de archivos del dispositivo desde el que se cargó este `.EFI`.
    pub fn boot_volume() -> Result<Self, Status> {
        let image: *mut LoadedImage = handle_protocol(firmware::image(), &LOADED_IMAGE_GUID)?;
        // SAFETY: protocolo devuelto por el firmware.
        let device = unsafe { (*image).device_handle };
        let sfs: *mut SimpleFileSystem = handle_protocol(device, &SIMPLE_FILE_SYSTEM_GUID)?;
        let mut root = null_mut();
        // SAFETY: llamada UEFI.
        unsafe { ((*sfs).open_volume)(sfs, &mut root) }.ok()?;
        Ok(Self { root })
    }

    /// Lee un archivo entero (`\kopit\kernel.elf` o `/kopit/kernel.elf`).
    pub fn read(&self, path: &str) -> Result<Vec<u8>, Status> {
        let name = path_to_ucs2(path);
        let mut file: *mut File = null_mut();
        // SAFETY: llamada UEFI con nombre UCS-2 terminado en 0.
        unsafe { ((*self.root).open)(self.root, &mut file, name.as_ptr(), FILE_MODE_READ, 0) }.ok()?;
        let result = (|| -> Result<Vec<u8>, Status> {
            let mut size = 0u64;
            // SAFETY: `file` abierto arriba. Posición u64::MAX = final del archivo.
            unsafe {
                ((*file).set_position)(file, u64::MAX).ok()?;
                ((*file).get_position)(file, &mut size).ok()?;
                ((*file).set_position)(file, 0).ok()?;
            }
            let mut data = alloc::vec![0u8; size as usize];
            let mut done = 0usize;
            while done < data.len() {
                let mut chunk = data.len() - done;
                // SAFETY: escribimos dentro de `data`.
                unsafe { ((*file).read)(file, &mut chunk, data.as_mut_ptr().add(done)) }.ok()?;
                if chunk == 0 {
                    break;
                }
                done += chunk;
            }
            data.truncate(done);
            Ok(data)
        })();
        // SAFETY: cerramos lo que abrimos.
        unsafe { ((*file).close)(file) };
        result
    }
}
