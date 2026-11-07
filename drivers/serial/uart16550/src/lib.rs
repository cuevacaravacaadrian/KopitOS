//! # UART 16550
//!
//! La consola serie es el primer y último recurso de un sistema operativo:
//! funciona antes que la pantalla y sigue funcionando en un pánico. Este
//! controlador es genérico sobre [`RegisterIo`], así que el **mismo código**
//! atiende al COM1 de un PC (puertos `0x3F8..`) y a la UART `ns16550a` de la
//! placa `virt` de RISC-V (MMIO en `0x1000_0000`).
//!
//! Detecta si el chip existe (prueba en bucle local): en un portátil sin
//! puerto serie no nos quedamos esperando para siempre a que se vacíe un
//! registro que no existe.

#![cfg_attr(not(test), no_std)]

use kopit_driver_api::{CharSink, RegisterIo};

const RBR_THR: u16 = 0; // recepción / transmisión (DLAB = 0) — divisor bajo (DLAB = 1)
const IER: u16 = 1; // interrupciones — divisor alto (DLAB = 1)
const FCR: u16 = 2; // control de FIFO
const LCR: u16 = 3; // control de línea
const MCR: u16 = 4; // control de módem
const LSR: u16 = 5; // estado de línea

const LSR_DATA_READY: u8 = 1 << 0;
const LSR_THR_EMPTY: u8 = 1 << 5;

/// Reloj de referencia del 16550 dividido entre 16.
const BASE_BAUD: u32 = 115_200;

pub struct Uart16550<R: RegisterIo> {
    regs: R,
    present: bool,
}

impl<R: RegisterIo> Uart16550<R> {
    pub const fn new(regs: R) -> Self {
        Self { regs, present: false }
    }

    /// Programa 8N1 a `baud` baudios con FIFO. Devuelve `false` si no hay chip.
    pub fn init(&mut self, baud: u32) -> bool {
        let r = &self.regs;
        let divisor = (BASE_BAUD / baud.clamp(1, BASE_BAUD)) as u16;
        r.write(IER, 0x00); // sin interrupciones mientras configuramos
        r.write(LCR, 0x80); // DLAB = 1: acceso al divisor
        r.write(RBR_THR, divisor as u8);
        r.write(IER, (divisor >> 8) as u8);
        r.write(LCR, 0x03); // 8 bits, sin paridad, 1 bit de parada
        r.write(FCR, 0xC7); // FIFO activo, vaciado, umbral de 14 bytes
        // Prueba en bucle local: lo que escribimos debe volver.
        r.write(MCR, 0x1E);
        r.write(RBR_THR, 0xAE);
        let echo = r.read(RBR_THR);
        // Modo normal: DTR + RTS + OUT1 + OUT2 (OUT2 enruta la IRQ en un PC).
        r.write(MCR, 0x0F);
        self.present = echo == 0xAE;
        self.present
    }

    /// Para UART ya configuradas por el firmware (QEMU `virt`, OpenSBI...).
    pub fn assume_present(&mut self) {
        self.present = true;
    }

    pub fn is_present(&self) -> bool {
        self.present
    }

    pub fn write_byte(&mut self, byte: u8) {
        if !self.present {
            return;
        }
        // Espera acotada: si el otro extremo no lee, no colgamos el núcleo.
        for _ in 0..100_000 {
            if self.regs.read(LSR) & LSR_THR_EMPTY != 0 {
                break;
            }
            core::hint::spin_loop();
        }
        self.regs.write(RBR_THR, byte);
    }

    pub fn try_read_byte(&mut self) -> Option<u8> {
        if self.present && self.regs.read(LSR) & LSR_DATA_READY != 0 { Some(self.regs.read(RBR_THR)) } else { None }
    }

    /// Activa la interrupción de "dato recibido".
    pub fn enable_rx_interrupt(&mut self) {
        if self.present {
            self.regs.write(IER, 0x01);
        }
    }
}

impl<R: RegisterIo> CharSink for Uart16550<R> {
    /// Escribe bytes traduciendo `\n` a `\r\n` (lo que esperan los terminales).
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
    use std::cell::RefCell;

    /// UART simulada: registra escrituras y emula el bucle local.
    #[derive(Default)]
    struct FakeUart {
        regs: RefCell<[u8; 8]>,
        sent: RefCell<Vec<u8>>,
        loopback: RefCell<bool>,
        present: bool,
    }

    impl RegisterIo for FakeUart {
        fn read(&self, reg: u16) -> u8 {
            if !self.present {
                return 0xFF; // bus flotante
            }
            match reg {
                LSR => LSR_THR_EMPTY,
                _ => self.regs.borrow()[reg as usize],
            }
        }
        fn write(&self, reg: u16, value: u8) {
            if reg == MCR {
                *self.loopback.borrow_mut() = value & 0x10 != 0;
            }
            if reg == RBR_THR && !*self.loopback.borrow() && self.regs.borrow()[LCR as usize] & 0x80 == 0 {
                self.sent.borrow_mut().push(value);
            }
            self.regs.borrow_mut()[reg as usize] = value;
        }
    }

    #[test]
    fn detecta_y_traduce_saltos_de_linea() {
        let mut uart = Uart16550::new(FakeUart { present: true, ..Default::default() });
        assert!(uart.init(115_200));
        uart.write_bytes(b"hola\n");
        assert_eq!(uart.regs.sent.borrow().as_slice(), b"hola\r\n");
    }

    #[test]
    fn sin_chip_no_se_cuelga() {
        let mut uart = Uart16550::new(FakeUart::default());
        assert!(!uart.init(115_200));
        uart.write_bytes(b"nadie escucha");
        assert!(uart.regs.sent.borrow().is_empty());
    }
}
