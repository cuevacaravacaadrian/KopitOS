//! # Superficies de píxeles
//!
//! Una [`Surface`] es un rectángulo de píxeles en memoria: la VRAM que nos
//! dio el firmware (GOP) o un **búfer sombra** en RAM.
//!
//! ## Por qué un búfer sombra
//!
//! La VRAM se mapea *write-combining*: escribir es rapidísimo, pero **leer**
//! es lentísimo (cada lectura atraviesa el bus PCIe sin caché). Desplazar la
//! consola una línea copiando VRAM → VRAM puede costar decenas de
//! milisegundos en hardware real. Con un búfer sombra todo se pinta y se
//! desplaza en RAM, y sólo se *copia* a la VRAM lo que cambió (ver
//! [`Surface::blit_from`]). La consola del núcleo empieza dibujando directo en
//! VRAM y pasa a doble búfer en cuanto existe el montículo.

#![cfg_attr(not(test), no_std)]

/// Cómo se codifica un color en un píxel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PixelFormat {
    pub bytes_per_pixel: u8,
    pub red_shift: u8,
    pub red_bits: u8,
    pub green_shift: u8,
    pub green_bits: u8,
    pub blue_shift: u8,
    pub blue_bits: u8,
}

impl PixelFormat {
    /// El formato más habitual de GOP: `0x00RRGGBB` (byte 0 = azul).
    pub const BGRX8888: Self = Self {
        bytes_per_pixel: 4,
        red_shift: 16,
        red_bits: 8,
        green_shift: 8,
        green_bits: 8,
        blue_shift: 0,
        blue_bits: 8,
    };
    /// Byte 0 = rojo.
    pub const RGBX8888: Self = Self {
        bytes_per_pixel: 4,
        red_shift: 0,
        red_bits: 8,
        green_shift: 8,
        green_bits: 8,
        blue_shift: 16,
        blue_bits: 8,
    };

    #[inline]
    pub fn encode(&self, r: u8, g: u8, b: u8) -> u32 {
        #[inline]
        fn chan(v: u8, bits: u8, shift: u8) -> u32 {
            let bits = bits.clamp(1, 8);
            ((v as u32) >> (8 - bits)) << shift
        }
        chan(r, self.red_bits, self.red_shift)
            | chan(g, self.green_bits, self.green_shift)
            | chan(b, self.blue_bits, self.blue_shift)
    }

    #[inline]
    pub fn decode(&self, px: u32) -> (u8, u8, u8) {
        #[inline]
        fn chan(px: u32, bits: u8, shift: u8) -> u8 {
            let bits = bits.clamp(1, 8);
            let v = (px >> shift) & ((1 << bits) - 1);
            (v << (8 - bits)) as u8
        }
        (
            chan(px, self.red_bits, self.red_shift),
            chan(px, self.green_bits, self.green_shift),
            chan(px, self.blue_bits, self.blue_shift),
        )
    }
}

/// Rectángulo de píxeles.
pub struct Surface {
    base: *mut u8,
    width: usize,
    height: usize,
    stride: usize,
    format: PixelFormat,
}

// SAFETY: una superficie es un puntero a memoria propia; el núcleo la protege con cerrojos.
unsafe impl Send for Surface {}

impl Surface {
    /// # Safety
    /// `base` debe apuntar a `height * stride` bytes escribibles durante toda la
    /// vida de la superficie, y `stride >= width * bytes_por_píxel`.
    pub unsafe fn new(base: *mut u8, width: usize, height: usize, stride: usize, format: PixelFormat) -> Self {
        debug_assert!(stride >= width * format.bytes_per_pixel as usize);
        Self { base, width, height, stride, format }
    }

    pub fn width(&self) -> usize {
        self.width
    }
    pub fn height(&self) -> usize {
        self.height
    }
    pub fn stride(&self) -> usize {
        self.stride
    }
    pub fn format(&self) -> PixelFormat {
        self.format
    }
    pub fn base_ptr(&self) -> *mut u8 {
        self.base
    }
    pub fn size_bytes(&self) -> usize {
        self.stride * self.height
    }

    #[inline]
    fn bpp(&self) -> usize {
        self.format.bytes_per_pixel as usize
    }

    #[inline]
    fn row(&self, y: usize) -> *mut u8 {
        // SAFETY: el llamante garantiza `y < height`.
        unsafe { self.base.add(y * self.stride) }
    }

    #[inline]
    pub fn encode(&self, r: u8, g: u8, b: u8) -> u32 {
        self.format.encode(r, g, b)
    }

    #[inline]
    unsafe fn write_px(&mut self, x: usize, y: usize, color: u32) {
        // SAFETY: el llamante comprobó los límites.
        unsafe {
            let p = self.row(y).add(x * self.bpp());
            match self.format.bytes_per_pixel {
                4 => (p as *mut u32).write_unaligned(color),
                3 => {
                    p.write(color as u8);
                    p.add(1).write((color >> 8) as u8);
                    p.add(2).write((color >> 16) as u8);
                }
                2 => (p as *mut u16).write_unaligned(color as u16),
                _ => p.write(color as u8),
            }
        }
    }

    pub fn put_pixel(&mut self, x: usize, y: usize, color: u32) {
        if x < self.width && y < self.height {
            // SAFETY: límites comprobados arriba.
            unsafe { self.write_px(x, y, color) }
        }
    }

    pub fn get_pixel(&self, x: usize, y: usize) -> u32 {
        if x >= self.width || y >= self.height {
            return 0;
        }
        // SAFETY: límites comprobados.
        unsafe {
            let p = self.row(y).add(x * self.bpp());
            match self.format.bytes_per_pixel {
                4 => (p as *const u32).read_unaligned(),
                3 => p.read() as u32 | (p.add(1).read() as u32) << 8 | (p.add(2).read() as u32) << 16,
                2 => (p as *const u16).read_unaligned() as u32,
                _ => p.read() as u32,
            }
        }
    }

    /// Rellena un rectángulo (recortado a la superficie).
    pub fn fill_rect(&mut self, x: usize, y: usize, w: usize, h: usize, color: u32) {
        let x1 = (x + w).min(self.width);
        let y1 = (y + h).min(self.height);
        if x >= x1 || y >= y1 {
            return;
        }
        for py in y..y1 {
            if self.format.bytes_per_pixel == 4 {
                // SAFETY: fila y columnas dentro de límites; píxeles de 4 bytes.
                unsafe {
                    let row = self.row(py).add(x * 4) as *mut u32;
                    for i in 0..(x1 - x) {
                        row.add(i).write_unaligned(color);
                    }
                }
            } else {
                for px in x..x1 {
                    // SAFETY: dentro de límites.
                    unsafe { self.write_px(px, py, color) }
                }
            }
        }
    }

    /// Dibuja un glifo monocromo. `rows[i]` es la fila `i`; el bit 0 es el
    /// píxel de la izquierda. Cada píxel del glifo ocupa `scale × scale`.
    /// Con `bg = None` el fondo es transparente.
    #[allow(clippy::too_many_arguments)]
    pub fn draw_glyph(
        &mut self,
        x: usize,
        y: usize,
        rows: &[u16],
        glyph_w: usize,
        scale: usize,
        fg: u32,
        bg: Option<u32>,
    ) {
        let scale = scale.max(1);
        let w = glyph_w * scale;
        let h = rows.len() * scale;
        let fully_inside = x + w <= self.width && y + h <= self.height;
        for (gy, &bits) in rows.iter().enumerate() {
            for sy in 0..scale {
                let py = y + gy * scale + sy;
                if py >= self.height {
                    return;
                }
                if fully_inside && self.format.bytes_per_pixel == 4 {
                    // Ruta rápida: una fila completa con un puntero.
                    // SAFETY: el glifo cabe entero (comprobado arriba).
                    unsafe {
                        let row = self.row(py).add(x * 4) as *mut u32;
                        for gx in 0..glyph_w {
                            let on = bits >> gx & 1 != 0;
                            let color = if on { Some(fg) } else { bg };
                            if let Some(c) = color {
                                for sx in 0..scale {
                                    row.add(gx * scale + sx).write_unaligned(c);
                                }
                            }
                        }
                    }
                } else {
                    for gx in 0..glyph_w {
                        let on = bits >> gx & 1 != 0;
                        if let Some(c) = if on { Some(fg) } else { bg } {
                            for sx in 0..scale {
                                self.put_pixel(x + gx * scale + sx, py, c);
                            }
                        }
                    }
                }
            }
        }
    }

    /// Desplaza hacia arriba `dy` píxeles la franja `[y0, y1)` y rellena el hueco.
    pub fn scroll_up(&mut self, y0: usize, y1: usize, dy: usize, fill: u32) {
        let y1 = y1.min(self.height);
        if y0 >= y1 {
            return;
        }
        if dy >= y1 - y0 {
            self.fill_rect(0, y0, self.width, y1 - y0, fill);
            return;
        }
        let bytes = (y1 - y0 - dy) * self.stride;
        // SAFETY: origen y destino dentro de la superficie; `copy` admite solape.
        unsafe { core::ptr::copy(self.row(y0 + dy), self.row(y0), bytes) };
        self.fill_rect(0, y1 - dy, self.width, dy, fill);
    }

    /// Copia un rectángulo desde otra superficie del mismo formato y tamaño
    /// (búfer sombra → VRAM).
    pub fn blit_from(&mut self, src: &Surface, x: usize, y: usize, w: usize, h: usize) {
        if src.format != self.format {
            return;
        }
        let x1 = (x + w).min(self.width).min(src.width);
        let y1 = (y + h).min(self.height).min(src.height);
        if x >= x1 || y >= y1 {
            return;
        }
        let bpp = self.bpp();
        for py in y..y1 {
            // SAFETY: filas dentro de ambas superficies, que no se solapan.
            unsafe {
                core::ptr::copy_nonoverlapping(src.row(py).add(x * bpp), self.row(py).add(x * bpp), (x1 - x) * bpp);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn surface(buf: &mut Vec<u32>, w: usize, h: usize) -> Surface {
        buf.resize(w * h, 0);
        // SAFETY: `buf` vive más que la superficie dentro de cada prueba.
        unsafe { Surface::new(buf.as_mut_ptr() as *mut u8, w, h, w * 4, PixelFormat::BGRX8888) }
    }

    #[test]
    fn codifica_colores() {
        let f = PixelFormat::BGRX8888;
        assert_eq!(f.encode(0x12, 0x34, 0x56), 0x0012_3456);
        assert_eq!(PixelFormat::RGBX8888.encode(0x12, 0x34, 0x56), 0x0056_3412);
        assert_eq!(f.decode(0x0012_3456), (0x12, 0x34, 0x56));
        let rgb565 = PixelFormat {
            bytes_per_pixel: 2,
            red_shift: 11,
            red_bits: 5,
            green_shift: 5,
            green_bits: 6,
            blue_shift: 0,
            blue_bits: 5,
        };
        assert_eq!(rgb565.encode(0xFF, 0xFF, 0xFF), 0xFFFF);
    }

    #[test]
    fn rectangulos_recortados() {
        let mut buf = Vec::new();
        let mut s = surface(&mut buf, 8, 4);
        s.fill_rect(6, 2, 10, 10, 7);
        assert_eq!(s.get_pixel(7, 3), 7);
        assert_eq!(s.get_pixel(5, 3), 0);
        assert_eq!(s.get_pixel(6, 1), 0);
    }

    #[test]
    fn glifos_escalados() {
        let mut buf = Vec::new();
        let mut s = surface(&mut buf, 16, 16);
        // Una "L" de 2x2 escalada x2.
        s.draw_glyph(0, 0, &[0b01, 0b11], 2, 2, 9, Some(1));
        assert_eq!(s.get_pixel(0, 0), 9);
        assert_eq!(s.get_pixel(1, 1), 9);
        assert_eq!(s.get_pixel(2, 0), 1);
        assert_eq!(s.get_pixel(3, 3), 9);
    }

    #[test]
    fn desplazar_y_copiar() {
        let mut buf = Vec::new();
        let mut s = surface(&mut buf, 4, 4);
        for y in 0..4 {
            s.fill_rect(0, y, 4, 1, y as u32 + 1);
        }
        s.scroll_up(0, 4, 1, 0xAA);
        assert_eq!(s.get_pixel(0, 0), 2);
        assert_eq!(s.get_pixel(0, 2), 4);
        assert_eq!(s.get_pixel(0, 3), 0xAA);

        let mut dst_buf = Vec::new();
        let mut d = surface(&mut dst_buf, 4, 4);
        d.blit_from(&s, 1, 1, 2, 2);
        assert_eq!(d.get_pixel(1, 1), 3);
        assert_eq!(d.get_pixel(0, 0), 0);
    }
}
