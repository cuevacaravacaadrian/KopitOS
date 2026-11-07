//! # UART ARM PL011
//!
//! La UART estándar de ARM (QEMU `virt`, Raspberry Pi 4/5, servidores
//! SBSA). Registros de 32 bits mapeados en memoria.

#![cfg_attr(not(test), no_std)]

use kopit_driver_api::{CharSink, MmioRegisters};

const DR: usize = 0x00; // datos
const FR: usize = 0x18; // indicadores
const IMSC: usize = 0x38; // máscara de interrupciones
const ICR: usize = 0x44; // borrado de interrupciones

const FR_RXFE: u32 = 1 << 4; // FIFO de recepción vacío
const FR_TXFF: u32 = 1 << 5; // FIFO de transmisión lleno
const IMSC_RXIM: u32 = 1 << 4;

pub struct Pl011 {
    regs: MmioRegisters,
}

impl Pl011 {
    /// La UART suele llegar ya configurada por el firmware: sólo la usamos.
    pub const fn new(regs: MmioRegisters) -> Self {
        Self { regs }
    }

    pub fn write_byte(&mut self, byte: u8) {
        for _ in 0..100_000 {
            if self.regs.read32(FR) & FR_TXFF == 0 {
                break;
            }
            core::hint::spin_loop();
        }
        self.regs.write32(DR, byte as u32);
    }

    pub fn try_read_byte(&mut self) -> Option<u8> {
        if self.regs.read32(FR) & FR_RXFE == 0 { Some((self.regs.read32(DR) & 0xFF) as u8) } else { None }
    }

    pub fn enable_rx_interrupt(&mut self) {
        self.regs.write32(ICR, 0x7FF);
        self.regs.write32(IMSC, IMSC_RXIM);
    }
}

impl CharSink for Pl011 {
    fn write_bytes(&mut self, bytes: &[u8]) {
        for &b in bytes {
            if b == b'\n' {
                self.write_byte(b'\r');
            }
            self.write_byte(b);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escribe_en_el_registro_de_datos() {
        // Banco de registros simulado en memoria normal.
        let mut regs = vec![0u32; 32];
        let base = regs.as_mut_ptr() as usize;
        // SAFETY: `regs` vive hasta el final de la prueba.
        let mut uart = Pl011::new(unsafe { MmioRegisters::new(base, 0) });
        uart.write_byte(b'K');
        assert_eq!(regs[DR / 4], b'K' as u32);
        regs[FR / 4] = 0; // hay un byte esperando
        regs[DR / 4] = b'x' as u32;
        assert_eq!(uart.try_read_byte(), Some(b'x'));
    }
}
