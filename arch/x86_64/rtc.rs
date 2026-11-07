//! Reloj CMOS (RTC): la hora cuando no la da el firmware (arranques PVH/BIOS).

use super::io::{inb, outb};
use kopit_boot_proto::WallClock;

fn read_reg(reg: u8) -> u8 {
    outb(0x70, reg | 0x80); // bit 7: NMI desactivada durante el acceso
    inb(0x71)
}

fn updating() -> bool {
    read_reg(0x0A) & 0x80 != 0
}

fn bcd(v: u8) -> u8 {
    (v & 0x0F) + (v >> 4) * 10
}

fn snapshot() -> [u8; 7] {
    while updating() {
        core::hint::spin_loop();
    }
    [read_reg(0x00), read_reg(0x02), read_reg(0x04), read_reg(0x07), read_reg(0x08), read_reg(0x09), read_reg(0x32)]
}

pub fn read() -> WallClock {
    // Se lee hasta obtener dos lecturas iguales (evita leer a mitad de una actualización).
    let mut a = snapshot();
    for _ in 0..5 {
        let b = snapshot();
        if a == b {
            break;
        }
        a = b;
    }
    let status_b = read_reg(0x0B);
    let binary = status_b & 0x04 != 0;
    let h24 = status_b & 0x02 != 0;
    let conv = |v: u8| if binary { v } else { bcd(v) };
    let mut hour = conv(a[2] & 0x7F);
    if !h24 && a[2] & 0x80 != 0 {
        hour = (hour % 12) + 12;
    }
    let century = match conv(a[6]) {
        19..=21 => conv(a[6]) as u16,
        _ => 20,
    };
    WallClock {
        year: century * 100 + conv(a[5]) as u16,
        month: conv(a[4]),
        day: conv(a[3]),
        hour,
        minute: conv(a[1]),
        second: conv(a[0]),
        valid: 1,
    }
}
