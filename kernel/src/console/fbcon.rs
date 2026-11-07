//! # Consola gráfica sobre el framebuffer
//!
//! ```text
//! ┌──────────────────────────────────────────────────────────────┐
//! │■ KopitOS 0.1.0 «Génesis» · x86_64 · desktop      mié 14:32:07│ ← barra superior
//! ├──────────────────────────────────────────────────────────────┤
//! │  0.004 INFO  mm::pmm: 498 MiB libres en 3 zonas               │
//! │  ...                                                          │ ← área de texto
//! ├──────────────────────────────────────────────────────────────┤
//! │ RAM 471/512 MiB · 1 CPU · 1000 Hz            tema kopit-noche │ ← barra inferior
//! └──────────────────────────────────────────────────────────────┘
//! ```
//!
//! * **Escala adaptativa**: la letra base es de 8x8; se escala ×1…×4 según la
//!   resolución (un móvil 4K y un servidor a 1024x768 se leen igual de bien).
//!   Las escalas pares usan los glifos suavizados Scale2x de `kopit-font`.
//! * **Doble búfer**: empieza pintando en la VRAM y, cuando hay montículo,
//!   pasa a un búfer sombra en RAM; a la VRAM sólo se copian las filas que
//!   cambiaron (el desplazamiento nunca lee de la VRAM, que es lentísima).
//! * Este módulo **no registra nada** (lo llama el propio registro: un
//!   `kinfo!` aquí dentro sería un interbloqueo).

use super::ScreenInfo;
use crate::log::FixedBuf;
use crate::version;
use alloc::vec::Vec;
use core::fmt::Write;
use kopit_boot_proto::Framebuffer;
use kopit_drv_fb::{PixelFormat, Surface};
use kopit_font::{glyph8, glyph16};
use kopit_theme::{Rgb, Theme};

pub struct FbCon {
    vram: Surface,
    shadow: Option<(Surface, Vec<u32>)>,
    theme: Theme,
    scale: usize,
    cell_w: usize,
    cell_h: usize,
    cols: usize,
    rows: usize,
    text_x: usize,
    text_y: usize,
    bar_h: usize,
    cx: usize,
    cy: usize,
    dirty: Option<(usize, usize)>,
    cursor_on: bool,
    /// Columna a la que vuelven las líneas largas al partirse (sangría colgante).
    wrap_indent: usize,
    title_right: FixedBuf<96>,
    status_left: FixedBuf<192>,
    status_right: FixedBuf<96>,
}

/// Escala automática: ×1 hasta ~1024 px de ancho, ×2 en 1280–1920, ×3, ×4 en 4K.
fn auto_scale(w: usize, h: usize) -> usize {
    (w / 640).min(h / 400).clamp(1, 4)
}

impl FbCon {
    pub fn new(base: *mut u8, fb: &Framebuffer, theme: Theme, scale: Option<usize>) -> Self {
        let format = PixelFormat {
            bytes_per_pixel: (fb.bpp / 8) as u8,
            red_shift: fb.red_shift,
            red_bits: fb.red_bits,
            green_shift: fb.green_shift,
            green_bits: fb.green_bits,
            blue_shift: fb.blue_shift,
            blue_bits: fb.blue_bits,
        };
        let (w, h) = (fb.width as usize, fb.height as usize);
        // SAFETY: el cargador garantiza que el framebuffer está mapeado (HHDM).
        let vram = unsafe { Surface::new(base, w, h, fb.stride as usize, format) };
        let scale = scale.unwrap_or_else(|| auto_scale(w, h));
        let cell_w = 8 * scale;
        let cell_h = 8 * scale;
        let bar_h = cell_h + 4 * scale;
        let text_x = cell_w;
        let text_y = bar_h + cell_h / 2;
        let cols = (w.saturating_sub(2 * text_x) / cell_w).max(1);
        let rows = (h.saturating_sub(2 * bar_h + cell_h) / cell_h).max(1);
        let mut con = Self {
            vram,
            shadow: None,
            theme,
            scale,
            cell_w,
            cell_h,
            cols,
            rows,
            text_x,
            text_y,
            bar_h,
            cx: 0,
            cy: 0,
            dirty: None,
            cursor_on: false,
            wrap_indent: 0,
            title_right: FixedBuf::new(),
            status_left: FixedBuf::new(),
            status_right: FixedBuf::new(),
        };
        con.status_left.set("arrancando…");
        con.repaint_all();
        con
    }

    pub fn info(&self) -> ScreenInfo {
        ScreenInfo {
            kind: "framebuffer",
            width: self.vram.width(),
            height: self.vram.height(),
            cols: self.cols,
            rows: self.rows,
            scale: self.scale,
            double_buffered: self.shadow.is_some(),
        }
    }

    fn surface(&mut self) -> &mut Surface {
        match self.shadow.as_mut() {
            Some((s, _)) => s,
            None => &mut self.vram,
        }
    }

    fn px(&self, c: Rgb) -> u32 {
        self.vram.format().encode(c.r, c.g, c.b)
    }

    fn mark_dirty(&mut self, y0: usize, y1: usize) {
        self.dirty = Some(match self.dirty {
            Some((a, b)) => (a.min(y0), b.max(y1)),
            None => (y0, y1),
        });
    }

    /// Copia al framebuffer real lo que cambió en el búfer sombra.
    fn present(&mut self) {
        if let (Some((shadow, _)), Some((y0, y1))) = (self.shadow.as_ref(), self.dirty) {
            let w = self.vram.width();
            self.vram.blit_from(shadow, 0, y0, w, y1 - y0);
        }
        self.dirty = None;
    }

    fn glyph_at(&mut self, x: usize, y: usize, c: char, fg: Rgb, bg: Rgb) {
        let (fg, bg) = (self.px(fg), self.px(bg));
        let scale = self.scale;
        if scale % 2 == 0 {
            let g = glyph16(c);
            self.surface().draw_glyph(x, y, &g, 16, scale / 2, fg, Some(bg));
        } else {
            let g8 = glyph8(c);
            let rows: [u16; 8] = core::array::from_fn(|i| g8[i] as u16);
            self.surface().draw_glyph(x, y, &rows, 8, scale, fg, Some(bg));
        }
    }

    /// Texto en una posición en píxeles (barras, paneles). Devuelve el ancho.
    fn text_at(&mut self, x: usize, y: usize, s: &str, fg: Rgb, bg: Rgb) -> usize {
        let mut cx = x;
        for c in s.chars() {
            if cx + self.cell_w > self.vram.width() {
                break;
            }
            self.glyph_at(cx, y, c, fg, bg);
            cx += self.cell_w;
        }
        self.mark_dirty(y, y + self.cell_h);
        cx - x
    }

    fn text_width(&self, s: &str) -> usize {
        s.chars().count() * self.cell_w
    }

    // --- Texto con cursor ---------------------------------------------------------

    pub fn set_wrap_indent(&mut self, cols: usize) {
        self.wrap_indent = cols.min(self.cols / 2);
    }

    /// Escribe texto partiendo las líneas largas por palabras, no por letras.
    pub fn write(&mut self, s: &str, fg: Rgb) {
        if self.cursor_on {
            self.erase_cursor();
        }
        for (i, line) in s.split('\n').enumerate() {
            if i > 0 {
                self.put('\n', fg);
            }
            for word in line.split_inclusive(' ') {
                let len = word.trim_end_matches(' ').chars().count();
                if len > 0
                    && self.cx + len > self.cols
                    && self.cx > self.wrap_indent
                    && len <= self.cols - self.wrap_indent
                {
                    self.wrap();
                }
                for c in word.chars() {
                    if c == ' ' && self.cx >= self.cols {
                        continue; // un espacio no merece una línea nueva
                    }
                    self.put(c, fg);
                }
            }
        }
        self.present();
    }

    /// Salto por línea llena: continúa en la columna de sangría.
    fn wrap(&mut self) {
        self.newline();
        self.cx = self.wrap_indent;
    }

    fn cell_xy(&self, col: usize, row: usize) -> (usize, usize) {
        (self.text_x + col * self.cell_w, self.text_y + row * self.cell_h)
    }

    fn put(&mut self, c: char, fg: Rgb) {
        match c {
            '\n' => self.newline(),
            '\r' => self.cx = 0,
            '\t' => {
                for _ in 0..(4 - self.cx % 4) {
                    self.put(' ', fg);
                }
            }
            '\u{8}' => {
                if self.cx > 0 {
                    self.cx -= 1;
                    let (x, y) = self.cell_xy(self.cx, self.cy);
                    let bg = self.theme.background;
                    self.glyph_at(x, y, ' ', fg, bg);
                    self.mark_dirty(y, y + self.cell_h);
                }
            }
            c if (c as u32) < 0x20 => {}
            c => {
                if self.cx >= self.cols {
                    self.wrap();
                }
                let (x, y) = self.cell_xy(self.cx, self.cy);
                let bg = self.theme.background;
                self.glyph_at(x, y, c, fg, bg);
                self.mark_dirty(y, y + self.cell_h);
                self.cx += 1;
            }
        }
    }

    fn newline(&mut self) {
        self.cx = 0;
        self.cy += 1;
        if self.cy >= self.rows {
            let bg = self.px(self.theme.background);
            let (y0, ch, rows) = (self.text_y, self.cell_h, self.rows);
            self.surface().scroll_up(y0, y0 + rows * ch, ch, bg);
            self.mark_dirty(y0, y0 + rows * ch);
            self.cy = self.rows - 1;
        }
    }

    fn cursor_rect(&self) -> (usize, usize, usize, usize) {
        let (col, row) = if self.cx >= self.cols { (self.cols - 1, self.cy) } else { (self.cx, self.cy) };
        let (x, y) = self.cell_xy(col, row);
        let thick = self.scale.max(1) * 2;
        (x, y + self.cell_h - thick, self.cell_w, thick)
    }

    fn erase_cursor(&mut self) {
        let (x, y, w, h) = self.cursor_rect();
        let bg = self.px(self.theme.background);
        self.surface().fill_rect(x, y, w, h, bg);
        self.mark_dirty(y, y + h);
        self.cursor_on = false;
    }

    pub fn set_cursor(&mut self, visible: bool) {
        if visible {
            let (x, y, w, h) = self.cursor_rect();
            let c = self.px(self.theme.cursor);
            self.surface().fill_rect(x, y, w, h, c);
            self.mark_dirty(y, y + h);
            self.cursor_on = true;
        } else if self.cursor_on {
            self.erase_cursor();
        }
        self.present();
    }

    pub fn clear(&mut self) {
        let bg = self.px(self.theme.background);
        let (w, top, bottom) = (self.vram.width(), self.bar_h, self.vram.height() - self.bar_h);
        self.surface().fill_rect(0, top, w, bottom - top, bg);
        self.mark_dirty(top, bottom);
        self.cx = 0;
        self.cy = 0;
        self.cursor_on = false;
        self.present();
    }

    // --- Barras ------------------------------------------------------------------------

    fn draw_top_bar(&mut self) {
        let t = self.theme;
        let (w, bh, s) = (self.vram.width(), self.bar_h, self.scale);
        let (bar, accent) = (self.px(t.bar_background), self.px(t.accent));
        self.surface().fill_rect(0, 0, w, bh, bar);
        self.surface().fill_rect(0, bh - s, w, s, accent);
        let y = (bh - s - self.cell_h) / 2;
        let mut x = self.cell_w / 2;
        x += self.text_at(x, y, "■ ", t.accent, t.bar_background);
        x += self.text_at(x, y, version::DISTRO_NAME, t.accent, t.bar_background);
        let mut rest = FixedBuf::<96>::new();
        let _ =
            write!(rest, " {} «{}» · {} · {}", version::VERSION, version::CODENAME, version::ARCH, version::PROFILE);
        self.text_at(x, y, rest.as_str(), t.bar_foreground, t.bar_background);
        let right = self.title_right;
        let rw = self.text_width(right.as_str());
        if rw > 0 && rw + self.cell_w < w {
            self.text_at(w - rw - self.cell_w / 2, y, right.as_str(), t.bar_foreground, t.bar_background);
        }
        self.mark_dirty(0, bh);
    }

    fn draw_bottom_bar(&mut self) {
        let t = self.theme;
        let (w, h, bh, s) = (self.vram.width(), self.vram.height(), self.bar_h, self.scale);
        let (bar, accent) = (self.px(t.bar_background), self.px(t.accent));
        let top = h - bh;
        self.surface().fill_rect(0, top, w, bh, bar);
        self.surface().fill_rect(0, top, w, s, accent);
        let y = top + s + (bh - s - self.cell_h) / 2;
        let right = self.status_right;
        let rw = self.text_width(right.as_str());
        // El texto de la izquierda se recorta para no pisar el de la derecha.
        let room = w.saturating_sub(rw + 2 * self.cell_w) / self.cell_w;
        let left = self.status_left;
        let cut = left.as_str().char_indices().nth(room).map(|(i, _)| i).unwrap_or(left.len());
        self.text_at(self.cell_w / 2, y, &left.as_str()[..cut], t.bar_foreground, t.bar_background);
        if rw > 0 && rw + self.cell_w < w {
            self.text_at(w - rw - self.cell_w / 2, y, right.as_str(), t.muted, t.bar_background);
        }
        self.mark_dirty(top, h);
    }

    pub fn set_bars(&mut self, title_right: &str, status_left: &str, status_right: &str) {
        let changed_top = self.title_right.as_str() != title_right;
        let changed_bottom = self.status_left.as_str() != status_left || self.status_right.as_str() != status_right;
        if changed_top {
            self.title_right.set(title_right);
            self.draw_top_bar();
        }
        if changed_bottom {
            self.status_left.set(status_left);
            self.status_right.set(status_right);
            self.draw_bottom_bar();
        }
        self.present();
    }

    pub fn repaint_all(&mut self) {
        let bg = self.px(self.theme.background);
        let (w, h) = (self.vram.width(), self.vram.height());
        self.surface().fill_rect(0, 0, w, h, bg);
        self.mark_dirty(0, h);
        self.draw_top_bar();
        self.draw_bottom_bar();
        self.cx = 0;
        self.cy = 0;
        self.cursor_on = false;
        self.present();
    }

    pub fn set_theme(&mut self, theme: Theme) {
        self.theme = theme;
        self.repaint_all();
    }

    // --- Memoria ---------------------------------------------------------------------

    /// Activa el búfer sombra. Devuelve `false` si no se puede (formato o memoria).
    pub fn enable_shadow(&mut self) -> bool {
        if self.shadow.is_some() {
            return true;
        }
        let (stride, h) = (self.vram.stride(), self.vram.height());
        if self.vram.format().bytes_per_pixel != 4 || stride % 4 != 0 {
            return false;
        }
        let words = stride / 4 * h;
        let mut buf: Vec<u32> = Vec::new();
        if buf.try_reserve_exact(words).is_err() {
            return false;
        }
        buf.resize(words, 0);
        // Una única lectura completa de la VRAM para no perder lo ya pintado.
        // SAFETY: ambos búferes miden `stride * h` bytes y no se solapan.
        unsafe { core::ptr::copy_nonoverlapping(self.vram.base_ptr(), buf.as_mut_ptr() as *mut u8, stride * h) };
        // SAFETY: `buf` vive dentro de `self.shadow` tanto como la superficie.
        let surface =
            unsafe { Surface::new(buf.as_mut_ptr() as *mut u8, self.vram.width(), h, stride, self.vram.format()) };
        self.shadow = Some((surface, buf));
        true
    }

    /// La VRAM se volvió a mapear en otra dirección (write-combining).
    pub fn relocate(&mut self, base: *mut u8) {
        let (w, h, stride, f) = (self.vram.width(), self.vram.height(), self.vram.stride(), self.vram.format());
        // SAFETY: misma memoria física, nueva dirección virtual mapeada.
        self.vram = unsafe { Surface::new(base, w, h, stride, f) };
    }

    // --- Extras ----------------------------------------------------------------------

    /// Muestras de todos los roles del tema (orden `colores`).
    pub fn draw_palette(&mut self) {
        let t = self.theme;
        let swatches: [(&str, Rgb); 14] = [
            ("fondo", t.background),
            ("texto", t.foreground),
            ("tenue", t.muted),
            ("acento", t.accent),
            ("barra", t.bar_background),
            ("panel", t.panel),
            ("error", t.error),
            ("aviso", t.warning),
            ("info", t.info),
            ("depur", t.debug),
            ("traza", t.trace),
            ("éxito", t.success),
            ("cursor", t.cursor),
            ("s/acento", t.on_accent),
        ];
        if self.cx != 0 {
            self.newline();
        }
        let per_row = (self.cols / 12).max(1);
        let lines_needed = swatches.len().div_ceil(per_row) * 2 + 2;
        for _ in 0..lines_needed {
            self.newline();
        }
        let first_row = (self.cy + 1).saturating_sub(lines_needed);
        let (cw, ch) = (self.cell_w, self.cell_h);
        for (i, (name, color)) in swatches.iter().enumerate() {
            let col = (i % per_row) * 12;
            let row = first_row + (i / per_row) * 2;
            let (x, y) = self.cell_xy(col, row);
            let c = self.px(*color);
            let border = self.px(t.muted);
            self.surface().fill_rect(x, y, cw * 2, ch, border);
            self.surface().fill_rect(x + 1, y + 1, cw * 2 - 2, ch - 2, c);
            let bg = t.background;
            self.text_at(x + cw * 3, y, name, t.foreground, bg);
        }
        // Degradado de fondo → acento → error en la última línea.
        let (gx, gy) = self.cell_xy(0, first_row + lines_needed - 1);
        let width = self.cols * cw;
        for i in 0..width {
            let a = (i * 510 / width.max(1)) as u32;
            let c = if a < 256 { t.background.mix(t.accent, a as u8) } else { t.accent.mix(t.error, (a - 255) as u8) };
            let px = self.px(c);
            self.surface().fill_rect(gx + i, gy + ch / 4, 1, ch / 2, px);
        }
        let (_, y0) = self.cell_xy(0, first_row);
        self.mark_dirty(y0, gy + ch);
        // El cursor queda debajo del degradado: lo siguiente no lo pisa.
        self.newline();
        self.present();
    }

    /// Recuadro de pánico centrado; no vuelve a pintarse nada encima.
    pub fn panic_screen(&mut self, title: &str, lines: &[&str]) {
        let t = self.theme;
        let (w, h) = (self.vram.width(), self.vram.height());
        let box_w = (self.cell_w * 76).min(w.saturating_sub(2 * self.cell_w));
        let box_h = self.cell_h * (lines.len() + 5);
        let (x0, y0) = ((w - box_w) / 2, (h.saturating_sub(box_h)) / 2);
        let dark = Rgb::hex(0x000000);
        let bg = t.error.mix(dark, 150);
        let border = self.px(t.error);
        let bgp = self.px(bg);
        let s = self.scale;
        self.surface().fill_rect(x0, y0, box_w, box_h, border);
        self.surface().fill_rect(x0 + 2 * s, y0 + 2 * s, box_w - 4 * s, box_h - 4 * s, bgp);
        let white = Rgb::hex(0xFFFFFF);
        let tx = x0 + self.cell_w;
        let mut y = y0 + self.cell_h;
        self.text_at(tx, y, title, white, bg);
        y += self.cell_h * 2;
        let max_chars = (box_w - 2 * self.cell_w) / self.cell_w;
        for l in lines {
            let end = l.char_indices().nth(max_chars).map(|(i, _)| i).unwrap_or(l.len());
            self.text_at(tx, y, &l[..end], white.mix(bg, 30), bg);
            y += self.cell_h;
        }
        self.mark_dirty(0, h);
        self.present();
    }
}
